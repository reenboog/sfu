use std::collections::HashMap;

use crate::{
	peer::{self, Peer, PeerTransport},
	uid::Uid,
};
use mediasoup::{
	prelude::{
		ConsumerId, ConsumerOptions, ProducerId, RtpCapabilities, Transport,
		WebRtcServer, WebRtcTransportOptions,
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
	CreateRtpTransport {
		user_id: Uid,
		produce: bool,
		consume: bool,
	},
	OnConsumerTransportClose {
		user_id: Uid,
		consumer_id: ConsumerId,
	},
	OnProducerClose {
		user_id: Uid,
		consumer_id: ConsumerId,
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
				// FIXME: check whether a default value is enough
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
						tracing::error!("{user_id} has already joined; something is messed up");
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
			Event::CreateRtpTransport {
				user_id,
				produce,
				consume,
			} => {
				if let Some(peer) = peers.get_mut(&user_id) {
					let transport = router
						.create_webrtc_transport(WebRtcTransportOptions::new_with_server(
							rtc_server.clone(),
						))
						.await
						.unwrap();

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

					// FIXME: implement on_* callbacks

					_ = peer
						.tx
						.send(peer::PeerEvent::OnNewTransport {
							id: tid,
							ice_candidates,
							ice_params,
							dtls_params,
						})
						.await;
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
				user_id,
				consumer_id,
			} => {
				if let Some(peer) = peers.get_mut(&user_id) {
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
