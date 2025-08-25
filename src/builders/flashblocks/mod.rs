use {
	crate::{args::BuilderArgs, platform::FlashBlocks},
	core::num::NonZero,
	rblib::{pool::OrderPool, prelude::*, steps::*},
	std::sync::Arc,
	step::PublishFlashblock,
	ws::WebSocketPublisher,
};

mod primitives;
mod step;
mod ws;

pub use primitives::*;

pub fn pipeline(
	cli_args: &BuilderArgs,
	pool: &OrderPool<FlashBlocks>,
) -> eyre::Result<Pipeline<FlashBlocks>> {
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

	let ws = Arc::new(WebSocketPublisher::new(socket_address)?);

	let pipeline = Pipeline::<FlashBlocks>::named("flashblocks")
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
						.with_limits(Scaled::default().deadline(Fixed(interval))),
				)
				.with_step(BreakAfterDeadline),
		)
		.with_limits(Scaled::default().deadline(total_building_time));

	ws.watch_shutdown(&pipeline);

	Ok(pipeline)
}
