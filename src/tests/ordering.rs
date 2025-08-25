/// This test ensures that the transactions are ordered by fee priority in the
/// block. This version of the test is only applicable to the standard builder
/// because in flashblocks the transaction order is commited by the block after
/// each flashblock is produced, so the order is only going to hold within one
/// flashblock, but not the entire block.
#[tokio::test]
async fn txs_ordered_by_priority_fee() -> eyre::Result<()> {
	todo!()
}
