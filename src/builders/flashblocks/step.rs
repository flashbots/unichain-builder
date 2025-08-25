use {
	super::{FlashblocksPayloadV1, ws::WebSocketPublisher},
	crate::{
		FlashBlocks,
		builders::flashblocks::{
			ExecutionPayloadBaseV1,
			ExecutionPayloadFlashblockDeltaV1,
		},
	},
	atomic_time::AtomicOptionInstant,
	core::sync::atomic::{AtomicU64, Ordering},
	parking_lot::RwLock,
	rblib::{
		alloy::{
			consensus::BlockHeader,
			eips::Encodable2718,
			primitives::{B256, Bloom, U256},
		},
		prelude::*,
	},
	reth_node_builder::PayloadBuilderAttributes,
	std::{sync::Arc, time::Instant},
};

pub struct PublishFlashblock {
	/// The websocket interface that manages flashblock publishing to external
	/// subscribers.
	sink: Arc<WebSocketPublisher>,

	/// Keeps track of the current flashblock number within the block.
	block_number: AtomicU64,

	/// Set once at the begining of the payload job, captures immutable
	/// information about the payload that is being built. This info is derived
	/// from the payload attributes parameter on the FCU from the EL node.
	block_base: RwLock<Option<ExecutionPayloadBaseV1>>,

	/// Metrics for monitoring flashblock publishing.
	metrics: Metrics,

	/// Timestamps for various stages of the flashblock publishing process. This
	/// information is used to produce some of the metrics.
	times: Times,
}

impl PublishFlashblock {
	pub fn to(sink: &Arc<WebSocketPublisher>) -> Self {
		Self {
			sink: Arc::clone(sink),
			block_number: AtomicU64::default(),
			block_base: RwLock::new(None),
			metrics: Metrics::default(),
			times: Times::default(),
		}
	}
}

impl Step<FlashBlocks> for PublishFlashblock {
	async fn step(
		self: std::sync::Arc<Self>,
		payload: Checkpoint<FlashBlocks>,
		ctx: StepContext<FlashBlocks>,
	) -> ControlFlow<FlashBlocks> {
		let this_block_span = self.unpublished_payload(&payload);
		let transactions: Vec<_> = this_block_span
			.transactions()
			.map(|tx| tx.encoded_2718().into())
			.collect();

		if transactions.is_empty() {
			// nothing to publish, empty flashblocks are not interesting, skip.
			return ControlFlow::Ok(payload);
		}

		// increment flashblock number
		let index = self.block_number.fetch_add(1, Ordering::SeqCst);

		let base = self.block_base.read().clone();
		let diff = ExecutionPayloadFlashblockDeltaV1 {
			state_root: B256::ZERO,       // TODO: compute state root
			receipts_root: B256::ZERO,    // TODO: compute receipts root
			logs_bloom: Bloom::default(), // TODO
			gas_used: payload.cumulative_gas_used(),
			block_hash: B256::ZERO, // TODO: compute block hash
			transactions,
			withdrawals: vec![],
			withdrawals_root: B256::ZERO, // TODO: compute withdrawals root
		};

		// Push the contents of the payload
		if let Err(e) = self.sink.publish(&FlashblocksPayloadV1 {
			base,
			diff,
			payload_id: ctx.block().payload_id(),
			index,
			metadata: serde_json::Value::Null,
		}) {
			tracing::error!("Failed to publish flashblock to websocket: {e}");
		}
		self.times.on_published_block(&self.metrics);
		self.capture_payload_metrics(&this_block_span);

		// Place a barrier after each published flashblock to freeze the contents
		// of the payload up to this point, since this becomes a publicly committed
		// state.
		ControlFlow::Ok(payload.barrier())
	}

	async fn before_job(
		self: Arc<Self>,
		ctx: StepContext<FlashBlocks>,
	) -> Result<(), PayloadBuilderError> {
		self.times.on_job_started(&self.metrics);

		// this remains constant for the entire payload job.
		self.block_base.write().replace(ExecutionPayloadBaseV1 {
			parent_beacon_block_root: ctx
				.block()
				.attributes()
				.parent_beacon_block_root()
				.unwrap_or_default(),
			parent_hash: ctx.block().parent().hash(),
			fee_recipient: ctx.block().coinbase(),
			prev_randao: ctx.block().attributes().prev_randao(),
			block_number: ctx.block().number(),
			gas_limit: ctx
				.block()
				.attributes()
				.gas_limit
				.unwrap_or_else(|| ctx.block().parent().header().gas_limit()),
			timestamp: ctx.block().timestamp(),
			extra_data: ctx.block().block_env().extra_data.clone(),
			base_fee_per_gas: U256::from(ctx.block().base_fee()),
		});

		Ok(())
	}

	async fn after_job(
		self: Arc<Self>,
		_: StepContext<FlashBlocks>,
		_: Arc<Result<types::BuiltPayload<FlashBlocks>, PayloadBuilderError>>,
	) -> Result<(), PayloadBuilderError> {
		self.times.on_job_ended(&self.metrics);

		// reset flashblocks block counter
		let count = self.block_number.swap(0, Ordering::SeqCst);
		self.metrics.blocks_per_payload_job.record(count as f64);
		*self.block_base.write() = None;

		Ok(())
	}

	fn setup(
		&mut self,
		ctx: InitContext<FlashBlocks>,
	) -> Result<(), PayloadBuilderError> {
		self.metrics = Metrics::with_scope(ctx.metrics_scope());
		Ok(())
	}
}

impl PublishFlashblock {
	// get a span that convers all payload checkpoints since the last barrier
	// those are the transactions that are going to be in this flashblock.
	// one exception is the first flashblock, we want to get all checkpoints
	// since the begining of the block, because the `OptimismPrologue` step
	// places a barrier after sequencer transactions and we want to broadcast
	// those transactions as well.
	fn unpublished_payload(
		&self,
		payload: &Checkpoint<FlashBlocks>,
	) -> Span<FlashBlocks> {
		if self.block_number.load(Ordering::SeqCst) == 0 {
			// first block, get all checkpoints, including sequencer txs
			payload.history()
		} else {
			// subsequent block, get all checkpoints since last barrier
			payload.history_mut()
		}
	}

	/// Called for each flashblock to capture metrics about the produced
	/// flashblock contents.
	fn capture_payload_metrics(&self, span: &Span<FlashBlocks>) {
		self.metrics.blocks_total.increment(1);
		self.metrics.gas_per_block.record(span.gas_used() as f64);
		self
			.metrics
			.blob_gas_per_block
			.record(span.blob_gas_used() as f64);
		self
			.metrics
			.txs_per_block
			.record(span.transactions().count() as f64);

		let bundles_count = span.iter().filter(|c| c.is_bundle()).count();
		self.metrics.bundles_per_block.record(bundles_count as f64);
	}
}

#[derive(MetricsSet)]
struct Metrics {
	/// Total number of flashblocks published across all payloads.
	pub blocks_total: Counter,

	/// Histogram of gas usage per flashblock.
	pub gas_per_block: Histogram,

	/// Histogram of blob gas usage per flashblock.
	pub blob_gas_per_block: Histogram,

	/// Histogram of transactions per flashblock.
	pub txs_per_block: Histogram,

	/// Histogram of the number of bundles per flashblock.
	pub bundles_per_block: Histogram,

	/// Histogram of flashblocks per job.
	pub blocks_per_payload_job: Histogram,

	/// The time interval flashblocks within one block.
	pub intra_block_interval: Histogram,

	/// The time interval between flashblocks from two consecutive payload jobs.
	/// This measures time between last flashblock from block N and first
	/// flashblock from block N+1.
	pub inter_block_interval: Histogram,

	/// The time interval between the end of one payload job and the start of the
	/// next.
	pub inter_jobs_interval: Histogram,

	/// The time it takes between the beginning of a payload job
	/// until the first flashblock is published.
	pub time_to_first_block: Histogram,

	/// The time beween the last published flashblock and the end of the payload
	/// job.
	pub idle_tail_time: Histogram,
}

#[derive(Default)]
struct Times {
	pub job_started: AtomicOptionInstant,
	pub job_ended: AtomicOptionInstant,
	pub first_block_at: AtomicOptionInstant,
	pub previous_block_at: AtomicOptionInstant,
	pub last_block_at: AtomicOptionInstant,
}

impl Times {
	pub fn on_job_started(&self, metrics: &Metrics) {
		let now = Instant::now();
		self.job_started.store(Some(now), Ordering::Relaxed);

		if let Some(ended_at) = self.job_ended.swap(None, Ordering::Relaxed) {
			let duration = ended_at.duration_since(now);
			metrics.inter_jobs_interval.record(duration);
		}
	}

	pub fn on_job_ended(&self, metrics: &Metrics) {
		let now = Instant::now();

		if let Some(last_block_at) = self.last_block_at.load(Ordering::Relaxed) {
			let idle_tail_time = now.duration_since(last_block_at);
			metrics.idle_tail_time.record(idle_tail_time);
		}

		self.job_ended.store(Some(now), Ordering::Relaxed);
		self.job_started.store(None, Ordering::Relaxed);
		self.first_block_at.store(None, Ordering::Relaxed);
		self.previous_block_at.store(None, Ordering::Relaxed);
	}

	pub fn on_published_block(&self, metrics: &Metrics) {
		let now = Instant::now();
		let last_block_at = self.last_block_at.load(Ordering::Relaxed);

		if self
			.first_block_at
			.compare_exchange(None, Some(now), Ordering::Relaxed, Ordering::Relaxed)
			.is_ok()
		{
			// this is the first block, capture inter-block interval
			if let Some(last_block_at) = last_block_at {
				let duration = now.duration_since(last_block_at);
				metrics.inter_block_interval.record(duration);
			}

			// capture time to first block
			let job_started = self.job_started.load(Ordering::Relaxed);
			if let Some(job_started) = job_started {
				let duration = now.duration_since(job_started);
				metrics.time_to_first_block.record(duration);
			}
		}

		// store now as the last block time
		let prev_at = self.last_block_at.swap(Some(now), Ordering::Relaxed);
		self.previous_block_at.store(prev_at, Ordering::Relaxed);

		// capture the duration between consecutive flashblocks from the same
		// payload job.
		if let (Some(last_block_at), Some(prev_at)) = (last_block_at, prev_at) {
			let duration = last_block_at.duration_since(prev_at);
			metrics.intra_block_interval.record(duration);
		}
	}
}
