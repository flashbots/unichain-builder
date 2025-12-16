use {
	crate::{
		args::{BuilderArgs, FlashblocksArgs},
		build_pipeline,
		platform::Flashblocks,
		rpc::TransactionStatusRpc,
		tests::framework::ws::WebSocketObserver,
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
			builder::rpc::BasicEngineValidatorBuilder,
			optimism::{
				chainspec,
				node::{OpEngineApiBuilder, OpEngineValidatorBuilder, OpNode},
			},
		},
		test_utils::*,
	},
	std::time::{SystemTime, UNIX_EPOCH},
};

pub struct Harness {
	node: LocalNode<Flashblocks, OptimismConsensusDriver>,
	ws_addr: SocketAddr,
}

impl Harness {
	pub async fn default() -> eyre::Result<Self> {
		let flashblocks_args = FlashblocksArgs {
			interval: Duration::from_millis(250),
			leeway_time: Duration::from_millis(75),
			ws_address: get_available_socket(),
			calculate_state_root: true,
		};
		let ws_addr = flashblocks_args.ws_address;

		let mut node = Flashblocks::create_test_node_with_args(
			Pipeline::default(),
			BuilderArgs {
				flashblocks_args,
				..Default::default()
			},
		)
		.await?;

		node.set_block_time(Duration::from_secs(2));

		Ok(Self { node, ws_addr })
	}

	pub async fn try_new(mut args: BuilderArgs) -> eyre::Result<Self> {
		let ws_addr = get_available_socket();
		args.flashblocks_args.ws_address = ws_addr;

		let mut node =
			Flashblocks::create_test_node_with_args(Pipeline::default(), args)
				.await?;

		node.set_block_time(Duration::from_secs(2));

		Ok(Self { node, ws_addr })
	}

	pub fn node(&self) -> &LocalNode<Flashblocks, OptimismConsensusDriver> {
		&self.node
	}

	pub async fn ws_observer(&self) -> eyre::Result<WebSocketObserver> {
		WebSocketObserver::try_new(self.ws_addr).await
	}
}

/// Gets an available socket by first binding to port 0 -- instructing the OS to
/// find and assign one. Then the listener is dropped when this goes out of
/// scope, freeing the port for the next time this function is called.
fn get_available_socket() -> SocketAddr {
	use std::net::TcpListener;

	TcpListener::bind("127.0.0.1:0")
		.expect("Failed to bind to random port")
		.local_addr()
		.expect("Failed to get local address")
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
