use {
	crate::signer::BuilderSigner,
	eyre::ContextCompat,
	futures::future::BoxFuture,
	rblib::alloy::{
		hex,
		primitives::{Bytes, keccak256},
	},
	reqwest::Client,
	tracing::info,
};

const DEBUG_QUOTE_SERVICE_URL: &str =
	"http://ns31695324.ip-141-94-163.eu:10080/attest";

/// Source of attestations for the TEE signer.
pub trait AttestationSource: Send + Sync {
	fn get_attestation<'a>(
		&'a self,
		signer: &'a BuilderSigner,
	) -> BoxFuture<'a, eyre::Result<Vec<u8>>>;
}

/// Remote attestation provider
#[derive(Debug, Clone)]
pub struct RemoteAttestationProvider {
	client: Client,
	service_url: String,
}

impl RemoteAttestationProvider {
	pub fn try_new(
		debug: bool,
		quote_provider: Option<String>,
	) -> eyre::Result<Self> {
		let client = Client::new();

		let service_url = if debug {
			quote_provider.unwrap_or(DEBUG_QUOTE_SERVICE_URL.to_string())
		} else {
			quote_provider.context(
				"remote quote provider must be specified when not in debug mode",
			)?
		};

		Ok(Self {
			client,
			service_url,
		})
	}
}

impl RemoteAttestationProvider {
	async fn fetch(
		&self,
		signer: &BuilderSigner,
	) -> eyre::Result<Vec<u8>> {
		let report_data = prepare_report_data(signer);
		self.query_client(report_data).await
	}

	async fn query_client(&self, report_data: [u8; 64]) -> eyre::Result<Vec<u8>> {
		let report_data_hex = hex::encode(report_data);
		let url = format!("{}/{}", self.service_url, report_data_hex);

		info!(target: "flashtestations", url = url, "fetching quote from remote attestation provider");

		let response = self
			.client
			.get(&url)
			.timeout(std::time::Duration::from_secs(10))
			.send()
			.await?
			.error_for_status()?;
		let body = response.bytes().await?.to_vec();

		// TODO: Validate response?

		Ok(body)
	}
}

impl AttestationSource for RemoteAttestationProvider {
	fn get_attestation<'a>(
		&'a self,
		signer: &'a BuilderSigner,
	) -> BoxFuture<'a, eyre::Result<Vec<u8>>> {
		Box::pin(async move { self.fetch(signer).await })
	}
}

fn prepare_report_data(signer: &BuilderSigner) -> [u8; 64] {
	// Prepare report data:
	// - TEE address (20 bytes) at reportData[0:20]
	// - Extended registration data hash (32 bytes) at reportData[20:52]
	// - Total: 52 bytes, padded to 64 bytes with zeros

	// Extract TEE address as 20 bytes
	let tee_address_bytes: [u8; 20] = signer.address().into();

	// Calculate keccak256 hash of empty bytes (32 bytes)
	let empty_registration_data = Bytes::default();
	let empty_registration_data_hash = keccak256(&empty_registration_data);

	// Create 64-byte report data array
	let mut report_data = [0u8; 64];

	// Copy TEE address (20 bytes) to positions 0-19
	report_data[0..20].copy_from_slice(&tee_address_bytes);

	// Copy extended registration data hash (32 bytes) to positions 20-51
	report_data[20..52].copy_from_slice(empty_registration_data_hash.as_ref());

	report_data
}
