use {
	crate::{Flashblocks, bundle::FlashblocksBundle, tests::random_valid_bundle},
	jsonrpsee::core::ClientError,
	rblib::pool::BundlesApiClient,
};

macro_rules! assert_ineligible {
	($result:expr) => {
		let result = $result;
		assert!(
			result.is_err(),
			"Expected error for this bundle, got {result:?}"
		);

		let Err(ClientError::Call(error)) = result else {
			panic!("Expected Call error, got {result:?}");
		};

		assert_eq!(
			error.code(),
			jsonrpsee::types::ErrorCode::InvalidParams.code()
		);

		assert_eq!(error.message(), "bundle is ineligible for inclusion");
	};
}

/// Bundles that will never be eligible for inclusion in any future block
/// should be rejected by the RPC before making it to the orders pool.
#[tokio::test]
async fn max_block_number_in_past() -> eyre::Result<()> {
	let node = Flashblocks::test_node().await?;

	let block = node.next_block().await?;
	assert_eq!(block.number(), 1);

	let block = node.next_block().await?;
	assert_eq!(block.number(), 2);

	let block = node.next_block().await?;
	assert_eq!(block.number(), 3);

	let block = node.next_block().await?;
	assert_eq!(block.number(), 4);

	let mut bundle = random_valid_bundle(1);
	bundle.max_block_number = Some(2);

	let result = BundlesApiClient::<Flashblocks>::send_bundle(
		&node.rpc_client().await?,
		bundle,
	)
	.await;

	assert_ineligible!(result);

	Ok(())
}

/// This bundle should be rejected by the RPC because its
/// `max_block_number` is set to a block number that is in the past and it will
/// never be eligible for inclusion in any future block.
#[tokio::test]
async fn max_block_timestamp_in_past() -> eyre::Result<()> {
	// node at genesis, block 0
	let node = Flashblocks::test_node().await?;
	let genesis_timestamp = node.config().chain.genesis_timestamp();
	let mut bundle = random_valid_bundle(1);
	bundle.max_timestamp = Some(genesis_timestamp.saturating_sub(1));

	let result = BundlesApiClient::<Flashblocks>::send_bundle(
		&node.rpc_client().await?,
		bundle,
	)
	.await;

	assert_ineligible!(result);

	Ok(())
}

#[tokio::test]
async fn empty_bundle_rejected_by_rpc() -> eyre::Result<()> {
	let node = Flashblocks::test_node().await?;

	let empty_bundle = FlashblocksBundle::with_transactions(vec![]);
	let result = BundlesApiClient::<Flashblocks>::send_bundle(
		&node.rpc_client().await?,
		empty_bundle,
	)
	.await;

	assert_ineligible!(result);

	Ok(())
}
