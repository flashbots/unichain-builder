//! Flashblocks WebSocket publisher
//!
//! This module defines:
//!
//! - The WebSocket sink that handles the physical websocket transport and
//!   external subscription management.
//!
//! - The flashblock publishing pipeline step that is used in
//!   `build_flashblocks_pipeline` to publish flashblocks during payload
//!   building jobs.

use {
	crate::{Flashblocks, primitives::*},
	atomic_time::AtomicOptionInstant,
	core::{
		net::SocketAddr,
		sync::atomic::{AtomicU64, Ordering},
	},
	futures::{SinkExt, StreamExt},
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
	std::{io, net::TcpListener, sync::Arc, time::Instant},
	tokio::{
		net::TcpStream,
		sync::{
			broadcast::{self, error::RecvError},
			watch,
		},
	},
	tokio_tungstenite::{
		WebSocketStream, accept_async,
		tungstenite::{Message, Utf8Bytes},
	},
	tracing::{debug, trace},
};

/// Flashblocks pipeline step for publishing flashblocks to external
/// subscribers.
///
/// This step will send a JSON serialized version of `FlashblocksPayloadV1` to
/// the websocket sink that spans all payload checkpoints since the last
/// barrier.
///
/// After publishing a flashblock it will place a new barrier in the payload
/// marking all checkpoints so far as immutable.
pub struct PublishFlashblock {
	/// The websocket interface that manages flashblock publishing to external
	/// subscribers.
	sink: Arc<WebSocketSink>,

	/// Keeps track of the current flashblock number within the payload job.
	flashblock_number: AtomicU64,

	/// Set once at the begining of the payload job, captures immutable
	/// information about the payload that is being built. This info is derived
	/// from the payload attributes parameter on the FCU from the EL node.
	block_base: RwLock<Option<ExecutionPayloadBaseV1>>,

	/// Metrics for monitoring flashblock publishing.
	metrics: Metrics,

	/// Timestamps for various stages of the flashblock publishing process. This
	/// information is used to produce some of the metrics.
	times: Times,

	/// Should we calculate the state root for each flashblock
	pub calculate_state_root: bool,
}

impl PublishFlashblock {
	pub fn new(sink: &Arc<WebSocketSink>, calculate_state_root: bool) -> Self {
		Self {
			sink: Arc::clone(sink),
			flashblock_number: AtomicU64::default(),
			block_base: RwLock::new(None),
			metrics: Metrics::default(),
			times: Times::default(),
			calculate_state_root,
		}
	}
}

impl Step<Flashblocks> for PublishFlashblock {
	async fn step(
		self: std::sync::Arc<Self>,
		payload: Checkpoint<Flashblocks>,
		ctx: StepContext<Flashblocks>,
	) -> ControlFlow<Flashblocks> {
		let this_block_span = self.unpublished_payload(&payload);
		let transactions: Vec<_> = this_block_span
			.transactions()
			.map(|tx| tx.encoded_2718().into())
			.collect();

		// increment flashblock number
		let index = self.flashblock_number.fetch_add(1, Ordering::SeqCst);

		let base = self.block_base.read().clone();
		let diff = ExecutionPayloadFlashblockDeltaV1 {
			state_root: self.compute_state_root(&payload, &ctx),
			receipts_root: B256::ZERO, // TODO: compute receipts root
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
			self.metrics.websocket_publish_errors_total.increment(1);
			tracing::error!("Failed to publish flashblock to websocket: {e}");

			// on transport error, do not place a barrier, just return the payload as
			// is. it may be picked up by the next iteration.
			return ControlFlow::Ok(payload);
		}

		// block published to WS successfully
		self.times.on_published_block(&self.metrics);
		self.capture_payload_metrics(&this_block_span);

		// Place a barrier after each published flashblock to freeze the contents
		// of the payload up to this point, since this becomes a publicly committed
		// state.
		ControlFlow::Ok(payload.barrier())
	}

	/// Before the payload job starts prepare the contents of the
	/// `ExecutionPayloadBaseV1` since at this point we have all the information
	/// we need to construct it and its content do not change throughout the job.
	async fn before_job(
		self: Arc<Self>,
		ctx: StepContext<Flashblocks>,
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

	/// After a payload job completes, capture metrics and reset payload job
	/// specific state.
	async fn after_job(
		self: Arc<Self>,
		_: StepContext<Flashblocks>,
		_: Arc<Result<types::BuiltPayload<Flashblocks>, PayloadBuilderError>>,
	) -> Result<(), PayloadBuilderError> {
		self.times.on_job_ended(&self.metrics);

		// reset flashblocks block counter
		let count = self.flashblock_number.swap(0, Ordering::SeqCst);
		self.metrics.blocks_per_payload_job.record(count as f64);
		*self.block_base.write() = None;

		Ok(())
	}

	/// Called during pipeline instantiation before any payload job is served.
	/// - Configure metrics scope.
	fn setup(
		&mut self,
		ctx: InitContext<Flashblocks>,
	) -> Result<(), PayloadBuilderError> {
		self.metrics = Metrics::with_scope(ctx.metrics_scope());
		Ok(())
	}
}

impl PublishFlashblock {
	/// Returns a span that covers all payload checkpoints since the last barrier.
	/// Those are the transactions that are going to be published in this
	/// flashblock.
	///
	/// One exception is the first flashblock, we want to get all checkpoints
	/// since the begining of the block, because the `OptimismPrologue` step
	/// places a barrier after sequencer transactions and we want to broadcast
	/// those transactions as well.
	fn unpublished_payload(
		&self,
		payload: &Checkpoint<Flashblocks>,
	) -> Span<Flashblocks> {
		if self.flashblock_number.load(Ordering::SeqCst) == 0 {
			// first block, get all checkpoints, including sequencer txs
			payload.history()
		} else {
			// subsequent block, get all checkpoints since last barrier
			payload.history_mut()
		}
	}

	fn compute_state_root(
		&self,
		payload: &Checkpoint<Flashblocks>,
		ctx: &StepContext<Flashblocks>,
	) -> B256 {
		if !self.calculate_state_root {
			return B256::ZERO;
		}

		let state_root_start_time = Instant::now();

		let state_provider = ctx.block().base_state();
		let hashed_state =
			state_provider.hashed_post_state(payload.state().unwrap());
		let (state_root, _trie_output) = state_provider
			.state_root_with_updates(hashed_state)
			.unwrap();

		let state_root_calculation_time = state_root_start_time.elapsed();
		self
			.metrics
			.state_root_calculation_duration
			.record(state_root_calculation_time);
		self
			.metrics
			.state_root_calculation_gauge
			.set(state_root_calculation_time);

		state_root
	}

	/// Called for each flashblock to capture metrics about the produced
	/// flashblock contents.
	fn capture_payload_metrics(&self, span: &Span<Flashblocks>) {
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

	/// The number of failures on the websocket transport layer.
	pub websocket_publish_errors_total: Counter,

	/// Histogram of state root calculation duration
	pub state_root_calculation_duration: Histogram,

	/// Latest state root calculation duration
	pub state_root_calculation_gauge: Gauge,
}

/// Used to track timing information for metrics.
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

/// Manages the physical WebSocket transport layer.
///
/// This type is responsible for listening on new client connections and
/// broadcasting new flashblocks to all connected peers through a websocket.
pub struct WebSocketSink {
	pipe: broadcast::Sender<Utf8Bytes>,
	term: watch::Sender<bool>,
}

impl WebSocketSink {
	pub fn new(address: SocketAddr) -> eyre::Result<Self> {
		let (pipe, _) = broadcast::channel(100);
		let (term, _) = watch::channel(false);

		let listener = TcpListener::bind(address)?;

		tokio::spawn(Self::listener_loop(
			listener,
			term.subscribe(),
			pipe.subscribe(),
		));

		Ok(Self { pipe, term })
	}

	/// Watch for pipeline shutdown signals and stops the WebSocket publisher.
	pub fn watch_shutdown(&self, pipeline: &Pipeline<Flashblocks>) {
		let term = self.term.clone();
		let mut dropped_signal = pipeline.subscribe::<PipelineDropped>();
		tokio::spawn(async move {
			while (dropped_signal.next().await).is_some() {
				let _ = term.send(true);
			}
		});
	}

	/// Called once by the `PublishFlashblock` pipeline step every time there is a
	/// non-empty flashblock that needs to be broadcasted to all external
	/// subscribers.
	pub fn publish(&self, payload: &FlashblocksPayloadV1) -> io::Result<usize> {
		// Serialize the payload to a UTF-8 string
		// serialize only once, then just copy around only a pointer
		// to the serialized data for each subscription.

		let serialized = serde_json::to_string(payload)?;
		let utf8_bytes = Utf8Bytes::from(serialized);
		let size = utf8_bytes.len();

		// send the serialized payload to all subscribers
		self
			.pipe
			.send(utf8_bytes)
			.map_err(|e| io::Error::new(io::ErrorKind::ConnectionAborted, e))?;

		trace!("Broadcasting flashblock: {:?}", payload);

		Ok(size)
	}

	async fn listener_loop(
		listener: TcpListener,
		term: watch::Receiver<bool>,
		payloads: broadcast::Receiver<Utf8Bytes>,
	) {
		listener
			.set_nonblocking(true)
			.expect("Failed to set TcpListener socket to non-blocking");

		let listener = tokio::net::TcpListener::from_std(listener)
			.expect("Failed to convert TcpListener to tokio TcpListener");

		let listen_addr = listener
			.local_addr()
			.expect("Failed to get local address of listener");
		tracing::info!("Flashblocks WebSocket listening on {listen_addr}");

		let mut term = term;

		loop {
			tokio::select! {
				// Stop this loop if the pipeline or the publisher insteances are dropped
				_ = term.changed() => {
					if *term.borrow() {
						return;
					}
				}

				// Accept new connections on the websocket listener
				// when a new connection is established, spawn a dedicated task to handle
				// the connection and broadcast with that connection.
				Ok((connection, peer_addr)) = listener.accept() => {
						let term = term.clone();
						let receiver_clone = payloads.resubscribe();

						match accept_async(connection).await {
							Ok(stream) => {
								tokio::spawn(async move {
									debug!("WebSocket connection established with {peer_addr}");
									// Handle the WebSocket connection in a dedicated task
									Self::broadcast_loop(stream, term, receiver_clone).await;
									debug!("WebSocket connection closed for {peer_addr}");
								});
							}
							Err(e) => {
								debug!("Failed to accept WebSocket connection from {peer_addr}: {e}");
							}
						}
				}
			}
		}
	}

	async fn broadcast_loop(
		stream: WebSocketStream<TcpStream>,
		term: watch::Receiver<bool>,
		blocks: broadcast::Receiver<Utf8Bytes>,
	) {
		let mut term = term;
		let mut blocks = blocks;
		let mut stream = stream;

		let Ok(peer_addr) = stream.get_ref().peer_addr() else {
			return;
		};

		loop {
			tokio::select! {
				_ = term.changed() => {
					if *term.borrow() {
						return;
					}
				}

				// Receive payloads from the broadcast channel
				payload = blocks.recv() => match payload {
					Ok(payload) => {
							if let Err(e) = stream.send(Message::Text(payload)).await {
									trace!("Closing flashblocks subscription for {peer_addr}: {e}");
									break; // Exit the loop if sending fails
							}
					}
					Err(RecvError::Closed) => {
							return;
					}
					Err(RecvError::Lagged(_)) => {
							trace!("Broadcast channel lagged, some messages were dropped for peer {peer_addr}");
					}
				},

				Some(message) = stream.next() => {
					match message {
						Ok(Message::Close(_)) => {
								trace!("Websocket connection closed graccefully by {peer_addr}");
								break;
						}
						Err(e) => {
								debug!("Websocket connection error for {peer_addr}: {e}");
								break;
						}
						_ => (),
					}
				}
			}
		}
	}
}

impl Drop for WebSocketSink {
	fn drop(&mut self) {
		// Notify the listener loop to terminate
		let _ = self.term.send(true);
	}
}
