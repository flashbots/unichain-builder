use {
	crate::{Flashblocks, limits::FlashblockLimits},
	core::time::Duration,
	rblib::{
		alloy::primitives::{Address, B256},
		prelude::*,
		reth::{
			optimism::{
				chainspec::{self, constants::BASE_MAINNET_MAX_GAS_LIMIT},
				node::OpPayloadBuilderAttributes,
			},
			payload::builder::EthPayloadBuilderAttributes,
		},
		test_utils::{GenesisProviderFactory, WithFundedAccounts},
	},
	std::{
		sync::{Arc, atomic::AtomicU64},
		time::{SystemTime, UNIX_EPOCH},
	},
};

// This is used to create a payload attributes with a block timestamp
// `offset_secs` seconds in the future.
fn checkpoint_with_future_offset_secs(
	offset_secs: u64,
) -> Checkpoint<Flashblocks> {
	let chainspec = chainspec::OP_DEV.as_ref().clone().with_funded_accounts();
	let provider =
		GenesisProviderFactory::<Flashblocks>::new(chainspec.clone().into());

	let now_secs = SystemTime::now()
		.duration_since(UNIX_EPOCH)
		.expect("time")
		.as_secs();
	let block_ts = now_secs.saturating_add(offset_secs);

	// Create an intermediate parent block with timestamp 2 seconds earlier than
	// target. This is so block_time gets calculaed as 2 seconds.
	let parent_ts = block_ts.saturating_sub(2);
	let mut parent_header = chainspec.genesis_header().clone();
	parent_header.timestamp = parent_ts;
	parent_header.number = 1; // Make it block #1 instead of genesis (block #0)

	// Create a hash for our intermediate parent block
	let parent_hash = B256::from([1u8; 32]); // Use a deterministic but non-zero hash
	let parent =
		rblib::reth::primitives::SealedHeader::new(parent_header, parent_hash);

	let payload_attributes =
		OpPayloadBuilderAttributes::<types::Transaction<Flashblocks>> {
			payload_attributes: EthPayloadBuilderAttributes::new(
				parent.hash(),
				rblib::reth::ethereum::node::engine::EthPayloadAttributes {
					timestamp: block_ts,
					prev_randao: B256::random(),
					suggested_fee_recipient: Address::random(),
					withdrawals: Some(vec![]),
					parent_beacon_block_root: Some(B256::ZERO),
				},
			),
			transactions: vec![],
			gas_limit: Some(BASE_MAINNET_MAX_GAS_LIMIT),
			..Default::default()
		};

	let block = BlockContext::<Flashblocks>::new(
		parent,
		payload_attributes,
		provider.state_provider(),
		chainspec.into(),
	)
	.expect("block context");

	block.start()
}

#[test]
fn leeway_0ms_remaining_time_2000ms_interval_250ms() {
	// With leeway = 0ms and target = now + 2000ms, then remaining_time should be
	// around 2000ms. 2000 % 250 = 0, so we expect ~250ms (flashblock interval)
	// for the first flashblock interval.
	let interval = Duration::from_millis(250);
	let max_flashblocks = Arc::new(AtomicU64::new(0));
	let limits = FlashblockLimits::new(interval, max_flashblocks);
	let checkpoint = checkpoint_with_future_offset_secs(2);
	let payload_deadline = Duration::from_millis(2000);

	let (num_flashblocks, first_flashblock_interval) =
		limits.calculate_flashblocks(&checkpoint, payload_deadline);
	assert_eq!(first_flashblock_interval, interval);
	// The number of flashblocks should be 8
	assert_eq!(num_flashblocks, 8);
}

#[test]
fn leeway_75ms_remaining_time_1925ms_interval_250ms() {
	// With leeway = 75ms and target = now + 2000ms, then remaining_time should be
	// around 1925ms. 1925 % 250 = 175, so we expect a reduced 175ms for the
	// first flashblock interval.
	let interval = Duration::from_millis(250);
	let max_flashblocks = Arc::new(AtomicU64::new(0));
	let limits = FlashblockLimits::new(interval, max_flashblocks);
	// Use a large block_time so it doesn't cap the time_drift calculation
	let checkpoint = checkpoint_with_future_offset_secs(2);
	let payload_deadline = Duration::from_millis(2000);

	let (num_flashblocks, first_flashblock_interval) =
		limits.calculate_flashblocks(&checkpoint, payload_deadline);

	// The first flashblock interval should be 175ms
	assert_eq!(first_flashblock_interval, Duration::from_millis(175));
	// The number of flashblocks should be 8
	assert_eq!(num_flashblocks, 8);
}

#[test]
fn leeway_75ms_remaining_time_1925ms_interval_750ms() {
	// remaining time is 2000ms - 75ms = 1925ms.
	// first flashblock interval is 425ms, followed by two flashblocks of 750ms
	// intervals.
	let interval = Duration::from_millis(750);
	let max_flashblocks = Arc::new(AtomicU64::new(0));
	let limits = FlashblockLimits::new(interval, max_flashblocks);
	let payload_deadline = Duration::from_millis(2000);

	let checkpoint = checkpoint_with_future_offset_secs(2);
	let (num_flashblocks, first_flashblock_interval) =
		limits.calculate_flashblocks(&checkpoint, payload_deadline);

	assert_eq!(first_flashblock_interval, Duration::from_millis(425));
	assert_eq!(num_flashblocks, 3);
}
