use std::collections::HashMap;

use axum::{
	async_trait,
	extract::ws::{self, WebSocket},
};
use futures::{
	stream::{SplitSink, SplitStream},
	SinkExt, StreamExt,
};
use mediasoup::{
	consumer::ConsumerType,
	prelude::{
		Consumer, ConsumerId, DtlsParameters, IceCandidate, IceParameters, MediaKind, Producer,
		ProducerId, RtpCapabilities, RtpCapabilitiesFinalized, RtpParameters, TransportId,
		WebRtcTransport,
	},
};
use serde::{Deserialize, Serialize};

use crate::{room, uid::Uid};

use tokio::sync::mpsc;

// sent by clients
#[derive(Serialize, Deserialize, Debug)]
pub enum Client2Server {
	CreateRtcTransport {
		force_tcp: Option<bool>,
		produce: bool,
		consume: bool,
	},
	ConnectRtcTransport {
		id: TransportId,
		dtls_params: DtlsParameters,
	},
	Join {
		rtp_caps: RtpCapabilities,
	},
}

// sent or relayed by the server
#[derive(Serialize, Deserialize, Debug)]
pub enum Server2Client {
	Welcome {
		rtp_caps: RtpCapabilitiesFinalized,
	},
	OnJoin {
		others: Vec<Uid>,
	},
	NewPeerJoined {
		user_id: Uid,
	},
	OnNewTransport {
		id: TransportId,
		ice_candidates: Vec<IceCandidate>,
		ice_params: IceParameters,
		dtls_params: DtlsParameters,
	},
	OnNewConsumer {
		producer_peer_id: Uid,
		producer_id: ProducerId,
		consumer_id: ConsumerId,
		kind: MediaKind,
		rtp_params: RtpParameters,
		consumer_type: ConsumerType,
		producer_paused: bool,
		is_share: bool,
	},
	ConsumerClosed {
		consumer_id: ConsumerId,
		// may be redundant
		producer_peer_id: Uid,
	},
	ConsumerPaused {
		consumer_id: ConsumerId,
	},
	ConsumerResumed {
		consumer_id: ConsumerId,
	},
}

// impl TryFrom

#[derive(Debug)]
pub enum PeerEvent {
	Welcome {
		rtp_caps: RtpCapabilitiesFinalized,
	},
	Denied,
	OnJoin {
		others: Vec<Uid>,
	},
	NewPeerJoined {
		user_id: Uid,
	},
	OnNewConsumer {
		producer_peer_id: Uid,
		producer_id: ProducerId,
		consumer_id: ConsumerId,
		kind: MediaKind,
		rtp_params: RtpParameters,
		consumer_type: ConsumerType,
		producer_paused: bool,
		is_share: bool,
	},
	// FIXME: send directly to the room?
	OnConsumerTransportClose {
		consumer_id: ConsumerId,
	},
	OnProducerClose {
		consumer_id: ConsumerId,
		producer_peer_id: Uid,
	},
	OnProducerPause {
		consumer_id: ConsumerId,
	},
	OnProducerResume {
		consumer_id: ConsumerId,
	},
	Rcvd(Client2Server),
	OnNewTransport {
		id: TransportId,
		ice_candidates: Vec<IceCandidate>,
		ice_params: IceParameters,
		dtls_params: DtlsParameters,
	},

	Close,
}

#[async_trait]
trait SigSender {
	async fn send(&mut self, msg: &Server2Client) -> Result<(), Error>;
}

struct WsSigSender {
	sock: SplitSink<WebSocket, ws::Message>,
}

#[async_trait]
impl SigSender for WsSigSender {
	async fn send(&mut self, msg: &Server2Client) -> Result<(), Error> {
		self.sock
			.send(ws::Message::Text(serde_json::to_string(msg).unwrap()))
			.await
			.map_err(|_| Error::Socket)
	}
}

#[derive(Clone)]
pub struct PeerTransport {
	pub transport: WebRtcTransport,
	pub produce: bool,
	pub consume: bool,
}

#[derive(Clone)]
pub struct PeerProducer {
	pub producer: Producer,
	pub is_share: bool,
}

#[derive(Clone)]
pub struct Peer {
	// these are all arcs, so should be ok to clone
	pub tx: mpsc::Sender<PeerEvent>,
	pub joined: bool,
	pub transports: HashMap<TransportId, PeerTransport>,
	pub consumers: HashMap<ConsumerId, Consumer>,
	pub producers: HashMap<ProducerId, PeerProducer>,
	pub rtp_caps: Option<RtpCapabilities>,
}

impl Peer {
	pub fn new(tx: mpsc::Sender<PeerEvent>) -> Self {
		Self {
			tx,
			joined: false,
			transports: HashMap::new(),
			consumers: HashMap::new(),
			producers: HashMap::new(),
			rtp_caps: None,
		}
	}
}

pub async fn create_and_start_receiving(
	user_id: Uid,
	room_tx: mpsc::Sender<room::Event>,
	sender: SplitSink<WebSocket, ws::Message>,
	receiver: SplitStream<WebSocket>,
) {
	let (tx, rx) = mpsc::channel(1024);

	// so, this should be a different event initially, eg KnockKnock
	_ = room_tx
		.send(room::Event::KnockKnock {
			user_id,
			tx: tx.clone(),
		})
		.await;

	run_loop(WsSigSender { sock: sender }, user_id, rx, room_tx).await;
	receive_from_ws(receiver, user_id, tx).await;
}

async fn run_loop(
	mut sender: WsSigSender,
	user_id: Uid,
	mut rx: mpsc::Receiver<PeerEvent>,
	room_tx: mpsc::Sender<room::Event>,
) {
	tokio::spawn(async move {
		while let Some(event) = rx.recv().await {
			match event {
				PeerEvent::Welcome { rtp_caps } => {
					_ = sender.send(&Server2Client::Welcome { rtp_caps }).await;
				}
				PeerEvent::Denied => {
					tracing::error!("access refused for {user_id}");

					drop(sender);
					break;
				}
				PeerEvent::OnJoin { others } => {
					tracing::debug!("{user_id} on join");
					_ = sender.send(&Server2Client::OnJoin { others }).await;
				}
				PeerEvent::NewPeerJoined { user_id } => {
					_ = sender.send(&Server2Client::NewPeerJoined { user_id }).await;
				}
				PeerEvent::OnNewConsumer {
					producer_peer_id,
					producer_id,
					consumer_id,
					kind,
					rtp_params,
					consumer_type,
					producer_paused,
					is_share,
				} => {
					_ = sender
						.send(&Server2Client::OnNewConsumer {
							producer_peer_id,
							producer_id,
							consumer_id,
							kind,
							rtp_params,
							consumer_type,
							producer_paused,
							is_share,
						})
						.await;
				}
				PeerEvent::OnNewTransport {
					id,
					ice_candidates,
					ice_params,
					dtls_params,
				} => {
					_ = sender
						.send(&Server2Client::OnNewTransport {
							id,
							ice_candidates,
							ice_params,
							dtls_params,
						})
						.await;
				}
				PeerEvent::OnConsumerTransportClose { consumer_id } => {
					_ = room_tx
						.send(room::Event::OnConsumerTransportClose {
							user_id,
							consumer_id,
						})
						.await;
				}
				PeerEvent::OnProducerClose {
					consumer_id,
					producer_peer_id,
				} => {
					_ = room_tx
						.send(room::Event::OnProducerClose {
							user_id,
							consumer_id,
						})
						.await;
					_ = sender
						.send(&Server2Client::ConsumerClosed {
							consumer_id,
							producer_peer_id,
						})
						.await;
				}
				PeerEvent::OnProducerPause { consumer_id } => {
					_ = sender
						.send(&Server2Client::ConsumerPaused { consumer_id })
						.await;
				}
				PeerEvent::OnProducerResume { consumer_id } => {
					_ = sender
						.send(&Server2Client::ConsumerResumed { consumer_id })
						.await;
				}
				PeerEvent::Rcvd(msg) => {
					tracing::debug!("received msg: {:?}", msg);

					match msg {
						Client2Server::CreateRtcTransport {
							force_tcp,
							produce,
							consume,
						} => {
							_ = room_tx
								.send(room::Event::CreateRtcTransport {
									user_id,
									force_tcp,
									produce,
									consume,
								})
								.await;
						}
						Client2Server::ConnectRtcTransport { id, dtls_params } => {
							_ = room_tx
								.send(room::Event::ConnectRtcTransport {
									user_id,
									transport_id: id,
									dtls_params,
								})
								.await;
						}
						Client2Server::Join { rtp_caps } => {
							_ = room_tx.send(room::Event::Join { user_id, rtp_caps }).await;
						}
					}
				}
				PeerEvent::Close => {
					// comes from ws/tcp, hence others should be notified
					// close the sending socket to leave the we loop
					// FIXME: notify other peers here or by the room, if joined == true
					// FIXME: close transport?
					drop(sender);
					_ = room_tx.send(room::Event::Leave { user_id }).await;
					break;
				}
			}
		}
	});
}

async fn receive_from_ws(
	mut receiver: SplitStream<WebSocket>,
	user_id: Uid,
	tx: mpsc::Sender<PeerEvent>,
) {
	use ws::Message::*;

	tokio::spawn(async move {
		while let Some(Ok(msg)) = receiver.next().await {
			match msg {
				Text(t) => {
					if let Ok(msg) = serde_json::from_str::<Client2Server>(&t) {
						_ = tx.send(PeerEvent::Rcvd(msg)).await;
					} else {
						tracing::debug!("{user_id} sent unknown text: {t:?}");
					}
				}
				Close(c) => {
					if let Some(cf) = c {
						tracing::warn!(
							"{} sent close with code {} and reason `{}`",
							user_id,
							cf.code,
							cf.reason
						);
					} else {
						tracing::warn!("{user_id} somehow sent close message without CloseFrame");
					}

					_ = tx.send(PeerEvent::Close).await;

					break;
				}

				msg => {
					tracing::error!("{user_id} sent unexpected msg type: {:?}.", msg);
				}
			}
		}
	});
}

pub enum Error {
	Socket,
	ConnectionClosed,
}

// type SignallingImpl = Wsockets;

// pub struct WsSender {
// 	pub sender: SplitSink<WebSocket, ws::Message>,
// 	// pub receiver: SplitStream<WebSocket>,
// }

// #[async_trait]
// pub trait SigSender {
// 	async fn send(&mut self, buf: &str) -> Result<(), Error>;
// }

// #[async_trait]
// impl SigSender for WsSender {
// 	async fn send(&mut self, buf: &str) -> Result<(), Error> {
// 		Ok(self
// 			.sender
// 			.send(ws::Message::Text(buf.to_string()))
// 			.await
// 			.map_err(|_| Error::Socket)?)
// 	}
// }

// async fn process_ws_message(
// 	msg: ws::Message,
// 	who: SocketAddr,
// 	peer: &Peer,
// ) -> ControlFlow<(), ()> {
// 	use ws::Message::*;

// 	match msg {
// 		Text(t) => {
// 			// json encoded
// 			tracing::debug!("{} @ {} sent str: {t:?}", peer.id.to_base64(), who);

// 			if let Ok(msg) = serde_json::from_str::<peer::Client2Server>(&t) {
// 				// handle errors somehow?
// 				// let r = peer.receive(msg);
// 			}
// 		}
// 		Close(c) => {
// 			// room close
// 			// server close room, if empty
// 			// state
// 			peer.close().await;

// 			if let Some(cf) = c {
// 				tracing::warn!(
// 					">>> {} sent close with code {} and reason `{}`",
// 					who,
// 					cf.code,
// 					cf.reason
// 				);
// 			} else {
// 				tracing::warn!(">>> {who} somehow sent close message without CloseFrame");
// 			}
// 			return ControlFlow::Break(());
// 		}

// 		msg => {
// 			tracing::error!("Received unexpected {:?}.", msg);
// 		}
// 	}
// 	ControlFlow::Continue(())
// }
