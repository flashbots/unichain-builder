//! Smoke tests for the Flashblocks blocks builder

#![allow(clippy::large_futures)]

use {
	crate::{Flashblocks, tests::*},
	rblib::alloy::{eips::Decodable2718, network::BlockResponse},
	std::time::{Duration, SystemTime, UNIX_EPOCH},
};

#[tokio::test]
async fn empty_blocks_smoke() -> eyre::Result<()> {
	let (node, ws_addr) = Flashblocks::test_node_with_flashblocks_on().await?;
	let ws = WebSocketObserver::new(ws_addr).await?;

	for i in 1..=5 {
		let block = node.next_block().await?;

		assert_eq!(block.number(), i);
		assert_eq!(block.tx_count(), 1); // sequencer deposit tx
		assert_has_sequencer_tx!(&block);

		// there should be only one flashblock produced for an empty block
		// because an empty block will only have the sequencer deposit tx
		// and we don't produce empty flashblocks.
		let fblocks = ws.by_block_number(block.number());
		assert_eq!(fblocks.len(), 1);
	}

	assert!(ws.has_no_errors());
	assert_eq!(ws.len(), 5); // one flashblock per block

	Ok(())
}

#[tokio::test]
async fn blocks_with_txs_smoke() -> eyre::Result<()> {
	const BLOCKS: usize = 5;
	const TXS_PER_BLOCK: usize = 60;

	let (node, ws_addr) = Flashblocks::test_node_with_flashblocks_on().await?;
	let ws = WebSocketObserver::new(ws_addr).await?;

	for i in 1..=BLOCKS {
		let mut sent_txs = Vec::new();

		let block = node
			.while_next_block(async {
				for _ in 0..TXS_PER_BLOCK {
					let tx = node
						.send_tx(node.build_tx().transfer().value(U256::from(1_234_000)))
						.await?;
					tokio::task::yield_now().await;
					sent_txs.push(*tx.tx_hash());
				}

				Ok(())
			})
			.await?;

		assert_eq!(block.number(), i as u64);
		assert_has_sequencer_tx!(&block);
		assert!(!sent_txs.is_empty());
		assert!(block.tx_count() > sent_txs.len());
		assert!(block.includes(sent_txs));

		let fblocks = ws.by_block_number(block.number());

		let txhashes: Vec<_> = fblocks
			.iter()
			.flat_map(|fb| {
				fb.block.diff.transactions.iter().map(|tx| {
					types::TxEnvelope::<Flashblocks>::decode_2718(&mut &tx[..])
						.unwrap()
						.tx_hash()
				})
			})
			.collect();

		// make sure that all transactions in flashblocks actually made
		// their way to the block.
		assert!(block.includes(&txhashes));
		assert_eq!(block.tx_count() as usize, txhashes.len());

		// ensure that transactions in flashblocks appear in the same order
		// as transactions in the final block
		assert_eq!(txhashes, block.transactions().hashes().collect::<Vec<_>>());
	}

	Ok(())
}

// This test is to check that we get 8 flashblocks with 2000ms remaining time
// and 250ms flashblock interval and 75ms leeway time
#[tokio::test]
async fn flashblock_timings_2000ms_block_time_0ms_leeway_time()
-> eyre::Result<()> {
	let (node, ws_addr) =
		Flashblocks::test_node_with_flashblocks_on_and_custom_leeway_time_and_interval(
			Duration::from_millis(0),
			Duration::from_millis(250),
		)
		.await?;
	let ws = WebSocketObserver::new(ws_addr).await?;

	// Create a block at the top of the timestamp second
	// Wait until the exact top of the next second
	let now = SystemTime::now().duration_since(UNIX_EPOCH)?;
	let current_millis = now.as_millis() % 1000;
	let sleep_duration = Duration::from_millis(1000 - current_millis as u64);
	tokio::time::sleep(sleep_duration).await;

	// let block = node.next_block().await?;
	const TXS_PER_BLOCK: usize = 60;
	let mut sent_txs = Vec::new();
	let block = node
		.while_next_block(async {
			for _ in 0..TXS_PER_BLOCK {
				let tx = node
					.send_tx(node.build_tx().transfer().value(U256::from(1_234_000)))
					.await?;
				tokio::task::yield_now().await;
				sent_txs.push(*tx.tx_hash());
			}

			Ok(())
		})
		.await?;

	assert_eq!(block.number(), 1);
	assert!(!sent_txs.is_empty());
	assert_has_sequencer_tx!(&block);

	// With 2s blocktime, 250ms flashblock interval, we should get 2000 / 250 = 8
	// flashblocks
	let fblocks = ws.by_block_number(block.number());
	assert_eq!(fblocks.len(), 8);

	assert!(ws.has_no_errors());
	Ok(())
}

#[tokio::test]
// This test is to check that we get 8 flashblocks with 2000ms - 75ms = 1925ms
// remaining time. 2000ms blocktimes, 250ms flashblock interval, and 75ms leeway
// time
async fn flashblock_timings_2000ms_block_time_75ms_leeway_time()
-> eyre::Result<()> {
	let (node, ws_addr) = Flashblocks::test_node_with_flashblocks_on().await?;
	let ws = WebSocketObserver::new(ws_addr).await?;

	// Create a block at the top of the timestamp second
	// Wait until the exact top of the next second
	let now = SystemTime::now().duration_since(UNIX_EPOCH)?;
	let current_millis = now.as_millis() % 1000;
	let sleep_duration = Duration::from_millis(1000 - current_millis as u64);
	tokio::time::sleep(sleep_duration).await;

	// let block = node.next_block().await?;
	const TXS_PER_BLOCK: usize = 60;
	let mut sent_txs = Vec::new();
	let block = node
		.while_next_block(async {
			for _ in 0..TXS_PER_BLOCK {
				let tx = node
					.send_tx(node.build_tx().transfer().value(U256::from(1_234_000)))
					.await?;
				tokio::task::yield_now().await;
				sent_txs.push(*tx.tx_hash());
			}

			Ok(())
		})
		.await?;

	assert_eq!(block.number(), 1);
	assert!(!sent_txs.is_empty());
	assert_has_sequencer_tx!(&block);

	// With 2000ms blocktime, minus 75ms leeway time for 1925ms remaining, we
	// should get a first flashblock of 175ms and 7 flashblocks at 250ms intervals
	let fblocks = ws.by_block_number(block.number());
	assert_eq!(fblocks.len(), 8);

	assert!(ws.has_no_errors());
	Ok(())
}

#[tokio::test]
// This test is to check that we get 3 flashblocks with 2000ms - 500ms = 1500ms
// remaining time. 2000ms blocktimes, 500ms flashblock interval, and 500ms
// leeway time
async fn flashblock_timings_2000ms_block_time_500ms_leeway_time()
-> eyre::Result<()> {
	let (node, ws_addr) =
		Flashblocks::test_node_with_flashblocks_on_and_custom_leeway_time_and_interval(
			Duration::from_millis(500),
			Duration::from_millis(500),
		)
		.await?;
	let ws = WebSocketObserver::new(ws_addr).await?;

	// Create a block at the top of the timestamp second
	// Wait until the exact top of the next second
	let now = SystemTime::now().duration_since(UNIX_EPOCH)?;
	let current_millis = now.as_millis() % 1000;
	let sleep_duration = Duration::from_millis(1000 - current_millis as u64);
	tokio::time::sleep(sleep_duration).await;

	// let block = node.next_block().await?;
	const TXS_PER_BLOCK: usize = 60;
	let mut sent_txs = Vec::new();
	let block = node
		.while_next_block(async {
			for _ in 0..TXS_PER_BLOCK {
				let tx = node
					.send_tx(node.build_tx().transfer().value(U256::from(1_234_000)))
					.await?;
				tokio::task::yield_now().await;
				sent_txs.push(*tx.tx_hash());
			}

			Ok(())
		})
		.await?;

	assert_eq!(block.number(), 1);
	assert!(!sent_txs.is_empty());
	assert_has_sequencer_tx!(&block);

	// With 2000ms blocktime, minus 500ms leeway time for 1500ms remaining, we
	// only have space for 1500 / 500 = 3 flashblocks at 500ms intervals each
	let fblocks = ws.by_block_number(block.number());
	assert_eq!(fblocks.len(), 3);

	assert!(ws.has_no_errors());
	Ok(())
}
