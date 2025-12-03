use {
	crate::{
		Flashblocks,
		flashtestations::{
			contracts::{
				BlockData,
				IBlockBuilderPolicy::{
					self,
					BlockBuilderProofVerified,
					IBlockBuilderPolicyErrors,
				},
				IERC20Permit,
				IFlashtestationRegistry::{
					self,
					IFlashtestationRegistryErrors,
					TEEServiceRegistered,
				},
			},
			service::FlashtestationsManager,
		},
		signer::BuilderSigner,
	},
	eyre::{Context, ContextCompat, bail},
	itertools::Itertools,
	rblib::{
		alloy::{
			consensus::BlockHeader,
			eips::Encodable2718,
			hex,
			network::TransactionBuilder,
			primitives::{Address, B256, U256, keccak256},
			signers::Signature,
			sol_types::{
				ContractError,
				Revert,
				SolCall,
				SolError,
				SolEvent,
				SolInterface,
				SolValue,
			},
		},
		prelude::*,
		reth::{chainspec::EthChainSpec, revm::context::result::ExecutionResult},
	},
	std::{fmt::Debug, marker::PhantomData},
};

pub struct FlashteststationsExecutor<'a, R> {
	manager: &'a FlashtestationsManager,
	runtime: R,
}

impl<'a, R> FlashteststationsExecutor<'a, R>
where
	R: FlashtestationsRuntime,
{
	pub fn new(manager: &'a FlashtestationsManager, runtime: R) -> Self {
		Self { manager, runtime }
	}

	/// Build and submit flashtestation transactions (TEE registration + optional
	/// block proof).
	pub fn add_transactions(
		&self,
		payload: &Checkpoint<Flashblocks>,
		ctx: &StepContext<Flashblocks>,
		attestation: Vec<u8>,
	) -> eyre::Result<Checkpoint<Flashblocks>> {
		let payload = self.add_permit_transaction(payload, ctx, attestation)?;

		if self.manager.enable_block_proofs {
			self.add_block_proof_transaction(&payload, ctx)
		} else {
			Ok(payload)
		}
	}

	fn add_permit_transaction(
		&self,
		payload: &Checkpoint<Flashblocks>,
		ctx: &StepContext<Flashblocks>,
		attestation: Vec<u8>,
	) -> eyre::Result<Checkpoint<Flashblocks>> {
		let permit_nonce = self.fetch_permit_nonce(payload, ctx)?;
		let struct_hash = self.compute_registration_hash(
			payload,
			ctx,
			attestation.clone(),
			permit_nonce,
		)?;
		let hash_typed_data =
			self.hash_registration_message(payload, ctx, struct_hash)?;
		let registration_permit_signature =
			self.manager.tee_signer.sign_message(hash_typed_data)?;
		self.submit_registration(
			payload,
			ctx,
			attestation,
			permit_nonce,
			registration_permit_signature,
		)
	}

	fn add_block_proof_transaction(
		&self,
		payload: &Checkpoint<Flashblocks>,
		ctx: &StepContext<Flashblocks>,
	) -> eyre::Result<Checkpoint<Flashblocks>> {
		let permit_nonce = self.fetch_permit_nonce(payload, ctx)?;
		let block_content_hash = Self::compute_block_content_hash(payload, ctx);
		let struct_hash = self.compute_block_proof_hash(
			payload,
			ctx,
			block_content_hash,
			permit_nonce,
		)?;
		let hash_typed_data =
			self.hash_block_proof_message(payload, ctx, struct_hash)?;

		let signature = self.manager.tee_signer.sign_message(hash_typed_data)?;

		self.submit_block_proof(
			payload,
			ctx,
			block_content_hash,
			permit_nonce,
			signature,
		)
	}

	fn submit_registration(
		&self,
		payload: &Checkpoint<Flashblocks>,
		ctx: &StepContext<Flashblocks>,
		attestation: Vec<u8>,
		permit_nonce: U256,
		signature: Signature,
	) -> eyre::Result<Checkpoint<Flashblocks>> {
		let calldata = IFlashtestationRegistry::permitRegisterTEEServiceCall {
			rawQuote: attestation.into(),
			extendedRegistrationData: self.manager.extra_registration_data.clone(),
			nonce: permit_nonce,
			deadline: U256::from(ctx.block().timestamp()),
			signature: signature.as_bytes().into(),
		};

		let payload = self.runtime.execute_transaction(
			payload,
			ctx,
			self.manager.registry_address,
			&calldata,
			&self.manager.builder_key,
		)?;

		self.runtime.validate_event_logged(
			&payload,
			TEEServiceRegistered::SIGNATURE_HASH,
		)?;

		Ok(payload)
	}

	fn submit_block_proof(
		&self,
		payload: &Checkpoint<Flashblocks>,
		ctx: &StepContext<Flashblocks>,
		block_content_hash: B256,
		permit_nonce: U256,
		signature: Signature,
	) -> eyre::Result<Checkpoint<Flashblocks>> {
		let calldata = IBlockBuilderPolicy::permitVerifyBlockBuilderProofCall {
			version: self.manager.builder_proof_version,
			blockContentHash: block_content_hash,
			nonce: permit_nonce,
			eip712Sig: signature.as_bytes().into(),
		};

		let payload = self.runtime.execute_transaction(
			payload,
			ctx,
			self.manager.builder_policy_address,
			&calldata,
			&self.manager.builder_key,
		)?;

		self.runtime.validate_event_logged(
			&payload,
			BlockBuilderProofVerified::SIGNATURE_HASH,
		)?;

		Ok(payload)
	}

	fn fetch_permit_nonce(
		&self,
		payload: &Checkpoint<Flashblocks>,
		ctx: &StepContext<Flashblocks>,
	) -> eyre::Result<U256> {
		self.runtime.registry_nonce(
			payload,
			ctx,
			self.manager.registry_address,
			self.manager.tee_signer.address(),
			&self.manager.tee_signer,
		)
	}

	fn compute_registration_hash(
		&self,
		payload: &Checkpoint<Flashblocks>,
		ctx: &StepContext<Flashblocks>,
		attestation: Vec<u8>,
		permit_nonce: U256,
	) -> eyre::Result<B256> {
		let calldata = IFlashtestationRegistry::computeStructHashCall {
			rawQuote: attestation.into(),
			extendedRegistrationData: self.manager.extra_registration_data.clone(),
			nonce: permit_nonce,
			deadline: U256::from(ctx.block().timestamp()),
		};

		self.runtime.registry_struct_hash(
			payload,
			ctx,
			self.manager.registry_address,
			&calldata,
			&self.manager.tee_signer,
		)
	}

	fn hash_registration_message(
		&self,
		payload: &Checkpoint<Flashblocks>,
		ctx: &StepContext<Flashblocks>,
		struct_hash: B256,
	) -> eyre::Result<B256> {
		let calldata = IFlashtestationRegistry::hashTypedDataV4Call {
			structHash: struct_hash,
		};

		self.runtime.registry_typed_data_hash(
			payload,
			ctx,
			self.manager.registry_address,
			&calldata,
			&self.manager.tee_signer,
		)
	}

	fn compute_block_content_hash(
		payload: &Checkpoint<Flashblocks>,
		ctx: &StepContext<Flashblocks>,
	) -> B256 {
		let parent_hash = ctx.block().parent().parent_hash;
		let block_number = ctx.block().number();
		let timestamp = ctx.block().timestamp();

		let tx_hashes: Vec<B256> = payload
			.transactions()
			.iter()
			.map(|tx| {
				// RLP encode the transaction and hash it
				let mut encoded = Vec::new();
				tx.encode_2718(&mut encoded);
				keccak256(&encoded)
			})
			.collect();

		// Create struct and ABI encode
		let block_data = BlockData {
			parentHash: parent_hash,
			blockNumber: U256::from(block_number),
			timestamp: U256::from(timestamp),
			transactionHashes: tx_hashes,
		};

		let encoded = block_data.abi_encode();
		keccak256(&encoded)
	}

	fn compute_block_proof_hash(
		&self,
		payload: &Checkpoint<Flashblocks>,
		ctx: &StepContext<Flashblocks>,
		block_content_hash: B256,
		permit_nonce: U256,
	) -> eyre::Result<B256> {
		let calldata = IBlockBuilderPolicy::computeStructHashCall {
			version: self.manager.builder_proof_version,
			blockContentHash: block_content_hash,
			nonce: permit_nonce,
		};

		self.runtime.policy_struct_hash(
			payload,
			ctx,
			self.manager.builder_policy_address,
			&calldata,
			&self.manager.tee_signer,
		)
	}

	fn hash_block_proof_message(
		&self,
		payload: &Checkpoint<Flashblocks>,
		ctx: &StepContext<Flashblocks>,
		struct_hash: B256,
	) -> eyre::Result<B256> {
		let calldata = IBlockBuilderPolicy::getHashedTypeDataV4Call {
			structHash: struct_hash,
		};

		self.runtime.policy_typed_data_hash(
			payload,
			ctx,
			self.manager.builder_policy_address,
			&calldata,
			&self.manager.tee_signer,
		)
	}
}

pub trait FlashtestationsRuntime: Clone + Send + Sync + 'static {
	fn registry_nonce(
		&self,
		payload: &Checkpoint<Flashblocks>,
		ctx: &StepContext<Flashblocks>,
		contract_address: Address,
		owner: Address,
		signer: &BuilderSigner,
	) -> eyre::Result<U256>;

	fn registry_struct_hash(
		&self,
		payload: &Checkpoint<Flashblocks>,
		ctx: &StepContext<Flashblocks>,
		contract_address: Address,
		calldata: &IFlashtestationRegistry::computeStructHashCall,
		signer: &BuilderSigner,
	) -> eyre::Result<B256>;

	fn registry_typed_data_hash(
		&self,
		payload: &Checkpoint<Flashblocks>,
		ctx: &StepContext<Flashblocks>,
		contract_address: Address,
		calldata: &IFlashtestationRegistry::hashTypedDataV4Call,
		signer: &BuilderSigner,
	) -> eyre::Result<B256>;

	fn policy_struct_hash(
		&self,
		payload: &Checkpoint<Flashblocks>,
		ctx: &StepContext<Flashblocks>,
		contract_address: Address,
		calldata: &IBlockBuilderPolicy::computeStructHashCall,
		signer: &BuilderSigner,
	) -> eyre::Result<B256>;

	fn policy_typed_data_hash(
		&self,
		payload: &Checkpoint<Flashblocks>,
		ctx: &StepContext<Flashblocks>,
		contract_address: Address,
		calldata: &IBlockBuilderPolicy::getHashedTypeDataV4Call,
		signer: &BuilderSigner,
	) -> eyre::Result<B256>;

	fn execute_transaction<T: SolCall + 'static>(
		&self,
		payload: &Checkpoint<Flashblocks>,
		ctx: &StepContext<Flashblocks>,
		contract_address: Address,
		calldata: &T,
		signer: &BuilderSigner,
	) -> eyre::Result<Checkpoint<Flashblocks>>;

	fn validate_event_logged(
		&self,
		payload: &Checkpoint<Flashblocks>,
		expected_topic: B256,
	) -> eyre::Result<()>;
}

#[derive(Clone, Copy, Default)]
pub struct RuntimeLive {
	_marker: PhantomData<()>,
}

impl RuntimeLive {
	pub fn new() -> Self {
		Self {
			_marker: PhantomData,
		}
	}

	fn simulate_contract_call<T, E>(
		payload: &Checkpoint<Flashblocks>,
		ctx: &StepContext<Flashblocks>,
		contract_address: Address,
		calldata: &T,
		signer: &BuilderSigner,
	) -> eyre::Result<T::Return>
	where
		T: SolCall + 'static,
		E: SolInterface + Debug,
	{
		let nonce = payload.nonce_of(signer.address())?;
		let tx_params = types::TransactionRequest::<Flashblocks>::default()
			.with_chain_id(ctx.block().chainspec().chain_id())
			.with_nonce(nonce)
			.with_gas_limit(ctx.block().parent().gas_limit())
			.with_max_fee_per_gas(ctx.block().base_fee().into())
			.with_to(contract_address)
			.with_input(calldata.abi_encode());

		let signed_tx = build_signed::<Flashblocks>(tx_params, signer.inner())?;
		let checkpoint = payload.apply(signed_tx)?;

		let result = checkpoint
			.result()
			.context("should not be a barrier checkpoint")?
			.results()
			.first()
			.context("should have a first element")?;
		match result {
			ExecutionResult::Success { output, .. } => {
				let return_output = T::abi_decode_returns(&output.clone().into_data())
					.context("could not decode output from contract call")?;
				Ok(return_output)
			}
			ExecutionResult::Revert { output, .. } => {
				let revert = ContractError::<E>::abi_decode(output)
					.map(|reason| Revert::from(format!("{reason:?}")))
					.or_else(|_| Revert::abi_decode(output))
					.unwrap_or_else(|_| {
						Revert::from(format!("unknown revert: {}", hex::encode(output)))
					});
				bail!("transaction reverted: {:?}", revert)
			}
			ExecutionResult::Halt { reason, .. } => {
				bail!("transaction halted: {:?}", reason)
			}
		}
	}

	fn execute_transaction_impl<T: SolCall + 'static>(
		payload: &Checkpoint<Flashblocks>,
		ctx: &StepContext<Flashblocks>,
		contract_address: Address,
		calldata: &T,
		signer: &BuilderSigner,
	) -> eyre::Result<Checkpoint<Flashblocks>> {
		let nonce = payload.nonce_of(signer.address())?;
		let tx_params = types::TransactionRequest::<Flashblocks>::default()
			.with_from(signer.address())
			.with_chain_id(ctx.block().chainspec().chain_id())
			.with_nonce(nonce)
			.with_gas_limit(ctx.block().parent().gas_limit())
			.with_max_fee_per_gas(ctx.block().base_fee().into())
			.with_to(contract_address)
			.with_input(calldata.abi_encode());

		let signed_tx = build_signed::<Flashblocks>(tx_params, signer.inner())?;
		payload.apply(signed_tx).wrap_err("failed to write tx")
	}

	fn validate_event(payload: &Checkpoint<Flashblocks>, expected_topic: B256) -> eyre::Result<()> {
		if let ExecutionResult::Success { logs, .. } = payload
			.result()
			.context("should not be a barrier checkpoint")?
			.results()
			.first()
			.context("should have a first element")?
		{
			if !logs
				.iter()
				.flat_map(|log| log.topics())
				.contains(&expected_topic)
			{
				bail!(
					"did not find expected event {:?} in transaction logs",
					expected_topic
				)
			}
		}

		Ok(())
	}
}

impl FlashtestationsRuntime for RuntimeLive {
	fn registry_nonce(
		&self,
		payload: &Checkpoint<Flashblocks>,
		ctx: &StepContext<Flashblocks>,
		contract_address: Address,
		owner: Address,
		signer: &BuilderSigner,
	) -> eyre::Result<U256> {
		let calldata = IERC20Permit::noncesCall { owner };
		Self::simulate_contract_call::<IERC20Permit::noncesCall, IBlockBuilderPolicyErrors>(
			payload,
			ctx,
			contract_address,
			&calldata,
			signer,
		)
	}

	fn registry_struct_hash(
		&self,
		payload: &Checkpoint<Flashblocks>,
		ctx: &StepContext<Flashblocks>,
		contract_address: Address,
		calldata: &IFlashtestationRegistry::computeStructHashCall,
		signer: &BuilderSigner,
	) -> eyre::Result<B256> {
		Self::simulate_contract_call::<IFlashtestationRegistry::computeStructHashCall, IFlashtestationRegistryErrors>(
			payload,
			ctx,
			contract_address,
			calldata,
			signer,
		)
	}

	fn registry_typed_data_hash(
		&self,
		payload: &Checkpoint<Flashblocks>,
		ctx: &StepContext<Flashblocks>,
		contract_address: Address,
		calldata: &IFlashtestationRegistry::hashTypedDataV4Call,
		signer: &BuilderSigner,
	) -> eyre::Result<B256> {
		Self::simulate_contract_call::<IFlashtestationRegistry::hashTypedDataV4Call, IFlashtestationRegistryErrors>(
			payload,
			ctx,
			contract_address,
			calldata,
			signer,
		)
	}

	fn policy_struct_hash(
		&self,
		payload: &Checkpoint<Flashblocks>,
		ctx: &StepContext<Flashblocks>,
		contract_address: Address,
		calldata: &IBlockBuilderPolicy::computeStructHashCall,
		signer: &BuilderSigner,
	) -> eyre::Result<B256> {
		Self::simulate_contract_call::<IBlockBuilderPolicy::computeStructHashCall, IBlockBuilderPolicyErrors>(
			payload,
			ctx,
			contract_address,
			calldata,
			signer,
		)
	}

	fn policy_typed_data_hash(
		&self,
		payload: &Checkpoint<Flashblocks>,
		ctx: &StepContext<Flashblocks>,
		contract_address: Address,
		calldata: &IBlockBuilderPolicy::getHashedTypeDataV4Call,
		signer: &BuilderSigner,
	) -> eyre::Result<B256> {
		Self::simulate_contract_call::<IBlockBuilderPolicy::getHashedTypeDataV4Call, IBlockBuilderPolicyErrors>(
			payload,
			ctx,
			contract_address,
			calldata,
			signer,
		)
	}

	fn execute_transaction<T: SolCall + 'static>(
		&self,
		payload: &Checkpoint<Flashblocks>,
		ctx: &StepContext<Flashblocks>,
		contract_address: Address,
		calldata: &T,
		signer: &BuilderSigner,
	) -> eyre::Result<Checkpoint<Flashblocks>> {
		Self::execute_transaction_impl(payload, ctx, contract_address, calldata, signer)
	}

	fn validate_event_logged(
		&self,
		payload: &Checkpoint<Flashblocks>,
		expected_topic: B256,
	) -> eyre::Result<()> {
		Self::validate_event(payload, expected_topic)
	}
}

#[cfg(test)]
mod tests {
	use {
		super::*,
		crate::{
			flashtestations::{
				FlashtestationsArgs,
				attestation::AttestationSource,
				service::FlashtestationsManager,
			},
			platform::Flashblocks,
			signer::BuilderSigner,
		},
		eyre::eyre,
		futures::future::BoxFuture,
		rblib::{prelude::*, test_utils::OneStep},
		std::{
			collections::VecDeque,
			sync::{Arc, Mutex},
		},
	};

	#[tokio::test]
	async fn adds_registration_and_block_proof_transactions() {
		let runtime = RecordedRuntime::new(
			RuntimeScenario::default()
				.with_event_logs(vec![
					vec![TEEServiceRegistered::SIGNATURE_HASH],
					vec![BlockBuilderProofVerified::SIGNATURE_HASH],
				])
				.with_responses(|r| RuntimeResponses {
					permit_nonce: U256::from(3),
					..r
				}),
		);
		let step = ExecutorStep::new(
			test_manager(true),
			runtime.clone(),
			vec![0x01, 0x02, 0x03],
		);

		let result = OneStep::<Flashblocks>::new(step)
			.with_payload_tx(|tx| tx.with_to(Address::ZERO).with_value(U256::from(1)))
			.run()
			.await
			.unwrap();

		assert!(matches!(result, ControlFlow::Ok(_)));
		let calls = runtime.calls();
		assert_eq!(
			calls
				.iter()
				.filter(|call| matches!(call, RuntimeCall::Execute { .. }))
				.count(),
			2,
			"expected registration + block proof transactions"
		);
		assert!(calls.iter().any(|call| matches!(call, RuntimeCall::PolicyStructHash { .. })));
	}

	#[tokio::test]
	async fn skips_block_proof_when_disabled() {
		let runtime = RecordedRuntime::new(
			RuntimeScenario::default()
				.with_event_logs(vec![vec![TEEServiceRegistered::SIGNATURE_HASH]]),
		);
		let step = ExecutorStep::new(
			test_manager(false),
			runtime.clone(),
			vec![0xAA, 0xBB],
		);

		let result = OneStep::<Flashblocks>::new(step)
			.with_payload_tx(|tx| tx.with_to(Address::ZERO))
			.run()
			.await
			.unwrap();

		assert!(matches!(result, ControlFlow::Ok(_)));
		let calls = runtime.calls();
		assert_eq!(
			calls
				.iter()
				.filter(|call| matches!(call, RuntimeCall::Execute { .. }))
				.count(),
			1,
			"block proof transaction should not execute"
		);
		assert!(
			!calls.iter().any(|call| matches!(call, RuntimeCall::PolicyStructHash { .. })),
			"block proof path must not be invoked"
		);
	}

	#[tokio::test]
	async fn surface_missing_events() {
		let runtime = RecordedRuntime::new(
			RuntimeScenario::default()
				.with_event_logs(vec![Vec::new()]), // no TEEServiceRegistered topic
		);
		let step = ExecutorStep::new(
			test_manager(false),
			runtime,
			vec![0x10, 0x20],
		);

		let result = OneStep::<Flashblocks>::new(step).run().await.unwrap();
		assert!(
			matches!(result, ControlFlow::Fail(PayloadBuilderError::Other(msg)) if msg.contains("did not find expected event")),
			"missing event must bubble up"
		);
	}

	#[tokio::test]
	async fn propagates_simulation_errors() {
		let runtime = RecordedRuntime::new(
			RuntimeScenario::default()
				.with_event_logs(vec![vec![TEEServiceRegistered::SIGNATURE_HASH]])
				.with_simulation_failure(SimCallKind::RegistryStructHash),
		);
		let step = ExecutorStep::new(
			test_manager(false),
			runtime,
			vec![0xAB],
		);

		let result = OneStep::<Flashblocks>::new(step).run().await.unwrap();
		assert!(
			matches!(result, ControlFlow::Fail(PayloadBuilderError::Other(msg)) if msg.contains("registry struct hash failed")),
			"struct hash errors must stop the step"
		);
	}

	/// Wraps the executor in a pipeline step so we can leverage [`OneStep`]
	/// harness.
	struct ExecutorStep<R: FlashtestationsRuntime> {
		manager: Arc<FlashtestationsManager>,
		runtime: R,
		attestation: Vec<u8>,
	}

	impl<R: FlashtestationsRuntime> ExecutorStep<R> {
		fn new(
			manager: Arc<FlashtestationsManager>,
			runtime: R,
			attestation: Vec<u8>,
		) -> Arc<Self> {
			Arc::new(Self {
				manager,
				runtime,
				attestation,
			})
		}
	}

	impl<R: FlashtestationsRuntime> Step<Flashblocks> for ExecutorStep<R> {
		async fn step(
			self: Arc<Self>,
			payload: Checkpoint<Flashblocks>,
			ctx: StepContext<Flashblocks>,
		) -> ControlFlow<Flashblocks> {
			let executor =
				FlashteststationsExecutor::new(self.manager.as_ref(), self.runtime.clone());
			match executor.add_transactions(&payload, &ctx, self.attestation.clone()) {
				Ok(new_payload) => ControlFlow::Ok(new_payload),
				Err(err) => {
					ControlFlow::Fail(PayloadBuilderError::Other(err.to_string().into()))
				}
			}
		}
	}

	fn test_manager(enable_block_proofs: bool) -> Arc<FlashtestationsManager> {
		let mut args = FlashtestationsArgs::default();
		args.flashtestations_enabled = true;
		args.debug = true;
		args.registry_address = Some(Address::from([0x11u8; 20]));
		args.builder_policy_address = Some(Address::from([0x22u8; 20]));
		args.enable_block_proofs = enable_block_proofs;
		args.quote_provider = Some("http://localhost".into());

		let builder_key = BuilderSigner::from_seed("builder-testing");
		let attestation_source =
			Arc::new(StaticAttestationSource::new(vec![0u8; 4]));

		Arc::new(
			FlashtestationsManager::try_new_with_source(
				args,
				builder_key,
				attestation_source,
			)
			.expect("manager"),
		)
	}

	#[derive(Clone)]
	struct StaticAttestationSource {
		quote: Vec<u8>,
	}

	impl StaticAttestationSource {
		fn new(quote: Vec<u8>) -> Self {
			Self { quote }
		}
	}

	impl AttestationSource for StaticAttestationSource {
		fn get_attestation<'a>(
			&'a self,
			_: &'a BuilderSigner,
		) -> BoxFuture<'a, eyre::Result<Vec<u8>>> {
			Box::pin(async move { Ok(self.quote.clone()) })
		}
	}

	#[derive(Clone)]
	struct RecordedRuntime {
		state: Arc<Mutex<RuntimeState>>,
	}

	#[derive(Clone)]
	struct RuntimeScenario {
		responses: RuntimeResponses,
		event_logs: VecDeque<Vec<B256>>,
		simulate_failure: Option<SimCallKind>,
	}

	impl RuntimeScenario {
		fn with_event_logs(mut self, logs: Vec<Vec<B256>>) -> Self {
			self.event_logs = logs.into();
			self
		}

		fn with_simulation_failure(mut self, failure: SimCallKind) -> Self {
			self.simulate_failure = Some(failure);
			self
		}

		fn with_responses(
			mut self,
			updater: impl FnOnce(RuntimeResponses) -> RuntimeResponses,
		) -> Self {
			self.responses = updater(self.responses.clone());
			self
		}
	}

	impl Default for RuntimeScenario {
		fn default() -> Self {
			Self {
				responses: RuntimeResponses::default(),
				event_logs: VecDeque::new(),
				simulate_failure: None,
			}
		}
	}

	#[derive(Clone)]
	struct RuntimeState {
		scenario: RuntimeScenario,
		pending_logs: VecDeque<Vec<B256>>,
		calls: Vec<RuntimeCall>,
	}

	#[derive(Clone)]
	struct RuntimeResponses {
		permit_nonce: U256,
		registration_struct_hash: B256,
		registration_typed_hash: B256,
		block_struct_hash: B256,
		block_typed_hash: B256,
	}

	impl Default for RuntimeResponses {
		fn default() -> Self {
			Self {
				permit_nonce: U256::from(1),
				registration_struct_hash: B256::repeat_byte(0x01),
				registration_typed_hash: B256::repeat_byte(0x02),
				block_struct_hash: B256::repeat_byte(0x03),
				block_typed_hash: B256::repeat_byte(0x04),
			}
		}
	}

	#[derive(Clone, Copy, Debug, PartialEq, Eq)]
	enum SimCallKind {
		RegistryNonce,
		RegistryStructHash,
		RegistryTypedData,
		PolicyStructHash,
		PolicyTypedData,
	}

	#[derive(Debug, Clone)]
	enum RuntimeCall {
		RegistryNonce {
			contract: Address,
			owner: Address,
		},
		RegistryStructHash {
			contract: Address,
			input: Vec<u8>,
		},
		RegistryTypedData {
			contract: Address,
			input: Vec<u8>,
		},
		PolicyStructHash {
			contract: Address,
			input: Vec<u8>,
		},
		PolicyTypedData {
			contract: Address,
			input: Vec<u8>,
		},
		Execute {
			contract: Address,
			input: Vec<u8>,
			signer: Address,
		},
		Validate {
			topic: B256,
		},
	}

	impl RecordedRuntime {
		fn new(scenario: RuntimeScenario) -> Self {
			Self {
				state: Arc::new(Mutex::new(RuntimeState {
					scenario,
					pending_logs: VecDeque::new(),
					calls: Vec::new(),
				})),
			}
		}

		fn calls(&self) -> Vec<RuntimeCall> {
			self.state.lock().unwrap().calls.clone()
		}

		fn should_fail(
			state: &mut RuntimeState,
			kind: SimCallKind,
		) -> Option<eyre::Report> {
			if matches!(state.scenario.simulate_failure, Some(failure) if failure == kind)
			{
				state.scenario.simulate_failure = None;
				Some(eyre!(
					"{} failed",
					format!("{kind:?}").to_lowercase()
				))
			} else {
				None
			}
		}
	}

	impl FlashtestationsRuntime for RecordedRuntime {
		fn registry_nonce(
			&self,
			_: &Checkpoint<Flashblocks>,
			_: &StepContext<Flashblocks>,
			contract_address: Address,
			owner: Address,
			_: &BuilderSigner,
		) -> eyre::Result<U256> {
			let mut state = self.state.lock().unwrap();
			state.calls.push(RuntimeCall::RegistryNonce {
				contract: contract_address,
				owner,
			});
			if let Some(err) =
				Self::should_fail(&mut state, SimCallKind::RegistryNonce)
			{
				return Err(err);
			}
			Ok(state.scenario.responses.permit_nonce)
		}

		fn registry_struct_hash(
			&self,
			_: &Checkpoint<Flashblocks>,
			_: &StepContext<Flashblocks>,
			contract_address: Address,
			calldata: &IFlashtestationRegistry::computeStructHashCall,
			_: &BuilderSigner,
		) -> eyre::Result<B256> {
			let mut state = self.state.lock().unwrap();
			state.calls.push(RuntimeCall::RegistryStructHash {
				contract: contract_address,
				input: calldata.abi_encode(),
			});
			if let Some(err) =
				Self::should_fail(&mut state, SimCallKind::RegistryStructHash)
			{
				return Err(eyre!("registry struct hash failed: {err}"));
			}
			Ok(state.scenario.responses.registration_struct_hash)
		}

		fn registry_typed_data_hash(
			&self,
			_: &Checkpoint<Flashblocks>,
			_: &StepContext<Flashblocks>,
			contract_address: Address,
			calldata: &IFlashtestationRegistry::hashTypedDataV4Call,
			_: &BuilderSigner,
		) -> eyre::Result<B256> {
			let mut state = self.state.lock().unwrap();
			state.calls.push(RuntimeCall::RegistryTypedData {
				contract: contract_address,
				input: calldata.abi_encode(),
			});
			if let Some(err) =
				Self::should_fail(&mut state, SimCallKind::RegistryTypedData)
			{
				return Err(err);
			}
			Ok(state.scenario.responses.registration_typed_hash)
		}

		fn policy_struct_hash(
			&self,
			_: &Checkpoint<Flashblocks>,
			_: &StepContext<Flashblocks>,
			contract_address: Address,
			calldata: &IBlockBuilderPolicy::computeStructHashCall,
			_: &BuilderSigner,
		) -> eyre::Result<B256> {
			let mut state = self.state.lock().unwrap();
			state.calls.push(RuntimeCall::PolicyStructHash {
				contract: contract_address,
				input: calldata.abi_encode(),
			});
			if let Some(err) =
				Self::should_fail(&mut state, SimCallKind::PolicyStructHash)
			{
				return Err(err);
			}
			Ok(state.scenario.responses.block_struct_hash)
		}

		fn policy_typed_data_hash(
			&self,
			_: &Checkpoint<Flashblocks>,
			_: &StepContext<Flashblocks>,
			contract_address: Address,
			calldata: &IBlockBuilderPolicy::getHashedTypeDataV4Call,
			_: &BuilderSigner,
		) -> eyre::Result<B256> {
			let mut state = self.state.lock().unwrap();
			state.calls.push(RuntimeCall::PolicyTypedData {
				contract: contract_address,
				input: calldata.abi_encode(),
			});
			if let Some(err) =
				Self::should_fail(&mut state, SimCallKind::PolicyTypedData)
			{
				return Err(err);
			}
			Ok(state.scenario.responses.block_typed_hash)
		}

		fn execute_transaction<T: SolCall + 'static>(
			&self,
			payload: &Checkpoint<Flashblocks>,
			_: &StepContext<Flashblocks>,
			contract_address: Address,
			calldata: &T,
			signer: &BuilderSigner,
		) -> eyre::Result<Checkpoint<Flashblocks>> {
			let mut state = self.state.lock().unwrap();
			state.calls.push(RuntimeCall::Execute {
				contract: contract_address,
				input: calldata.abi_encode(),
				signer: signer.address(),
			});
			let logs = state.scenario.event_logs.pop_front().unwrap_or_default();
			state.pending_logs.push_back(logs);
			Ok(payload.clone())
		}

		fn validate_event_logged(
			&self,
			_: &Checkpoint<Flashblocks>,
			expected_topic: B256,
		) -> eyre::Result<()> {
			let mut state = self.state.lock().unwrap();
			state.calls.push(RuntimeCall::Validate {
				topic: expected_topic,
			});
			let logs = state.pending_logs.pop_front().unwrap_or_default();
			if logs.iter().any(|topic| *topic == expected_topic) {
				Ok(())
			} else {
				Err(eyre!(
					"did not find expected event {:?} in transaction logs",
					expected_topic
				))
			}
		}
	}
}
