//! `/ws/forward/{pane_id}?port=N[&address=IP]`: one TCP connection to a port
//! that a pane's program listens on, as binary frames, with an `eof` text
//! frame for each direction's end. `btmux forward` opens one per connection
//! it accepts.

use std::collections::HashSet;
use std::net::{IpAddr, SocketAddr};
use std::sync::Mutex;
use std::time::Instant;

use axum::body::Bytes;
use axum::extract::ws::{Message, WebSocket};
use axum::extract::{Path, Query, State, WebSocketUpgrade};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use futures_util::{Sink, SinkExt, Stream, StreamExt};
use serde::Deserialize;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use uuid::Uuid;

use crate::pane_ports::{listing_age, pane_shells, pick, snapshot, Unavailable};
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

/// Sent as text when a side's TCP read ends, so the other shuts down its
/// write side and keeps relaying replies: a client may close its request
/// before reading the response.
pub const EOF: &str = "eof";

/// What travels over a forward's WebSocket, on either side.
pub enum Frame {
    Data(Bytes),
    /// The sender's TCP connection sent everything.
    Eof,
    /// Abandons the connection, or ends it once both sides sent `Eof`.
    Close,
    /// Pings and the like.
    Other,
}

/// Relays between a TCP connection and a forward's frames until both
/// directions reach EOF, or either side fails or closes.
pub async fn pipe<Tx, Rx>(tx: Tx, mut rx: Rx, tcp: TcpStream)
where
    Tx: Sink<Frame> + Unpin,
    Rx: Stream<Item = Frame> + Unpin,
{
    let (mut tcp_rx, mut tcp_tx) = tcp.into_split();
    let incoming = async {
        loop {
            match rx.next().await {
                Some(Frame::Data(bytes)) => tcp_tx.write_all(&bytes).await.map_err(|_| ())?,
                Some(Frame::Eof) => {
                    let _ = tcp_tx.shutdown().await;
                    return Ok(());
                }
                Some(Frame::Other) => {}
                Some(Frame::Close) | None => return Err(()),
            }
        }
    };
    let outgoing = async move {
        let mut tx = tx;
        let mut buffer = vec![0; READ_SIZE];
        loop {
            match tcp_rx.read(&mut buffer).await {
                Ok(0) => {
                    tx.send(Frame::Eof).await.map_err(|_| ())?;
                    return Ok(tx);
                }
                Ok(n) => {
                    let bytes = Bytes::copy_from_slice(&buffer[..n]);
                    tx.send(Frame::Data(bytes)).await.map_err(|_| ())?;
                }
                Err(_) => return Err(()),
            }
        }
    };
    tokio::pin!(incoming, outgoing);
    let (mut received, mut sent) = (false, None);
    loop {
        tokio::select! {
            result = &mut incoming, if !received => match result {
                Ok(()) => received = true,
                Err(()) => return,
            },
            result = &mut outgoing, if sent.is_none() => match result {
                Ok(tx) => sent = Some(tx),
                Err(()) => return,
            },
        }
        if received {
            if let Some(mut tx) = sent.take() {
                let _ = tx.send(Frame::Close).await;
                return;
            }
        }
    }
}

#[derive(Deserialize)]
pub struct ForwardQuery {
    port: u16,
    /// The listener's address, when the pane holds the port on several.
    address: Option<IpAddr>,
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
        return conflict(
            "this connection came from the server's own forward: a `btmux forward` on the \
             server's machine listens where the pane's program does",
        );
    }
    let shells = match pane_shells(&state, pane).await {
        Ok(shells) => shells,
        Err(unavailable) => return unavailable.into_response(),
    };
    let mut picked = Err(String::new());
    // A recent sample can predate a program that just started listening.
    for not_before in [listing_age(), Instant::now()] {
        let snapshot = match snapshot(not_before).await {
            Ok(snapshot) => snapshot,
            Err(error) => return Unavailable::Failed(error).into_response(),
        };
        picked = pick(
            &snapshot.listeners(&shells, pane),
            Some(query.port),
            query.address,
        );
        if picked.is_ok() {
            break;
        }
    }
    let listener = match picked {
        Ok(listener) => listener,
        Err(error) => return conflict(error),
    };
    let tcp = match TcpStream::connect(listener.connect_address()).await {
        Ok(tcp) => tcp,
        Err(error) => {
            return (
                StatusCode::BAD_GATEWAY,
                format!(
                    "could not connect to {}: {error}",
                    listener.connect_address()
                ),
            )
                .into_response()
        }
    };
    let outbound = Outbound::new(&tcp);
    // Whatever accepted it must still be the pane's program: the listener can
    // have changed hands since the sample, or another can shadow it.
    let dialed = Instant::now();
    let checked = match (tcp.local_addr(), tcp.peer_addr()) {
        (Ok(ours), Ok(theirs)) => match snapshot(dialed).await {
            Ok(snapshot) => snapshot.check(&shells, pane, ours, theirs),
            Err(error) => return Unavailable::Failed(error).into_response(),
        },
        _ => Err("the connection closed".into()),
    };
    if let Err(error) = checked {
        return conflict(error);
    }
    let _ = tcp.set_nodelay(true);
    ws.on_upgrade(move |socket| async move {
        relay(socket, tcp).await;
        drop(outbound);
    })
}

/// The server's relay, for the CLI's tests.
#[cfg(test)]
pub async fn relay_for_tests(socket: WebSocket, tcp: TcpStream) {
    relay(socket, tcp).await
}

fn conflict(message: impl Into<String>) -> Response {
    (StatusCode::CONFLICT, message.into()).into_response()
}

/// `pipe` over the server's WebSocket.
async fn relay(socket: WebSocket, tcp: TcpStream) {
    let (tx, rx) = socket.split();
    let tx = tx.with(|frame: Frame| async move {
        Ok::<_, axum::Error>(match frame {
            Frame::Data(bytes) => Message::Binary(bytes),
            Frame::Eof => Message::Text(EOF.into()),
            Frame::Close | Frame::Other => Message::Close(None),
        })
    });
    let rx = rx.map(|message| match message {
        Ok(Message::Binary(bytes)) => Frame::Data(bytes),
        Ok(Message::Text(text)) if text.as_str() == EOF => Frame::Eof,
        Ok(Message::Close(_)) | Err(_) => Frame::Close,
        Ok(_) => Frame::Other,
    });
    pipe(Box::pin(tx), Box::pin(rx), tcp).await;
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
                Client::Text(text) if text.as_str() == EOF => break,
                other => panic!("{other:?}"),
            }
        }
        // The echo server closing sends EOF, and ours ends the connection.
        assert_eq!(received, b"HELLO");
        socket.send(Client::Text(EOF.into())).await.unwrap();
        assert!(matches!(
            socket.next().await,
            Some(Ok(Client::Close(_))) | None
        ));
    }
}
