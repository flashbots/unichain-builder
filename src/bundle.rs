//! Flashblocks bundle
//!
//! This module defines the bundle type that is recognized by the Flashblocks
//! builder. This bundle type is:
//!
//! - The bundle validation logic used by the builder to determine if a bundle
//!   is valid and should be included in a block.
//!
//! - The `eth_sendBundle` input parameters and their validation.

use {
	crate::platform::Flashblocks,
	core::convert::Infallible,
	rblib::{
		alloy::{
			consensus::BlockHeader,
			eips::Decodable2718,
			primitives::{B256, Bytes, Keccak256, TxHash},
		},
		prelude::*,
		reth::{
			core::primitives::SignerRecoverable,
			primitives::{Recovered, SealedHeader},
		},
	},
	serde::{Deserialize, Deserializer, Serialize, Serializer},
};

/// Represents a bundle of transactions.
///
/// This type is received from the `eth_sendBundle` RPC method.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct FlashblocksBundle {
	/// The list of transactions in the bundle.
	///
	/// Notes:
	///  - The transactions are EIP-2718 encoded when serialized.
	///  - Bundles must contain at least one transaction.
	#[serde(with = "encoded_2718")]
	pub txs: Vec<Recovered<types::Transaction<Flashblocks>>>,

	/// The list of transaction hashes in this bundle that are allowed to revert.
	#[serde(default, skip_serializing_if = "Vec::is_empty")]
	pub reverting_tx_hashes: Vec<B256>,

	/// The list of transaction hashes in this bundle that are allowed to be
	/// removed from the bundle.
	#[serde(default, skip_serializing_if = "Vec::is_empty")]
	pub dropping_tx_hashes: Vec<TxHash>,

	#[serde(
		default,
		with = "alloy_serde::quantity::opt",
		skip_serializing_if = "Option::is_none"
	)]
	pub min_block_number: Option<u64>,

	/// (Optional) Transactions from this bundle should never be included in
	/// blocks with a block number higher than this value.
	#[serde(
		default,
		with = "alloy_serde::quantity::opt",
		skip_serializing_if = "Option::is_none"
	)]
	pub max_block_number: Option<u64>,

	/// (Optional) Specifies the minimum timestamp after which transactions in
	/// this bundle may be included in a block.
	///
	/// Note: Not recommended because this is subject to the builder node clock.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub min_timestamp: Option<u64>,

	/// (Optional) Specifies the maximum timestamp after which transactions in
	/// this bundle may not be included in a block.
	///
	/// Note: Not recommended because this is subject to the builder node clock.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub max_timestamp: Option<u64>,

	/// Minimum flashblock number at which this bundle can be included.
	///
	/// Flashblocks are preconfirmations that are built incrementally. This
	/// field along with `maxFlashblockNumber` allows bundles to be scheduled
	/// for more precise execution.
	#[serde(
		default,
		with = "alloy_serde::quantity::opt",
		skip_serializing_if = "Option::is_none"
	)]
	pub min_flashblock_number: Option<u64>,

	/// Maximum flashblock number at which this bundle can be included.
	///
	/// Similar to `minFlashblockNumber`, this sets an upper bound on which
	/// flashblocks can include this bundle.
	#[serde(
		default,
		with = "alloy_serde::quantity::opt",
		skip_serializing_if = "Option::is_none"
	)]
	pub max_flashblock_number: Option<u64>,
}

impl FlashblocksBundle {
	#[allow(dead_code)]
	pub fn with_transactions(
		txs: Vec<Recovered<types::Transaction<Flashblocks>>>,
	) -> Self {
		Self {
			txs,
			reverting_tx_hashes: Vec::new(),
			dropping_tx_hashes: Vec::new(),
			min_block_number: None,
			max_block_number: None,
			min_timestamp: None,
			max_timestamp: None,
			min_flashblock_number: None,
			max_flashblock_number: None,
		}
	}
}

/// Implements rblib Bundle semantics for the `FlashblocksBundle` type.
impl Bundle<Flashblocks> for FlashblocksBundle {
	type PostExecutionError = Infallible;

	fn transactions(&self) -> &[Recovered<types::Transaction<Flashblocks>>] {
		&self.txs
	}

	fn without_transaction(self, tx: TxHash) -> Self {
		let mut bundle = self;

		bundle.txs.retain(|t| t.tx_hash() != tx);

		bundle
			.reverting_tx_hashes
			.retain(|&reverting_tx| reverting_tx != tx);

		bundle
			.dropping_tx_hashes
			.retain(|&dropping_tx| dropping_tx != tx);

		bundle
	}

	/// Tests the eligibility of the bundle for inclusion in a block before
	/// executing any of its transactions.
	fn is_eligible(&self, block: &BlockContext<Flashblocks>) -> Eligibility {
		if self.txs.is_empty() {
			// empty bundles are never eligible
			return Eligibility::PermanentlyIneligible;
		}

		if self
			.max_block_number
			.is_some_and(|max_bn| max_bn < block.number())
		{
			// this bundle will never be eligible for inclusion anymore
			return Eligibility::PermanentlyIneligible;
		}

		if self
			.min_block_number
			.is_some_and(|min_bn| min_bn > block.number())
		{
			// this bundle is not eligible yet
			return Eligibility::TemporarilyIneligible;
		}

		if self
			.max_timestamp
			.is_some_and(|max_ts| max_ts < block.timestamp())
		{
			// this bundle will never be eligible for inclusion anymore
			return Eligibility::PermanentlyIneligible;
		}

		if self
			.min_timestamp
			.is_some_and(|min_ts| min_ts < block.timestamp())
		{
			// this bundle is not eligible yet
			return Eligibility::TemporarilyIneligible;
		}

		Eligibility::Eligible
	}

	/// Tests the ineligibility of the bundle for inclusion in a block given some
	/// recent committed block header. This is used mostly to drop bundles at the
	/// RPC level before they are sent to the order pool.
	///
	/// Returns true only when the bundle is definitively and irrevocably
	/// ineligible (no false positives). It may return false for bundles that are
	/// already permanently ineligible but not provable with the available header
	/// data (possible false negatives).
	fn is_permanently_ineligible(
		&self,
		block: &SealedHeader<types::Header<Flashblocks>>,
	) -> bool {
		if self.transactions().is_empty() {
			// empty bundles are never eligible
			return true;
		}

		if self
			.max_block_number
			.is_some_and(|max_bn| max_bn < block.number())
		{
			return true;
		}

		if self
			.max_timestamp
			.is_some_and(|max_ts| max_ts < block.timestamp())
		{
			return true;
		}

		false
	}

	/// Returns true if the bundle will be valid if a transaction with the given
	/// hash reverts. Otherwise, returns false and it signals that this
	/// transaction must have a successful (non-revert and non-fail) for the
	/// bundle to be eligible for inclusion in a block.
	fn is_allowed_to_fail(&self, tx: &TxHash) -> bool {
		self.reverting_tx_hashes.contains(tx)
	}

	/// Returns true if the bundle will be valid if a transaction with the given
	/// hash is removed from this bundle. Otherwise, returns false and it signals
	/// that this transaction might not be removed from the bundle e.g. during
	/// revert protection.
	fn is_optional(&self, tx: &TxHash) -> bool {
		self.dropping_tx_hashes.contains(tx)
	}

	fn hash(&self) -> B256 {
		let mut hasher = Keccak256::default();

		for tx in &self.txs {
			hasher.update(tx.tx_hash());
		}

		for tx in &self.reverting_tx_hashes {
			hasher.update(tx);
		}

		for tx in &self.dropping_tx_hashes {
			hasher.update(tx);
		}

		if let Some(min_bn) = self.min_block_number {
			hasher.update(min_bn.to_be_bytes());
		}
		if let Some(max_bn) = self.max_block_number {
			hasher.update(max_bn.to_be_bytes());
		}
		if let Some(min_ts) = self.min_timestamp {
			hasher.update(min_ts.to_be_bytes());
		}
		if let Some(max_ts) = self.max_timestamp {
			hasher.update(max_ts.to_be_bytes());
		}

		hasher.finalize()
	}
}

/// Implements the encoding and decoding of transactions to and from EIP-2718
/// hex bytes.
mod encoded_2718 {
	use {super::*, rblib::alloy::eips::Encodable2718};

	type TxType = Recovered<types::Transaction<Flashblocks>>;

	pub fn serialize<S>(
		txs: &Vec<TxType>,
		serializer: S,
	) -> Result<S::Ok, S::Error>
	where
		S: Serializer,
	{
		let encoded: Vec<Bytes> =
			txs.iter().map(|tx| tx.encoded_2718().into()).collect();
		encoded.serialize(serializer)
	}

	pub fn deserialize<'de, D>(deserializer: D) -> Result<Vec<TxType>, D::Error>
	where
		D: Deserializer<'de>,
	{
		let encoded = Vec::<Bytes>::deserialize(deserializer)?;

		let txs = encoded
			.into_iter()
			.map(|tx| types::Transaction::<Flashblocks>::decode_2718(&mut &tx[..]))
			.collect::<Result<Vec<_>, _>>()
			.map_err(serde::de::Error::custom)?;

		let recovered = txs
			.into_iter()
			.map(SignerRecoverable::try_into_recovered)
			.collect::<Result<Vec<_>, _>>()
			.map_err(serde::de::Error::custom)?;

		Ok(recovered)
	}
}
