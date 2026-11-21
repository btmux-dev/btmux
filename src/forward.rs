//! `btmux forward`: listen on a local port and tunnel each connection to a
//! port that a pane's program listens on, over `/ws/forward`.

use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::time::{Duration, Instant};

use futures_util::{SinkExt, StreamExt};
use reqwest::Method;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::StatusCode;
use tokio_tungstenite::tungstenite::{Error as WsError, Message};
use tokio_tungstenite::Connector;
use uuid::Uuid;

use crate::api_client::{request, request_with_status};
use crate::pane_ports::Listener;
use crate::send_notification::{server, SERVER_URL};

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
    /// The pane's port. Default: its only listening port.
    port: Option<u16>,
    /// Local address to listen on.
    #[arg(long, default_value = "127.0.0.1")]
    bind: IpAddr,
    /// Local port. Default: the pane's port when it's free here, else any.
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
    let listener = choose(&listeners, args.port)?;

    let (server_url, token) = server(profile)?;
    // On the server's own machine the pane's port is taken, or worse, a
    // wildcard listener's loopback port isn't, and taking it shadows the pane.
    let remote_port = (!is_loopback(&server_url)).then_some(listener.port);
    let local = bind(args.bind, args.local_port, remote_port).await?;
    let address = local.local_addr().map_err(|e| e.to_string())?;
    let url = local_url(address);
    println!("{url}");
    eprintln!(
        "btmux: forwarding {address} to port {} ({}) in pane {pane}; Ctrl-C stops",
        listener.port,
        listener.process.as_deref().unwrap_or("unknown program"),
    );
    if args.open {
        if let Err(error) = open::that_detached(&url) {
            eprintln!("btmux: couldn't open {url}: {error}");
        }
    }

    let tunnel = Arc::new(Tunnel {
        url: format!(
            "{}/ws/forward/{pane}?port={}",
            websocket_base(&server_url)?,
            listener.port
        ),
        token,
        connector: connector()?,
    });
    let (gone_tx, mut gone_rx) = tokio::sync::mpsc::channel::<()>(1);
    let checker = tokio::spawn(watch_pane(
        profile.map(str::to_string),
        pane,
        gone_tx.clone(),
    ));
    let code = loop {
        tokio::select! {
            accepted = local.accept() => {
                let Ok((tcp, peer)) = accepted else { continue };
                let _ = tcp.set_nodelay(true);
                let tunnel = tunnel.clone();
                let gone = gone_tx.clone();
                tokio::spawn(async move {
                    if let Err(error) = tunnel.carry(tcp, peer).await {
                        if error == Failure::PaneGone {
                            let _ = gone.try_send(());
                        } else if let Failure::Other(message) = error {
                            eprintln!("btmux: {message}");
                        }
                    }
                });
            }
            _ = gone_rx.recv() => {
                eprintln!("btmux: pane {pane} is gone");
                break PANE_GONE;
            }
            _ = tokio::signal::ctrl_c() => break 0,
        }
    };
    checker.abort();
    Ok(code)
}

/// The listener for `port`, else the pane's only one.
fn choose(listeners: &[Listener], port: Option<u16>) -> Result<Listener, String> {
    let describe = |l: &Listener| {
        format!(
            "  {:>5}  {:<15}  {}",
            l.port,
            l.address,
            l.process.as_deref().unwrap_or("?")
        )
    };
    if let Some(port) = port {
        return listeners
            .iter()
            .find(|l| l.port == port)
            .cloned()
            .ok_or_else(|| format!("no program in the pane listens on port {port}"));
    }
    let mut ports: Vec<u16> = listeners.iter().map(|l| l.port).collect();
    ports.dedup();
    match ports.as_slice() {
        [] => Err("no program in the pane listens on a TCP port".into()),
        [_] => Ok(listeners[0].clone()),
        _ => Err(format!(
            "the pane listens on several ports; name one:\n{}",
            listeners
                .iter()
                .map(describe)
                .collect::<Vec<_>>()
                .join("\n")
        )),
    }
}

/// Listens on `local` if given, else on `remote` when it's free, else on any
/// port.
async fn bind(
    address: IpAddr,
    local: Option<u16>,
    remote: Option<u16>,
) -> Result<TcpListener, String> {
    let at = |port| SocketAddr::new(address, port);
    if let Some(port) = local {
        return TcpListener::bind(at(port))
            .await
            .map_err(|e| format!("can't listen on {}: {e}", at(port)));
    }
    if let Some(Ok(listener)) = match remote {
        Some(port) => Some(TcpListener::bind(at(port)).await),
        None => None,
    } {
        return Ok(listener);
    }
    TcpListener::bind(at(0))
        .await
        .map_err(|e| format!("can't listen on {address}: {e}"))
}

/// Whether `url` names this machine by a loopback address or `localhost`.
fn is_loopback(url: &str) -> bool {
    let Ok(url) = reqwest::Url::parse(url) else {
        return false;
    };
    let Some(host) = url.host_str() else {
        return false;
    };
    let host = host.trim_start_matches('[').trim_end_matches(']');
    host.eq_ignore_ascii_case("localhost")
        || host.parse::<IpAddr>().is_ok_and(|ip| ip.is_loopback())
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
    let path = format!("/api/panes/{pane}/ports");
    loop {
        tokio::time::sleep(PANE_CHECK).await;
        // Other failures, like a server mid-upgrade, aren't the pane closing.
        if let Ok((StatusCode::NOT_FOUND, _)) =
            request_with_status(profile.as_deref(), Method::GET, &path, None, TIMEOUT).await
        {
            let _ = gone.send(()).await;
            return;
        }
    }
}

#[derive(Debug, PartialEq)]
enum Failure {
    PaneGone,
    Other(String),
}

struct Tunnel {
    url: String,
    token: String,
    connector: Connector,
}

impl Tunnel {
    /// Carries one local connection to the pane's port.
    async fn carry(&self, tcp: TcpStream, peer: SocketAddr) -> Result<(), Failure> {
        let socket = self.connect(peer).await?;
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
            let mut buffer = vec![0; 64 * 1024];
            loop {
                match tcp_rx.read(&mut buffer).await {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        let bytes = buffer[..n].to_vec().into();
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
        Ok(())
    }

    async fn connect(
        &self,
        peer: SocketAddr,
    ) -> Result<
        tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<TcpStream>>,
        Failure,
    > {
        let started = Instant::now();
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
            request.headers_mut().insert(
                crate::ws::forward::PEER_HEADER,
                peer.to_string()
                    .parse()
                    .expect("an address is a valid header"),
            );
            match tokio_tungstenite::connect_async_tls_with_config(
                request,
                None,
                true,
                Some(self.connector.clone()),
            )
            .await
            {
                Ok((socket, _)) => return Ok(socket),
                Err(WsError::Http(response)) => {
                    let status = response.status();
                    if status == StatusCode::NOT_FOUND {
                        return Err(Failure::PaneGone);
                    }
                    let body = response
                        .body()
                        .as_deref()
                        .map(String::from_utf8_lossy)
                        .filter(|b| !b.is_empty())
                        .map_or_else(|| status.to_string(), |b| b.into_owned());
                    return Err(Failure::Other(body));
                }
                // The server may be restarting or upgrading.
                Err(WsError::Io(error)) if started.elapsed() < RECONNECT_FOR => {
                    tracing::debug!("forward: {error}; retrying");
                    tokio::time::sleep(Duration::from_millis(250)).await;
                }
                Err(error) => return Err(Failure::Other(error.to_string())),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn listener(port: u16) -> Listener {
        Listener {
            port,
            address: "127.0.0.1".into(),
            pid: Some(1),
            process: Some("node".into()),
            shadowed: false,
        }
    }

    #[test]
    fn choose_takes_the_only_port_or_the_named_one() {
        assert_eq!(choose(&[listener(5173)], None).unwrap().port, 5173);
        // One port on two addresses is still one port.
        let both = [
            listener(5173),
            Listener {
                address: "::1".into(),
                ..listener(5173)
            },
        ];
        assert_eq!(choose(&both, None).unwrap().port, 5173);
        let two = [listener(3000), listener(5173)];
        assert!(choose(&two, None).unwrap_err().contains("several ports"));
        assert_eq!(choose(&two, Some(3000)).unwrap().port, 3000);
        assert!(choose(&two, Some(1)).is_err());
        assert!(choose(&[], None).is_err());
    }

    #[test]
    fn urls() {
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
        assert!(is_loopback("http://127.0.0.1:8004"));
        assert!(is_loopback("http://localhost:8004/"));
        assert!(is_loopback("http://[::1]:8004"));
        assert!(!is_loopback("https://machine.tailnet.ts.net:8004"));
    }
}
