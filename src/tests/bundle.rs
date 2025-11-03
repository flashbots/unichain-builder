use {
	crate::{
		Flashblocks,
		tests::{assert_has_sequencer_tx, random_valid_bundle},
	},
	rblib::{
		alloy::{consensus::Transaction, primitives::U256},
		pool::{BundleResult, BundlesApiClient},
		prelude::*,
		test_utils::*,
	},
	tracing::debug,
};

#[tokio::test]
async fn one_valid_tx_included() -> eyre::Result<()> {
	let (node, _) = Flashblocks::test_node().await?;

	let bundle_with_one_tx = random_valid_bundle(1);
	let bundle_hash = bundle_with_one_tx.hash();

	let result = BundlesApiClient::<Flashblocks>::send_bundle(
		&node.rpc_client().await?,
		bundle_with_one_tx,
	)
	.await?;

	assert_eq!(result, BundleResult { bundle_hash });

	let block = node.next_block().await?;
	debug!("Built block: {block:#?}");

	assert_eq!(block.number(), 1);
	assert_has_sequencer_tx!(&block);
	assert_eq!(block.tx_count(), 2); // sequencer deposit tx + 1 bundle tx
	assert_eq!(block.tx(1).unwrap().value(), U256::from(1_000_000));

	Ok(())
}

#[tokio::test]
async fn two_valid_txs_included() -> eyre::Result<()> {
	let (node, _) = Flashblocks::test_node().await?;

	let bundle_with_two_txs = random_valid_bundle(2);
	let bundle_hash = bundle_with_two_txs.hash();

	let result = BundlesApiClient::<Flashblocks>::send_bundle(
		&node.rpc_client().await?,
		bundle_with_two_txs,
	)
	.await?;

	assert_eq!(result, BundleResult { bundle_hash });

	let block = node.next_block().await?;
	debug!("Built block: {block:#?}");

	assert_eq!(block.number(), 1);
	assert_has_sequencer_tx!(&block);
	assert_eq!(block.tx_count(), 3); // sequencer deposit tx + 2 bundle txs
	assert_eq!(block.tx(1).unwrap().value(), U256::from(1_000_000));
	assert_eq!(block.tx(2).unwrap().value(), U256::from(1_000_001));

	Ok(())
}

#[tokio::test]
async fn min_block_timestamp_constraint() -> eyre::Result<()> {
	let (node, _) = Flashblocks::test_node().await?;

	let mut bundle_with_one_tx = random_valid_bundle(1);
	bundle_with_one_tx.min_block_number = Some(3);
	let bundle_hash = bundle_with_one_tx.hash();
	let txhash = bundle_with_one_tx.transactions()[0].tx_hash();

	let result = BundlesApiClient::<Flashblocks>::send_bundle(
		&node.rpc_client().await?,
		bundle_with_one_tx,
	)
	.await?;

	assert_eq!(result, BundleResult { bundle_hash });

	let block = node.next_block().await?; // block 1
	assert_eq!(block.number(), 1);
	assert_eq!(block.tx_count(), 1); // only sequencer tx
	assert_has_sequencer_tx!(&block);

	let block = node.next_block().await?; // block 2
	assert_eq!(block.number(), 2);
	assert_eq!(block.tx_count(), 1); // only sequencer tx
	assert_has_sequencer_tx!(&block);

	let block = node.next_block().await?; // block 3
	assert_eq!(block.number(), 3);
	assert_eq!(block.tx_count(), 2); // sequencer tx + 1 bundle tx
	assert_has_sequencer_tx!(&block);

	assert!(block.includes(txhash));

	Ok(())
}
