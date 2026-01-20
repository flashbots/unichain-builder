use {
	crate::{
		args::{BuilderArgs, Cli, CliExt},
		flashtestations::FlashtestationsPrologue,
		limits::FlashblockLimits,
		publish::{PublishFlashblock, WebSocketSink},
		rpc::TransactionStatusRpc,
		signer::BuilderSigner,
		stop::BreakAfterMaxFlashblocks,
		version::set_version_metric,
	},
	platform::Flashblocks,
	rblib::{
		pool::*,
		prelude::*,
		reth::{
			builder::{NodeBuilder, WithLaunchContext},
			cli::commands::launcher::Launcher,
			db::DatabaseEnv,
			optimism::{
				chainspec::OpChainSpec,
				cli::chainspec::OpChainSpecParser,
				node::{
					OpAddOns,
					OpEngineApiBuilder,
					OpEngineValidatorBuilder,
					OpNode,
				},
				rpc::OpEthApiBuilder,
			},
		},
		steps::*,
	},
	std::sync::Arc,
	tracing::info,
};

mod args;
mod bundle;
mod flashtestations;
mod limits;
mod platform;
mod playground;
mod publish;
mod rpc;
mod signer;
mod state;
mod stop;
mod version;

#[cfg(test)]
mod tests;

fn main() {
	#[cfg_attr(not(feature = "debug"), allow(unused_mut))]
	let mut cli = Cli::parsed().configure();

	#[cfg(feature = "debug")]
	{
		let console_layer = console_subscriber::spawn();
		cli
			.access_tracing_layers()
			.expect("failed to access tracing layers")
			.add_layer(console_layer);
	}

	cli.run(LauncherImpl).unwrap();
}

struct LauncherImpl;

impl Launcher<OpChainSpecParser, BuilderArgs> for LauncherImpl {
	async fn entrypoint(
		self,
		builder: WithLaunchContext<NodeBuilder<Arc<DatabaseEnv>, OpChainSpec>>,
		builder_args: BuilderArgs,
	) -> eyre::Result<()> {
		let pool = OrderPool::<Flashblocks>::default();
		let pipeline = build_pipeline(&builder_args, &pool)?;
		let opnode = OpNode::new(builder_args.rollup_args.clone());
		let tx_status_rpc = TransactionStatusRpc::new(&pipeline);

		let addons: OpAddOns<
			_,
			OpEthApiBuilder,
			OpEngineValidatorBuilder,
			OpEngineApiBuilder<OpEngineValidatorBuilder>,
		> = opnode
			.add_ons_builder::<types::RpcTypes<Flashblocks>>()
			.build();

		let handle = builder
			.with_types::<OpNode>()
			.with_components(
				opnode
					.components()
					.attach_pool(&pool)
					.payload(pipeline.into_service()),
			)
			.with_add_ons(addons)
			.extend_rpc_modules(move |mut rpc_ctx| {
				pool.attach_rpc(&mut rpc_ctx)?;
				tx_status_rpc.attach_rpc(&mut rpc_ctx)?;
				Ok(())
			})
			.on_node_started(move |_ctx| {
				set_version_metric();
				Ok(())
			})
			.launch()
			.await?;

		handle.wait_for_node_exit().await
	}
}

fn build_pipeline(
	cli_args: &BuilderArgs,
	pool: &OrderPool<Flashblocks>,
) -> eyre::Result<Pipeline<Flashblocks>> {
	info!("ARGS: {cli_args:?}");
	let flashblock_interval = cli_args.flashblocks_args.interval;

	// time by which flashblocks will be delivered earlier to account for latency
	let leeway_time = cli_args.flashblocks_args.leeway_time;

	// Flashblocks builder will always take as long as the payload job deadline,
	// this value specifies how much buffer we want to give between flashblocks
	// building and the payload job deadline that is given by the CL.
	let total_building_time = Minus(leeway_time);

	let ws = Arc::new(WebSocketSink::new(cli_args.flashblocks_args.ws_address)?);

	// TODO: Think about a better way to conditionally add steps so we don't
	// 		 have to default to a random signer.
	let builder_signer = cli_args
		.builder_signer
		.clone()
		.unwrap_or(BuilderSigner::random());

	let pipeline = Pipeline::<Flashblocks>::named("block")
		.with_step(OptimismPrologue)
		.with_step_if(
			cli_args.flashtestations.flashtestations_enabled
				&& cli_args.builder_signer.is_some(),
			FlashtestationsPrologue::try_new(
				cli_args.flashtestations.clone(),
				builder_signer.clone(),
			)?,
		)
		.with_pipeline(
			Loop,
			Pipeline::named("flashblocks")
				.with_pipeline(
					Loop,
					Pipeline::named("single_flashblock")
						.with_step(AppendOrders::from_pool(pool).with_ok_on_limit())
						.with_step(OrderByPriorityFee::default())
						.with_step_if(
							cli_args.revert_protection,
							RemoveRevertedTransactions::default(),
						)
						.with_step(BreakAfterDeadline)
						.with_limits(FlashblockLimits::new(flashblock_interval)),
				)
				.with_step_if(
					cli_args.builder_signer.is_some(),
					BuilderEpilogue::with_signer(builder_signer.clone().into())
						.with_message(|block| format!("Block Number: {}", block.number())),
				)
				.with_step(PublishFlashblock::new(
					ws.clone(),
					cli_args.flashblocks_args.calculate_state_root,
				))
				.with_step(BreakAfterMaxFlashblocks::new(flashblock_interval))
				.with_limits(Scaled::default().deadline(total_building_time)),
		);

	ws.watch_shutdown(&pipeline);
	pool.attach_pipeline(&pipeline);

	Ok(pipeline)
}
