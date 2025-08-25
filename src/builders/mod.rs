use {
	crate::{args::BuilderArgs, platform::FlashBlocks},
	rblib::{pool::OrderPool, prelude::*},
};

pub mod flashblocks;
pub mod standard;

/// Returns a payload building pipeline instance configured based on the CLI
/// arguments passed at startup.
pub fn pipeline(
	cli_args: &BuilderArgs,
	pool: &OrderPool<FlashBlocks>,
) -> eyre::Result<Pipeline<FlashBlocks>> {
	if cli_args.flashblocks_args.enabled() {
		flashblocks::pipeline(cli_args, pool)
	} else {
		Ok(standard::pipeline(cli_args, pool))
	}
}
