use {
	crate::bundle::FlashblocksBundle,
	rblib::{prelude::*, reth::providers::StateProvider},
	serde::{Deserialize, Serialize},
	std::sync::Arc,
};

/// Defines the `Flashblocks` platform.
///
/// This platform is derived from the stock [`rblib::Optimism`] platform and
/// inherits all its types and behaviors except the bundle definition.
///
/// See [`FlashblocksBundle`] for more details on how bundles behave in
/// the `Flashblocks` platform.
///
/// See [`rblib::Platform`] for more details on platform definitions.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Flashblocks;

impl Platform for Flashblocks {
	type Bundle = FlashblocksBundle;
	type DefaultLimits = types::DefaultLimits<Optimism>;
	type EvmConfig = types::EvmConfig<Optimism>;
	type NodeTypes = types::NodeTypes<Optimism>;
	type PooledTransaction = types::PooledTransaction<Optimism>;

	fn evm_config<P>(chainspec: Arc<types::ChainSpec<P>>) -> Self::EvmConfig
	where
		P: traits::PlatformExecBounds<Self>,
	{
		Optimism::evm_config::<Self>(chainspec)
	}

	fn next_block_environment_context<P>(
		chainspec: &types::ChainSpec<P>,
		parent: &types::Header<P>,
		attributes: &types::PayloadBuilderAttributes<P>,
	) -> types::NextBlockEnvContext<P>
	where
		P: traits::PlatformExecBounds<Self>,
	{
		Optimism::next_block_environment_context::<Self>(
			chainspec, parent, attributes,
		)
	}

	fn build_payload<P>(
		payload: Checkpoint<P>,
		provider: &dyn StateProvider,
	) -> Result<types::BuiltPayload<P>, PayloadBuilderError>
	where
		P: traits::PlatformExecBounds<Self>,
	{
		Optimism::build_payload::<P>(payload, provider)
	}
}

/// Inherits all optimism RPC types for the `Flashblocks` platform.
impl PlatformWithRpcTypes for Flashblocks {
	type RpcTypes = types::RpcTypes<Optimism>;
}
