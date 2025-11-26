use {
	crate::{
		args::{BuilderArgs, Cli, CliExt},
		flashtestations::FlashtestationsPrologue,
		limits::FlashblockLimits,
		publish::{PublishFlashblock, WebSocketSink},
		rpc::TransactionStatusRpc,
		signer::BuilderSigner,
		state::TargetFlashblocks,
		stop::BreakAfterMaxFlashblocks,
	},
	platform::Flashblocks,
	rblib::{
		pool::*,
		prelude::*,
		reth::optimism::{
			node::{OpAddOns, OpEngineApiBuilder, OpEngineValidatorBuilder, OpNode},
			rpc::OpEthApiBuilder,
		},
		steps::*,
	},
	std::sync::Arc,
};

mod args;
mod bundle;
mod flashtestations;
mod limits;
mod platform;
mod playground;
mod primitives;
mod publish;
mod rpc;
mod signer;
mod state;
mod stop;

#[cfg(test)]
mod tests;

fn main() {
	#[cfg(feature = "debug")]
	console_subscriber::init();

	Cli::parsed()
		.run(|builder, cli_args| async move {
			let pool = OrderPool::<Flashblocks>::default();
			let pipeline = build_pipeline(&cli_args, &pool)?;
			let opnode = OpNode::new(cli_args.rollup_args.clone());
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
				.launch()
				.await?;

			handle.wait_for_node_exit().await
		})
		.unwrap();
}

fn build_pipeline(
	cli_args: &BuilderArgs,
	pool: &OrderPool<Flashblocks>,
) -> eyre::Result<Pipeline<Flashblocks>> {
	// how often a flashblock is published
	let interval = cli_args.flashblocks_args.interval;

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

	let target_flashblocks = Arc::new(TargetFlashblocks::new());

	let pipeline = Pipeline::<Flashblocks>::named("top")
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
			Pipeline::named("n_flashblocks")
				.with_pipeline(
					Once,
					Pipeline::named("single_flashblock")
						.with_pipeline(
							Loop,
							Pipeline::named("flashblock_steps")
								.with_step(AppendOrders::from_pool(pool).with_ok_on_limit())
								.with_step(OrderByPriorityFee::default())
								.with_step_if(
									cli_args.revert_protection,
									RemoveRevertedTransactions::default(),
								)
								.with_step(BreakAfterDeadline)
								.with_epilogue_if(
									cli_args.builder_signer.is_some(),
									BuilderEpilogue::with_signer(builder_signer.clone().into())
										.with_message(|block| {
											format!("Block Number: {}", block.number())
										}),
								)
								.with_epilogue(PublishFlashblock::new(
									ws.clone(),
									cli_args.flashblocks_args.calculate_state_root,
								))
								.with_limits(FlashblockLimits::new(interval)),
						)
						.with_step(BreakAfterDeadline),
				)
				.with_step(BreakAfterMaxFlashblocks::new(target_flashblocks)),
		)
		.with_limits(Scaled::default().deadline(total_building_time));

	ws.watch_shutdown(&pipeline);
	pool.attach_pipeline(&pipeline);

	Ok(pipeline)
}
