//! Integration tests for flashblock timing and limits
//!
//! These tests verify that flashblocks are produced at the correct times based
//! on the configured interval and leeway time. They use the WebSocket observer
//! to monitor flashblock production in real time.

use {
	crate::{Flashblocks, tests::*},
	rblib::alloy::eips::Decodable2718,
	std::time::{Duration, SystemTime, UNIX_EPOCH},
	tracing::debug,
};

/// Helper to wait until the start of the next second for deterministic timing
async fn wait_for_second_boundary() -> eyre::Result<()> {
	let now = SystemTime::now().duration_since(UNIX_EPOCH)?;
	let current_millis = now.as_millis() % 1000;
	let sleep_duration = Duration::from_millis(1000 - current_millis as u64);
	tokio::time::sleep(sleep_duration).await;
	Ok(())
}

/// Helper to build a block with different timing settings. Builds a single
/// block and returns the flashblocks that were produced. Also makes some basic
/// checks on the block like that it contains the sequencer tx and it contains
/// the transactions that we sent to it.
async fn run_test(
	leeway_time: Duration,
	flashblocks_interval: Duration,
) -> eyre::Result<Vec<ObservedFlashblock>> {
	const TXS_PER_BLOCK: usize = 60;

	let (node, ws_addr) =
		Flashblocks::test_node_with_custom_leeway_time_and_interval(
			leeway_time,
			flashblocks_interval,
		)
		.await?;
	let ws = WebSocketObserver::new(ws_addr).await?;

	wait_for_second_boundary().await?;

	let mut sent_txs = Vec::new();
	let block = Box::pin(node.while_next_block(async {
		for _ in 0..TXS_PER_BLOCK {
			let tx = node
				.send_tx(node.build_tx().transfer().value(U256::from(1_234_000)))
				.await?;
			tokio::task::yield_now().await;
			sent_txs.push(*tx.tx_hash());
		}
		Ok(())
	}))
	.await?;

	assert_eq!(block.number(), 1);
	assert!(!sent_txs.is_empty());
	assert_has_sequencer_tx!(&block);
	assert!(ws.has_no_errors());

	let flashblocks = ws.by_block_number(block.number());

	// Verify that transactions in flashblocks match final block transactions
	let fblock_txhashes: Vec<_> = flashblocks
		.iter()
		.flat_map(|fb| {
			fb.block.diff.transactions.iter().map(|tx| {
				types::TxEnvelope::<Flashblocks>::decode_2718(&mut &tx[..])
					.unwrap()
					.tx_hash()
			})
		})
		.collect();

	// All flashblock transactions should be in the final block
	assert!(block.includes(&fblock_txhashes));

	Ok(flashblocks)
}

/// Helper to verify flashblock timing intervals
fn verify_flashblock_timing(
	flashblocks: &[ObservedFlashblock],
	expected_interval: Duration,
) {
	for (i, flashblock) in flashblocks.iter().enumerate() {
		let tx_count = flashblock.block.diff.transactions.len();

		if i == 0 {
			debug!(
				"Flashblock {}: {} txs, observed at {:?}",
				i, tx_count, flashblock.at,
			);
		} else {
			let prev = &flashblocks[i - 1];
			let elapsed = flashblock.at.duration_since(prev.at);
			debug!(
				"Flashblock {}: {} txs, observed at {:?}, elapsed since previous: {:?}",
				i, tx_count, flashblock.at, elapsed
			);

			// You can optionally assert timing constraints
			// Allow some tolerance for execution overhead
			let tolerance = Duration::from_millis(50);
			assert!(
				elapsed >= expected_interval.saturating_sub(tolerance),
				"Flashblock {i} produced too quickly: {elapsed:?} < expected \
				 {expected_interval:?}",
			);
		}
	}
}

/// Test: 2s block time, 0ms leeway, 250ms interval = 8 flashblocks
///
/// With no leeway time and 2000ms of available time, dividing by 250ms
/// intervals gives exactly 8 flashblocks.
#[tokio::test]
async fn flashblock_count_2000ms_block_time_0ms_leeway_250ms_interval()
-> eyre::Result<()> {
	let flashblocks = Box::pin(run_test(
		Duration::from_millis(0),
		Duration::from_millis(250),
	))
	.await?;

	verify_flashblock_timing(&flashblocks, Duration::from_millis(250));

	// 2000ms / 250ms = 8 flashblocks exactly
	assert_eq!(
		flashblocks.len(),
		8,
		"Expected 8 flashblocks with 2000ms available time and 250ms interval"
	);

	Ok(())
}

/// Test: 2s block time, 75ms leeway, 250ms interval = 8 flashblocks
///
/// With 75ms leeway, remaining time is 1925ms. Dividing by 250ms:
/// 1925 / 250 = 7 remainder 175
/// So we get 8 flashblocks total (7 at 250ms + 1 at 175ms)
#[tokio::test]
async fn flashblock_count_2000ms_block_time_75ms_leeway_250ms_interval()
-> eyre::Result<()> {
	let flashblocks = Box::pin(run_test(
		Duration::from_millis(75),
		Duration::from_millis(250),
	))
	.await?;

	verify_flashblock_timing(&flashblocks, Duration::from_millis(250));

	// 1925ms / 250ms = 7 remainder 175, so 8 flashblocks total
	assert_eq!(
		flashblocks.len(),
		8,
		"Expected 8 flashblocks with 1925ms remaining time and 250ms interval"
	);

	Ok(())
}

/// Test: 2s block time, 500ms leeway, 500ms interval = 3 flashblocks
///
/// With 500ms leeway, remaining time is 1500ms. Dividing by 500ms:
/// 1500 / 500 = 3 exactly
#[tokio::test]
async fn flashblock_count_2000ms_block_time_500ms_leeway_500ms_interval()
-> eyre::Result<()> {
	let flashblocks = Box::pin(run_test(
		Duration::from_millis(500),
		Duration::from_millis(500),
	))
	.await?;

	verify_flashblock_timing(&flashblocks, Duration::from_millis(500));

	// 1500ms / 500ms = 3 flashblocks exactly
	assert_eq!(
		flashblocks.len(),
		3,
		"Expected 3 flashblocks with 1500ms remaining time and 500ms interval"
	);

	Ok(())
}

/// Test: 2s block time, 750ms leeway, 750ms interval = 2 flashblocks
///
/// But with 750ms leeway, remaining time is 1250ms. Dividing by 750ms:
/// 1250 / 750 = 1 remainder 500
/// So we actually get 2 flashblocks total (1 at 750ms + 1 at 500ms)
#[tokio::test]
async fn flashblock_count_2000ms_block_time_750ms_leeway_750ms_interval()
-> eyre::Result<()> {
	let flashblocks = Box::pin(run_test(
		Duration::from_millis(750),
		Duration::from_millis(750),
	))
	.await?;

	verify_flashblock_timing(&flashblocks, Duration::from_millis(500));

	// 1250ms / 750ms = 1 remainder 500, so 2 flashblocks expected
	assert_eq!(
		flashblocks.len(),
		2,
		"Expected 2 flashblocks with 1250ms remaining time and 750ms interval"
	);

	Ok(())
}
