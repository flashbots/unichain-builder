use rblib::{self, alloy::primitives::U256, prelude::*, test_utils::*};

mod args;
mod bundle;
mod flashblocks;
mod limits;
mod ordering;
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
