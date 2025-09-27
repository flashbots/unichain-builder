//! Flashblocks payload limits
//!
//! Code in this module is responsible for setting limits on the payload of
//! individual flashblocks. This is essentially where we define the block /
//! flashblock partitioning logic.

use {
	crate::Flashblocks,
	core::time::Duration,
	rblib::{alloy::consensus::BlockHeader, prelude::*},
	std::ops::{Div, Rem},
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

		let is_first_block = self.is_first_block(payload);

		// Calculate the number of remaining flashblocks, and the interval for the
		// current flashblock.
		let (remaining_blocks, current_flashblock_interval) = if is_first_block {
			// First block absorbs the leeway time by having a shorter deadline.
			self.calculate_flashblocks(payload, remaining_time)
		} else {
			// Subsequent blocks get the normal interval.
			#[allow(clippy::cast_possible_truncation)]
			let remaining_blocks =
				(remaining_time.as_millis() / self.interval.as_millis()) as u64;
			(remaining_blocks, self.interval)
		};

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

		tracing::info!(
			">--> payload txs: {}, gas used: {} ({}%), gas_remaining: {} ({}%), \
			 next_block_gas_limit: {} ({}%), gas per block: {} ({}%), remaining \
			 blocks: {}, remaining time: {:?}, leeway: {:?}, \
			 current_flashblock_interval: {:?}",
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
			current_flashblock_interval
		);

		enclosing
			.with_deadline(current_flashblock_interval)
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

	// This calculates the number of flashblocks in a payload job, and the
	// interval for the first flashblock. if leeway_time is non-zero, the first
	// flashblock gets additional reduction in time to absorb the leeway_time
	// offset.
	pub fn calculate_flashblocks(
		&self,
		payload: &Checkpoint<Flashblocks>,
		remaining_time: Duration,
	) -> (u64, Duration) {
		let block_time = Duration::from_secs(
			payload
				.block()
				.timestamp()
				.saturating_sub(payload.block().parent().header().timestamp()),
		);
		let remaining_time = remaining_time
			.min(block_time);
		let interval_millis = self.interval.as_millis() as u64;
		let remaining_time_millis = remaining_time.as_millis() as u64;
		let first_flashblock_offset = remaining_time_millis.rem(interval_millis);

		if first_flashblock_offset == 0 {
			// We have perfect division, so we use interval as first fb offset
			(remaining_time_millis.div(interval_millis), self.interval)
		} else {
			// Non-perfect division, so we account for the shortened flashblock.
			(
				remaining_time_millis.div(interval_millis) + 1,
				Duration::from_millis(first_flashblock_offset),
			)
		}
	}

	/// Determines if this is the first block in a payload job, by checking if
	/// there are any flashblock barriers. If no flashblock barriers exist, this
	/// is considered the first block.
	pub fn is_first_block(&self, payload: &Checkpoint<Flashblocks>) -> bool {
		payload
			.history()
			.iter()
			.filter(|c| c.is_named_barrier("flashblock"))
			.count()
			== 0
	}
}
