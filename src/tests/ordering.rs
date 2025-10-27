use {
	crate::{platform::Flashblocks, tests::assert_has_sequencer_tx},
	itertools::Itertools,
	rblib::{
		alloy::primitives::U256,
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
	let node = Flashblocks::test_node().await?;

	let tx_tips = vec![100, 300, 200, 500, 400];
	let mut sent_txs = Vec::new();
	for (i, tip) in tx_tips.iter().enumerate() {
		let tx = node
			.send_tx(
				node
					.build_tx()
					.transfer()
					.with_funded_signer(i.try_into().unwrap())
					.value(U256::from(1_234_000))
					.max_priority_fee_per_gas(*tip),
			)
			.await?;
		sent_txs.push(*tx.tx_hash());
	}

	// We need to wait to build the block
	tokio::time::sleep(Duration::from_millis(100)).await;

	let sorted_sent_txs: Vec<_> = tx_tips
		.into_iter()
		.zip(sent_txs)
		.inspect(|(tip, hash)| println!("tip: {tip}, hash: {hash}"))
		.sorted_by_key(|tuple| tuple.0)
		.rev()
		.map(|(_tip, hash)| hash)
		.collect();

	let block = node.next_block().await?;
	assert_eq!(block.number(), 1);
	assert_has_sequencer_tx!(&block);
	assert_eq!(block.tx_count(), 6); // sequencer deposit tx + the 5 we sent

	let hashes: Vec<_> = block
		.transactions
		.into_transactions()
		.map(|tx| tx.inner.inner.tx_hash())
		.collect();

	assert_eq!(sorted_sent_txs, hashes[1..]);

	Ok(())
}
