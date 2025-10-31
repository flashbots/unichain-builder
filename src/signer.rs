use {
	derive_more::{Deref, From, FromStr, Into},
	eyre::{Context, bail},
	rblib::alloy::{
		hex,
		primitives::B256,
		signers::{
			Signature,
			SignerSync,
			k256::{
				SecretKey,
				sha2::{Digest, Sha256},
			},
			local::PrivateKeySigner,
		},
	},
	std::{fs::OpenOptions, io::Write, os::unix::fs::OpenOptionsExt, path::Path},
	tracing::warn,
};

/// This type is used to store the builder's secret key for signing the last
/// transaction in the block.
#[derive(Debug, Clone, Deref, FromStr, Into, From)]
pub struct BuilderSigner {
	signer: PrivateKeySigner,
}

impl BuilderSigner {
	pub fn inner(&self) -> &PrivateKeySigner {
		&self.signer
	}

	pub fn random() -> Self {
		Self {
			signer: PrivateKeySigner::random(),
		}
	}

	pub fn try_from_bytes(bytes: &B256) -> eyre::Result<Self> {
		Ok(Self {
			signer: PrivateKeySigner::from_bytes(bytes)?,
		})
	}

	pub fn try_from_file(path: &Path) -> eyre::Result<Self> {
		// Try to load existing key
		if !path.exists() {
			bail!("path doesn't exist")
		}

		let key_hex =
			std::fs::read_to_string(path).context("failed to read key file")?;

		let secret_bytes = B256::try_from(
			hex::decode(key_hex.trim())
				.context("failed to decode hex from file")?
				.as_slice(),
		)
		.context("failed to parse key from file")?;

		Self::try_from_bytes(&secret_bytes)
			.context("failed to create signer from key")
	}

	pub fn write_to_file(&self, path: &Path) -> eyre::Result<()> {
		// Create file with 0600 permissions atomically and write the key there
		let key_hex = hex::encode(self.to_bytes());
		OpenOptions::new()
			.write(true)
			.create(true)
			.truncate(true)
			.mode(0o600)
			.open(path)
			.and_then(|mut file| file.write_all(key_hex.as_bytes()))
			.inspect_err(|e| warn!("Failed to write key to {:?}: {:?}", path, e))?;

		Ok(())
	}

	// Generate a key deterministically from a seed for debug and testing
	// Do not use in production
	pub fn from_seed(seed: &str) -> Self {
		// Hash the seed
		let mut hasher = Sha256::new();
		hasher.update(seed.as_bytes());
		let hash = hasher.finalize();

		// Create signing key
		let private_key =
			SecretKey::from_slice(&hash).expect("Failed to create private key");

		let signer = PrivateKeySigner::from_signing_key(private_key.into());

		Self { signer }
	}

	pub fn to_bytes(&self) -> B256 {
		self.signer.to_bytes()
	}

	pub fn sign_message(&self, message: B256) -> eyre::Result<Signature> {
		self
			.signer
			.sign_message_sync(message.as_slice())
			.wrap_err("could not sign message")
	}
}

impl PartialEq for BuilderSigner {
	fn eq(&self, other: &Self) -> bool {
		self.signer.address() == other.signer.address()
	}
}

impl Eq for BuilderSigner {}
