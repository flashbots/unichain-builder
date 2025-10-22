use {
	crate::{
		args::{BuilderArgs, Cli, CliExt},
		limits::FlashblockLimits,
		publish::{PublishFlashblock, WebSocketSink},
		rpc::TransactionStatusRpc,
	},
	platform::Flashblocks,
	rblib::{pool::*, prelude::*, steps::*},
	reth_optimism_node::{
		OpAddOns,
		OpEngineApiBuilder,
		OpEngineValidatorBuilder,
		OpNode,
	},
	reth_optimism_rpc::OpEthApiBuilder,
	std::sync::{Arc, atomic::AtomicU64},
	tracing::warn,
};

mod args;
mod bundle;
mod limits;
mod platform;
mod playground;
mod primitives;
mod publish;
mod rpc;

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
	let pipeline = if cli_args.flashblocks_args.enabled() {
		build_flashblocks_pipeline(cli_args, pool)?
	} else {
		build_classic_pipeline(cli_args, pool)
	};

	pool.attach_pipeline(&pipeline);

	Ok(pipeline)
}

/// Classic block builder
///
/// Block building strategy that builds blocks using the classic approach by
/// producing one block payload per CL payload job.
fn build_classic_pipeline(
	cli_args: &BuilderArgs,
	pool: &OrderPool<Flashblocks>,
) -> Pipeline<Flashblocks> {
	if cli_args.revert_protection {
		Pipeline::<Flashblocks>::named("classic")
			.with_prologue(OptimismPrologue)
			.with_pipeline(
				Loop,
				(
					AppendOrders::from_pool(pool),
					OrderByPriorityFee::default(),
					RemoveRevertedTransactions::default(),
				),
			)
	} else {
		Pipeline::<Flashblocks>::named("classic")
			.with_prologue(OptimismPrologue)
			.with_pipeline(
				Loop,
				(AppendOrders::from_pool(pool), OrderByPriorityFee::default()),
			)
	}
}

fn build_flashblocks_pipeline(
	cli_args: &BuilderArgs,
	pool: &OrderPool<Flashblocks>,
) -> eyre::Result<Pipeline<Flashblocks>> {
	let socket_address = cli_args
		.flashblocks_args
		.ws_address()
		.expect("WebSocket address must be set for Flashblocks");

	// how often a flashblock is published
	let interval = cli_args.flashblocks_args.interval;

	// time by which flashblocks will be delivered earlier to account for latency
	let leeway_time = cli_args.flashblocks_args.leeway_time;

	// Flashblocks builder will always take as long as the payload job deadline,
	// this value specifies how much buffer we want to give between flashblocks
	// building and the payload job deadline that is given by the CL.
	let total_building_time = Minus(leeway_time);

	let ws = Arc::new(WebSocketSink::new(socket_address)?);

	// TODO: this is super crutch until we have a way to break from outer payload
	// in limits
	let max_flashblocks = Arc::new(AtomicU64::new(0));

	let flashblock_building_pipeline_steps = (
		AppendOrders::from_pool(pool).with_ok_on_limit(),
		OrderByPriorityFee::default(),
		RemoveRevertedTransactions::default(),
		BreakAfterDeadline,
	)
		.into_pipeline();

	let flashblock_building_pipeline_steps = if let Some(ref signer) =
		cli_args.builder_signer
	{
		flashblock_building_pipeline_steps
			.with_epilogue(BuilderEpilogue::with_signer(signer.clone().into()))
	} else {
		warn!("BUILDER_SECRET_KEY is not specified, skipping builder transactions");
		flashblock_building_pipeline_steps
	};

	let flashblock_building_pipeline = flashblock_building_pipeline_steps
		.with_epilogue(PublishFlashblock::new(
			&ws,
			cli_args.flashblocks_args.calculate_state_root,
			max_flashblocks.clone(),
		))
		.with_limits(FlashblockLimits::new(interval, max_flashblocks));

	let block_building_pipeline = Pipeline::default()
		.with_pipeline(Loop, flashblock_building_pipeline)
		.with_step(BreakAfterDeadline);

	let pipeline = Pipeline::<Flashblocks>::named("flashblocks")
		.with_prologue(OptimismPrologue)
		.with_pipeline(Loop, block_building_pipeline)
		.with_limits(Scaled::default().deadline(total_building_time));
	ws.watch_shutdown(&pipeline);

	Ok(pipeline)
}
