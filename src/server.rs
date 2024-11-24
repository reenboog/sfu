use std::{
	collections::HashMap,
	net::IpAddr,
	num::{NonZeroU32, NonZeroU8},
};

use mediasoup::{
	prelude::{
		ListenInfo, MimeTypeAudio, MimeTypeVideo, Protocol, RtcpFeedback, RtpCodecCapability,
		RtpCodecParametersParameters, WebRtcServer, WebRtcServerListenInfos, WebRtcServerOptions,
		WorkerManager,
	},
	router::RouterOptions,
	worker::{Worker, WorkerLogLevel, WorkerLogTag, WorkerSettings},
};
use tokio::sync::mpsc;

use crate::{audio_observer, room, uid::Uid};

#[derive(Debug)]
pub enum Error {
	Unknown,
	Worker,
	RtcServer,
	RoomAlreadyExists(Uid),
}

pub struct Server {
	mngr: WorkerManager,
	workers: Vec<(Worker, WebRtcServer)>,
	nex_worker_idx: usize,
	rooms: HashMap<Uid, mpsc::Sender<room::Event>>,
}

impl Server {
	pub async fn new(
		num_workers: usize,
		local_ip: IpAddr,
		announced_ip: &str,
		worker_min_port: u16,
		worker_max_port: u16,
		server_port: u16,
	) -> Result<Self, Error> {
		let mngr = WorkerManager::new();
		let workers = Self::create_workers(
			&mngr,
			num_workers,
			local_ip,
			announced_ip,
			worker_min_port,
			worker_max_port,
			server_port,
		)
		.await?;

		Ok(Self {
			mngr,
			workers,
			nex_worker_idx: 0,
			rooms: HashMap::new(),
		})
	}

	// TODO: inject listen ip, announcement ip, ports
	async fn create_workers(
		mngr: &WorkerManager,
		num_workers: usize,
		local_ip: IpAddr,
		announced_ip: &str,
		worker_min_port: u16,
		worker_max_port: u16,
		server_port: u16,
	) -> Result<Vec<(Worker, WebRtcServer)>, Error> {
		let mut workers = Vec::new();

		for worker_idx in 0..num_workers {
			let mut settings = WorkerSettings::default();
			// TODO: apply certs?
			settings.rtc_port_range = worker_min_port..=worker_max_port;
			settings.log_level = WorkerLogLevel::Debug;
			settings.log_tags = vec![
				WorkerLogTag::Info,
				WorkerLogTag::Ice,
				WorkerLogTag::Dtls,
				WorkerLogTag::Rtp,
				WorkerLogTag::Srtp,
				WorkerLogTag::Rtcp,
				WorkerLogTag::Rtx,
				WorkerLogTag::Bwe,
				WorkerLogTag::Score,
				WorkerLogTag::Simulcast,
				WorkerLogTag::Svc,
				WorkerLogTag::Sctp,
				WorkerLogTag::Message,
			];

			// FIXME: use real certificates?
			// settings.dtls_files = Some(WorkerDtlsFiles {
			// 	certificate: todo!(),
			// 	private_key: todo!(),
			// });

			let worker = mngr
				.create_worker(settings)
				.await
				.map_err(|_| Error::Worker)?;

			let info = ListenInfo {
				protocol: Protocol::Udp,
				ip: local_ip,
				announced_address: Some(announced_ip.to_string()),
				port: Some(server_port + worker_idx as u16),
				port_range: Some(worker_min_port..=worker_max_port),
				flags: None,
				send_buffer_size: None,
				recv_buffer_size: None,
			};
			let listen_infos = WebRtcServerListenInfos::new(info);
			let options = WebRtcServerOptions::new(listen_infos);
			let server = worker
				.create_webrtc_server(options)
				.await
				.map_err(|_| Error::RtcServer)?;

			workers.push((worker, server));
		}

		Ok(workers)
	}

	// round-robins to evenly distribute rooms across the workers
	fn get_next_worker(&mut self) -> (Worker, WebRtcServer) {
		// both, Worker and WebRtcServer are just arc, so we're good to clone
		let worker = self.workers[self.nex_worker_idx].clone();

		self.nex_worker_idx = (self.nex_worker_idx + 1) % self.workers.len();

		worker
	}

	pub fn get_room(&self, id: Uid) -> Option<&mpsc::Sender<room::Event>> {
		self.rooms.get(&id)
	}

	pub async fn create_room(&mut self) -> mpsc::Sender<room::Event> {
		let (event_tx, event_rx) = mpsc::channel(1024);
		let id = Uid::generate();

		self.rooms.insert(id, event_tx.clone());

		let (worker, rtc_server) = self.get_next_worker();
		let router = worker
			.create_router(RouterOptions::new(media_codecs()))
			.await
			.unwrap();

		let audio_observer = audio_observer::create(event_tx.clone(), router.clone()).await;

		tokio::spawn(room::create_and_start_receiving(
			id,
			router,
			rtc_server,
			event_rx,
			audio_observer,
		));

		event_tx
	}
}

fn media_codecs() -> Vec<RtpCodecCapability> {
	vec![
		RtpCodecCapability::Audio {
			mime_type: MimeTypeAudio::Opus,
			preferred_payload_type: None,
			clock_rate: NonZeroU32::new(48000).unwrap(),
			channels: NonZeroU8::new(2).unwrap(),
			parameters: RtpCodecParametersParameters::from([("useinbandfec", 1_u32.into())]),
			rtcp_feedback: vec![RtcpFeedback::TransportCc],
		},
		RtpCodecCapability::Video {
			mime_type: MimeTypeVideo::Vp8,
			preferred_payload_type: None,
			clock_rate: NonZeroU32::new(90000).unwrap(),
			parameters: RtpCodecParametersParameters::default(),
			rtcp_feedback: vec![
				RtcpFeedback::Nack,
				RtcpFeedback::NackPli,
				RtcpFeedback::CcmFir,
				RtcpFeedback::GoogRemb,
				RtcpFeedback::TransportCc,
			],
		},
		RtpCodecCapability::Video {
			mime_type: MimeTypeVideo::Vp9,
			preferred_payload_type: None,
			clock_rate: NonZeroU32::new(90000).unwrap(),
			parameters: RtpCodecParametersParameters::default(),
			rtcp_feedback: vec![
				RtcpFeedback::Nack,
				RtcpFeedback::NackPli,
				RtcpFeedback::CcmFir,
				RtcpFeedback::GoogRemb,
				RtcpFeedback::TransportCc,
			],
		},
	]
}
