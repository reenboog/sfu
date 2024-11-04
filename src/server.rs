use std::collections::HashMap;

use mediasoup::{
	prelude::{
		ListenInfo, Protocol, WebRtcServer, WebRtcServerListenInfos, WebRtcServerOptions,
		WorkerManager,
	},
	router::Router,
	worker::{Worker, WorkerSettings},
};

use crate::{room::Room, uid::Uid};

#[derive(Debug)]
pub enum Error {
	Unknown,
	RoomExists(Uid),
}

pub struct Server {
	mngr: WorkerManager,
	workers: Vec<Worker>,
	nex_worker_idx: usize,

	// FIXME: replace with WeakRoom to allow self-destruction when the last participant leaves
	rooms: HashMap<Uid, Room>,
}

impl Server {
	pub async fn new(num_workers: usize) -> Result<Self, Error> {
		let mngr = WorkerManager::new();
		let workers = Self::create_workers(&mngr, num_workers).await?;

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
	) -> Result<Vec<Worker>, Error> {
		let mut workers = Vec::new();
		// FIXME: respect tdls certificate & key
		// FIXME: respect min & max ports
		// FIXME: user WebRtcServer

		// webRtcServerOptions :
		// {
		// 	listenInfos :
		// 	[
		// 		{
		// 			protocol    : 'udp',
		// 			ip          : process.env.MEDIASOUP_LISTEN_IP || '0.0.0.0',
		// 			announcedIp : process.env.MEDIASOUP_ANNOUNCED_IP,
		// 			port        : 44444
		// 		},
		// 		{
		// 			protocol    : 'tcp',
		// 			ip          : process.env.MEDIASOUP_LISTEN_IP || '0.0.0.0',
		// 			announcedIp : process.env.MEDIASOUP_ANNOUNCED_IP,
		// 			port        : 44444
		// 		}
		// 	]
		// },

		for _ in 0..num_workers {
			let worker = mngr
				.create_worker(WorkerSettings::default())
				.await
				.map_err(|_| Error::Unknown)?;

			let info = ListenInfo {
				protocol: Protocol::Udp,
				ip: todo!(),
				announced_address: todo!(),
				port: todo!(),
				port_range: todo!(),
				flags: todo!(),
				send_buffer_size: todo!(),
				recv_buffer_size: todo!(),
			};
			let listen_infos = WebRtcServerListenInfos::new(info);
			let options = WebRtcServerOptions::new(listen_infos);
			let server = worker
				.create_webrtc_server(options)
				.await
				.map_err(|_| Error::Unknown)?;

			workers.push(worker);
		}

		Ok(workers)
	}

	// round-robins to evenly distribute rooms across the workers
	fn get_next_worker(&mut self) -> &Worker {
		let worker = &self.workers[self.nex_worker_idx];

		self.nex_worker_idx = (self.nex_worker_idx + 1) % self.workers.len();

		worker
	}

	pub fn create_room(&mut self) -> Room {
		Room::new(self.get_next_worker(), Uid::generate())
	}

	pub fn run(&self) {
		println!("workers: {}", self.workers.len());
		// FIXME: room.on('close', () => rooms.delete(roomId));
	}
}

// this is for ws only

// fn load_rustls_config() -> rustls::ServerConfig {
// 	rustls::crypto::aws_lc_rs::default_provider()
// 			.install_default()
// 			.unwrap();

// 	// let key = "/etc/letsencrypt/live/p2q.live/privkey.pem";
// 	// let cert = "/etc/letsencrypt/live/p2q.live/fullchain.pem";
// 	let key = "/home/admin/proj/sfu/mediasoup/certs/privkey.pem";
// 	let cert = "/home/admin/proj/sfu/mediasoup/certs/fullchain.pem";

// 	// init server config builder with safe defaults
// 	let config = ServerConfig::builder().with_no_client_auth();

// 	// load TLS key/cert files
// 	let key_file = &mut BufReader::new(File::open(key).unwrap());
// 	let cert_file = &mut BufReader::new(File::open(cert).unwrap());

// 	// convert files to key/cert objects
// 	let cert_chain = certs(cert_file).collect::<Result<Vec<_>, _>>().unwrap();
// 	let mut keys = pkcs8_private_keys(key_file)
// 			.map(|key| key.map(PrivateKeyDer::Pkcs8))
// 			.collect::<Result<Vec<_>, _>>()
// 			.unwrap();

// 	// exit if no keys could be parsed
// 	if keys.is_empty() {
// 			eprintln!("Could not locate PKCS 8 private keys.");
// 			std::process::exit(1);
// 	}

// 	config.with_single_cert(cert_chain, keys.remove(0)).unwrap()
// }
