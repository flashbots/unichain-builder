use {
	crate::{
		args::{Cli, CliExt},
		rpc::TransactionStatusRpc,
	},
	rblib::{
		pool::*,
		prelude::*,
		reth::optimism::node::{
			OpEngineApiBuilder,
			OpEngineValidatorBuilder,
			OpNode,
		},
	},
};

mod args;
mod builders;
mod bundle;
mod platform;
mod rpc;

#[cfg(test)]
mod tests;

pub use platform::FlashBlocks;

fn main() {
	Cli::parsed()
		.run(|builder, cli_args| async move {
			let pool = OrderPool::<FlashBlocks>::default();
			let pipeline = builders::pipeline(&cli_args, &pool)?;
			let opnode = OpNode::new(cli_args.rollup_args.clone());
			let tx_status_rpc = TransactionStatusRpc::new(&pipeline);

			#[expect(clippy::large_futures)]
			let handle = builder
				.with_types::<OpNode>()
				.with_components(
					opnode
						.components()
						.attach_pool(&pool)
						.payload(pipeline.into_service()),
				)
				.with_add_ons(
					opnode
						.add_ons_builder::<types::RpcTypes<FlashBlocks>>()
						.build::<_, OpEngineValidatorBuilder, OpEngineApiBuilder<OpEngineValidatorBuilder>>(),
				)
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
