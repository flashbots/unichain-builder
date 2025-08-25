use {
	super::primitives::FlashblocksPayloadV1,
	crate::platform::FlashBlocks,
	core::net::SocketAddr,
	futures::{SinkExt, StreamExt},
	rblib::prelude::*,
	std::{io, net::TcpListener},
	tokio::{
		net::TcpStream,
		sync::{
			broadcast::{self, error::RecvError},
			watch,
		},
	},
	tokio_tungstenite::{
		WebSocketStream,
		accept_async,
		tungstenite::{Message, Utf8Bytes},
	},
	tracing::{debug, trace},
};

pub struct WebSocketPublisher {
	pipe: broadcast::Sender<Utf8Bytes>,
	term: watch::Sender<bool>,
}

impl WebSocketPublisher {
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
	pub fn watch_shutdown(&self, pipeline: &Pipeline<FlashBlocks>) {
		let term = self.term.clone();
		let mut dropped_signal = pipeline.subscribe::<PipelineDropped>();
		tokio::spawn(async move {
			while (dropped_signal.next().await).is_some() {
				let _ = term.send(true);
			}
		});
	}

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
									trace!("WebSocket connection established with {peer_addr}");
									// Handle the WebSocket connection in a dedicated task
									Self::broadcast_loop(stream, term, receiver_clone).await;
									trace!("WebSocket connection closed for {peer_addr}");
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

impl Drop for WebSocketPublisher {
	fn drop(&mut self) {
		// Notify the listener loop to terminate
		let _ = self.term.send(true);
	}
}
