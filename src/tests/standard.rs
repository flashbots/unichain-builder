//! Smoke tests for the standard blocks builder

use {
	crate::{Flashblocks, tests::assert_has_sequencer_tx},
	rblib::{
		alloy::{
			consensus::Transaction,
			network::TransactionResponse,
			primitives::{Address, U256},
		},
		test_utils::*,
	},
	tracing::debug,
};

/// This is a smoke test that ensures that sequencer transactions are included
/// in blocks and that the block generator is functioning correctly.
#[tokio::test]
async fn chain_produces_empty_blocks() -> eyre::Result<()> {
	// builders signer is not configured, so won't produce a builder tx
	let (node, _) = Flashblocks::test_node().await?;

	for i in 1..5 {
		let block = node.next_block().await?;
		debug!("produced block: {block:#?}");

		assert_eq!(block.number(), i);
		assert_eq!(block.tx_count(), 1); // sequencer deposit tx
		assert_has_sequencer_tx!(&block);
	}

	Ok(())
}

#[tokio::test]
async fn chain_produces_blocks_with_txs() -> eyre::Result<()> {
	const BLOCKS: usize = 5;
	const TXS_PER_BLOCK: usize = 5;

	// builders signer is not configured, so won't produce a builder tx
	let (node, _) = Flashblocks::test_node().await?;

	for i in 1..=BLOCKS {
		let mut txs = Vec::with_capacity(TXS_PER_BLOCK);
		for _ in 0..TXS_PER_BLOCK {
			let tx = node
				.send_tx(node.build_tx().transfer().value(U256::from(1_234_000)))
				.await?;
			txs.push(*tx.tx_hash());
		}

		let block = node.next_block().await?;
		debug!("produced block: {block:#?}");

		assert_eq!(block.number(), i as u64);
		assert_eq!(block.tx_count(), 1 + TXS_PER_BLOCK); // sequencer deposit tx + user txs
		assert_has_sequencer_tx!(&block);
		assert!(block.includes(txs));
	}

	Ok(())
}

/// Ensure that the chain produces blocks with a builder transaction
/// when the builder signer is provided in the CLI arguments.
#[tokio::test]
async fn blocks_have_builder_tx() -> eyre::Result<()> {
	let (node, _) = Flashblocks::test_node_with_builder_signer().await?;

	let block = node.next_block().await?;
	debug!("produced block: {block:#?}");

	assert_eq!(block.number(), 1);
	assert_eq!(block.tx_count(), 9); // sequencer deposit tx + 8 builder txs for each flashblock

	let builder_tx = block.tx(1).unwrap();
	assert_eq!(builder_tx.nonce(), 0);
	assert_eq!(builder_tx.value(), U256::ZERO);
	assert_eq!(builder_tx.to(), Some(Address::ZERO));
	assert_eq!(builder_tx.input(), "Block Number: 1".as_bytes());
	assert_eq!(builder_tx.from(), FundedAccounts::signer(0).address());

	Ok(())
}
