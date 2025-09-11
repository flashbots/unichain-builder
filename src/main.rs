use {
	crate::args::CliExt,
	platform::Flashblocks,
	rblib::{
		pool::*,
		reth::{node::builder::Node, optimism::node::OpNode},
	},
};

mod args;
mod bundle;
mod limits;
mod pipeline;
mod platform;
mod playground;
mod primitives;
mod publish;

#[cfg(test)]
mod tests;

fn main() -> eyre::Result<()> {
	color_eyre::install()?;

	args::Cli::parsed().run(|builder, cli_args| async move {
		let pool = OrderPool::<Flashblocks>::default();
		let pipeline = pipeline::build(&cli_args, &pool)?;
		let opnode = OpNode::new(cli_args.rollup_args.clone());

		#[expect(clippy::large_futures)]
		let handle = builder
			.with_types::<OpNode>()
			.with_components(opnode.components().payload(pipeline.into_service()))
			.with_add_ons(opnode.add_ons())
			.extend_rpc_modules(move |mut rpc_ctx| pool.attach_rpc(&mut rpc_ctx))
			.launch()
			.await?;

		handle.wait_for_node_exit().await
	})
}
