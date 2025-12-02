use {
	crate::{
		args::{BuilderArgs, FlashblocksArgs},
		build_pipeline,
		platform::Flashblocks,
		rpc::TransactionStatusRpc,
	},
	core::{net::SocketAddr, time::Duration},
	rblib::{
		alloy::{
			consensus::BlockHeader,
			eips::BlockNumberOrTag,
			network::BlockResponse,
			providers::Provider,
		},
		pool::*,
		prelude::*,
		reth::{
			node::builder::rpc::BasicEngineValidatorBuilder,
			optimism::{
				chainspec,
				node::{OpEngineApiBuilder, OpEngineValidatorBuilder, OpNode},
			},
		},
		test_utils::*,
	},
	std::time::{SystemTime, UNIX_EPOCH},
};

impl Flashblocks {
	async fn test_node_with_cli_args(
		cli_args: BuilderArgs,
	) -> eyre::Result<LocalNode<Flashblocks, OptimismConsensusDriver>> {
		Flashblocks::create_test_node_with_args(Pipeline::default(), cli_args).await
	}

	/// Creates a new flashblocks enabled test node and returns the assigned
	/// socket address of the websocket.
	///
	/// Returns an instance of a local node and the socket address of the
	/// WebSocket.
	///
	/// Flashblocks tests have block times of 2s.
	pub async fn test_node()
	-> eyre::Result<(LocalNode<Flashblocks, OptimismConsensusDriver>, SocketAddr)>
	{
		let flashblocks_args = FlashblocksArgs::default_on_for_tests();

		#[allow(clippy::missing_panics_doc)]
		let ws_addr = flashblocks_args.ws_address;

		let mut node = Flashblocks::test_node_with_cli_args(BuilderArgs {
			flashblocks_args,
			..Default::default()
		})
		.await?;

		node.set_block_time(Duration::from_secs(2));

		Ok((node, ws_addr))
	}

	// The same as test_node, but with a `FundedAccounts::signer(0)` as the
	// builder's signer
	pub async fn test_node_with_builder_signer()
	-> eyre::Result<(LocalNode<Flashblocks, OptimismConsensusDriver>, SocketAddr)>
	{
		let flashblocks_args = FlashblocksArgs::default_on_for_tests();

		#[allow(clippy::missing_panics_doc)]
		let ws_addr = flashblocks_args.ws_address;

		let mut node = Flashblocks::test_node_with_cli_args(BuilderArgs {
			flashblocks_args,
			builder_signer: Some(FundedAccounts::signer(0).into()),
			..Default::default()
		})
		.await?;

		node.set_block_time(Duration::from_secs(2));

		Ok((node, ws_addr))
	}

	// The same as test_node, but with revert protection turned off
	pub async fn test_node_with_revert_protection_off()
	-> eyre::Result<(LocalNode<Flashblocks, OptimismConsensusDriver>, SocketAddr)>
	{
		let flashblocks_args = FlashblocksArgs::default_on_for_tests();

		#[allow(clippy::missing_panics_doc)]
		let ws_addr = flashblocks_args.ws_address;

		let mut node = Flashblocks::test_node_with_cli_args(BuilderArgs {
			flashblocks_args,
			revert_protection: false,
			..Default::default()
		})
		.await?;

		node.set_block_time(Duration::from_secs(2));

		Ok((node, ws_addr))
	}

	// The same as test_node, but with a custom leeway time
	pub async fn test_node_with_custom_leeway_time_and_interval(
		leeway_time: Duration,
		interval: Duration,
	) -> eyre::Result<(LocalNode<Flashblocks, OptimismConsensusDriver>, SocketAddr)>
	{
		let flashblocks_args =
			FlashblocksArgs::default_on_custom_leeway_time_and_interval_for_tests(
				leeway_time,
				interval,
			);

		#[allow(clippy::missing_panics_doc)]
		let ws_addr = flashblocks_args.ws_address;

		let mut node = Flashblocks::test_node_with_cli_args(BuilderArgs {
			flashblocks_args,
			..Default::default()
		})
		.await?;

		node.set_block_time(Duration::from_secs(2));

		Ok((node, ws_addr))
	}
}

pub trait LocalNodeFlashblocksExt {
	async fn while_next_block<F>(
		&self,
		work: F,
	) -> eyre::Result<types::BlockResponse<Flashblocks>>
	where
		F: Future<Output = eyre::Result<()>> + Send;
}

// async block building
impl LocalNodeFlashblocksExt
	for LocalNode<Flashblocks, OptimismConsensusDriver>
{
	async fn while_next_block<F>(
		&self,
		work: F,
	) -> eyre::Result<types::BlockResponse<Flashblocks>>
	where
		F: Future<Output = eyre::Result<()>> + Send,
	{
		let latest_block = self
			.provider()
			.get_block_by_number(BlockNumberOrTag::Latest)
			.await?
			.ok_or_else(|| eyre::eyre!("Failed to get latest block from the node"))?;

		let latest_timestamp =
			Duration::from_secs(latest_block.header().timestamp());

		// calculate the timestamp for the new block
		let current_timestamp = SystemTime::now().duration_since(UNIX_EPOCH)?;
		let elapsed_time = current_timestamp.saturating_sub(latest_timestamp);
		let target_timestamp = latest_timestamp + self.block_time() + elapsed_time;

		// start the payload building process
		let payload_id = self
			.consensus()
			.start_building(self, target_timestamp.as_secs(), &())
			.await?;

		let sleep = tokio::time::sleep(self.block_time());

		tokio::pin!(work, sleep);
		tokio::select! {
			res = &mut work => {
				res?; // propagate work error
				(&mut sleep).await;
			},
			() = &mut sleep => {
				// block time elapsed first; dropping `work` cancels it.
			}
		};

		self
			.consensus()
			.finish_building(self, payload_id, &())
			.await
	}
}

impl TestNodeFactory<Flashblocks> for Flashblocks {
	type CliExtArgs = BuilderArgs;
	type ConsensusDriver = OptimismConsensusDriver;

	/// Notes:
	///
	/// - Here we are ignoring the `pipeline` argument because we are not
	///   interested in running arbitrary pipelines for this platform, instead we
	///   construct the pipeline based on the CLI arguments.
	async fn create_test_node_with_args(
		_: Pipeline<Flashblocks>,
		cli_args: Self::CliExtArgs,
	) -> eyre::Result<LocalNode<Flashblocks, Self::ConsensusDriver>> {
		let chainspec = chainspec::OP_DEV.as_ref().clone().with_funded_accounts();
		let pool = OrderPool::<Flashblocks>::default();
		let pipeline = build_pipeline(&cli_args, &pool)?;

		LocalNode::new(OptimismConsensusDriver, chainspec, move |builder| {
			let opnode = OpNode::new(cli_args.rollup_args.clone());
			let tx_status_rpc = TransactionStatusRpc::new(&pipeline);

			builder
				.with_types::<OpNode>()
				.with_components(
					opnode
						.components()
						.attach_pool(&pool)
						.payload(pipeline.into_service()),
				)
				.with_add_ons(opnode
						.add_ons_builder::<types::RpcTypes<Flashblocks>>()
						.build::<_, OpEngineValidatorBuilder, OpEngineApiBuilder<OpEngineValidatorBuilder>, BasicEngineValidatorBuilder<OpEngineValidatorBuilder>>())
				.extend_rpc_modules(move |mut rpc_ctx| {
					pool.attach_rpc(&mut rpc_ctx)?;
					tx_status_rpc.attach_rpc(&mut rpc_ctx)?;
					Ok(())
				})
		})
		.await
	}
}
