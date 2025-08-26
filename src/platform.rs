use {
	crate::bundle::FlashBlocksBundle,
	rblib::prelude::*,
	serde::{Deserialize, Serialize},
	std::sync::Arc,
};

/// Defines the `FlashBlocks` platform.
///
/// This platform is derived from the stock [`rblib::Optimism`] platform and
/// inherits all its types and behaviors except the bundle definition.
///
/// See [`FlashBlocksBundle`] for more details on how bundles behave in
/// the `FlashBlocks` platform.
///
/// See [`rblib::Platform`] for more details on platform definitions.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct FlashBlocks;

impl Platform for FlashBlocks {
	type Bundle = FlashBlocksBundle;
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

	fn build_payload<P, Provider>(
		payload: Checkpoint<P>,
		provider: &Provider,
	) -> Result<types::BuiltPayload<P>, PayloadBuilderError>
	where
		P: traits::PlatformExecBounds<Self>,
		Provider: traits::ProviderBounds<Self>,
	{
		Optimism::build_payload::<P, Provider>(payload, provider)
	}
}

/// Inherits all optimism RPC types for the `FlashBlocks` platform.
impl PlatformWithRpcTypes for FlashBlocks {
	type RpcTypes = types::RpcTypes<Optimism>;
}
