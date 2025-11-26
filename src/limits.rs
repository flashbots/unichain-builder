//! Flashblocks payload limits
//!
//! Code in this module is responsible for setting limits on the payload of
//! individual flashblocks. This is essentially where we define the block /
//! flashblock partitioning logic.

use {
	crate::{
		Flashblocks,
		state::{FlashblockNumber, TargetFlashblocks},
	},
	core::time::Duration,
	rblib::{alloy::consensus::BlockHeader, prelude::*},
	std::sync::{Arc, Mutex},
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
	/// Current block number being built, or `None` if uninitialized.
	current_block: Option<u64>,
	/// Current flashblock number. Used to check if we're on the first
	/// flashblock or to adjust the target number of flashblocks for a block.
	target_flashblocks: Arc<TargetFlashblocks>,
	/// Duration for the first flashblock, which may be shortened to absorb
	/// timing variance.
	first_flashblock_interval: Duration,
	/// Gas allocated per flashblock (total gas limit divided by flashblock
	/// count).
	gas_per_flashblock: u64,
}

impl FlashblockState {
	fn current_gas_limit(&self, flashblock_number: &FlashblockNumber) -> u64 {
		self
			.gas_per_flashblock
			.saturating_mul(flashblock_number.current())
	}
}

impl FlashblockLimits {
	pub fn new(interval: Duration) -> Self {
		let state = FlashblockState {
			..Default::default()
		};
		FlashblockLimits {
			interval,
			state: Arc::new(Mutex::new(state)),
		}
	}

	/// Resets state when starting a new block, calculating target flashblock
	/// count.
	///
	/// If a new block is detected (different block number than current state),
	/// initializes the flashblock partition for this block by:
	/// - Calculating available time and dividing it into flashblock intervals
	/// - Computing gas per flashblock from the total gas limit
	/// - Resetting the current flashblock counter to 0
	/// - Adjusting the target number of flashblocks
	pub fn update_state(
		&self,
		payload: &Checkpoint<Flashblocks>,
		enclosing: &Limits<Flashblocks>,
	) {
		let mut state = self.state.lock().expect("mutex is not poisoned");

		if state.current_block != Some(payload.block().number()) {
			let payload_deadline = enclosing.deadline.expect(
				"Flashblock limit require its enclosing scope to have a deadline",
			);
			let elapsed = payload.building_since().elapsed();
			let remaining_time = payload_deadline.saturating_sub(elapsed);

			let (target_flashblock, first_flashblock_interval) =
				self.calculate_flashblocks(payload, remaining_time);

			state.gas_per_flashblock = enclosing
				.gas_limit
				.checked_div(target_flashblock)
				.unwrap_or(enclosing.gas_limit);
			state.current_block = Some(payload.block().number());
			state.first_flashblock_interval = first_flashblock_interval;
			state.target_flashblocks.set(target_flashblock);
		}
	}

	/// Returns limits for the current flashblock.
	///
	/// If all flashblocks have been produced, returns a deadline of 1ms to stop
	/// production.
	pub fn get_limits(
		&self,
		enclosing: &Limits<Flashblocks>,
		flashblock_number: &FlashblockNumber,
	) -> Limits<Flashblocks> {
		let state = self.state.lock().expect("mutex is not poisoned");
		// If flashblock number == 1, we're building the first flashblock
		let deadline = if flashblock_number.current() == 1 {
			state.first_flashblock_interval
		} else {
			self.interval
		};

		enclosing
			.with_deadline(deadline)
			.with_gas_limit(state.current_gas_limit(flashblock_number))
	}

	/// Calculates the number of flashblocks and first flashblock interval for
	/// this block.
	///
	/// Extracts block time from block timestamps, then partitions the remaining
	/// time into flashblock intervals.
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

		partition_time_into_flashblocks(block_time, remaining_time, self.interval)
	}
}

impl ScopedLimits<Flashblocks> for FlashblockLimits {
	/// Creates the payload limits for the next flashblock in a new payload job.
	fn create(
		&self,
		payload: &Checkpoint<Flashblocks>,
		enclosing: &Limits<Flashblocks>,
	) -> Limits<Flashblocks> {
		let flashblock_number = payload.context();
		// Check the state and reset if we started building next block
		self.update_state(payload, enclosing);

		let limits = self.get_limits(enclosing, flashblock_number);

		let state = self.state.lock().expect("mutex is not poisoned");
		let flashblock_number = payload.context();
		if flashblock_number.current() <= state.target_flashblocks.get() {
			let gas_used = payload.cumulative_gas_used();
			let remaining_gas = enclosing.gas_limit.saturating_sub(gas_used);
			tracing::info!(
				"Creating flashblocks limits: {}, payload txs: {}, gas used: {} \
				 ({}%), gas_remaining: {} ({}%), next_block_gas_limit: {} ({}%), gas \
				 per block: {} ({}%), remaining_time: {}ms, gas_limit: {}",
				flashblock_number,
				payload.history().transactions().count(),
				gas_used,
				(gas_used * 100 / enclosing.gas_limit),
				remaining_gas,
				(remaining_gas * 100 / enclosing.gas_limit),
				state.current_gas_limit(flashblock_number),
				(state.current_gas_limit(flashblock_number) * 100
					/ enclosing.gas_limit),
				state.gas_per_flashblock,
				(state.gas_per_flashblock * 100 / enclosing.gas_limit),
				limits.deadline.expect("deadline is set").as_millis(),
				limits.gas_limit
			);
		}

		limits
	}
}

/// Partitions available time into flashblock intervals.
///
/// Divides `remaining_time` by `flashblock_interval` to determine how many
/// flashblocks can fit. If there's a remainder, adds one additional flashblock
/// with shortened duration. This ensures the sum of all flashblock durations
/// equals the total remaining time.
///
/// When `remaining_time` doesn't divide evenly by the interval, the first
/// flashblock gets a shortened interval to absorb the remainder, and subsequent
/// flashblocks use the full `flashblock_interval`.
///
/// # Arguments
/// - `block_time`: The actual time available for block production
/// - `remaining_time`: Payload deadline remaining (capped by `block_time`)
/// - `flashblock_interval`: Target duration for each flashblock
///
/// # Returns
/// `(num_flashblocks, first_flashblock_interval)`
fn partition_time_into_flashblocks(
	block_time: Duration,
	remaining_time: Duration,
	flashblock_interval: Duration,
) -> (u64, Duration) {
	let remaining_time = remaining_time.min(block_time);

	let remaining_millis = u64::try_from(remaining_time.as_millis())
		.expect("remaining_time should never exceed u64::MAX milliseconds");
	let interval_millis = u64::try_from(flashblock_interval.as_millis())
		.expect("flashblock_interval should never exceed u64::MAX milliseconds");

	let first_offset_millis = remaining_millis % interval_millis;

	if first_offset_millis == 0 {
		// Perfect division: remaining time is exact multiple of interval
		(remaining_millis / interval_millis, flashblock_interval)
	} else {
		// Non-perfect division: add extra flashblock with shortened first interval
		(
			remaining_millis / interval_millis + 1,
			Duration::from_millis(first_offset_millis),
		)
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	// ============================================================================
	// Basic Functionality Tests
	// ============================================================================

	#[test]
	fn perfect_division_single_interval() {
		// Remaining time equals exactly one interval.
		let block_time = Duration::from_secs(2);
		let remaining_time = Duration::from_millis(500);
		let interval = Duration::from_millis(500);

		let (num_flashblocks, first_interval) =
			partition_time_into_flashblocks(block_time, remaining_time, interval);

		assert_eq!(first_interval, interval);
		assert_eq!(num_flashblocks, 1);
	}

	#[test]
	fn perfect_division_multiple_intervals() {
		// Remaining time is exact multiple of interval.
		let block_time = Duration::from_secs(2);
		let remaining_time = Duration::from_millis(2000);
		let interval = Duration::from_millis(250);

		let (num_flashblocks, first_interval) =
			partition_time_into_flashblocks(block_time, remaining_time, interval);

		assert_eq!(first_interval, interval);
		assert_eq!(num_flashblocks, 8); // 2000 / 250 = 8
	}

	#[test]
	fn imperfect_division_remainder() {
		// Remaining time leaves a remainder when divided by interval.
		let block_time = Duration::from_secs(2);
		let remaining_time = Duration::from_millis(1925);
		let interval = Duration::from_millis(250);

		let (num_flashblocks, first_interval) =
			partition_time_into_flashblocks(block_time, remaining_time, interval);

		// 1925 / 250 = 7 with remainder 175
		assert_eq!(first_interval, Duration::from_millis(175));
		assert_eq!(num_flashblocks, 8);
	}

	#[test]
	fn imperfect_division_large_remainder() {
		// Remaining time with large remainder when divided by interval.
		// Note: remaining_time is capped by block_time (2000ms)
		let block_time = Duration::from_secs(2);
		let remaining_time = Duration::from_millis(2050);
		let interval = Duration::from_millis(300);

		let (num_flashblocks, first_interval) =
			partition_time_into_flashblocks(block_time, remaining_time, interval);

		// Capped to 2000ms: 2000 / 300 = 6 with remainder 200, so 7 flashblocks
		// total
		assert_eq!(first_interval, Duration::from_millis(200));
		assert_eq!(num_flashblocks, 7);
	}

	// ============================================================================
	// Edge Case Tests - Small Values
	// ============================================================================

	#[test]
	fn single_flashblock_less_than_interval() {
		// Remaining time is smaller than a single interval.
		let block_time = Duration::from_secs(2);
		let remaining_time = Duration::from_millis(350);
		let interval = Duration::from_millis(500);

		let (num_flashblocks, first_interval) =
			partition_time_into_flashblocks(block_time, remaining_time, interval);

		assert_eq!(first_interval, Duration::from_millis(350));
		assert_eq!(num_flashblocks, 1);
	}

	#[test]
	fn minimal_interval_with_large_remaining_time() {
		// Very small interval (1ms) with large remaining time.
		let block_time = Duration::from_secs(2);
		let remaining_time = Duration::from_millis(100);
		let interval = Duration::from_millis(1);

		let (num_flashblocks, first_interval) =
			partition_time_into_flashblocks(block_time, remaining_time, interval);

		assert_eq!(first_interval, interval);
		assert_eq!(num_flashblocks, 100); // 100ms / 1ms = 100
	}

	#[test]
	fn remainder_is_one_millisecond() {
		// Remaining time leaves exactly 1ms remainder.
		let block_time = Duration::from_secs(2);
		let remaining_time = Duration::from_millis(501); // 5 * 100 + 1
		let interval = Duration::from_millis(100);

		let (num_flashblocks, first_interval) =
			partition_time_into_flashblocks(block_time, remaining_time, interval);

		assert_eq!(first_interval, Duration::from_millis(1));
		assert_eq!(num_flashblocks, 6);
	}

	// ============================================================================
	// Edge Case Tests - Large Values
	// ============================================================================

	#[test]
	fn large_interval_with_small_remaining_time() {
		// Very large interval (10 seconds) with small remaining time.
		let block_time = Duration::from_secs(2);
		let remaining_time = Duration::from_millis(500);
		let interval = Duration::from_secs(10);

		let (num_flashblocks, first_interval) =
			partition_time_into_flashblocks(block_time, remaining_time, interval);

		assert_eq!(first_interval, Duration::from_millis(500));
		assert_eq!(num_flashblocks, 1);
	}

	#[test]
	fn many_flashblocks_with_large_remaining_time() {
		// Large remaining time that divides perfectly by interval.
		let block_time = Duration::from_secs(2);
		let remaining_time = Duration::from_millis(2000);
		let interval = Duration::from_millis(200);

		let (num_flashblocks, first_interval) =
			partition_time_into_flashblocks(block_time, remaining_time, interval);

		assert_eq!(first_interval, interval);
		assert_eq!(num_flashblocks, 10); // 2000 / 200 = 10
	}

	// ============================================================================
	// Invariant Property Tests
	// ============================================================================

	#[test]
	fn first_flashblock_interval_never_exceeds_configured_interval() {
		// Property: first flashblock interval should never exceed the configured
		// interval.
		let test_cases = vec![
			(
				Duration::from_secs(2),
				Duration::from_millis(750),
				Duration::from_millis(100),
			),
			(
				Duration::from_secs(2),
				Duration::from_millis(1500),
				Duration::from_millis(250),
			),
			(
				Duration::from_secs(2),
				Duration::from_millis(3000),
				Duration::from_millis(500),
			),
			(
				Duration::from_secs(2),
				Duration::from_millis(2500),
				Duration::from_millis(1000),
			),
		];

		for (block_time, remaining_time, interval) in test_cases {
			let (_, first_interval) =
				partition_time_into_flashblocks(block_time, remaining_time, interval);

			assert!(
				first_interval <= interval,
				"First flashblock interval ({first_interval:?}) should not exceed \
				 interval ({interval:?})",
			);
		}
	}

	#[test]
	fn total_time_equals_remaining_time() {
		// Property: sum of all flashblock intervals should equal remaining time.
		// first_interval + (num_flashblocks - 1) * interval = remaining_time
		let block_time = Duration::from_secs(2);
		let remaining_time = Duration::from_millis(1925);
		let interval = Duration::from_millis(250);

		let (num_flashblocks, first_interval) =
			partition_time_into_flashblocks(block_time, remaining_time, interval);

		let num_remaining_intervals = num_flashblocks.saturating_sub(1);
		let total_time = first_interval
			+ interval
				.saturating_mul(u32::try_from(num_remaining_intervals).unwrap());
		assert_eq!(total_time, remaining_time);
	}

	#[test]
	fn time_sum_invariant_with_multiple_cases() {
		// Property test: time sum should always equal remaining time
		let test_cases = vec![
			(
				Duration::from_secs(2),
				Duration::from_millis(333),
				Duration::from_millis(100),
			),
			(
				Duration::from_secs(2),
				Duration::from_millis(1234),
				Duration::from_millis(200),
			),
			(
				Duration::from_secs(2),
				Duration::from_millis(999),
				Duration::from_millis(111),
			),
			(
				Duration::from_secs(2),
				Duration::from_millis(2000),
				Duration::from_millis(333),
			),
		];

		for (block_time, remaining_time, interval) in test_cases {
			let (num_flashblocks, first_interval) =
				partition_time_into_flashblocks(block_time, remaining_time, interval);

			let num_remaining_intervals = num_flashblocks.saturating_sub(1);
			let total_time = first_interval
				+ interval
					.saturating_mul(u32::try_from(num_remaining_intervals).unwrap());
			assert_eq!(
				total_time, remaining_time,
				"Time sum mismatch for interval={interval:?}, \
				 remaining_time={remaining_time:?}",
			);
		}
	}

	// ============================================================================
	// Odd Division Cases
	// ============================================================================

	#[test]
	fn odd_division_333_ms_interval() {
		// When remaining time divides unevenly with odd numbers.
		// 1000 / 333 = 3 with remainder 1
		let block_time = Duration::from_secs(2);
		let remaining_time = Duration::from_millis(1000);
		let interval = Duration::from_millis(333);

		let (num_flashblocks, first_interval) =
			partition_time_into_flashblocks(block_time, remaining_time, interval);

		assert_eq!(first_interval, Duration::from_millis(1));
		assert_eq!(num_flashblocks, 4);
	}

	#[test]
	fn odd_division_various_primes() {
		// Test with prime numbers to ensure no edge cases with divisibility.
		let block_time = Duration::from_secs(2);
		let remaining_time = Duration::from_millis(1000);
		let interval = Duration::from_millis(7);

		let (num_flashblocks, first_interval) =
			partition_time_into_flashblocks(block_time, remaining_time, interval);

		// 1000 / 7 = 142 with remainder 6
		assert_eq!(first_interval, Duration::from_millis(6));
		assert_eq!(num_flashblocks, 143);
	}

	// ============================================================================
	// Zero remaining time -- This happens when FCU arrives after the deadline
	// ============================================================================

	#[test]
	fn zero_time_left() {
		// We should get zero flashblocks since there's no time left
		let block_time = Duration::from_secs(2);
		let remaining_time = Duration::from_millis(0);
		let interval = Duration::from_millis(200);

		let (num_flashblocks, first_interval) =
			partition_time_into_flashblocks(block_time, remaining_time, interval);

		// 0 / 200 = 0 with remainder 0
		assert_eq!(first_interval, Duration::from_millis(200));
		assert_eq!(num_flashblocks, 0);
	}
}
