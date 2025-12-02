use {
	super::*,
	jsonrpsee::{
		core::{RpcResult, async_trait},
		proc_macros::rpc,
		types::ErrorObjectOwned,
	},
	moka::sync::Cache,
	rblib::{
		alloy::primitives::{B256, TxHash},
		reth::{
			ethereum::rpc::eth::EthApiError,
			node::builder::{FullNodeComponents, rpc::RpcContext},
			rpc::{
				api::eth::helpers::FullEthApi,
				compat::RpcTypes,
				eth::EthApiTypes,
			},
		},
	},
};

/// This RPC exension provides more accurate transaction status information for
/// transactions that were dropped during revert protection. Specifically, it:
///  - Listens on events emitted by the `RemoveRevertedTransactions` step, and
///    reports transactions and bundles that were dropped from the payload as
///    dropped when `eth_getTransactionReceipt` is called on the RPC.
pub struct TransactionStatusRpc<P: PlatformWithRpcTypes> {
	phantom: std::marker::PhantomData<P>,
}

impl<P: PlatformWithRpcTypes> TransactionStatusRpc<P> {
	pub fn new(_pipeline: &Pipeline<P>) -> Self {
		Self {
			phantom: std::marker::PhantomData,
		}
	}

	#[expect(clippy::unused_self)]
	pub fn attach_rpc<Node, EthApi>(
		self,
		rpc_context: &mut RpcContext<Node, EthApi>,
	) -> eyre::Result<()>
	where
		P: PlatformWithRpcTypes,
		Node: FullNodeComponents<Types = types::NodeTypes<P>>,
		EthApi:
			FullEthApi<NetworkTypes: RpcTypes<Receipt = types::ReceiptResponse<P>>>,
		ErrorObjectOwned: From<<EthApi as EthApiTypes>::Error>,
	{
		rpc_context.modules.add_or_replace_configured(
			TransactionStatusRpcImpl::<P, EthApi> {
				eth_api: rpc_context.registry.eth_api().clone(),
				dropped_txs: Cache::builder().max_capacity(5000).build(),
				phantom: std::marker::PhantomData,
			}
			.into_rpc(),
		)?;

		Ok(())
	}
}

struct TransactionStatusRpcImpl<P: PlatformWithRpcTypes, EthApi> {
	eth_api: EthApi,
	dropped_txs: Cache<TxHash, ()>,
	phantom: std::marker::PhantomData<P>,
}

#[rpc(server, namespace = "eth")]
pub trait TransactionStatusApi<P: PlatformWithRpcTypes> {
	#[method(name = "getTransactionReceipt")]
	async fn transaction_receipt(
		&self,
		hash: B256,
	) -> RpcResult<Option<types::ReceiptResponse<P>>>;
}

#[async_trait]
impl<P, EthApi> TransactionStatusApiServer<P>
	for TransactionStatusRpcImpl<P, EthApi>
where
	P: PlatformWithRpcTypes,
	EthApi:
		FullEthApi<NetworkTypes: RpcTypes<Receipt = types::ReceiptResponse<P>>>,
	ErrorObjectOwned: From<<EthApi as EthApiTypes>::Error>,
{
	async fn transaction_receipt(
		&self,
		hash: B256,
	) -> RpcResult<Option<types::ReceiptResponse<P>>> {
		if let Some(receipt) = self.eth_api.transaction_receipt(hash).await? {
			Ok(Some(receipt))
		} else {
			if self.dropped_txs.contains_key(&hash) {
				return Err(
					EthApiError::InvalidParams("transaction dropped".into()).into(),
				);
			}
			Ok(None)
		}
	}
}
