//! Flashblocks payload limits
//!
//! Code in this module is responsible for setting limits on the payload of
//! individual flashblocks. This is essentially where we define the block /
//! flashblock partitioning logic.

use {
	crate::Flashblocks,
	core::time::Duration,
	rblib::{alloy::consensus::BlockHeader, prelude::*},
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
	/// The time interval between flashblocks within one payload job.
	interval: Duration,
}

impl FlashblockLimits {
	pub fn new(interval: Duration) -> Self {
		FlashblockLimits { interval }
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

		let payload_deadline = enclosing.deadline.expect(
			"FlashblockLimits requires its enclosing scope to have a deadline",
		);
		let elapsed = payload.building_since().elapsed();
		let remaining_time = payload_deadline.saturating_sub(elapsed);

		let block_time = Duration::from_secs(
			payload
				.block()
				.timestamp()
				.saturating_sub(payload.block().parent().header().timestamp()),
		);

		let (target_flashblocks, deadline) = partition_time_into_flashblocks(
			block_time,
			remaining_time,
			self.interval,
		);

		let gas_limit = enclosing
			.gas_limit
			.checked_div(target_flashblocks)
			.unwrap_or(enclosing.gas_limit)
			.saturating_mul(flashblock_number.current());

		enclosing.with_deadline(deadline).with_gas_limit(gas_limit)
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

	let remaining_ms = u64::try_from(remaining_time.as_millis())
		.expect("remaining_time should never exceed u64::MAX milliseconds");
	let flashblock_interval_ms = u64::try_from(flashblock_interval.as_millis())
		.expect("flashblock_interval should never exceed u64::MAX milliseconds");

	let first_offset_ms = remaining_ms % flashblock_interval_ms;

	if first_offset_ms == 0 {
		// Perfect division: remaining time is exact multiple of interval
		(remaining_ms / flashblock_interval_ms, flashblock_interval)
	} else {
		// Non-perfect division: add extra flashblock with shortened first interval
		(
			remaining_ms / flashblock_interval_ms + 1,
			Duration::from_millis(first_offset_ms),
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
