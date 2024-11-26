use std::{
	env,
	net::{IpAddr, Ipv4Addr, SocketAddr},
	path::PathBuf,
	str::FromStr,
	sync::Arc,
};

use axum::{
	extract::{self, ws::WebSocket, WebSocketUpgrade},
	http::StatusCode,
	response::IntoResponse,
	routing::any,
};
use axum_extra::{headers, TypedHeader};
use axum_server::tls_rustls::RustlsConfig;
use futures::stream::StreamExt;
use serde::{Deserialize, Serialize};
use server::Server;
use tokio::sync::{mpsc, Mutex};
use tower_http::trace::{DefaultMakeSpan, TraceLayer};
use tracing::Level;
use uid::Uid;

use axum::extract::connect_info::ConnectInfo;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

mod audio_observer;
mod peer;
mod room;
mod server;
mod uid;

#[derive(Clone)]
struct State {
	server: Arc<Mutex<Server>>,
	// rooms instead?
	// make sure this containes only arcs, since Clone
	// peers: Arc<Mutex<HashMap<Uid, Peer>>>,
}

impl State {
	fn new(server: Server) -> Self {
		Self {
			server: Arc::new(Mutex::new(server)),
			// peers: Arc::new(Mutex::new(HashMap::new())),
		}
	}
}

#[derive(Serialize, Deserialize)]
struct CreateReq {
	#[serde(rename = "userId")]
	pub user_id: Uid,
	// participants: Vec<Uid>,
	// options
}

#[derive(Serialize, Deserialize)]
struct JoinReq {
	#[serde(rename = "userId")]
	pub user_id: Uid,
	#[serde(rename = "roomId")]
	pub room_id: Uid,
}

#[tokio::main]
async fn main() -> Result<(), String> {
	tracing_subscriber::registry()
		.with(
			tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| {
				format!(
					"{}=trace,tower_http=debug,axum::rejection=trace",
					env!("CARGO_CRATE_NAME")
				)
				.into()
			}),
		)
		.with(tracing_subscriber::fmt::layer())
		.init();

	tracing::info!("starting...");

	let ws_port = 3001;
	let use_tls = env::var("USE_TLS").unwrap_or_else(|_| "false".into()) == "true";
	// an "external"/public ip, visible to the outside world
	let announced_ip = env::var("ANNOUNCED_IP").unwrap_or(String::from("127.0.0.1"));
	// usually 127.0.0.1 when tested locally, 0.0.0.0 when running on a server
	let local_ip = env::var("LOCAL_IP").unwrap_or(String::from("127.0.0.1"));
	let local_ip = IpAddr::V4(Ipv4Addr::from_str(&local_ip).map_err(|e| e.to_string())?);
	let addr = SocketAddr::from(([0, 0, 0, 0], ws_port));
	let server = Server::new(
		num_cpus::get(),
		local_ip,
		&announced_ip,
		// TODO: read from env
		40000,
		40100,
		44444,
	)
	.await
	.unwrap();
	// s.run();

	let state = State::new(server);
	let router = router(state);

	if use_tls {
		tracing::warn!("tl enabled");
		// let domain = env::var("DOMAIN").unwrap();
		let base_dir = PathBuf::from("../certs/");
		let config = RustlsConfig::from_pem_file(
			base_dir.join("fullchain.pem"),
			base_dir.join("privkey.pem"),
		)
		.await
		.unwrap();

		tracing::warn!("certs found...");

		axum_server::bind_rustls(addr, config)
			.serve(router.into_make_service_with_connect_info::<SocketAddr>())
			.await
			.unwrap();
	} else {
		tracing::warn!("no tls");

		axum_server::Server::bind(addr)
			.serve(router.into_make_service_with_connect_info::<SocketAddr>())
			.await
			.unwrap();
	}

	Ok(())
}

fn router(state: State) -> axum::Router {
	axum::Router::new()
		// rate limit?
		// /challenge?userId=base64_encoded_user_id
		// create?userId=base64_encoded_user_id
		.route("/create", any(create_room))
		.route("/join", any(join_room))
		// .route("/join", post(join_call))
		.with_state(state)
		.layer(
			TraceLayer::new_for_http().make_span_with(
				DefaultMakeSpan::new()
					.level(Level::TRACE)
					.include_headers(true),
			),
		)
}

async fn create_room(
	ws: WebSocketUpgrade,
	extract::State(state): extract::State<State>,
	user_agent: Option<TypedHeader<headers::UserAgent>>,
	ConnectInfo(addr): ConnectInfo<SocketAddr>,
	extract::Query(req): extract::Query<CreateReq>,
) -> impl IntoResponse {
	let user_agent = if let Some(TypedHeader(user_agent)) = user_agent {
		user_agent.to_string()
	} else {
		String::from("Unknown browser")
	};

	tracing::info!(
		"{} on `{}` @ {} connected.",
		req.user_id.to_base64(),
		user_agent,
		addr
	);

	let room_tx = state.server.lock().await.create_room().await;

	ws.on_upgrade(move |socket| handle_ws(socket, req.user_id, room_tx))
}

// TODO: introduce intent?
async fn handle_ws(socket: WebSocket, user_id: Uid, room_tx: mpsc::Sender<room::Event>) {
	let (sender, receiver) = socket.split();

	peer::create_and_start_receiving(user_id, room_tx, sender, receiver).await;
}

async fn join_room(
	ws: WebSocketUpgrade,
	extract::State(state): extract::State<State>,
	user_agent: Option<TypedHeader<headers::UserAgent>>,
	ConnectInfo(addr): ConnectInfo<SocketAddr>,
	extract::Query(req): extract::Query<JoinReq>,
) -> impl IntoResponse {
	let user_agent = if let Some(TypedHeader(user_agent)) = user_agent {
		user_agent.to_string()
	} else {
		String::from("Unknown browser")
	};

	tracing::info!(
		"{} on `{}` @ {} connected.",
		req.user_id.to_base64(),
		user_agent,
		addr
	);

	if let Some(room_tx) = state.server.lock().await.get_room(req.room_id).cloned() {
		ws.on_upgrade(move |socket| handle_ws(socket, req.user_id, room_tx))
	} else {
		StatusCode::NOT_FOUND.into_response()
	}
}

#[cfg(test)]
mod tests {
	use crate::uid::Uid;

	#[test]
	fn test_one() {
		assert!(true);

		let creator = Uid::generate();
		println!("{}", creator.to_base64());
	}

	#[test]
	fn test_two() {
		assert!(true);
	}
}
