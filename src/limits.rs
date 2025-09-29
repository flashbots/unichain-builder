//! Flashblocks payload limits
//!
//! Code in this module is responsible for setting limits on the payload of
//! individual flashblocks. This is essentially where we define the block /
//! flashblock partitioning logic.

use std::sync::{Arc, Mutex};
use {
	crate::Flashblocks,
	core::time::Duration,
	rblib::{alloy::consensus::BlockHeader, prelude::*},
	std::ops::{Div, Rem},
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
#[derive(Debug, Clone)]
pub struct FlashblockLimits {
	/// Holds flashblock limits state, uninitialized then called for the first time
	state: Arc<Mutex<Option<FlashblockLimitsState>>>,
	/// The time interval between flashblocks within one payload job.
	interval: Duration,
	/// Time by which flashblocks will be delivered earlier to account for
	/// latency. This time is absorbed by the first flashblock.
	leeway_time: Duration,
}

#[derive(Debug, Clone, Copy)]
struct FlashblockLimitsState {
	/// Current flashblock being build
	current_flashblock: u64,
	// How many flashblocks to issue
	target_flashblock: u64,
	// How many gas each flashblock should fit
	gas_per_flashblock: u64,
}

impl FlashblockLimitsState {
	/// Returns 'number' of current flashblock.
	fn current_flashblock_number(&self) -> u64 {
		self.current_flashblock.saturating_add(1)
	}

	/// Returns number of flashblock planned to be built for this block
	fn remaining_blocks(&self) -> u64 {
		// TODO: check +1
		self.target_flashblock.saturating_sub(self.current_flashblock)
	}

	/// Return gas limit for current flashblock
	fn gas_limit(&self) -> u64 {
		self.gas_per_flashblock * self.current_flashblock_number()
	}

	/// Progresses state to the next flashblock
	fn progress_flashblock(&mut self) {
		self.current_flashblock += 1;
		tracing::info!("Progressing state, new {}", self.current_flashblock);
	}
}

impl ScopedLimits<Flashblocks> for FlashblockLimits {
	/// Creates the payload limits for the next flashblock in a new payload job.
	fn create(
		&self,
		payload: &Checkpoint<Flashblocks>,
		enclosing: &Limits,
	) -> Limits {
		tracing::info!("gas limit: {}",enclosing.gas_limit);
		let payload_deadline = enclosing.deadline.expect(
			"Flashblock limit require its enclosing scope to have a deadline",
		);
		let remaining_time =
			payload_deadline.saturating_sub(payload.building_since().elapsed());
		let current_state = *self.state.lock().expect("mutex is poisoned");
		let (current_flashblock_interval, mut state)= if let Some(state) = current_state {
			(self.interval, state)
		} else {
		// If inner not initialised that means we are building first flashblock
			let (target_flashblock, first_flashblock_interval) = self.calculate_flashblocks(payload, remaining_time);
			let gas_per_flashblock = enclosing.gas_limit / target_flashblock;
			let state = FlashblockLimitsState {
				current_flashblock: 0,
				target_flashblock,
				gas_per_flashblock,
			};
			(first_flashblock_interval, state)
		};

		tracing::info!("current_flashblock: {}", state.current_flashblock);
		tracing::info!("target_flashblock: {}", state.target_flashblock);

		if state.remaining_blocks() < 1 {
			// we don't have enough time for more than one block
			// saturate the payload gas within the remaining time
			return enclosing.with_deadline(remaining_time);
		}

		let gas_used = payload.cumulative_gas_used();
		let remaining_gas = enclosing.gas_limit.saturating_sub(gas_used);

		// We have capacity to build new flashblock, progressing state
		state.progress_flashblock();
		self.state.lock().expect("mutex is poisoned").replace(state);

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
			state.gas_limit(),
			(state.gas_limit() * 100 / enclosing.gas_limit),
			state.gas_per_flashblock,
			(state.gas_per_flashblock * 100 / enclosing.gas_limit),
			state.remaining_blocks(),
			remaining_time,
			self.leeway_time,
			current_flashblock_interval
		);

		tracing::info!("current_flashblock_interval: {:?}", current_flashblock_interval);
		tracing::info!("state.gas_limit(): {}", state.gas_limit());
		enclosing
			.with_deadline(current_flashblock_interval)
			.with_gas_limit(state.gas_limit())
	}
}

impl FlashblockLimits {
	pub fn new(interval: Duration, leeway_time: Duration) -> Self {
		FlashblockLimits {
			state: Arc::new(Mutex::new(None)),
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
}
