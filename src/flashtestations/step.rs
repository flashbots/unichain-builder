use {
	crate::{
		Flashblocks,
		flashtestations::{
			FlashtestationsArgs,
			executor::FlashteststationsExecutor,
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

pub struct FlashtestationsPrologue {
	// We determine if Flashtestations is enabled by if the manager exists or not
	manager: Option<FlashtestationsManager>,
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
		builder_key: Option<BuilderSigner>,
	) -> eyre::Result<Self> {
		if !args.flashtestations_enabled {
			return Ok(Self {
				manager: None,
				state: Arc::default(),
			});
		}

		let manager = FlashtestationsManager::try_new(
			args,
			builder_key.context("builder key required for flashtestations")?,
		)?;

		Ok(Self {
			manager: Some(manager),
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

impl Step<Flashblocks> for FlashtestationsPrologue {
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
		let executor = FlashteststationsExecutor::new(manager);

		let new_payload = executor
			.add_transactions(&payload, &ctx, attestation)
			.inspect_err(|e| {
				warn!("failed to add flashtestations transactions: {e:?}");
			})
			.unwrap_or(payload);

		ControlFlow::Ok(new_payload)
	}
}
