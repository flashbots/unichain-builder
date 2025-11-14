use {
	core::{
		net::{IpAddr, Ipv4Addr, SocketAddr},
		ops::Deref,
	},
	derive_more::Deref,
	futures::{FutureExt, StreamExt},
	itertools::Itertools,
	orx_concurrent_vec::ConcurrentVec,
	rollup_boost_types::flashblocks::FlashblocksPayloadV1,
	std::{sync::Arc, time::Instant},
	tokio::{net::TcpStream, sync::oneshot, task::JoinHandle},
	tokio_tungstenite::{
		MaybeTlsStream,
		WebSocketStream,
		connect_async,
		tungstenite::{self, Message},
	},
	tracing::debug,
};

/// Attaches to a Flashblocks websocket and stores everything that is
/// broadcasted by the builder. Allows testing various expectations.
pub struct WebSocketObserver {
	observations: Arc<Observations>,
	term: Option<oneshot::Sender<()>>,
	worker: Option<JoinHandle<eyre::Result<()>>>,
}

impl WebSocketObserver {
	pub async fn new(socket_addr: SocketAddr) -> eyre::Result<Self> {
		let mut socket_addr = socket_addr;
		if socket_addr.ip().is_unspecified() {
			socket_addr.set_ip(IpAddr::V4(Ipv4Addr::LOCALHOST));
		}

		let observations = Arc::new(Observations::default());
		let (ws_stream, _) = connect_async(format!("ws://{socket_addr}")).await?;
		let (term_tx, term_rx) = oneshot::channel();

		let task = tokio::spawn(Self::runloop(
			ws_stream,
			term_rx,
			Arc::clone(&observations),
		));

		Ok(Self {
			observations,
			term: Some(term_tx),
			worker: Some(task),
		})
	}

	async fn runloop(
		ws_stream: WebSocketStream<MaybeTlsStream<TcpStream>>,
		term: oneshot::Receiver<()>,
		observations: Arc<Observations>,
	) -> eyre::Result<()> {
		let (_, mut ws_read) = ws_stream.split();
		let mut term = term.into_stream();

		loop {
			tokio::select! {
				Some(msg) = ws_read.next() => observations.record_ws(msg),
				_ = term.next() => break Ok(())
			}
		}
	}
}

impl Drop for WebSocketObserver {
	fn drop(&mut self) {
		if let (Some(term), Some(worker)) = (self.term.take(), self.worker.take()) {
			let _ = term.send(());
			worker.abort();
		}
	}
}

impl Deref for WebSocketObserver {
	type Target = Observations;

	fn deref(&self) -> &Self::Target {
		&self.observations
	}
}

#[derive(Clone, Debug, Deref)]
pub struct ObservedFlashblock {
	#[deref]
	pub block: FlashblocksPayloadV1,
	pub at: Instant,
}

#[derive(Debug, Default)]
pub struct Observations {
	errors: ConcurrentVec<eyre::Error>,
	messages: ConcurrentVec<Message>,
	flashblocks: ConcurrentVec<ObservedFlashblock>,
}

/// public validation api
impl Observations {
	pub fn len(&self) -> usize {
		self.flashblocks.len()
	}

	pub fn has_no_errors(&self) -> bool {
		self.errors.is_empty()
	}

	/// Iterates over all seen flashblocks.
	pub fn iter(&self) -> impl Iterator<Item = ObservedFlashblock> {
		self.flashblocks.iter_cloned()
	}

	/// Returns all observed flashblocks produced for a given block number
	/// The returned flashblocks will be returned in the same order in which they
	/// were observed.
	pub fn by_block_number(&self, block_number: u64) -> Vec<ObservedFlashblock> {
		// Only the base flashblock contains the block number
		let base_flashblock = self
			.iter()
			.find(|fb| {
				fb.block
					.base
					.as_ref()
					.is_some_and(|b| b.block_number == block_number)
			})
			.unwrap_or_else(|| {
				panic!("no base flashblock found for block {block_number}")
			});

		self
			.iter()
			.filter(|fb| fb.payload_id == base_flashblock.payload_id)
			.sorted_by_cached_key(|fb| fb.at) // sort by observation time
			.collect()
	}
}

// internal write API
impl Observations {
	fn record_ws(&self, msg: Result<Message, tungstenite::Error>) {
		match msg {
			Ok(msg) => self.record_message(msg),
			Err(error) => {
				self.errors.push(error.into());
			}
		}
	}

	fn record_message(&self, msg: Message) {
		if let Message::Text(string) = &msg {
			match serde_json::from_str::<FlashblocksPayloadV1>(string) {
				Ok(fb_payload) => self.record_flashblock(fb_payload),
				Err(error) => {
					self.errors.push(error.into());
				}
			}
		}

		self.messages.push(msg);
	}

	fn record_flashblock(&self, payload: FlashblocksPayloadV1) {
		debug!("observed flashblock: {payload:#?}");
		self.flashblocks.push(ObservedFlashblock {
			block: payload,
			at: Instant::now(),
		});
	}
}
