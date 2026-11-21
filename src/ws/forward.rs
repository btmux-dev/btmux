//! `/ws/forward/{pane_id}?port=N`: one TCP connection to a port that a pane's
//! program listens on, as binary frames. `btmux forward` opens one per
//! connection it accepts.

use std::collections::HashSet;
use std::net::SocketAddr;
use std::sync::Mutex;

use axum::extract::ws::{Message, WebSocket};
use axum::extract::{Path, Query, State, WebSocketUpgrade};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use uuid::Uuid;

use crate::pane_ports::pane_listeners;
use crate::AppState;

const READ_SIZE: usize = 64 * 1024;

/// The address of the connection that `btmux forward` accepted, which it
/// sends so the server can tell its own connections coming back.
pub const PEER_HEADER: &str = "x-btmux-forward-peer";

/// This end of each connection the server makes for a forward. A forward on
/// the server's machine can listen where the server connects (a wildcard
/// listener's loopback address); its connections would then loop.
static OUTBOUND: Mutex<Option<HashSet<SocketAddr>>> = Mutex::new(None);

/// Holds an `OUTBOUND` entry while its connection is open.
struct Outbound(Option<SocketAddr>);

impl Outbound {
    fn new(tcp: &TcpStream) -> Self {
        let address = tcp.local_addr().ok();
        if let Some(address) = address {
            OUTBOUND
                .lock()
                .unwrap()
                .get_or_insert_default()
                .insert(address);
        }
        Self(address)
    }

    fn contains(address: SocketAddr) -> bool {
        OUTBOUND
            .lock()
            .unwrap()
            .as_ref()
            .is_some_and(|set| set.contains(&address))
    }
}

impl Drop for Outbound {
    fn drop(&mut self) {
        if let (Some(address), Some(set)) = (self.0, OUTBOUND.lock().unwrap().as_mut()) {
            set.remove(&address);
        }
    }
}

#[derive(Deserialize)]
pub struct ForwardQuery {
    port: u16,
}

pub async fn handle(
    State(state): State<AppState>,
    Path(pane): Path<Uuid>,
    Query(query): Query<ForwardQuery>,
    headers: HeaderMap,
    ws: WebSocketUpgrade,
) -> Response {
    let peer = headers
        .get(PEER_HEADER)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<SocketAddr>().ok());
    if peer.is_some_and(Outbound::contains) {
        return (
            StatusCode::CONFLICT,
            "this connection came from the server's own forward: a `btmux forward` on the \
             server's machine listens where the pane's program does",
        )
            .into_response();
    }
    let mut listener = None;
    // A cached sample can predate a server that just started.
    for fresh in [false, true] {
        match pane_listeners(&state, pane, fresh).await {
            Ok(listeners) => {
                listener = listeners.into_iter().find(|l| l.port == query.port);
                if listener.is_some() {
                    break;
                }
            }
            Err(unavailable) => return unavailable.into_response(),
        }
    }
    let Some(listener) = listener else {
        return (
            StatusCode::CONFLICT,
            format!("no program in the pane listens on port {}", query.port),
        )
            .into_response();
    };
    if listener.shadowed {
        return (
            StatusCode::CONFLICT,
            format!(
                "port {} listens on every address, but another program here holds its loopback \
                 address (a `btmux forward` on this machine?)",
                query.port
            ),
        )
            .into_response();
    }
    let tcp = match TcpStream::connect(listener.connect_address()).await {
        Ok(tcp) => tcp,
        Err(error) => {
            return (
                StatusCode::BAD_GATEWAY,
                format!("could not connect to port {}: {error}", query.port),
            )
                .into_response()
        }
    };
    let _ = tcp.set_nodelay(true);
    let outbound = Outbound::new(&tcp);
    ws.on_upgrade(move |socket| async move {
        relay(socket, tcp).await;
        drop(outbound);
    })
}

/// Copies bytes both ways until either side closes.
async fn relay(socket: WebSocket, tcp: TcpStream) {
    let (mut ws_tx, mut ws_rx) = socket.split();
    let (mut tcp_rx, mut tcp_tx) = tcp.into_split();
    let inbound = async {
        while let Some(Ok(message)) = ws_rx.next().await {
            match message {
                Message::Binary(bytes) => {
                    if tcp_tx.write_all(&bytes).await.is_err() {
                        break;
                    }
                }
                Message::Close(_) => break,
                _ => {}
            }
        }
        let _ = tcp_tx.shutdown().await;
    };
    let outbound = async {
        let mut buffer = vec![0; READ_SIZE];
        loop {
            match tcp_rx.read(&mut buffer).await {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    let bytes = axum::body::Bytes::copy_from_slice(&buffer[..n]);
                    if ws_tx.send(Message::Binary(bytes)).await.is_err() {
                        return;
                    }
                }
            }
        }
        let _ = ws_tx.send(Message::Close(None)).await;
    };
    tokio::select! {
        () = inbound => {}
        () = outbound => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio_tungstenite::tungstenite::Message as Client;

    fn state() -> AppState {
        let (exit_tx, _) = tokio::sync::mpsc::unbounded_channel();
        let (meta_tx, _) = tokio::sync::mpsc::unbounded_channel();
        let manager = crate::session::manager::SessionManager::new(
            "/bin/sh".into(),
            crate::config::FileConfig::default(),
            exit_tx,
            meta_tx,
            8004,
            None,
        );
        std::sync::Arc::new(tokio::sync::RwLock::new(manager))
    }

    async fn serve(router: axum::Router) -> std::net::SocketAddr {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        address
    }

    fn status(error: tokio_tungstenite::tungstenite::Error) -> u16 {
        match error {
            tokio_tungstenite::tungstenite::Error::Http(response) => response.status().as_u16(),
            other => panic!("{other}"),
        }
    }

    #[tokio::test]
    async fn refuses_unknown_panes_and_ports_nothing_listens_on() {
        let state = state();
        let pane = {
            let mut manager = state.write().await;
            manager.create_session(None).await;
            manager.pane_infos()[0].pane_id
        };
        let address = serve(
            axum::Router::new()
                .route("/ws/forward/{pane_id}", axum::routing::get(handle))
                .with_state(state),
        )
        .await;
        let connect = |pane: Uuid| {
            tokio_tungstenite::connect_async(format!("ws://{address}/ws/forward/{pane}?port=9"))
        };
        assert_eq!(status(connect(Uuid::nil()).await.unwrap_err()), 404);
        // The pane's shell hasn't started, so nothing of it listens.
        assert_eq!(status(connect(pane).await.unwrap_err()), 409);
    }

    #[tokio::test]
    async fn refuses_connections_that_loop_back() {
        let target = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let tcp = TcpStream::connect(target.local_addr().unwrap())
            .await
            .unwrap();
        let outbound = Outbound::new(&tcp);
        let address = serve(
            axum::Router::new()
                .route("/ws/forward/{pane_id}", axum::routing::get(handle))
                .with_state(state()),
        )
        .await;
        let request = |peer: SocketAddr| {
            use tokio_tungstenite::tungstenite::client::IntoClientRequest;
            let mut request = format!("ws://{address}/ws/forward/{}?port=9", Uuid::nil())
                .into_client_request()
                .unwrap();
            request
                .headers_mut()
                .insert(PEER_HEADER, peer.to_string().parse().unwrap());
            tokio_tungstenite::connect_async(request)
        };
        let own = tcp.local_addr().unwrap();
        assert_eq!(status(request(own).await.unwrap_err()), 409);
        drop(outbound);
        // Once the connection closes, its address means nothing.
        assert_eq!(status(request(own).await.unwrap_err()), 404);
    }

    #[tokio::test]
    async fn relays_bytes_both_ways() {
        let echo = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let target = echo.local_addr().unwrap();
        tokio::spawn(async move {
            let (mut tcp, _) = echo.accept().await.unwrap();
            let mut buffer = [0; 5];
            tcp.read_exact(&mut buffer).await.unwrap();
            tcp.write_all(&buffer.to_ascii_uppercase()).await.unwrap();
        });
        let address = serve(axum::Router::new().route(
            "/",
            axum::routing::get(move |ws: WebSocketUpgrade| async move {
                let tcp = TcpStream::connect(target).await.unwrap();
                ws.on_upgrade(move |socket| relay(socket, tcp))
            }),
        ))
        .await;
        let (mut socket, _) = tokio_tungstenite::connect_async(format!("ws://{address}/"))
            .await
            .unwrap();
        socket
            .send(Client::Binary(b"hello".to_vec().into()))
            .await
            .unwrap();
        let mut received = Vec::new();
        while let Some(message) = socket.next().await {
            match message.unwrap() {
                Client::Binary(bytes) => received.extend_from_slice(&bytes),
                Client::Close(_) => break,
                _ => {}
            }
        }
        // The echo server closing ends the stream.
        assert_eq!(received, b"HELLO");
    }
}
