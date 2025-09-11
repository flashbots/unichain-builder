use {
	crate::{
		args::BuilderArgs,
		limits::FlashblockLimits,
		platform::Flashblocks,
		publish::{PublishFlashblock, WebSocketSink},
	},
	core::num::NonZero,
	rblib::{pool::*, prelude::*, steps::*},
	std::sync::Arc,
};

pub fn build(
	cli_args: &BuilderArgs,
	pool: &OrderPool<Flashblocks>,
) -> eyre::Result<Pipeline<Flashblocks>> {
	let mut pipeline = if cli_args.flashblocks_args.enabled() {
		build_flashblocks_pipeline(cli_args, pool)?
	} else {
		build_classic_pipeline(cli_args, pool)
	};

	if let Some(ref signer) = cli_args.builder_signer {
		let epilogue = BuilderEpilogue::with_signer(signer.clone().into());
		let limiter = epilogue.limiter();
		pipeline = pipeline.with_epilogue(epilogue).with_limits(limiter);
	}

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

	// Flashblocks builder will always take as long as the payload job deadline,
	// this value specifies how much buffer we want to give between flashblocks
	// building and the payload job deadline that is given by the CL.
	let total_building_time = Fraction(95, NonZero::new(100).unwrap());

	let ws = Arc::new(WebSocketSink::new(socket_address)?);

	let pipeline = Pipeline::<Flashblocks>::named("flashblocks")
		.with_prologue(OptimismPrologue)
		.with_pipeline(
			Loop,
			Pipeline::default()
				.with_pipeline(
					Loop,
					(
						AppendOrders::from_pool(pool).with_ok_on_limit(),
						OrderByPriorityFee::default(),
						RemoveRevertedTransactions::default(),
						BreakAfterDeadline,
					)
						.with_epilogue(PublishFlashblock::to(&ws))
						.with_limits(FlashblockLimits::with_interval(interval)),
				)
				.with_step(BreakAfterDeadline),
		)
		.with_limits(Scaled::default().deadline(total_building_time));

	ws.watch_shutdown(&pipeline);

	Ok(pipeline)
}
