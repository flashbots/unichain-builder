//! Flashblocks payload limits
//!
//! Code in this module is responsible for setting limits on the payload of
//! individual flashblocks. This is essentially where we define the block /
//! flashblock partitioning logic.

use {
	crate::Flashblocks,
	core::time::Duration,
	rblib::{alloy::consensus::BlockHeader, prelude::*},
	std::ops::Rem,
	tracing::{debug, error},
};

/// Specifies the limits for individual flashblocks.
///
/// This limits factory instance should be applied to the pipeline that is
/// responsible for producing individual flashblocks. This is usually the
/// pipeline that has the `PublishFlashblock` epilogue step.
///
/// At the beginning of a payload job this instance will calculate the number of
/// target flashblocks for the given job by dividing the total payload job
/// deadline by the flashblock interval.
pub struct FlashblockLimits {
	/// The time interval between flashblocks within one payload job.
	interval: Duration,
	/// Time by which flashblocks will be delivered earlier to account for
	/// latency. This time is absorbed by the first flashblock.
	leeway_time: Duration,
}

impl ScopedLimits<Flashblocks> for FlashblockLimits {
	/// Creates the payload limits for the next flashblock in a new payload job.
	fn create(
		&self,
		payload: &Checkpoint<Flashblocks>,
		enclosing: &Limits,
	) -> Limits {
		let payload_deadline = enclosing.deadline.expect(
			"Flashblock limit require its enclosing scope to have a deadline",
		);
		let remaining_time =
			payload_deadline.saturating_sub(payload.building_since().elapsed());

		#[allow(clippy::cast_possible_truncation)]
		let remaining_blocks =
			(remaining_time.as_millis() / self.interval.as_millis()) as u64;

		if remaining_blocks <= 1 {
			// we don't have enough time for more than one block
			// saturate the payload gas within the remaining time
			return enclosing.with_deadline(remaining_time);
		}

		let gas_used = payload.cumulative_gas_used();
		let remaining_gas = enclosing.gas_limit.saturating_sub(gas_used);

		if remaining_gas == 0 {
			debug!("No remaining gas for flashblocks, but still have time left");
			return enclosing.with_deadline(remaining_time);
		}

		let gas_per_block = remaining_gas / remaining_blocks;
		let next_block_gas_limit = gas_used.saturating_add(gas_per_block);

		// For a large number of transactions, checking if one transaction is not a
		// deposit is faster than checking if all transactions are deposits.
		// If all transactions so far are DEPOSIT transactions, that means that
		// there are no user transactions appended yet, which means that this is
		// the first block.
		let is_first_block = !payload.history().transactions().any(|tx| {
			rblib::alloy::consensus::Typed2718::ty(tx)
				!= rblib::alloy::optimism::consensus::DEPOSIT_TX_TYPE_ID
		});
		// Calculate the deadline for the current flashblock.
		let current_flashblock_deadline = if is_first_block {
			// First block absorbs the leeway time by having a shorter deadline.
			self.calculate_first_flashblock_deadline(payload)
		} else {
			// Subsequent blocks get the normal interval.
			self.interval
		};

		tracing::info!(
			">--> payload txs: {}, gas used: {} ({}%), gas_remaining: {} ({}%), \
			 next_block_gas_limit: {} ({}%), gas per block: {} ({}%), remaining \
			 blocks: {}, remaining time: {:?}, leeway: {:?}, \
			 current_flashblock_deadline: {:?}",
			payload.history().transactions().count(),
			gas_used,
			(gas_used * 100 / enclosing.gas_limit),
			remaining_gas,
			(remaining_gas * 100 / enclosing.gas_limit),
			next_block_gas_limit,
			(next_block_gas_limit * 100 / enclosing.gas_limit),
			gas_per_block,
			(gas_per_block * 100 / enclosing.gas_limit),
			remaining_blocks,
			remaining_time,
			self.leeway_time,
			current_flashblock_deadline
		);

		enclosing
			.with_deadline(current_flashblock_deadline)
			.with_gas_limit(next_block_gas_limit)
	}
}

impl FlashblockLimits {
	pub fn new(interval: Duration, leeway_time: Duration) -> Self {
		FlashblockLimits {
			interval,
			leeway_time,
		}
	}

	// This calculates the deadline for the first flashblock in a payload job.
	// The first flashblock gets reduced time to absorb the leeway_time offset.
	pub fn calculate_first_flashblock_deadline(
		&self,
		payload: &Checkpoint<Flashblocks>,
	) -> Duration {
		// Get the timestamp of the block (not flashblock) we're building towards
		let timestamp_to_build_block_by = payload.block().timestamp();

		// Logic below is originally from `op-rbuilder`: https://github.com/flashbots/op-rbuilder/blob/6267095f51bfe5c40da655f8faa40f360413c7f1/crates/op-rbuilder/src/builders/flashblocks/payload.rs#L817-L867
		// We use this system time to determine remining time to build a block
        // Things to consider:
        // FCU(a) - FCU with attributes
        // FCU(a) could arrive with `block_time - fb_time < delay`. In this case we could only produce 1 flashblock
        // FCU(a) could arrive with `delay < fb_time` - in this case we will shrink first flashblock
        // FCU(a) could arrive with `fb_time < delay < block_time - fb_time` - in this case we will issue less flashblocks
		let target_time = std::time::SystemTime::UNIX_EPOCH
			+ Duration::from_secs(timestamp_to_build_block_by)
				.saturating_sub(self.leeway_time);
		let now = std::time::SystemTime::now();
		let Ok(time_drift) = target_time.duration_since(now) else {
			error!(
				target: "flashblocks_pipeline",
				message = "FCU arrived too late or system clock are unsynced",
				?target_time,
				?now,
			);
			return self.interval;
		};

		// This is extra check to ensure that we would account at least for block
		// time in case we have any timer discrepancies.
		let block_time = Duration::from_secs(
			payload
				.block()
				.timestamp()
				.saturating_sub(payload.block().parent().header().timestamp()),
		);
		let time_drift = time_drift.min(block_time);
		let interval_millis = self.interval.as_millis();
		let time_drift = time_drift.as_millis();
		let first_flashblock_offset = time_drift.rem(interval_millis);
		if first_flashblock_offset == 0 {
			// We have perfect division, so we use interval as first fb offset
			self.interval
		} else {
			// Non-perfect division, so we account for it.
			#[allow(clippy::cast_possible_truncation)]
			Duration::from_millis(first_flashblock_offset as u64)
		}
	}
}
