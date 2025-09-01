use {
	crate::{args::BuilderArgs, bundle::FlashblocksBundle},
	rand::{Rng, rng},
	rblib::{
		alloy::{
			network::{TransactionBuilder, TransactionResponse, TxSignerSync},
			optimism::{consensus::OpTxEnvelope, rpc_types::OpTransactionRequest},
			primitives::{Address, U256},
			signers::local::PrivateKeySigner,
		},
		pool::*,
		prelude::*,
		reth::{
			core::primitives::SignedTransaction,
			optimism::primitives::OpTransactionSigned,
			primitives::Recovered,
		},
		test_utils::*,
	},
};

mod bundle;
mod flashblocks;
mod ordering;
mod revert;
mod rpc;
mod standard;
mod txpool;
mod utils;

pub use utils::*;

macro_rules! assert_is_sequencer_tx {
	($tx:expr) => {
		assert_eq!(
			rblib::alloy::consensus::Typed2718::ty($tx),
			rblib::alloy::optimism::consensus::DEPOSIT_TX_TYPE_ID,
			"Optimism sequencer transaction should be a deposit tx"
		);
	};
}

macro_rules! assert_has_sequencer_tx {
	($block:expr) => {
		assert!(
			rblib::test_utils::BlockResponseExt::tx_count($block) >= 1,
			"Block should have one transaction"
		);
		let sequencer_tx =
			rblib::test_utils::BlockResponseExt::tx($block, 0).unwrap();
		$crate::tests::assert_is_sequencer_tx!(sequencer_tx);
	};
}

pub(crate) use {assert_has_sequencer_tx, assert_is_sequencer_tx};

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
