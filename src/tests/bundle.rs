use {
	crate::{
		Flashblocks,
		args::BuilderArgs,
		bundle::FlashblocksBundle,
		tests::{Harness, assert_has_sequencer_tx},
	},
	jsonrpsee::core::ClientError,
	macros::unichain_test,
	rand::{Rng, rng},
	rblib::{
		alloy::{
			consensus::Transaction,
			network::{TransactionBuilder, TxSignerSync},
			optimism::{consensus::OpTxEnvelope, rpc_types::OpTransactionRequest},
			primitives::{Address, U256},
			signers::local::PrivateKeySigner,
		},
		pool::{BundleResult, BundlesApiClient},
		prelude::*,
		reth::{
			core::primitives::SignedTransaction,
			optimism::primitives::OpTransactionSigned,
			primitives::Recovered,
		},
		test_utils::*,
	},
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

pub fn transfer_tx(
	signer: &PrivateKeySigner,
	nonce: u64,
	value: U256,
) -> Recovered<OpTxEnvelope> {
	let mut tx = OpTransactionRequest::default()
		.with_nonce(nonce)
		.with_to(Address::random())
		.value(value)
		.with_gas_price(1_000_000_000)
		.with_gas_limit(21_000)
		.with_max_priority_fee_per_gas(1_000_000)
		.with_max_fee_per_gas(2_000_000)
		.build_unsigned()
		.expect("valid transaction request");

	let sig = signer
		.sign_transaction_sync(&mut tx)
		.expect("signing should succeed");

	OpTransactionSigned::new_unhashed(tx, sig) //
		.with_signer(signer.address())
}

pub fn transfer_tx_compact(
	signer: u32,
	nonce: u64,
	value: u64,
) -> Recovered<OpTxEnvelope> {
	let signer = FundedAccounts::signer(signer);
	transfer_tx(&signer, nonce, U256::from(value))
}

/// Will generate a random bundle with a given number of valid transactions.
/// Transaction will be sending `1_000_000` + index wei to a random address.
pub fn random_valid_bundle(tx_count: usize) -> FlashblocksBundle {
	random_bundle_with_reverts(tx_count, 0)
}

/// Non-reverting transactions amount value is `1_000_000` + index wei.
/// Reverting transactions amount value is `2_000_000` + index wei.
pub fn random_bundle_with_reverts(
	non_reverting: usize,
	reverting: usize,
) -> FlashblocksBundle {
	const SIGNERS_COUNT: usize = FundedAccounts::len();
	let mut txs = Vec::new();
	let mut nonces = [0u64; SIGNERS_COUNT];

	// first valid transactions
	for i in 0..non_reverting {
		let signer = rng().random_range(0..SIGNERS_COUNT);
		let nonce = nonces[signer];
		let amount = 1_000_000 + i as u64;

		#[expect(clippy::cast_possible_truncation)]
		let tx = transfer_tx_compact(signer as u32, nonce, amount);
		txs.push(tx);
		nonces[signer] += 1;
	}

	// then reverting transactions
	for i in 0..reverting {
		let signer = rng().random_range(0..SIGNERS_COUNT);
		let nonce = nonces[signer];
		nonces[signer] += 1;

		#[expect(clippy::cast_possible_truncation)]
		let signer = FundedAccounts::signer(signer as u32);
		let amount = 2_000_000 + i as u64;
		let mut tx = OpTransactionRequest::default()
			.with_nonce(nonce)
			.value(U256::from(amount))
			.reverting()
			.with_gas_price(1_000_000_000)
			.with_gas_limit(100_000)
			.with_max_priority_fee_per_gas(1_000_000)
			.with_max_fee_per_gas(2_000_000)
			.build_unsigned()
			.expect("valid transaction request");

		let sig = signer
			.sign_transaction_sync(&mut tx)
			.expect("signing should succeed");

		let tx = OpTransactionSigned::new_unhashed(tx, sig) //
			.with_signer(signer.address());
		txs.push(tx);
	}

	FlashblocksBundle::with_transactions(txs)
}

#[unichain_test]
async fn empty_bundle_rejected(harness: Harness) -> eyre::Result<()> {
	let node = harness.node();

	let empty_bundle = FlashblocksBundle::with_transactions(vec![]);
	let result = BundlesApiClient::<Flashblocks>::send_bundle(
		&node.rpc_client().await?,
		empty_bundle,
	)
	.await;

	assert_ineligible!(result);

	Ok(())
}

/// This bundle should be rejected by because we only support bundles with one
/// transaction
#[unichain_test]
async fn bundle_with_two_txs_rejected(harness: Harness) -> eyre::Result<()> {
	let node = harness.node();

	let bundle_with_two_txs = random_valid_bundle(2);

	let result = BundlesApiClient::<Flashblocks>::send_bundle(
		&node.rpc_client().await?,
		bundle_with_two_txs,
	)
	.await;

	assert_ineligible!(result);

	Ok(())
}

#[unichain_test]
async fn valid_tx_included(harness: Harness) -> eyre::Result<()> {
	let node = harness.node();

	let bundle_with_one_tx = random_valid_bundle(1);
	let bundle_hash = bundle_with_one_tx.hash();

	let result = BundlesApiClient::<Flashblocks>::send_bundle(
		&node.rpc_client().await?,
		bundle_with_one_tx,
	)
	.await?;

	assert_eq!(result, BundleResult { bundle_hash });

	let block = node.next_block().await?;

	assert_eq!(block.number(), 1);
	assert_has_sequencer_tx!(&block);
	assert_eq!(block.tx_count(), 2); // sequencer deposit tx + 1 bundle tx
	assert_eq!(block.tx(1).unwrap().value(), U256::from(1_000_000));

	Ok(())
}

#[unichain_test]
async fn reverted_tx_not_included(harness: Harness) -> eyre::Result<()> {
	let node = harness.node();

	let bundle_with_reverts = random_bundle_with_reverts(0, 1);

	BundlesApiClient::<Flashblocks>::send_bundle(
		&node.rpc_client().await?,
		bundle_with_reverts.clone(),
	)
	.await?;

	let block = node.next_block().await?;

	assert_eq!(block.number(), 1);
	assert_eq!(block.tx_count(), 1); // only sequencer deposit tx

	assert_has_sequencer_tx!(&block);

	Ok(())
}

/// Bundles that will never be eligible for inclusion in any future block
/// should be rejected by the RPC before making it to the orders pool.
#[unichain_test]
async fn max_block_number_in_past(harness: Harness) -> eyre::Result<()> {
	let node = harness.node();

	let block = node.next_block().await?;
	assert_eq!(block.number(), 1);

	let block = node.next_block().await?;
	assert_eq!(block.number(), 2);

	let mut bundle = random_valid_bundle(1);
	bundle.max_block_number = Some(1);

	let result = BundlesApiClient::<Flashblocks>::send_bundle(
		&node.rpc_client().await?,
		bundle,
	)
	.await;

	assert_ineligible!(result);

	Ok(())
}

/// This bundle should be rejected because its `max_timestamp` is in the past
/// and it will never be eligible for inclusion in any future block.
#[unichain_test]
async fn max_block_timestamp_in_past(harness: Harness) -> eyre::Result<()> {
	// node at genesis, block 0
	let node = harness.node();
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

#[unichain_test]
async fn min_block_greater_than_max_block(
	harness: Harness,
) -> eyre::Result<()> {
	// node at genesis, block 0
	let node = harness.node();
	let mut bundle = random_valid_bundle(1);
	bundle.min_block_number = Some(2);
	bundle.max_block_number = Some(1);

	let result = BundlesApiClient::<Flashblocks>::send_bundle(
		&node.rpc_client().await?,
		bundle,
	)
	.await;

	assert_ineligible!(result);

	Ok(())
}

/// Test that a bundle with the `min_block_number` param set to a future block
/// isn't included until that block.
#[unichain_test]
async fn min_block_number_in_future(harness: Harness) -> eyre::Result<()> {
	let node = harness.node();

	let mut bundle_with_one_tx = random_valid_bundle(1);
	bundle_with_one_tx.min_block_number = Some(2);
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
	assert_eq!(block.tx_count(), 2); // sequencer tx + bundle tx
	assert_has_sequencer_tx!(&block);

	assert!(block.includes(txhash));

	Ok(())
}

#[unichain_test(args = BuilderArgs {
    revert_protection: false,
    ..Default::default()
})]
async fn when_disabled_reverted_txs_are_included(
	harness: Harness,
) -> eyre::Result<()> {
	let node = harness.node();

	// create a bundle with one valid and one reverting tx
	let mut bundle_with_reverts = random_bundle_with_reverts(0, 1);
	let txs = bundle_with_reverts.transactions().to_vec();

	// mark the transaction (reverting) in the bundle as allowed to revert
	// and optional (i.e. it can be removed from the bundle)
	bundle_with_reverts.reverting_tx_hashes = vec![txs[0].tx_hash()];
	bundle_with_reverts.dropping_tx_hashes = vec![txs[0].tx_hash()];

	BundlesApiClient::<Flashblocks>::send_bundle(
		&node.rpc_client().await?,
		bundle_with_reverts.clone(),
	)
	.await?;

	let block = node.next_block().await?;

	assert_eq!(block.number(), 1);
	assert_eq!(block.tx_count(), 2);

	assert_has_sequencer_tx!(&block);
	assert!(block.includes(txs[0].tx_hash()));

	Ok(())
}
