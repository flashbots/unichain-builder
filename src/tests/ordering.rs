use {
	crate::{platform::Flashblocks, tests::assert_has_sequencer_tx},
	rblib::{
		alloy::{consensus::Transaction, primitives::U256},
		test_utils::{BlockResponseExt, TransactionRequestExt},
	},
	std::time::Duration,
};

/// This test ensures that the transactions are ordered by fee priority in the
/// block. This version of the test is only applicable to the standard builder
/// because in flashblocks the transaction order is commited by the block after
/// each flashblock is produced, so the order is only going to hold within one
/// flashblock, but not the entire block.
#[tokio::test]
async fn txs_ordered_by_priority_fee() -> eyre::Result<()> {
	let (node, _) = Flashblocks::test_node().await?;

	let tx_tips = [100, 300, 200, 500, 400];
	for (i, tip) in tx_tips.iter().enumerate() {
		let _tx = node
			.send_tx(
				node
					.build_tx()
					.transfer()
					.with_funded_signer(i.try_into().unwrap())
					.value(U256::from(1_234_000))
					.max_priority_fee_per_gas(*tip),
			)
			.await?;
	}

	// We need to wait to build the block
	tokio::time::sleep(Duration::from_millis(100)).await;

	let block = node.next_block().await?;
	assert_eq!(block.number(), 1);
	assert_has_sequencer_tx!(&block);
	assert_eq!(block.tx_count(), 6); // sequencer deposit tx + the 5 we sent

	let base_fee = block.header.base_fee_per_gas.unwrap();

	let tx_tips: Vec<_> = block
		.transactions
		.into_transactions()
		.skip(1) // skip the deposit transaction
		.map(|tx| tx.effective_tip_per_gas(base_fee))
		.rev() // we want to check descending order
		.collect();

	assert!(
		tx_tips.is_sorted(),
		"Transactions not ordered by fee priority"
	);

	Ok(())
}
