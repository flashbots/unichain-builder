use {
	crate::{
		flashtestations::{
			FlashtestationsArgs,
			attestation::{
				AttestationSource,
				RemoteAttestationProvider,
			},
		},
		signer::BuilderSigner,
	},
	eyre::ContextCompat,
	rblib::alloy::primitives::{Address, Bytes},
	std::{path::Path, sync::Arc},
	tracing::{info, warn},
};

pub struct FlashtestationsManager {
	// TEE service generated key
	pub tee_signer: BuilderSigner,

	// Builder key for the flashtestation permit tx
	pub builder_key: BuilderSigner,

	// Extra registration data for the builder, empty for now
	pub extra_registration_data: Bytes,

	// Registry address for the attestation
	pub registry_address: Address,

	// Builder policy address for the block builder proof
	pub builder_policy_address: Address,
	// Builder proof version
	pub builder_proof_version: u8,

	// Whether block proofs are enabled
	pub enable_block_proofs: bool,

	pub attestation_provider: Arc<dyn AttestationSource>,
}

impl FlashtestationsManager {
	pub fn try_new(
		args: FlashtestationsArgs,
		builder_key: BuilderSigner,
	) -> eyre::Result<Self> {
		let attestation_provider = Arc::new(RemoteAttestationProvider::try_new(
			args.debug,
			args.quote_provider.clone(),
		)?);
		Self::try_new_with_source(args, builder_key, attestation_provider)
	}

	pub fn try_new_with_source(
		args: FlashtestationsArgs,
		builder_key: BuilderSigner,
		attestation_provider: Arc<dyn AttestationSource>,
	) -> eyre::Result<Self> {
		let tee_signer = load_or_generate_tee_key(
			&args.flashtestations_key_path,
			args.debug,
			&args.debug_tee_key_seed,
		)?;

		info!("Flashtestations TEE address: {}", tee_signer.address());

		let registry_address = args
			.registry_address
			.context("registry address required when flashtestations enabled")?;
		let builder_policy_address = args.builder_policy_address.context(
			"builder policy address required when flashtestations enabled",
		)?;

		Ok(Self {
			tee_signer,
			builder_key,
			extra_registration_data: Bytes::from(b""),
			registry_address,
			builder_policy_address,
			builder_proof_version: args.builder_proof_version,
			enable_block_proofs: args.enable_block_proofs,
			attestation_provider,
		})
	}

	pub async fn get_attestation(&self) -> eyre::Result<Vec<u8>> {
		info!(target: "flashtestations", "requesting TDX attestation");
		self.attestation_provider.get_attestation(&self.tee_signer).await
	}
}

/// Load ephemeral TEE key from file, or generate and save a new one
fn load_or_generate_tee_key(
	key_path: &str,
	debug: bool,
	debug_seed: &str,
) -> eyre::Result<BuilderSigner> {
	if debug {
		info!("Flashtestations debug mode enabled, generating debug key from seed");
		return Ok(BuilderSigner::from_seed(debug_seed));
	}

	let path = Path::new(key_path);

	info!("Attempting to load TEE key from {:?}", path);
	if let Ok(signer) = BuilderSigner::try_from_file(path)
		.inspect_err(|e| warn!("Did not load tee key from file: {e}"))
	{
		return Ok(signer);
	}

	// Generate new key if we didn't get one from the file
	info!("Generating new ephemeral TEE key");
	let signer = BuilderSigner::random();
	signer.write_to_file(path)?;

	Ok(signer)
}
