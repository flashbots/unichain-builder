use {
	crate::{
		Flashblocks,
			flashtestations::{
				FlashtestationsArgs,
				executor::{
					FlashtestationsExecutor,
					FlashtestationsRuntime,
					RuntimeLive,
				},
				service::FlashtestationsManager,
			},
		signer::BuilderSigner,
	},
	eyre::ContextCompat,
	rblib::{self, prelude::*},
	std::sync::Arc,
	tokio::sync::Mutex,
	tracing::warn,
};

pub struct FlashtestationsPrologue<R = RuntimeLive> {
	// We determine if Flashtestations is enabled by if the manager exists or not
	manager: Option<FlashtestationsManager>,
	runtime: R,
	state: Arc<Mutex<State>>,
}

#[derive(Default)]
struct State {
	// Whether the workload and address has been registered
	// TODO: Make use of this field later
	#[expect(dead_code)]
	registered: Option<bool>,

	// Attestation for the builder
	attestation: Option<Vec<u8>>,
}

impl FlashtestationsPrologue {
	pub fn try_new(
		args: FlashtestationsArgs,
		builder_key: BuilderSigner,
	) -> eyre::Result<Self> {
		FlashtestationsPrologue::<RuntimeLive>::with_runtime(
			args,
			builder_key,
			RuntimeLive::new(),
		)
	}
}

impl<R> FlashtestationsPrologue<R>
where
	R: FlashtestationsRuntime,
{
	pub fn with_runtime(
		args: FlashtestationsArgs,
		builder_key: BuilderSigner,
		runtime: R,
	) -> eyre::Result<Self> {
		if !args.flashtestations_enabled {
			return Ok(Self {
				manager: None,
				runtime,
				state: Arc::default(),
			});
		}

		let manager = FlashtestationsManager::try_new(args, builder_key)?;

		Ok(Self {
			manager: Some(manager),
			runtime,
			state: Arc::default(),
		})
	}

	async fn get_attestation(&self) -> eyre::Result<Vec<u8>> {
		self
			.state
			.lock()
			.await
			.attestation
			.clone()
			.context("no attestation found")
	}
}

impl<R> Step<Flashblocks> for FlashtestationsPrologue<R>
where
	R: FlashtestationsRuntime,
{
	async fn setup(
		&mut self,
		_: InitContext<Flashblocks>,
	) -> Result<(), PayloadBuilderError> {
		let mut state = self.state.lock().await;

		// Check if Flashtestations is enabled
		let Some(ref manager) = self.manager else {
			return Ok(());
		};

		if state.attestation.is_some() {
			return Ok(());
		}

		if let Ok(attestation) = manager
			.get_attestation()
			.await
			.inspect_err(|e| warn!("could not get attestation: {e:?}"))
		{
			state.attestation = Some(attestation);
		}

		Ok(())
	}

	async fn step(
		self: Arc<Self>,
		payload: Checkpoint<Flashblocks>,
		ctx: StepContext<Flashblocks>,
	) -> ControlFlow<Flashblocks> {
		// Check if Flashtestations is enabled
		let Some(ref manager) = self.manager else {
			return ControlFlow::Ok(payload);
		};

		let attestation = match self.get_attestation().await {
			Ok(attestation) => attestation,
			Err(e) => {
				warn!("no attestation found: {e:?}");
				return ControlFlow::Ok(payload);
			}
		};

		// Create ephemeral executor for this step
		let executor =
			FlashteststationsExecutor::new(manager, self.runtime.clone());

		let new_payload = executor
			.add_transactions(&payload, &ctx, attestation)
			.inspect_err(|e| {
				warn!("failed to add flashtestations transactions: {e:?}");
			})
			.unwrap_or(payload);

		ControlFlow::Ok(new_payload)
	}
}

#[cfg(test)]
mod tests {
	use {
		super::*,
		crate::flashtestations::attestation::AttestationSource,
		eyre::eyre,
		futures::future::BoxFuture,
		rblib::test_utils::OneStep,
		std::sync::{Arc, Mutex},
	};

	#[tokio::test]
	async fn prologue_invokes_executor_when_attestation_available() {
		let runtime = RecordingRuntime::default();
		let prologue = FlashtestationsPrologue {
			manager: Some(test_manager(false)),
			runtime: runtime.clone(),
			state: Arc::default(),
		};

		let result = OneStep::<Flashblocks>::new(prologue)
			.with_payload_tx(|tx| tx.with_to(Address::ZERO))
			.run()
			.await
			.unwrap();

		assert!(matches!(result, ControlFlow::Ok(_)));
		assert_eq!(runtime.executions(), 1, "permit transaction should run");
		assert_eq!(runtime.policy_calls(), 0, "block proofs disabled");
	}

	#[tokio::test]
	async fn prologue_skips_when_attestation_missing() {
		let runtime = RecordingRuntime::default();
		let prologue = FlashtestationsPrologue {
			manager: Some(test_manager_with_source(false, Arc::new(FailingAttestationSource))),
			runtime: runtime.clone(),
			state: Arc::default(),
		};

		let result = OneStep::<Flashblocks>::new(prologue).run().await.unwrap();

		assert!(matches!(result, ControlFlow::Ok(_)));
		assert_eq!(runtime.executions(), 0, "no attestation => no tx");
	}

	fn test_manager(enable_block_proofs: bool) -> FlashtestationsManager {
		test_manager_with_source(
			enable_block_proofs,
			Arc::new(StaticAttestationSource::new(vec![0x42])),
		)
	}

	fn test_manager_with_source(
		enable_block_proofs: bool,
		source: Arc<dyn AttestationSource>,
	) -> FlashtestationsManager {
		let mut args = FlashtestationsArgs::default();
		args.flashtestations_enabled = true;
		args.debug = true;
		args.registry_address = Some(Address::from([0x31u8; 20]));
		args.builder_policy_address = Some(Address::from([0x41u8; 20]));
		args.enable_block_proofs = enable_block_proofs;
		args.quote_provider = Some("http://localhost".into());

		FlashtestationsManager::try_new_with_source(
			args,
			BuilderSigner::from_seed("prologue-tests"),
			source,
		)
		.expect("manager")
	}

	#[derive(Clone, Default)]
	struct RecordingRuntime {
		executions: Arc<Mutex<usize>>,
		policy_struct_calls: Arc<Mutex<usize>>,
	}

	impl RecordingRuntime {
		fn executions(&self) -> usize {
			*self.executions.lock().unwrap()
		}

		fn policy_calls(&self) -> usize {
			*self.policy_struct_calls.lock().unwrap()
		}
	}

	impl FlashtestationsRuntime for RecordingRuntime {
		fn registry_nonce(
			&self,
			_: &Checkpoint<Flashblocks>,
			_: &StepContext<Flashblocks>,
			_: Address,
			_: Address,
			_: &BuilderSigner,
		) -> eyre::Result<U256> {
			Ok(U256::from(1))
		}

		fn registry_struct_hash(
			&self,
			_: &Checkpoint<Flashblocks>,
			_: &StepContext<Flashblocks>,
			_: Address,
			_: &IFlashtestationRegistry::computeStructHashCall,
			_: &BuilderSigner,
		) -> eyre::Result<B256> {
			Ok(B256::repeat_byte(0x11))
		}

		fn registry_typed_data_hash(
			&self,
			_: &Checkpoint<Flashblocks>,
			_: &StepContext<Flashblocks>,
			_: Address,
			_: &IFlashtestationRegistry::hashTypedDataV4Call,
			_: &BuilderSigner,
		) -> eyre::Result<B256> {
			Ok(B256::repeat_byte(0x22))
		}

		fn policy_struct_hash(
			&self,
			_: &Checkpoint<Flashblocks>,
			_: &StepContext<Flashblocks>,
			_: Address,
			_: &IBlockBuilderPolicy::computeStructHashCall,
			_: &BuilderSigner,
		) -> eyre::Result<B256> {
			*self.policy_struct_calls.lock().unwrap() += 1;
			Ok(B256::repeat_byte(0x33))
		}

		fn policy_typed_data_hash(
			&self,
			_: &Checkpoint<Flashblocks>,
			_: &StepContext<Flashblocks>,
			_: Address,
			_: &IBlockBuilderPolicy::getHashedTypeDataV4Call,
			_: &BuilderSigner,
		) -> eyre::Result<B256> {
			Ok(B256::repeat_byte(0x44))
		}

		fn execute_transaction<T: SolCall + 'static>(
			&self,
			payload: &Checkpoint<Flashblocks>,
			_: &StepContext<Flashblocks>,
			_: Address,
			_: &T,
			_: &BuilderSigner,
		) -> eyre::Result<Checkpoint<Flashblocks>> {
			*self.executions.lock().unwrap() += 1;
			Ok(payload.clone())
		}

		fn validate_event_logged(
			&self,
			_: &Checkpoint<Flashblocks>,
			_: B256,
		) -> eyre::Result<()> {
			Ok(())
		}
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
			let quote = self.quote.clone();
			Box::pin(async move { Ok(quote) })
		}
	}

	#[derive(Clone)]
	struct FailingAttestationSource;

	impl AttestationSource for FailingAttestationSource {
		fn get_attestation<'a>(
			&'a self,
			_: &'a BuilderSigner,
		) -> BoxFuture<'a, eyre::Result<Vec<u8>>> {
			Box::pin(async move { Err(eyre!("no attestation")) })
		}
	}
}
