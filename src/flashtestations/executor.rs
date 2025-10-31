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
	std::fmt::Debug,
};

pub struct FlashteststationsExecutor<'a> {
	manager: &'a FlashtestationsManager,
}

impl<'a> FlashteststationsExecutor<'a> {
	pub fn new(manager: &'a FlashtestationsManager) -> Self {
		Self { manager }
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

		let payload = self.execute_transaction(
			payload,
			ctx,
			self.manager.registry_address,
			&calldata,
		)?;

		validate_event_logged(&payload, TEEServiceRegistered::SIGNATURE_HASH)?;

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

		let payload = self.execute_transaction(
			payload,
			ctx,
			self.manager.builder_policy_address,
			&calldata,
		)?;

		validate_event_logged(&payload, BlockBuilderProofVerified::SIGNATURE_HASH)?;

		Ok(payload)
	}

	fn fetch_permit_nonce(
		&self,
		payload: &Checkpoint<Flashblocks>,
		ctx: &StepContext<Flashblocks>,
	) -> eyre::Result<U256> {
		let calldata = IERC20Permit::noncesCall {
			owner: self.manager.tee_signer.address(),
		};
		self.simulate_contract_call::<IERC20Permit::noncesCall, IBlockBuilderPolicyErrors>(
			payload,
			ctx,
			self.manager.registry_address,
			&calldata,
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

		self.simulate_contract_call::<IFlashtestationRegistry::computeStructHashCall, IFlashtestationRegistryErrors>(
			payload,
			ctx,
			self.manager.registry_address,
			&calldata,
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

		self.simulate_contract_call::<IFlashtestationRegistry::hashTypedDataV4Call, IFlashtestationRegistryErrors>(
			payload,
			ctx,
			self.manager.registry_address,
			&calldata,
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

		self.simulate_contract_call::<IBlockBuilderPolicy::computeStructHashCall, IBlockBuilderPolicyErrors>(
			payload,
			ctx,
			self.manager.builder_policy_address,
			&calldata,
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

		self
			.simulate_contract_call::<IBlockBuilderPolicy::getHashedTypeDataV4Call, IBlockBuilderPolicyErrors>(
				payload,
				ctx,
				self.manager.builder_policy_address,
				&calldata,
			)
	}

	/// Simulate a read-only contract call (using TEE signer).
	fn simulate_contract_call<T: SolCall, E: SolInterface + Debug>(
		&self,
		payload: &Checkpoint<Flashblocks>,
		ctx: &StepContext<Flashblocks>,
		contract_address: Address,
		calldata: &T,
	) -> eyre::Result<T::Return> {
		let nonce = payload.nonce_of(self.manager.tee_signer.address())?;
		let tx_params = types::TransactionRequest::<Flashblocks>::default()
			.with_chain_id(ctx.block().chainspec().chain_id())
			.with_nonce(nonce)
			.with_gas_limit(ctx.block().parent().gas_limit())
			.with_max_fee_per_gas(ctx.block().base_fee().into())
			.with_to(contract_address)
			.with_input(calldata.abi_encode());

		let signed_tx =
			build_signed::<Flashblocks>(tx_params, self.manager.tee_signer.inner())?;
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

	/// Execute a state-changing contract transaction (using builder signer).
	fn execute_transaction<T: SolCall>(
		&self,
		payload: &Checkpoint<Flashblocks>,
		ctx: &StepContext<Flashblocks>,
		contract_address: Address,
		calldata: &T,
	) -> eyre::Result<Checkpoint<Flashblocks>> {
		let nonce = payload.nonce_of(self.manager.builder_key.address())?;
		let tx_params = types::TransactionRequest::<Flashblocks>::default()
			.with_from(self.manager.builder_key.address())
			.with_chain_id(ctx.block().chainspec().chain_id())
			.with_nonce(nonce)
			.with_gas_limit(ctx.block().parent().gas_limit())
			.with_max_fee_per_gas(ctx.block().base_fee().into())
			.with_to(contract_address)
			.with_input(calldata.abi_encode());

		let signed_tx =
			build_signed::<Flashblocks>(tx_params, self.manager.builder_key.inner())?;
		payload.apply(signed_tx).wrap_err("failed to write tx")
	}
}

/// Validate that the expected event was logged in the transaction execution.
fn validate_event_logged(
	payload: &Checkpoint<Flashblocks>,
	expected_topic: B256,
) -> eyre::Result<()> {
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
