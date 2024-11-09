use std::collections::HashMap;

use crate::{
	peer::{self, Peer, PeerProducer, PeerTransport},
	uid::Uid,
};
use mediasoup::{
	consumer,
	prelude::{
		ConsumerId, ConsumerLayers, ConsumerOptions, DtlsParameters, MediaKind, ProducerId,
		ProducerOptions, RtpCapabilities, RtpParameters, Transport, TransportId, WebRtcServer,
		WebRtcTransportOptions, WebRtcTransportRemoteParameters,
	},
	router::Router,
};
use tokio::sync::mpsc;

pub enum Event {
	KnockKnock {
		user_id: Uid,
		tx: mpsc::Sender<peer::PeerEvent>,
	},
	Join {
		user_id: Uid,
		rtp_caps: RtpCapabilities,
	},
	Produce {
		user_id: Uid,
		transport_id: TransportId,
		kind: MediaKind,
		rtp_params: RtpParameters,
		is_share: bool,
	},
	CreateRtcTransport {
		user_id: Uid,
		force_tcp: Option<bool>,
		produce: bool,
		consume: bool,
	},
	ConnectRtcTransport {
		user_id: Uid,
		transport_id: TransportId,
		dtls_params: DtlsParameters,
	},
	RestartIce {
		user_id: Uid,
		transport_id: TransportId,
	},
	OnConsumerTransportClose {
		user_id: Uid,
		consumer_id: ConsumerId,
	},
	OnProducerClose {
		consumer_peer_id: Uid,
		consumer_id: ConsumerId,
	},
	CloseProducer {
		producer_peer_id: Uid,
		id: ProducerId,
	},
	PauseProducer {
		producer_peer_id: Uid,
		id: ProducerId,
	},
	ResumeProducer {
		producer_peer_id: Uid,
		id: ProducerId,
	},
	PauseConsumer {
		consumer_peer_id: Uid,
		id: ConsumerId,
	},
	ResumeConsumer {
		consumer_peer_id: Uid,
		id: ConsumerId,
	},
	SetConsumerLayers {
		user_id: Uid,
		consumer_id: ConsumerId,
		spatial: u8,
		temporal: u8,
	},
	SetConsumerPriority {
		user_id: Uid,
		consumer_id: ConsumerId,
		priority: u8,
	},
	Leave {
		user_id: Uid,
	},
	KillAll,
}

trait PeerFilter {
	fn joined_excluding(&self, peer: Uid) -> HashMap<Uid, Peer>;
}

impl PeerFilter for HashMap<Uid, Peer> {
	fn joined_excluding(&self, peer: Uid) -> HashMap<Uid, Peer> {
		self.iter()
			.filter_map(|(id, p)| (p.joined && *id != peer).then(|| (id.clone(), p.clone())))
			.collect()
	}
}

async fn create_consumer(
	consumer_peer: &mut Peer,
	producer_peer_id: &Uid,
	producer_id: &ProducerId,
	is_share: bool,
	router: &Router,
) {
	// no need to create a consumer, unless one is supported via caps and can be consumed
	if let Some(ref caps) = consumer_peer.rtp_caps {
		if router.can_consume(producer_id, caps) {
			if let Some(transport) = consumer_peer
				.transports
				.values()
				.find(|t| t.consume)
				.as_mut()
			{
				let mut options = ConsumerOptions::new(producer_id.clone(), caps.clone());
				options.paused = true;
				// FIXME: check whether default value is enough
				// true is for opus NACKs
				// options.enable_rtx = Some(true);

				if let Ok(consumer) = transport.transport.consume(options).await {
					// FIXME: do I need to detach?

					let user_tx = consumer_peer.tx.clone();
					let consumer_id = consumer.id();
					consumer
						.on_transport_close(move || {
							tokio::spawn(async move {
								_ = user_tx
									.send(peer::PeerEvent::OnConsumerTransportClose { consumer_id })
									.await;
							});
						})
						.detach();

					let user_tx = consumer_peer.tx.clone();
					let producer_peer_id = producer_peer_id.clone();
					consumer
						.on_producer_close(move || {
							tokio::spawn(async move {
								_ = user_tx
									.send(peer::PeerEvent::OnProducerClose {
										consumer_id,
										producer_peer_id,
									})
									.await;
							});
						})
						.detach();

					let user_tx = consumer_peer.tx.clone();
					consumer
						.on_producer_pause(move || {
							let user_tx = user_tx.clone();
							tokio::spawn(async move {
								_ = user_tx
									.send(peer::PeerEvent::OnProducerPause { consumer_id })
									.await;
							});
						})
						.detach();

					let user_tx = consumer_peer.tx.clone();
					consumer
						.on_producer_resume(move || {
							let user_tx = user_tx.clone();
							tokio::spawn(async move {
								_ = user_tx
									.send(peer::PeerEvent::OnProducerResume { consumer_id })
									.await;
							});
						})
						.detach();

					// TODO: implement score, layerschange, trace (ui/log-related mostly)

					_ = consumer_peer
						.tx
						.send(peer::PeerEvent::OnNewConsumer {
							producer_peer_id: producer_peer_id.clone(),
							producer_id: producer_id.clone(),
							consumer_id: consumer_id,
							kind: consumer.kind(),
							rtp_params: consumer.rtp_parameters().clone(),
							consumer_type: consumer.r#type(),
							producer_paused: consumer.producer_paused(),
							is_share,
						})
						.await;

					consumer_peer.consumers.insert(consumer_id, consumer);
					// TODO: the client is to send ResumeConsumer { consumer_id } to resume the consumer
					// TODO: send OnConsumerScore to consumer_peer? -probably not, as long as `on_score` is implemented
				} else {
					tracing::error!("failed to create a consumer for producer_id {producer_id} of peer {producer_peer_id}");
				}
			}
		}
	}
}

// dspawn here
pub async fn create_and_start_receiving(
	id: Uid,
	router: Router,
	rtc_server: WebRtcServer,
	mut event_rx: mpsc::Receiver<Event>,
) {
	// do I need 'joined'?
	let mut peers: HashMap<Uid, Peer> = HashMap::new();

	tracing::warn!("created room {id}");

	while let Some(event) = event_rx.recv().await {
		match event {
			Event::KnockKnock { user_id, tx } => {
				if peers.contains_key(&user_id) {
					// or kick out the one who's already been accepted?
					_ = tx.send(peer::PeerEvent::Denied).await;
				} else {
					peers.insert(user_id, Peer::new(tx.clone()));

					_ = tx
						.send(peer::PeerEvent::Welcome {
							rtp_caps: router.rtp_capabilities().clone(),
						})
						.await;

					tracing::info!("{user_id} is knocking; {} users now connected", peers.len());
				}
			}
			// FIXME: make rtp_caps optional, unless we want to consume?
			Event::Join { user_id, rtp_caps } => {
				// knock-knock is required before joining
				let others = peers.joined_excluding(user_id);

				if let Some(peer) = peers.get_mut(&user_id) {
					if peer.joined {
						tracing::error!("{user_id} has already joined");
					} else {
						peer.joined = true;
						peer.rtp_caps = Some(rtp_caps);

						// sent rooster to the newly joined peer
						_ = peer
							.tx
							.send(peer::PeerEvent::OnJoin {
								others: others.iter().map(|(&id, _)| id).collect(),
							})
							.await;

						for (other_id, other) in others.iter() {
							for (producer_id, producer) in other.producers.iter() {
								create_consumer(
									peer,
									other_id,
									producer_id,
									producer.is_share,
									&router,
								)
								.await;
							}
						}

						// notify the rest
						for (_, peer) in others {
							_ = peer
								.tx
								.send(peer::PeerEvent::NewPeerJoined { user_id })
								.await;
						}
					}
				} else {
					tracing::error!("{user_id} is trying to join, but does not belong to {id}");
				}
			}
			Event::Produce {
				user_id,
				transport_id,
				kind,
				rtp_params,
				is_share,
			} => {
				let mut others = peers.joined_excluding(user_id);

				if let Some(peer) = peers.get_mut(&user_id) {
					if peer.joined {
						if let Some(transport) = peer.transports.get_mut(&transport_id) {
							let options = ProducerOptions::new(kind, rtp_params);
							if let Ok(producer) = transport.transport.produce(options).await {
								let pid = producer.id();

								peer.producers
									.insert(pid, PeerProducer { producer, is_share });

								_ = peer
									.tx
									.send(peer::PeerEvent::OnNewProducer { id: pid })
									.await;

								for (_, other) in others.iter_mut() {
									create_consumer(other, &user_id, &pid, is_share, &router).await;
								}

								// TODO: implement score, videoorientationchange, trace
								// TODO: if audio, implement audio lever/active speaker observers
							} else {
								tracing::error!("failed to create a producer for user {user_id} on transport {transport_id}");
							}
						} else {
							tracing::error!("no transport {transport_id} found for {user_id}");
						}
					} else {
						tracing::error!("{user_id} has not yet joined, but trying to produce on transport {transport_id}");
					}
				}
			}
			Event::CreateRtcTransport {
				user_id,
				force_tcp,
				produce,
				consume,
			} => {
				if let Some(peer) = peers.get_mut(&user_id) {
					let mut options = WebRtcTransportOptions::new_with_server(rtc_server.clone());

					if force_tcp.unwrap_or(false) {
						options.enable_udp = false;
						options.enable_tcp = true;
					}

					let transport = router.create_webrtc_transport(options).await.unwrap();

					let tid = transport.id();
					let ice_candidates = transport.ice_candidates().clone();
					let ice_params = transport.ice_parameters().clone();
					let dtls_params = transport.dtls_parameters().clone();

					peer.transports.insert(
						tid,
						PeerTransport {
							transport,
							produce,
							consume,
						},
					);

					// TODO: implement dtlsstatechange, trace: ui/logging mostly
					// transport.enable_trace_event(vec![Bwe]);

					_ = peer
						.tx
						.send(peer::PeerEvent::OnNewTransport {
							id: tid,
							ice_candidates,
							ice_params,
							dtls_params,
						})
						.await;
				} else {
					tracing::error!("create rtc transport failed: no user {user_id} found");
				}
			}
			Event::ConnectRtcTransport {
				user_id,
				transport_id,
				dtls_params,
			} => {
				if let Some(peer) = peers.get_mut(&user_id) {
					if let Some(transport) = peer.transports.get_mut(&transport_id) {
						_ = transport
							.transport
							.connect(WebRtcTransportRemoteParameters {
								dtls_parameters: dtls_params,
							})
							.await;
					} else {
						tracing::error!("no transport {transport_id} found for {user_id}");
					}
				}
			}
			Event::RestartIce {
				user_id,
				transport_id,
			} => {
				if let Some(peer) = peers.get_mut(&user_id) {
					if let Some(transport) = peer.transports.get_mut(&transport_id) {
						if let Ok(ice_params) = transport.transport.restart_ice().await {
							_ = peer
								.tx
								.send(peer::PeerEvent::IceRestarted { ice_params })
								.await
						} else {
							tracing::error!("failed to restart ice for transport {transport_id} found for {user_id}");
						}
					}
				}
			}
			Event::CloseProducer {
				producer_peer_id,
				id,
			} => {
				if let Some(peer) = peers.get_mut(&producer_peer_id) {
					if peer.joined && peer.producers.remove(&id).is_some() {
						// just drop to close
						// this will trigger on_producer_close for each of this producer's consumers
						tracing::debug!("closed producer {id} on {producer_peer_id}");
					} else {
						tracing::error!("no producer {id} found for user {producer_peer_id}");
					}
				}
			}
			Event::PauseProducer {
				producer_peer_id,
				id,
			} => {
				if let Some(peer) = peers.get_mut(&producer_peer_id) {
					if let Some(producer) = peer.producers.get_mut(&id) {
						if peer.joined && producer.producer.pause().await.is_ok() {
							tracing::debug!("paused producer {id} on {producer_peer_id}");
						}
					} else {
						tracing::error!("no producer {id} found for user {producer_peer_id}");
					}
				}
			}
			Event::ResumeProducer {
				producer_peer_id,
				id,
			} => {
				if let Some(peer) = peers.get_mut(&producer_peer_id) {
					if let Some(producer) = peer.producers.get_mut(&id) {
						if peer.joined && producer.producer.resume().await.is_ok() {
							tracing::debug!("resumed producer {id} on {producer_peer_id}");
						}
					} else {
						tracing::error!("no producer {id} found for user {producer_peer_id}");
					}
				}
			}
			Event::PauseConsumer {
				consumer_peer_id,
				id,
			} => {
				if let Some(peer) = peers.get_mut(&consumer_peer_id) {
					if let Some(consumer) = peer.consumers.get_mut(&id) {
						if peer.joined && consumer.pause().await.is_ok() {
							tracing::debug!("paused consumer {id} on {consumer_peer_id}");
						}
					} else {
						tracing::error!("no consumer {id} found for user {consumer_peer_id}");
					}
				}
			}
			Event::ResumeConsumer {
				consumer_peer_id,
				id,
			} => {
				if let Some(peer) = peers.get_mut(&consumer_peer_id) {
					if let Some(consumer) = peer.consumers.get_mut(&id) {
						if peer.joined && consumer.resume().await.is_ok() {
							tracing::debug!("resumed consumer {id} on {consumer_peer_id}");
						}
					} else {
						tracing::error!("no consumer {id} found for user {consumer_peer_id}");
					}
				}
			}
			Event::SetConsumerLayers {
				user_id,
				consumer_id,
				spatial,
				temporal,
			} => {
				if let Some(peer) = peers.get_mut(&user_id) {
					if let Some(consumer) = peer.consumers.get_mut(&consumer_id) {
						if peer.joined
							&& consumer
								.set_preferred_layers(ConsumerLayers {
									spatial_layer: spatial,
									temporal_layer: Some(temporal),
								})
								.await
								.is_ok()
						{
							tracing::debug!("set layers on consumer {consumer_id} for user {user_id}; s: {spatial}, t: {temporal}");
						}
					}
				}
			}
			Event::SetConsumerPriority {
				user_id,
				consumer_id,
				priority,
			} => {
				if let Some(peer) = peers.get_mut(&user_id) {
					if let Some(consumer) = peer.consumers.get_mut(&consumer_id) {
						if peer.joined && consumer.set_priority(priority).await.is_ok() {
							tracing::debug!("set priority {priority} on consumer {consumer_id} for user {user_id}");
						}
					}
				}
			}
			Event::OnConsumerTransportClose {
				user_id,
				consumer_id,
			} => {
				if let Some(peer) = peers.get_mut(&user_id) {
					peer.consumers.remove(&consumer_id);
				}
			}
			Event::OnProducerClose {
				consumer_peer_id,
				consumer_id,
			} => {
				// FIXME: this is confusing
				if let Some(peer) = peers.get_mut(&consumer_peer_id) {
					peer.consumers.remove(&consumer_id);
				}
			}
			Event::Leave { user_id } => {
				peers.remove(&user_id);

				tracing::info!("{user_id} left; {} users now connected", peers.len());

				if peers.is_empty() {
					tracing::warn!("room is empty, closing");
					// FIXME: send to serve as well
					break;
				}
			}
			Event::KillAll => {
				// force close the room
			}
		}
	}
}
