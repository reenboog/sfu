use std::{collections::HashMap, env, net::SocketAddr, ops::ControlFlow, sync::Arc};

use axum::{
	extract::{
		self,
		ws::{self, WebSocket},
		WebSocketUpgrade,
	},
	response::IntoResponse,
	routing::any,
};
use axum_extra::{headers, TypedHeader};
use futures::{
	stream::{SplitSink, StreamExt},
	SinkExt,
};
use peer::Peer;
use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio::sync::Mutex;
// use server::Server;
use tower_http::trace::{DefaultMakeSpan, TraceLayer};
use tracing::Level;
use uid::Uid;

use axum::extract::connect_info::ConnectInfo;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

mod peer;
mod room;
mod server;
mod uid;

#[derive(Clone)]
struct State {
	// rooms instead?
	// make sure this containes only arcs, since Clone
	peers: Arc<Mutex<HashMap<Uid, Peer>>>,
}

impl State {
	fn new() -> Self {
		Self {
			peers: Arc::new(Mutex::new(HashMap::new())),
		}
	}
}

// sent by clients
#[derive(Serialize, Deserialize)]
enum ClientMsg {
	//
}

// sent by server
#[derive(Serialize, Deserialize)]
enum ServerMsg {
	//
}

#[tokio::main]
async fn main() {
	// let s = Server::new(num_cpus::get()).await.unwrap();

	// s.run();

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
	let addr = SocketAddr::from(([0, 0, 0, 0], ws_port));
	let state = State::new();
	let router = router(state);

	if use_tls {
		tracing::warn!("tl enabled");
		// let domain = env::var("DOMAIN").unwrap();
		// let base_dir = PathBuf::from("/etc/letsencrypt/live/").join(&domain);
		// let config = RustlsConfig::from_pem_file(
		// 	base_dir.join("fullchain.pem"),
		// 	base_dir.join("privkey.pem"),
		// )
		// .await
		// .unwrap();

		// println!("certs found...");

		// axum_server::bind_rustls(addr, config)
		// 	.serve(router.into_make_service())
		// 	.await
		// 	.unwrap();
	} else {
		tracing::warn!("no tls");

		axum_server::Server::bind(addr)
			.serve(router.into_make_service_with_connect_info::<SocketAddr>())
			.await
			.unwrap();
	}
}

fn router(state: State) -> axum::Router {
	// TODO: introduce create, accept, invite
	axum::Router::new()
		.route("/ws", any(ws_handler))
		.with_state(state)
		.layer(TraceLayer::new_for_http().make_span_with(
			DefaultMakeSpan::new().level(Level::TRACE),
			// .include_headers(true),
		))
}

async fn ws_handler(
	ws: WebSocketUpgrade,
	extract::State(state): extract::State<State>,
	user_agent: Option<TypedHeader<headers::UserAgent>>,
	ConnectInfo(addr): ConnectInfo<SocketAddr>,
) -> impl IntoResponse {
	let user_agent = if let Some(TypedHeader(user_agent)) = user_agent {
		user_agent.to_string()
	} else {
		String::from("Unknown browser")
	};

	tracing::info!("`{user_agent}` at {addr} connected.");

	ws.on_upgrade(move |socket| handle_socket(socket, addr, state))
}

async fn handle_socket(socket: WebSocket, who: SocketAddr, state: State) {
	let (mut sender, mut receiver) = socket.split();

	// TODO: authenticate
	// TODO: there should always be either create or accept, should it not?

	tokio::spawn(async move {
		while let Some(Ok(msg)) = receiver.next().await {
			// print message and break if instructed to do so
			// FIXME: probably rely on an event loop instead
			if process_ws_message(&mut sender, msg, who, &state)
				.await
				.is_break()
			{
				break;
			}
		}
	});
}

async fn process_ws_message(
	socket: &mut SplitSink<WebSocket, ws::Message>,
	msg: ws::Message,
	who: SocketAddr,
	_state: &State,
) -> ControlFlow<(), ()> {
	use ws::Message::*;

	match msg {
		Text(t) => {
			// json encoded messages
			tracing::debug!(">>> {who} sent str: {t:?}");
			let reply = format!("thanks for {}", t).to_string();
			let reply = json!({ "reply": reply }).to_string();
			_ = socket.send(ws::Message::Text(reply)).await;
		}
		Close(c) => {
			// state
			if let Some(cf) = c {
				tracing::warn!(
					">>> {} sent close with code {} and reason `{}`",
					who,
					cf.code,
					cf.reason
				);
			} else {
				tracing::warn!(">>> {who} somehow sent close message without CloseFrame");
			}
			return ControlFlow::Break(());
		}

		msg => {
			tracing::error!("Received unexpected {:?}.", msg);
		}
	}
	ControlFlow::Continue(())
}

#[cfg(test)]
mod tests {
	#[test]
	fn test_one() {
		assert!(true);
	}

	#[test]
	fn test_two() {
		assert!(true);
	}
}
