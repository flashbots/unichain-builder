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
	std::time::{SystemTime, UNIX_EPOCH},
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

// This is used to create a payload attributes with a block timestamp of
// `block_ts`.
fn checkpoint_with_block_timestamp(block_ts: u64) -> Checkpoint<Flashblocks> {
	let chainspec = chainspec::OP_DEV.as_ref().clone().with_funded_accounts();
	let provider =
		GenesisProviderFactory::<Flashblocks>::new(chainspec.clone().into());

	let parent = rblib::reth::primitives::SealedHeader::new(
		chainspec.genesis_header().clone(),
		chainspec.genesis_hash(),
	);

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
fn first_deadline_when_target_in_past_returns_full_interval() {
	// This test forces the target_time to be in the past, so
	// target_time.duration_since(now) fail. This means
	// calculate_first_flashblock_deadline is expected to return the full
	// interval.

	let interval = Duration::from_millis(750);
	let leeway = Duration::from_secs(10);
	let limits = FlashblockLimits::new(interval, leeway);

	let now_secs = SystemTime::now()
		.duration_since(UNIX_EPOCH)
		.expect("time")
		.as_secs();
	// Set block timestamp to 1s before now; with 10s leeway, target_time < now
	let block_ts = now_secs.saturating_sub(1);
	let checkpoint = checkpoint_with_block_timestamp(block_ts);
	let d = limits.calculate_first_flashblock_deadline(&checkpoint);
	assert_eq!(d, interval);
}

#[test]
fn first_deadline_leeway_0_time_drift_2000ms_interval_250ms() {
	// With leeway = 0ms and target = now + 2000ms, then time_drift should be
	// around 2000ms. 2000 % 250 = 0, so we expect ~250ms (flashblock interval)
	// for the first flashblock interval.
	let interval = Duration::from_millis(250);
	let leeway = Duration::from_millis(0);
	let limits = FlashblockLimits::new(interval, leeway);
	let checkpoint = checkpoint_with_future_offset_secs(2);

	let actual = limits.calculate_first_flashblock_deadline(&checkpoint);
	assert_eq!(actual, interval);
}

#[test]
fn first_deadline_leeway_75ms_time_drift_1925ms_interval_250ms() {
	// With leeway = 75ms and target = now + 2000ms, then time_drift should be
	// around 1925ms. 1925 % 250 = 175, so we expect a reduced ~175ms for the
	// first flashblock interval. Due to relying on SystemTime::now(), the result
	// is not deterministic and has the potential to be flaky.
	let interval = Duration::from_millis(250);
	let leeway = Duration::from_millis(75);
	let limits = FlashblockLimits::new(interval, leeway);
	// Use a large block_time so it doesn't cap the time_drift calculation
	let checkpoint = checkpoint_with_future_offset_secs(2);

	let actual = limits.calculate_first_flashblock_deadline(&checkpoint);

	// The result should be in the valid range (0, interval), which is strictly
	// less than 250ms
	assert!(actual > Duration::from_millis(0) && actual < interval);
}
