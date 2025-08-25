use {
	crate::{args::BuilderArgs, platform::FlashBlocks},
	rblib::{pool::OrderPool, prelude::*, steps::*},
};

pub fn pipeline(
	cli_args: &BuilderArgs,
	pool: &OrderPool<FlashBlocks>,
) -> Pipeline<FlashBlocks> {
	let mut pipeline = if cli_args.revert_protection {
		Pipeline::<FlashBlocks>::named("standard")
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
		Pipeline::<FlashBlocks>::named("standard")
			.with_prologue(OptimismPrologue)
			.with_pipeline(
				Loop,
				(AppendOrders::from_pool(pool), OrderByPriorityFee::default()),
			)
	};

	if let Some(ref signer) = cli_args.builder_signer {
		let epilogue = BuilderEpilogue::with_signer(signer.clone().into());
		let limiter = epilogue.limiter();
		pipeline = pipeline.with_epilogue(epilogue).with_limits(limiter);
	}

	pool.attach_pipeline(&pipeline);

	pipeline
}
