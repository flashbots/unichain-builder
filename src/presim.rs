//! Top-of-block pre-simulation for filtering reverting transactions.
//!
//! Inspired by rbuilder's `OrderSimulationPool`, this step simulates
//! pending pool transactions at the top of a fresh block before the
//! building loop begins. Transactions that revert during pre-simulation
//! are removed from the pool, preventing them from consuming block
//! building time during flashblock production.
//!
//! A random coinbase address is optionally used during simulation to
//! prevent adversaries from detecting the simulation environment via
//! the `COINBASE` opcode.

use {
	crate::Flashblocks,
	core::sync::atomic::{AtomicU32, AtomicU64, Ordering},
	rblib::{
		alloy::{consensus::transaction::TxHashRef, primitives::Address},
		pool::*,
		prelude::*,
		reth::{
			evm::{ConfigureEvm, Evm},
			revm::{State, db::WrapDatabaseRef},
		},
	},
	std::sync::Arc,
	tracing::debug,
};

/// Pipeline step that pre-simulates pending transactions against the
/// top-of-block state and removes those that revert.
///
/// This step runs once per block, after sequencer transactions have
/// been applied (by `OptimismPrologue`) but before the flashblock
/// building loop begins. It pulls all pending orders from the pool,
/// simulates each loose transaction, and removes those that revert.
///
/// Only loose transactions are pre-simulated. Bundles are left for the
/// existing `RemoveRevertedTransactions` step during block building.
pub struct PreSimulateOrders {
	pool: OrderPool<Flashblocks>,
	random_coinbase: bool,
	metrics: Metrics,
	per_job: PerJobCounters,
}

impl PreSimulateOrders {
	pub fn new(pool: &OrderPool<Flashblocks>, random_coinbase: bool) -> Self {
		Self {
			pool: pool.clone(),
			random_coinbase,
			metrics: Metrics::default(),
			per_job: PerJobCounters::default(),
		}
	}
}

impl Step<Flashblocks> for PreSimulateOrders {
	async fn setup(
		&mut self,
		init: InitContext<Flashblocks>,
	) -> Result<(), PayloadBuilderError> {
		self.metrics = Metrics::with_scope(init.metrics_scope());
		Ok(())
	}

	async fn after_job(
		self: Arc<Self>,
		_: StepContext<Flashblocks>,
		_: Arc<Result<types::BuiltPayload<Flashblocks>, PayloadBuilderError>>,
	) -> Result<(), PayloadBuilderError> {
		let simulated = self.per_job.simulated.load(Ordering::Relaxed);
		let dropped = self.per_job.dropped.load(Ordering::Relaxed);
		let gas = self.per_job.gas_saved.load(Ordering::Relaxed);

		self.metrics.txs_simulated_total.increment(simulated.into());
		self.metrics.txs_dropped_total.increment(dropped.into());
		self.metrics.gas_saved_total.increment(gas);
		self.metrics.txs_simulated_per_job.record(simulated);
		self.metrics.txs_dropped_per_job.record(dropped);
		self.metrics.gas_saved_per_job.record(gas as f64);

		self.per_job.reset();
		Ok(())
	}

	async fn step(
		self: Arc<Self>,
		payload: Checkpoint<Flashblocks>,
		_ctx: StepContext<Flashblocks>,
	) -> ControlFlow<Flashblocks> {
		let started = std::time::Instant::now();
		let block = payload.block();
		let evm_config = block.evm_config();

		// Clone the EVM env and optionally randomize coinbase to prevent
		// adversaries from detecting top-of-block simulation via COINBASE.
		let mut env = block.evm_env().clone();
		if self.random_coinbase {
			env.block_env.beneficiary = Address::random();
		}

		// Collect all pending orders so we don't hold a borrow on the pool
		// while mutating it.
		let orders: Vec<_> = self
			.pool
			.best_orders_for_block(block, payload.context())
			.collect();

		for order in orders {
			// Only pre-simulate loose transactions. Bundles have their own
			// revert semantics handled by RemoveRevertedTransactions.
			let Order::Transaction(ref tx) = order else {
				continue;
			};

			let tx_hash = *tx.tx_hash();

			self.per_job.simulated.fetch_add(1, Ordering::Relaxed);

			// Simulate the transaction without committing state changes.
			// Each tx is simulated independently against the top-of-block
			// state. Transactions with future nonces will fail at the EVM
			// level (Err) and are kept — only clear reverts are dropped.
			let mut state = State::builder()
				.with_database(WrapDatabaseRef(&payload))
				.build();

			let result = evm_config
				.evm_with_env(&mut state, env.clone())
				.transact(tx);

			match result {
				Ok(res) if !res.result.is_success() => {
					let gas_used = res.result.gas_used();
					debug!(
						%tx_hash,
						gas_used,
						"presim: dropping reverting transaction"
					);
					self.pool.remove_any_with(tx_hash);
					self.per_job.dropped.fetch_add(1, Ordering::Relaxed);
					self
						.per_job
						.gas_saved
						.fetch_add(gas_used, Ordering::Relaxed);
				}
				Err(_) => {
					// EVM-level error (wrong nonce, insufficient balance,
					// etc.). These transactions may become valid once
					// earlier transactions execute, so keep them.
				}
				Ok(_) => {
					// Transaction succeeds at top of block — keep it.
				}
			}
		}

		let elapsed = started.elapsed();
		self.metrics.presim_duration.record(elapsed.as_secs_f64());

		debug!(
			simulated = self.per_job.simulated.load(Ordering::Relaxed),
			dropped = self.per_job.dropped.load(Ordering::Relaxed),
			gas_saved = self.per_job.gas_saved.load(Ordering::Relaxed),
			elapsed_ms = elapsed.as_millis(),
			random_coinbase = self.random_coinbase,
			"presim: top-of-block simulation complete"
		);

		ControlFlow::Ok(payload)
	}
}

#[derive(MetricsSet)]
struct Metrics {
	/// Total transactions pre-simulated across all blocks.
	pub txs_simulated_total: Counter,

	/// Transactions dropped by pre-simulation across all blocks.
	pub txs_dropped_total: Counter,

	/// Cumulative gas saved by filtering reverting transactions.
	pub gas_saved_total: Counter,

	/// Transactions simulated per payload job.
	pub txs_simulated_per_job: Histogram,

	/// Transactions dropped per payload job.
	pub txs_dropped_per_job: Histogram,

	/// Gas saved per payload job.
	pub gas_saved_per_job: Histogram,

	/// Duration of the pre-simulation step per block.
	pub presim_duration: Histogram,
}

#[derive(Default)]
struct PerJobCounters {
	simulated: AtomicU32,
	dropped: AtomicU32,
	gas_saved: AtomicU64,
}

impl PerJobCounters {
	fn reset(&self) {
		self.simulated.store(0, Ordering::Relaxed);
		self.dropped.store(0, Ordering::Relaxed);
		self.gas_saved.store(0, Ordering::Relaxed);
	}
}
