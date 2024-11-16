use std::collections::HashMap;

use crate::{
	peer::{self, Peer, PeerProducer, PeerTransport},
	uid::Uid,
};
use mediasoup::{
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
		rtp_caps: Option<RtpCapabilities>,
	},
	Produce {
		user_id: Uid,
		transport_id: TransportId,
		kind: MediaKind,
		rtp_params: RtpParameters,
		is_share: bool,
	},
	CreateRtcTransport {
		tag: Option<String>,
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
				// true is for opus NACKs
				options.enable_rtx = Some(true);

				if let Ok(consumer) = transport.transport.consume(options).await {
					let user_tx = consumer_peer.tx.clone();
					let consumer_id = consumer.id();
					consumer
						.on_transport_close(move || {
							_ = user_tx.try_send(peer::PeerEvent::OnConsumerTransportClose {
								consumer_id,
							});
						})
						.detach();

					let user_tx = consumer_peer.tx.clone();
					let producer_peer_id = producer_peer_id.clone();
					// called on MY consumer attached to THEIR producer when the producer closes
					// NOT related to any my producers!
					consumer
						.on_producer_close(move || {
							// this is called from a c++ worker thread, so we can't tokio::spawn to async-send
							// here; hence, try sending immediately which should work in all cases unless the queue
							// is full for some reason (should not be); if not, recipients still need to remove all
							// consumers associated with this peer when handling PeerLeft
							_ = user_tx.try_send(peer::PeerEvent::OnProducerClose {
								consumer_id,
								producer_peer_id,
							});
						})
						.detach();

					let user_tx = consumer_peer.tx.clone();
					consumer
						.on_producer_pause(move || {
							let user_tx = user_tx.clone();
							_ = user_tx.try_send(peer::PeerEvent::OnProducerPause { consumer_id });
						})
						.detach();

					let user_tx = consumer_peer.tx.clone();
					consumer
						.on_producer_resume(move || {
							let user_tx = user_tx.clone();
							_ = user_tx.try_send(peer::PeerEvent::OnProducerResume { consumer_id });
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

	tracing::info!("created room {id}");

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
							room_id: id,
							rtp_caps: router.rtp_capabilities().clone(),
						})
						.await;

					tracing::info!("{user_id} is knocking; {} users now connected", peers.len());
				}
			}
			Event::Join { user_id, rtp_caps } => {
				// 1 knock-knock
				// 2 create remote transports: snd & rcv
				// 3 create local snd & rcv transports(remote_params):
				//	 on_connect => `ConnectRtcTransport`, on_produce => `Produce`
				// 4 join
				// 5 start producing
				if let Some(mut peer) = peers.remove(&user_id) {
					if peer.joined {
						tracing::error!("{user_id} has already joined");
					} else {
						peer.joined = true;
						peer.rtp_caps = rtp_caps;

						// sent user ids to the newly joined peer
						_ = peer
							.tx
							.send(peer::PeerEvent::OnJoin {
								others: peers.iter().map(|(&id, _)| id).collect(),
							})
							.await;

						for (other_id, other) in peers.iter() {
							for (producer_id, producer) in other.producers.iter() {
								create_consumer(
									&mut peer,
									other_id,
									producer_id,
									producer.is_share,
									&router,
								)
								.await;
							}
						}

						// notify the rest
						for (_, peer) in peers.iter() {
							_ = peer
								.tx
								.send(peer::PeerEvent::NewPeerJoined { user_id })
								.await;
						}
					}

					peers.insert(user_id, peer);
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
				// when mic/cam is created, its transport first emits `Connect` and then `Produce`
				if let Some(mut peer) = peers.remove(&user_id) {
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

								for (_, other) in peers.iter_mut() {
									// TODO: execute in parallel?
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

					peers.insert(user_id, peer);
				}
			}
			Event::CreateRtcTransport {
				tag,
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

					transport.on_ice_state_change(move |state| {
						tracing::info!("ice state for user {user_id} is {:?} for tid {}", state, tid);
					}).detach();

					// TODO: implement dtlsstatechange, trace: ui/logging mostly
					peer.transports.insert(
						tid,
						PeerTransport {
							transport,
							produce,
							consume,
						},
					);
					
					// transport.enable_trace_event(vec![Bwe]);

					_ = peer
						.tx
						.send(peer::PeerEvent::OnNewTransport {
							tag,
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
						// this will trigger on_producer_close for each of this producer's consumers of other peers
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
						tracing::error!("no consumer {id} found for user {consumer_peer_id}; consumer has: {:?}", peer.consumers.keys());
					}
				}
			}
			Event::SetConsumerLayers {
				user_id,
				consumer_id,
				spatial,
				temporal,
			} => {
				// specifies the quality of the media I'm receiving from other people
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
				// when my consumer's producer's transport closes
				if let Some(peer) = peers.get_mut(&user_id) {
					peer.consumers.remove(&consumer_id);
				}
			}
			Event::OnProducerClose {
				consumer_peer_id,
				consumer_id,
			} => {
				// when my consumer's producer closes
				if let Some(peer) = peers.get_mut(&consumer_peer_id) {
					peer.consumers.remove(&consumer_id);
				}
			}
			Event::Leave { user_id } => {
				if let Some(peer) = peers.remove(&user_id) {
					if peer.joined {
						for (_, other) in peers.iter() {
							_ = other
								.tx
								.send(peer::PeerEvent::PeerLeft { id: user_id })
								.await;
						}
						// no need to manually close peer's transports – drop is enough
					}
				}

				tracing::info!("{user_id} left; {} users now connected", peers.len());

				if peers.is_empty() {
					// TODO: remove this room from the list
					tracing::warn!("room is empty, closing");
					break;
				}
			}
			Event::KillAll => {
				// force close the room
			}
		}
	}
}
