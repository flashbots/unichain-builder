//! Flashblocks payload limits
//!
//! Code in this module is responsible for setting limits on the payload of
//! individual flashblocks. This is essentially where we define the block /
//! flashblock partitioning logic.

use {
	crate::Flashblocks,
	core::time::Duration,
	rblib::prelude::*,
	tracing::debug,
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

		// Calculate the deadline for the current flashblock.
		// The first flashblock gets reduced time to absorb the leeway_time offset.
		let is_first_block = payload.history().transactions().count() == 0;
		let current_block_deadline = if is_first_block {
			// First block absorbs the leeway time by having a shorter deadline.
			self.interval.saturating_sub(self.leeway_time)
		} else {
			// Subsequent blocks get the normal interval.
			self.interval
		};

		tracing::info!(
			">--> payload txs: {}, gas used: {} ({}%), gas_remaining: {} ({}%), \
			 next_block_gas_limit: {} ({}%), gas per block: {} ({}%), remaining \
			 blocks: {}, remaining time: {:?}, leeway: {:?}, \
			 current_block_deadline: {:?}",
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
			current_block_deadline
		);

		enclosing
			.with_deadline(current_block_deadline)
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
}
