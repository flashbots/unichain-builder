//! Flashblocks payload limits
//!
//! Code in this module is responsible for setting limits on the payload of
//! individual flashblocks. This is essentially where we define the block /
//! flashblock partitioning logic.

use {
	crate::Flashblocks,
	core::time::Duration,
	rblib::{alloy::consensus::BlockHeader, prelude::*},
	std::{
		ops::{Div, Rem},
		sync::{
			Arc,
			Mutex,
			atomic::{AtomicU64, Ordering},
		},
	},
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
#[derive(Debug, Clone, Default)]
pub struct FlashblockLimits {
	state: Arc<Mutex<FlashblockState>>,
	/// The time interval between flashblocks within one payload job.
	interval: Duration,
}

#[derive(Debug, Clone, Default)]
pub struct FlashblockState {
	/// Current block being built
	/// None - uninitialized state, happens only on first block building job
	current_block: Option<u64>,
	/// Current flashblock being built, number based
	/// 0 - uninitialized state, use `progress_state` to initialize it
	current_flashblock: u64,
	/// Interval of the first flashblock, it absorbs leeway time and network lag
	first_flashblock_interval: Duration,
	/// Gas for flashblock used for current block
	gas_per_flashblock: u64,
	/// Used to communicate maximum number of flashblocks on every blocks for
	/// other steps
	// TODO: once we remove max_flashblocks from publish step we could change it
	// to u64
	max_flashblocks: Arc<AtomicU64>,
}

impl FlashblockState {
	fn current_gas_limit(&self) -> u64 {
		self
			.gas_per_flashblock
			.saturating_mul(self.current_flashblock)
	}
}
impl FlashblockLimits {
	pub fn new(interval: Duration, max_flashblocks: Arc<AtomicU64>) -> Self {
		let state = FlashblockState {
			max_flashblocks,
			..Default::default()
		};
		FlashblockLimits {
			interval,
			state: Arc::new(Mutex::new(state)),
		}
	}

	/// Checks if we have started building new block, if so we need to reset the
	/// state This will produce empty state, progress state before using it
	pub fn update_state(
		&self,
		payload: &Checkpoint<Flashblocks>,
		enclosing: &Limits,
	) {
		let mut state = self.state.lock().expect("mutex is not poisoned");

		if state.current_block != Some(payload.block().number()) {
			let payload_deadline = enclosing.deadline.expect(
				"Flashblock limit require its enclosing scope to have a deadline",
			);
			let remaining_time =
				payload_deadline.saturating_sub(payload.building_since().elapsed());

			let (target_flashblock, first_flashblock_interval) =
				self.calculate_flashblocks(payload, remaining_time);

			state.gas_per_flashblock = enclosing
				.gas_limit
				.checked_div(target_flashblock)
				.unwrap_or(enclosing.gas_limit);
			state.current_block = Some(payload.block().number());
			state.current_flashblock = 0;
			state.first_flashblock_interval = first_flashblock_interval;
			state
				.max_flashblocks
				.store(target_flashblock, Ordering::Relaxed);
		}
	}

	/// Progresses the state to the next flashblock
	pub fn progress_state(&self) {
		let mut state = self.state.lock().expect("mutex is not poisoned");
		state.current_flashblock += 1;
	}

	/// Return limits for the current flashblock
	pub fn get_limits(&self, enclosing: &Limits) -> Limits {
		let state = self.state.lock().expect("mutex is not poisoned");
		// Check that state was progressed at least once
		assert_ne!(
			state.current_flashblock, 0,
			"Get limits on uninitialized state"
		);
		// If we don't need to create new flashblocks - exit with immediate deadline
		if state.current_flashblock > state.max_flashblocks.load(Ordering::Relaxed)
		{
			enclosing.with_deadline(Duration::from_millis(1))
		} else {
			// If self.current_flashblock == 1, we are building first flashblock
			let enclosing = if state.current_flashblock == 1 {
				enclosing.with_deadline(state.first_flashblock_interval)
			} else {
				enclosing.with_deadline(self.interval)
			};
			enclosing.with_gas_limit(state.current_gas_limit())
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
		let remaining_time = remaining_time.min(block_time);
		let interval_millis = u64::try_from(self.interval.as_millis())
			.expect("interval_millis should never be greater than u64::MAX");
		let remaining_time_millis = u64::try_from(remaining_time.as_millis())
			.expect("remaining_time_millis should never be greater than u64::MAX");
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

impl ScopedLimits<Flashblocks> for FlashblockLimits {
	/// Creates the payload limits for the next flashblock in a new payload job.
	fn create(
		&self,
		payload: &Checkpoint<Flashblocks>,
		enclosing: &Limits,
	) -> Limits {
		// Check the state and reset if we started building next block
		self.update_state(payload, enclosing);

		// Update flashblock state
		self.progress_state();

		let limits = self.get_limits(enclosing);

		let state = self.state.lock().expect("mutex is not poisoned");
		if state.current_flashblock <= state.max_flashblocks.load(Ordering::Relaxed)
		{
			let gas_used = payload.cumulative_gas_used();
			let remaining_gas = enclosing.gas_limit.saturating_sub(gas_used);
			tracing::warn!(
				">---> flashblocks: {}/{}, payload txs: {}, gas used: {} ({}%), \
				 gas_remaining: {} ({}%), next_block_gas_limit: {} ({}%), gas per \
				 block: {} ({}%), remaining_time: {}ms, gas_limit: {}",
				state.current_flashblock,
				state.max_flashblocks.load(Ordering::Relaxed),
				payload.history().transactions().count(),
				gas_used,
				(gas_used * 100 / enclosing.gas_limit),
				remaining_gas,
				(remaining_gas * 100 / enclosing.gas_limit),
				state.current_gas_limit(),
				(state.current_gas_limit() * 100 / enclosing.gas_limit),
				state.gas_per_flashblock,
				(state.gas_per_flashblock * 100 / enclosing.gas_limit),
				limits.deadline.expect("deadline is set").as_millis(),
				limits.gas_limit,
			);
		}
		limits
	}
}
