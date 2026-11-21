//! `btmux forward`: listen on a local port and tunnel each connection to a
//! port that a pane's program listens on, over `/ws/forward`.

use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::time::{Duration, Instant};

use futures_util::{SinkExt, StreamExt};
use reqwest::Method;
use tokio::net::{TcpListener, TcpStream};
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::StatusCode;
use tokio_tungstenite::tungstenite::{Error as WsError, Message};
use tokio_tungstenite::Connector;
use uuid::Uuid;

use crate::api_client::{request, request_with_status};
use crate::pane_ports::{pick, Listener};
use crate::send_notification::{server, SERVER_URL};
use crate::ws::forward::{pipe, Frame, EOF};

const TIMEOUT: Duration = Duration::from_secs(10);
/// How long a connection waits for a server that is restarting or upgrading.
const RECONNECT_FOR: Duration = Duration::from_secs(10);
/// How often the pane is checked, so the command ends with it.
const PANE_CHECK: Duration = Duration::from_secs(5);
/// Exit status when the pane closes.
const PANE_GONE: i32 = 3;

#[derive(clap::Args, Clone, Debug)]
pub struct ForwardArgs {
    #[arg(
        short = 't',
        help = "The pane: an ID or SESSION[:WINDOW[.PANE]]. Default: this pane"
    )]
    target: Option<String>,
    /// The pane's port, as PORT or ADDRESS:PORT ([::1]:PORT for IPv6) when
    /// different programs hold it on different addresses. Default: its only
    /// listening port.
    #[arg(value_name = "[ADDRESS:]PORT")]
    port: Option<String>,
    /// Local address to listen on.
    #[arg(long, default_value = "127.0.0.1")]
    bind: IpAddr,
    /// Local port. Default: the pane's port when it's free here and doesn't
    /// stand in the pane's way, else any.
    #[arg(long)]
    local_port: Option<u16>,
    /// Open the forwarded URL in the default browser.
    #[arg(long)]
    open: bool,
    /// Server URL, such as https://machine.tailnet.ts.net:8004. The token is
    /// read from BTMUX_AUTH_TOKEN.
    #[arg(long, value_name = "URL")]
    server: Option<String>,
}

pub async fn run(profile: Option<&str>, args: &ForwardArgs) -> i32 {
    match forward(profile, args).await {
        Ok(code) => code,
        Err(error) => {
            eprintln!("btmux: {error}");
            1
        }
    }
}

async fn forward(profile: Option<&str>, args: &ForwardArgs) -> Result<i32, String> {
    if let Some(url) = &args.server {
        if args.target.is_none() {
            return Err("with --server, name the pane with -t".into());
        }
        let _ = SERVER_URL.set(url.clone());
    }
    let pane = crate::tmux_cli::resolve_pane(profile, args.target.as_deref()).await?;
    let listeners: Vec<Listener> = serde_json::from_str(
        &request(
            profile,
            Method::GET,
            &format!("/api/panes/{pane}/ports"),
            None,
            TIMEOUT,
        )
        .await?,
    )
    .map_err(|e| e.to_string())?;
    let (address, port) = match args.port.as_deref() {
        Some(target) => parse_target(target).map(|(a, p)| (a, Some(p)))?,
        None => (None, None),
    };
    let listener = pick(&listeners, port, address)?;

    let (server_url, token) = server(profile)?;
    let path = format!(
        "/ws/forward/{pane}?port={}&address={}",
        listener.port, listener.address
    );
    let local = listen(profile, &path, args.bind, args.local_port, listener.port).await?;
    let address = local.local_addr().map_err(|e| e.to_string())?;
    let tunnel = Arc::new(Tunnel {
        url: format!(
            "{}{path}&{}",
            websocket_base(&server_url)?,
            forwarder_query(address)
        ),
        token,
        connector: connector()?,
        retry_for: RECONNECT_FOR,
    });
    let url = local_url(address);
    println!("{url}");
    eprintln!(
        "btmux: forwarding {address} to {} ({}) in pane {pane}; Ctrl+C stops",
        SocketAddr::new(
            listener.address.parse().unwrap_or(address.ip()),
            listener.port
        ),
        listener.process.as_deref().unwrap_or("unknown program"),
    );
    if args.open {
        if let Err(error) = open::that_detached(&url) {
            eprintln!("btmux: couldn't open {url}: {error}");
        }
    }

    let (gone_tx, mut gone_rx) = tokio::sync::mpsc::channel::<()>(1);
    let checker = tokio::spawn(watch_pane(
        profile.map(str::to_string),
        pane,
        gone_tx.clone(),
    ));
    let interrupted = tokio::signal::ctrl_c();
    tokio::pin!(interrupted);
    let code = loop {
        tokio::select! {
            accepted = local.accept() => {
                let tcp = match accepted {
                    Ok((tcp, _)) => tcp,
                    // Out of file descriptors, say: the connection waits in
                    // the backlog, so retrying at once would spin.
                    Err(error) => {
                        eprintln!("btmux: can't accept a connection: {error}");
                        tokio::time::sleep(Duration::from_millis(100)).await;
                        continue;
                    }
                };
                let _ = tcp.set_nodelay(true);
                let tunnel = tunnel.clone();
                let gone = gone_tx.clone();
                tokio::spawn(async move {
                    if let Err(error) = tunnel.carry(tcp).await {
                        if error == Failure::PaneGone {
                            let _ = gone.try_send(());
                        } else {
                            eprintln!("btmux: {}", error.message());
                        }
                    }
                });
            }
            _ = gone_rx.recv() => {
                eprintln!("btmux: pane {pane} is gone");
                break PANE_GONE;
            }
            _ = &mut interrupted => break 0,
        }
    };
    checker.abort();
    Ok(code)
}

/// `PORT`, `ADDRESS:PORT`, or `[IPV6]:PORT`.
fn parse_target(target: &str) -> Result<(Option<IpAddr>, u16), String> {
    if let Ok(port) = target.parse::<u16>() {
        return Ok((None, port));
    }
    target
        .parse::<SocketAddr>()
        .map(|at| (Some(at.ip()), at.port()))
        .map_err(|_| format!("{target} isn't PORT or ADDRESS:PORT"))
}

/// Listens on `local` if given, else on the pane's port when it's free, else
/// on any port, after the server confirms, without connecting to the
/// program, that a connection would reach it. On the server's own machine,
/// taking the pane's port can stand in front of a wildcard listener, so that
/// falls back to any port.
async fn listen(
    profile: Option<&str>,
    path: &str,
    address: IpAddr,
    local: Option<u16>,
    remote: u16,
) -> Result<TcpListener, String> {
    let at = |port| SocketAddr::new(address, port);
    let bind = |port| async move {
        TcpListener::bind(at(port))
            .await
            .map_err(|e| format!("can't listen on {}: {e}", at(port)))
    };
    let probe = |listener: TcpListener| async move {
        let bound = listener.local_addr().map_err(|e| e.to_string())?;
        let path = format!("{path}&{}&probe=true", forwarder_query(bound));
        match request_with_status(profile, Method::GET, &path, None, TIMEOUT).await {
            Ok((StatusCode::NO_CONTENT, _)) => Ok(Ok(listener)),
            Ok((StatusCode::CONFLICT, body)) => Ok(Err(body)),
            Ok((StatusCode::NOT_FOUND, _)) => Err("the pane is gone".to_string()),
            Ok((status, body)) => Err(if body.is_empty() {
                status.to_string()
            } else {
                body
            }),
            Err(error) => Err(error),
        }
    };
    if let Some(port) = local {
        return probe(bind(port).await?).await?;
    }
    if let Ok(listener) = bind(remote).await {
        if let Ok(listener) = probe(listener).await? {
            return Ok(listener);
        }
    }
    probe(bind(0).await?).await?
}

/// Names this process and its listener to the server.
fn forwarder_query(listener: SocketAddr) -> String {
    format!(
        "forwarder_pid={}&forwarder_listener={}",
        std::process::id(),
        urlencoding(&listener.to_string())
    )
}

/// Percent-encodes the brackets and colons of an IPv6 socket address.
fn urlencoding(text: &str) -> String {
    text.replace('[', "%5B")
        .replace(']', "%5D")
        .replace(':', "%3A")
}

fn local_url(address: SocketAddr) -> String {
    let ip = match address.ip() {
        ip if ip.is_unspecified() && ip.is_ipv4() => IpAddr::from([127, 0, 0, 1]),
        ip if ip.is_unspecified() => IpAddr::from(std::net::Ipv6Addr::LOCALHOST),
        ip => ip,
    };
    format!("http://{}/", SocketAddr::new(ip, address.port()))
}

fn websocket_base(url: &str) -> Result<String, String> {
    if let Some(rest) = url.strip_prefix("https://") {
        Ok(format!("wss://{rest}"))
    } else if let Some(rest) = url.strip_prefix("http://") {
        Ok(format!("ws://{rest}"))
    } else {
        Err(format!("{url} isn't an http or https URL"))
    }
}

/// TLS trusting the platform's roots, as browsers do.
fn connector() -> Result<Connector, String> {
    use rustls_platform_verifier::BuilderVerifierExt;
    let config = rustls::ClientConfig::builder_with_provider(crate::connect::tunnel::provider())
        .with_safe_default_protocol_versions()
        .map_err(|e| e.to_string())?
        .with_platform_verifier()
        .map_err(|e| e.to_string())?
        .with_no_client_auth();
    Ok(Connector::Rustls(Arc::new(config)))
}

/// Ends the command once the pane is gone.
async fn watch_pane(profile: Option<String>, pane: Uuid, gone: tokio::sync::mpsc::Sender<()>) {
    loop {
        tokio::time::sleep(PANE_CHECK).await;
        // Other failures, like a server mid-upgrade, aren't the pane closing.
        let Ok(body) = request(profile.as_deref(), Method::GET, "/api/panes", None, TIMEOUT).await
        else {
            continue;
        };
        let Ok(panes) = serde_json::from_str::<Vec<crate::session::PaneInfo>>(&body) else {
            continue;
        };
        if !panes.iter().any(|p| p.pane_id == pane) {
            let _ = gone.send(()).await;
            return;
        }
    }
}

#[derive(Debug, PartialEq)]
enum Failure {
    PaneGone,
    /// The server won't forward this connection (409).
    Refused(String),
    Other(String),
}

impl Failure {
    fn message(self) -> String {
        match self {
            Failure::PaneGone => "the pane is gone".into(),
            Failure::Refused(message) | Failure::Other(message) => message,
        }
    }
}

type Socket = tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<TcpStream>>;

struct Tunnel {
    url: String,
    token: String,
    connector: Connector,
    /// How long a connection waits for a server that is restarting or
    /// upgrading, across attempts.
    retry_for: Duration,
}

impl Tunnel {
    /// Carries one local connection to the pane's port.
    async fn carry(&self, tcp: TcpStream) -> Result<(), Failure> {
        let socket = self.connect().await?;
        let (tx, rx) = socket.split();
        let tx = tx.with(|frame: Frame| async move {
            Ok::<_, WsError>(match frame {
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
        Ok(())
    }

    async fn connect(&self) -> Result<Socket, Failure> {
        let deadline = Instant::now() + self.retry_for;
        loop {
            let mut request = self
                .url
                .as_str()
                .into_client_request()
                .map_err(|e| Failure::Other(e.to_string()))?;
            request.headers_mut().insert(
                "authorization",
                format!("Bearer {}", self.token)
                    .parse()
                    .map_err(|_| Failure::Other("the token isn't a valid header".into()))?,
            );
            let attempt = tokio::time::timeout_at(
                deadline.into(),
                tokio_tungstenite::connect_async_tls_with_config(
                    request,
                    None,
                    true,
                    Some(self.connector.clone()),
                ),
            )
            .await;
            let last = match attempt {
                Ok(Ok((socket, _))) => return Ok(socket),
                Ok(Err(WsError::Http(response))) => {
                    let status = response.status();
                    let body = response
                        .body()
                        .as_deref()
                        .map(String::from_utf8_lossy)
                        .filter(|b| !b.is_empty())
                        .map_or_else(|| status.to_string(), |b| b.into_owned());
                    match status {
                        StatusCode::NOT_FOUND => return Err(Failure::PaneGone),
                        StatusCode::CONFLICT => return Err(Failure::Refused(body)),
                        // A proxy in front of a server that is restarting.
                        StatusCode::BAD_GATEWAY
                        | StatusCode::SERVICE_UNAVAILABLE
                        | StatusCode::GATEWAY_TIMEOUT => body,
                        _ => return Err(Failure::Other(body)),
                    }
                }
                // The server may be restarting or upgrading.
                Ok(Err(WsError::Io(error))) => error.to_string(),
                Ok(Err(error)) => return Err(Failure::Other(error.to_string())),
                Err(_) => {
                    return Err(Failure::Other(format!(
                        "the server didn't answer within {}s",
                        self.retry_for.as_secs_f32()
                    )))
                }
            };
            let retry = Instant::now() + Duration::from_millis(250);
            if retry >= deadline {
                return Err(Failure::Other(last));
            }
            tracing::debug!("forward: {last}; retrying");
            tokio::time::sleep_until(retry.into()).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    async fn serve(router: axum::Router) -> SocketAddr {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        address
    }

    fn tunnel(address: SocketAddr, retry_for: Duration) -> Tunnel {
        Tunnel {
            url: format!("ws://{address}/"),
            token: "token".into(),
            connector: connector().unwrap(),
            retry_for,
        }
    }

    /// A local connection the tunnel carries, as `forward` accepts them.
    async fn carried(tunnel: Tunnel) -> TcpStream {
        let local = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let client = TcpStream::connect(local.local_addr().unwrap())
            .await
            .unwrap();
        let (accepted, _) = local.accept().await.unwrap();
        tokio::spawn(async move { tunnel.carry(accepted).await });
        client
    }

    #[test]
    fn targets_and_urls() {
        assert_eq!(parse_target("5173").unwrap(), (None, 5173));
        assert_eq!(
            parse_target("127.0.0.1:5173").unwrap(),
            (Some([127, 0, 0, 1].into()), 5173)
        );
        assert_eq!(
            parse_target("[::1]:5173").unwrap(),
            (Some(std::net::Ipv6Addr::LOCALHOST.into()), 5173)
        );
        assert!(parse_target("::1:5173").is_err());
        assert!(parse_target("http").is_err());
        assert_eq!(
            websocket_base("https://h.ts.net:8004").unwrap(),
            "wss://h.ts.net:8004"
        );
        assert_eq!(
            websocket_base("http://127.0.0.1:8004").unwrap(),
            "ws://127.0.0.1:8004"
        );
        assert!(websocket_base("ftp://x").is_err());
        assert_eq!(
            local_url("0.0.0.0:80".parse().unwrap()),
            "http://127.0.0.1:80/"
        );
        assert_eq!(local_url("[::]:80".parse().unwrap()), "http://[::1]:80/");
    }

    /// A client that ends its request with a half-close still gets the reply
    /// the program sends after reading to EOF, through both ends of the relay.
    #[tokio::test]
    async fn a_half_closed_request_still_gets_its_reply() {
        let service = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let target = service.local_addr().unwrap();
        tokio::spawn(async move {
            let (mut tcp, _) = service.accept().await.unwrap();
            let mut request = Vec::new();
            tcp.read_to_end(&mut request).await.unwrap();
            tcp.write_all(&[b"reply:".as_slice(), &request].concat())
                .await
                .unwrap();
        });
        let server = serve(axum::Router::new().route(
            "/",
            axum::routing::get(move |ws: axum::extract::WebSocketUpgrade| async move {
                let tcp = TcpStream::connect(target).await.unwrap();
                ws.on_upgrade(move |socket| crate::ws::forward::relay_for_tests(socket, tcp))
            }),
        ))
        .await;
        let mut client = carried(tunnel(server, RECONNECT_FOR)).await;
        client.write_all(b"request").await.unwrap();
        client.shutdown().await.unwrap();
        let mut reply = Vec::new();
        tokio::time::timeout(Duration::from_secs(5), client.read_to_end(&mut reply))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(reply, b"reply:request");
    }

    #[tokio::test]
    async fn a_stalled_handshake_gives_up_at_the_deadline() {
        let stalled = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = stalled.local_addr().unwrap();
        tokio::spawn(async move {
            let mut held = Vec::new();
            while let Ok((tcp, _)) = stalled.accept().await {
                held.push(tcp);
            }
        });
        let started = Instant::now();
        let failure = tunnel(address, Duration::from_millis(300))
            .connect()
            .await
            .unwrap_err();
        assert!(started.elapsed() < Duration::from_secs(2));
        assert!(failure.message().contains("didn't answer"));
    }

    /// Counts requests; answers each status in turn, then upgrades.
    async fn answering(statuses: &'static [u16]) -> (SocketAddr, Arc<AtomicUsize>) {
        let count = Arc::new(AtomicUsize::new(0));
        let seen = count.clone();
        let address = serve(axum::Router::new().route(
            "/",
            axum::routing::get(move |ws: axum::extract::WebSocketUpgrade| {
                let n = seen.fetch_add(1, Ordering::SeqCst);
                async move {
                    use axum::response::IntoResponse;
                    match statuses.get(n) {
                        Some(&status) => axum::http::StatusCode::from_u16(status)
                            .unwrap()
                            .into_response(),
                        None => ws.on_upgrade(|_| async {}),
                    }
                }
            }),
        ))
        .await;
        (address, count)
    }

    #[tokio::test]
    async fn temporary_proxy_errors_are_retried_and_others_are_not() {
        let (address, count) = answering(&[503, 502]).await;
        tunnel(address, RECONNECT_FOR).connect().await.unwrap();
        assert_eq!(count.load(Ordering::SeqCst), 3);

        let (address, count) = answering(&[401]).await;
        let failure = tunnel(address, RECONNECT_FOR).connect().await;
        assert!(matches!(failure, Err(Failure::Other(_))));
        assert_eq!(count.load(Ordering::SeqCst), 1);

        let (address, _) = answering(&[409]).await;
        let failure = tunnel(address, RECONNECT_FOR).connect().await;
        assert!(matches!(failure, Err(Failure::Refused(_))));
        let (address, _) = answering(&[404]).await;
        let failure = tunnel(address, RECONNECT_FOR).connect().await;
        assert_eq!(failure.unwrap_err(), Failure::PaneGone);
    }

    #[tokio::test]
    async fn retries_stop_at_the_deadline() {
        let (address, count) = answering(&[503; 64]).await;
        let started = Instant::now();
        let failure = tunnel(address, Duration::from_millis(600))
            .connect()
            .await
            .unwrap_err();
        assert!(started.elapsed() < Duration::from_secs(2));
        assert!(matches!(failure, Failure::Other(_)));
        assert!(count.load(Ordering::SeqCst) >= 2);
    }
}
