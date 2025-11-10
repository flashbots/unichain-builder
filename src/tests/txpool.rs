use {
	crate::{Flashblocks, tests::assert_has_sequencer_tx},
	rblib::{
		alloy::{
			consensus::Transaction,
			network::ReceiptResponse,
			primitives::U256,
			providers::Provider,
		},
		test_utils::*,
	},
	tracing::debug,
};

/// Ensure that user transactions send to the RPC interface of the node
/// that are not part of a bundle make their way into the block.
#[tokio::test]
async fn non_bundle_tx_included_in_block() -> eyre::Result<()> {
	let (node, _) = Flashblocks::test_node().await?;

	let txhash = *node
		.send_tx(node.build_tx().transfer().value(U256::from(1_234_000)))
		.await
		.expect("transaction should be sent successfully")
		.tx_hash();

	let block = node.next_block().await?;
	debug!("produced block: {block:#?}");

	assert_eq!(block.number(), 1);
	assert_eq!(block.tx_count(), 2); // sequencer deposit tx + 1 user tx
	assert_has_sequencer_tx!(&block);
	assert!(block.includes(txhash));

	assert_eq!(block.tx(1).unwrap().value(), U256::from(1_234_000));

	Ok(())
}

/// Ensure that a reverted transaction is reported as dropped by the RPC.
#[tokio::test]
async fn reverted_transaction_reports_dropped_status() -> eyre::Result<()> {
	let (node, _) = Flashblocks::test_node().await?;

	let ok_txhash = *node
		.send_tx(node.build_tx().transfer().value(U256::from(1_234_000)))
		.await
		.expect("transaction should be sent successfully")
		.tx_hash();

	let reverted_txhash = *node
		.send_tx(node.build_tx().reverting().value(U256::from(2_234_000)))
		.await
		.expect("transaction should be sent successfully")
		.tx_hash();

	let block = node.next_block().await?;
	debug!("produced block: {block:#?}");

	assert_eq!(block.number(), 1);
	assert_eq!(block.tx_count(), 2); // sequencer deposit tx + 1 user tx
	assert_has_sequencer_tx!(&block);
	assert!(block.includes(ok_txhash));
	assert!(!block.includes(reverted_txhash));

	let ok_receipt = node
		.provider()
		.get_transaction_receipt(ok_txhash)
		.await?
		.unwrap();

	assert!(ok_receipt.status());
	assert_eq!(ok_receipt.transaction_hash(), ok_txhash);

	let reverted_receipt = node
		.provider()
		.get_transaction_receipt(reverted_txhash)
		.await;

	tracing::info!("ok_receipt: {ok_receipt:#?}");
	tracing::info!("reverted_receipt: {reverted_receipt:#?}");

	assert!(reverted_receipt.is_ok());
	assert!(reverted_receipt.unwrap().is_none());

	Ok(())
}
