//! Tests for revert protection in `Flashblocks` bundles.
//!
//! Nomenclature (see `rblib::platform::ext::BundleExt` for more details):
//!
//! - fallible: transactions that are allowed to fail without affecting the
//!   bundle validity when included in the payload.
//! - optional: transactions that can be removed from the bundle without
//!   affecting the bundle validity when included in the payload.
//! - required: transactions that are required to be present for the bundle to
//!   be valid. This includes also transactions that may be allowed to fail, but
//!   must be present.
//! - critical: transactions that must be included and may not fail for the
//!   bundle to be valid. This is the default mode for transactions in a bundle.

use {
	crate::{Flashblocks, tests::*},
	tracing::debug,
};

#[tokio::test]
async fn critical_reverted_tx_not_included() -> eyre::Result<()> {
	let (node, _) = Flashblocks::test_node().await?;

	let bundle_without_reverts = random_valid_bundle(1);
	let bundle_with_reverts = random_bundle_with_reverts(0, 1);

	BundlesApiClient::<Flashblocks>::send_bundle(
		&node.rpc_client().await?,
		bundle_without_reverts.clone(),
	)
	.await?;

	BundlesApiClient::<Flashblocks>::send_bundle(
		&node.rpc_client().await?,
		bundle_with_reverts.clone(),
	)
	.await?;

	let block = node.next_block().await?;
	debug!("Built block: {block:#?}");

	assert_eq!(block.number(), 1);
	assert_eq!(block.tx_count(), 2); // sequencer deposit tx + 1 valid bundle tx

	assert_has_sequencer_tx!(&block);

	let tx = block.tx(1).unwrap();
	assert_eq!(
		tx.tx_hash(),
		bundle_without_reverts.transactions()[0].tx_hash()
	);
	Ok(())
}

#[tokio::test]
async fn fallible_reverted_included() -> eyre::Result<()> {
	let (node, _) = Flashblocks::test_node().await?;

	// create a bundle with one valid tx
	let bundle_without_reverts = random_valid_bundle(1);

	// create a bundle with one valid and one reverting tx
	let mut bundle_with_reverts = random_bundle_with_reverts(1, 1);

	let bundle1_txs = bundle_without_reverts.transactions().to_vec();
	let bundle2_txs = bundle_with_reverts.transactions().to_vec();

	// mark the second transaction (reverting) in the bundle as allowed to revert
	bundle_with_reverts.reverting_tx_hashes = vec![bundle2_txs[1].tx_hash()];

	BundlesApiClient::<Flashblocks>::send_bundle(
		&node.rpc_client().await?,
		bundle_without_reverts.clone(),
	)
	.await?;

	BundlesApiClient::<Flashblocks>::send_bundle(
		&node.rpc_client().await?,
		bundle_with_reverts.clone(),
	)
	.await?;

	let block = node.next_block().await?;
	debug!("Built block: {block:#?}");

	assert_eq!(block.number(), 1);
	assert_eq!(block.tx_count(), 4);

	assert_has_sequencer_tx!(&block);
	assert!(block.includes(bundle1_txs[0].tx_hash()));
	assert!(block.includes(bundle2_txs[0].tx_hash()));
	assert!(block.includes(bundle2_txs[1].tx_hash()));

	Ok(())
}

#[tokio::test]
async fn fallible_optional_reverted_not_included() -> eyre::Result<()> {
	let (node, _) = Flashblocks::test_node().await?;

	// create a bundle with one valid and one reverting tx
	let mut bundle_with_reverts = random_bundle_with_reverts(1, 1);
	let txs = bundle_with_reverts.transactions().to_vec();

	// mark the second transaction (reverting) in the bundle as allowed to revert
	// and optional (i.e. it can be removed from the bundle)
	bundle_with_reverts.reverting_tx_hashes = vec![txs[1].tx_hash()];
	bundle_with_reverts.dropping_tx_hashes = vec![txs[1].tx_hash()];

	BundlesApiClient::<Flashblocks>::send_bundle(
		&node.rpc_client().await?,
		bundle_with_reverts.clone(),
	)
	.await?;

	let block = node.next_block().await?;
	debug!("Built block: {block:#?}");

	assert_eq!(block.number(), 1);
	assert_eq!(block.tx_count(), 2);

	assert_has_sequencer_tx!(&block);
	assert!(block.includes(txs[0].tx_hash()));

	Ok(())
}

#[tokio::test]
async fn when_disabled_reverted_txs_are_included() -> eyre::Result<()> {
	let (node, _) = Flashblocks::test_node_with_revert_protection_off().await?;

	// create a bundle with one valid and one reverting tx
	let mut bundle_with_reverts = random_bundle_with_reverts(1, 1);
	let txs = bundle_with_reverts.transactions().to_vec();

	// mark the second transaction (reverting) in the bundle as allowed to revert
	// and optional (i.e. it can be removed from the bundle)
	bundle_with_reverts.reverting_tx_hashes = vec![txs[1].tx_hash()];
	bundle_with_reverts.dropping_tx_hashes = vec![txs[1].tx_hash()];

	BundlesApiClient::<Flashblocks>::send_bundle(
		&node.rpc_client().await?,
		bundle_with_reverts.clone(),
	)
	.await?;

	let block = node.next_block().await?;
	debug!("Built block: {block:#?}");

	assert_eq!(block.number(), 1);
	assert_eq!(block.tx_count(), 3);

	assert_has_sequencer_tx!(&block);
	assert!(block.includes(txs[0].tx_hash()));
	assert!(block.includes(txs[1].tx_hash()));

	Ok(())
}
