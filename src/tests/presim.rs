//! Tests for top-of-block pre-simulation.
//!
//! These tests verify that `PreSimulateOrders` correctly filters
//! reverting transactions from the pool before the flashblock building
//! loop begins. The key test is with revert protection disabled, which
//! proves that presim alone is sufficient to filter reverting txs.

use {
	crate::{Flashblocks, tests::*},
	rblib::alloy::{
		consensus::Transaction,
		primitives::U256,
		providers::Provider,
	},
	tracing::debug,
};

/// With presim enabled and revert protection disabled, reverting
/// transactions should still be excluded from the block because
/// presim removes them from the pool before building starts.
#[tokio::test]
async fn presim_filters_reverting_tx_without_revert_protection()
-> eyre::Result<()> {
	let (node, _) = Flashblocks::test_node_with_presim_only().await?;

	let ok_txhash = *node
		.send_tx(node.build_tx().transfer().value(U256::from(1_000_000)))
		.await
		.expect("valid tx should be sent")
		.tx_hash();

	let reverted_txhash = *node
		.send_tx(node.build_tx().reverting().value(U256::from(2_000_000)))
		.await
		.expect("reverting tx should be sent")
		.tx_hash();

	let block = node.next_block().await?;
	debug!("Built block: {block:#?}");

	assert_eq!(block.number(), 1);
	assert_has_sequencer_tx!(&block);
	assert!(block.includes(ok_txhash));
	assert!(
		!block.includes(reverted_txhash),
		"reverting tx should be filtered by presim"
	);

	Ok(())
}

/// With presim enabled alongside revert protection, reverting txs
/// should still be excluded (belt and suspenders).
#[tokio::test]
async fn presim_and_revert_protection_both_active() -> eyre::Result<()> {
	let (node, _) = Flashblocks::test_node().await?;

	let ok_txhash = *node
		.send_tx(node.build_tx().transfer().value(U256::from(1_000_000)))
		.await
		.expect("valid tx should be sent")
		.tx_hash();

	let reverted_txhash = *node
		.send_tx(node.build_tx().reverting().value(U256::from(2_000_000)))
		.await
		.expect("reverting tx should be sent")
		.tx_hash();

	let block = node.next_block().await?;
	debug!("Built block: {block:#?}");

	assert_eq!(block.number(), 1);
	assert_has_sequencer_tx!(&block);
	assert!(block.includes(ok_txhash));
	assert!(!block.includes(reverted_txhash));

	Ok(())
}

/// Non-reverting transactions should always be included regardless
/// of presim being active.
#[tokio::test]
async fn presim_keeps_valid_transactions() -> eyre::Result<()> {
	let (node, _) = Flashblocks::test_node_with_presim_only().await?;

	let tx1 = *node
		.send_tx(node.build_tx().transfer().value(U256::from(100)))
		.await
		.expect("tx should be sent")
		.tx_hash();

	let tx2 = *node
		.send_tx(node.build_tx().transfer().value(U256::from(200)))
		.await
		.expect("tx should be sent")
		.tx_hash();

	let block = node.next_block().await?;
	debug!("Built block: {block:#?}");

	assert_eq!(block.number(), 1);
	assert_eq!(block.tx_count(), 3); // sequencer + 2 user txs
	assert_has_sequencer_tx!(&block);
	assert!(block.includes(tx1));
	assert!(block.includes(tx2));

	Ok(())
}
