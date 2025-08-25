//! Smoke tests for the Flashblocks blocks builder

#![allow(clippy::large_futures)]

use {
	crate::{FlashBlocks, tests::*},
	rblib::alloy::{eips::Decodable2718, network::BlockResponse},
};

#[tokio::test]
async fn empty_blocks_smoke() -> eyre::Result<()> {
	let (node, ws_addr) = FlashBlocks::test_node_with_flashblocks_on().await?;
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

	let (node, ws_addr) = FlashBlocks::test_node_with_flashblocks_on().await?;
	let ws = WebSocketObserver::new(ws_addr).await?;

	for i in 1..=BLOCKS {
		let mut send_txs = Vec::new();

		let block = node
			.while_next_block(async {
				loop {
					let tx = node
						.send_tx(node.build_tx().transfer().value(U256::from(1_234_000)))
						.await?;
					tokio::task::yield_now().await;
					send_txs.push(*tx.tx_hash());
				}
			})
			.await?;

		assert_eq!(block.number(), i as u64);
		assert_has_sequencer_tx!(&block);
		assert!(block.tx_count() > send_txs.len());
		assert!(block.includes(send_txs));

		let fblocks = ws.by_block_number(block.number());

		let txhashes: Vec<_> = fblocks
			.iter()
			.flat_map(|fb| {
				fb.block.diff.transactions.iter().map(|tx| {
					types::TxEnvelope::<FlashBlocks>::decode_2718(&mut &tx[..])
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
