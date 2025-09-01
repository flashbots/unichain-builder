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
}

impl ScopedLimits<Flashblocks> for FlashblockLimits {
	/// Creates the payload limits for the first flashblock in a new payload job.
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

		tracing::info!(
			">--> payload txs: {}, gas used: {} ({}%), gas_remaining: {} ({}%), \
			 next_block_gas_limit: {} ({}%), gas per block: {} ({}%), remaining \
			 blocks: {}, remaining time: {:?}",
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
			remaining_time
		);

		enclosing
			.with_deadline(self.interval)
			.with_gas_limit(next_block_gas_limit)
	}
}

impl FlashblockLimits {
	pub fn with_interval(interval: Duration) -> Self {
		FlashblockLimits { interval }
	}
}
