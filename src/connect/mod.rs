//! Connections that reach the server through a btmux connect relay: the
//! tunnel (`tunnel.rs`), or the hidden `--connect-listen` listener, which
//! tests use in its place.
//!
//! Relayed tabs get no browser or VS Code surface: the browser streams a JPEG
//! per paint, which the relay can't afford, and `code tunnel` already covers
//! VS Code. The routes refuse them, and each relayed control socket gets a
//! config without them.

use axum::{
    extract::Request,
    http::StatusCode,
    middleware::Next,
    response::{IntoResponse, Response},
    Extension, Router,
};
use serde_json::Value;

pub mod acme;
pub mod passkeys;
pub mod session;
pub mod signup;
pub mod status;
pub mod tunnel;

use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};

/// What starting btmux connect needs from the running server; installed at
/// startup, so sign-up can start it later too.
pub struct Server {
    pub app: Router,
    pub auth: Arc<crate::auth::Auth>,
    pub events: tokio::sync::broadcast::Sender<String>,
    pub state_file: PathBuf,
    /// Set once connect starts.
    pub status: OnceLock<Arc<status::Status>>,
}

static SERVER: OnceLock<Arc<Server>> = OnceLock::new();
/// The tunnel, until shutdown takes it; `None` inside once shut down.
static TUNNEL: Mutex<Option<Option<tunnel::Tunnel>>> = Mutex::new(Some(None));

pub fn install(server: Server) {
    let _ = SERVER.set(Arc::new(server));
}

/// What Settings shows, once btmux connect started.
pub fn status() -> Option<status::ConnectStatus> {
    Some(SERVER.get()?.status.get()?.get())
}

/// Brings up btmux connect for `registration`: relayed sign-in, Settings'
/// status, and the tunnel. Before any relayed listener: without the
/// registration's origin, relayed requests are refused.
pub fn start(registration: tunnel::Registration) -> Result<(), String> {
    start_with(
        SERVER.get().ok_or("btmux connect needs a state file")?,
        registration,
    )
}

fn start_with(server: &Server, registration: tunnel::Registration) -> Result<(), String> {
    let status = status::Status::new(&registration, Some(server.events.clone()));
    server
        .status
        .set(status.clone())
        .map_err(|_| "btmux connect is already running".to_owned())?;
    // Replaces Settings' sign-up form.
    status.publish();
    let failed = |error: String| {
        status.update(|s| {
            s.tunnel = status::TunnelState::Failed {
                error: error.clone(),
            }
        });
        error
    };
    server
        .auth
        .set_relayed_origin(&registration.public_url)
        .map_err(failed)?;
    server.auth.set_passkeys(
        passkeys::Passkeys::new(&registration, &server.state_file, Some(status.clone()))
            .map_err(failed)?,
    );
    // Expired sessions lose their connections within a minute.
    if let Some(passkeys) = server.auth.passkeys().cloned() {
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(std::time::Duration::from_secs(60));
            loop {
                tick.tick().await;
                passkeys.cut_expired();
            }
        });
    }
    // Under the lock, so a sign-up that finishes during shutdown starts no
    // tunnel that shutdown would miss.
    let mut slot = TUNNEL.lock().unwrap();
    let Some(slot) = slot.as_mut() else {
        return Ok(());
    };
    match tunnel::spawn(
        registration,
        &server.state_file,
        server.app.clone(),
        status.clone(),
    ) {
        Ok(tunnel) => *slot = Some(tunnel),
        Err(error) => {
            tracing::error!("btmux connect: {error}");
            status.update(|s| s.tunnel = status::TunnelState::Failed { error });
        }
    }
    Ok(())
}

/// Closes the tunnel, so the relay drops the route at once.
pub async fn shutdown() {
    let tunnel = TUNNEL.lock().unwrap().take().flatten();
    if let Some(tunnel) = tunnel {
        tunnel.close().await;
    }
}

pub fn routes() -> Router {
    Router::new().route(
        "/api/connect/register",
        axum::routing::get(available).post(register),
    )
}

/// Whether this server can sign up: it needs a state file to keep the
/// registration in.
async fn available() -> axum::Json<Value> {
    axum::Json(serde_json::json!({ "available": SERVER.get().is_some() }))
}

#[derive(serde::Deserialize)]
struct SignUp {
    invite: String,
    #[serde(default)]
    accept_acme_terms: bool,
}

/// One sign-up at a time, so two can't both register.
static SIGNING_UP: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Redeems an invite for a registration and starts btmux connect with it.
async fn register(
    tunneled: Option<Extension<Tunneled>>,
    axum::Json(request): axum::Json<SignUp>,
) -> Response {
    if tunneled.is_some() {
        return (StatusCode::FORBIDDEN, "Sign up on the machine itself.").into_response();
    }
    if !request.accept_acme_terms {
        return (
            StatusCode::BAD_REQUEST,
            format!(
                "Agree to Let's Encrypt's Subscriber Agreement ({}), which issues this \
                 machine's certificate.",
                acme::LETS_ENCRYPT_TERMS
            ),
        )
            .into_response();
    }
    let Some(server) = SERVER.get() else {
        return (
            StatusCode::CONFLICT,
            "This server keeps no state, so it can't hold a registration.",
        )
            .into_response();
    };
    let service = match signup::service() {
        Ok(service) => service,
        Err(failure) => return (failure.status, failure.message).into_response(),
    };
    sign_up(server.clone(), service, request.invite).await
}

/// Runs the sign-up in a task of its own and waits for it: a client that
/// leaves can't cancel it after the service redeemed the invite, which it
/// would refuse a second time.
async fn sign_up(server: Arc<Server>, service: String, invite: String) -> Response {
    match tokio::spawn(redeem(server, service, invite)).await {
        Ok(Ok(answer)) => axum::Json(answer).into_response(),
        Ok(Err((status, message))) => (status, message).into_response(),
        Err(error) => (StatusCode::INTERNAL_SERVER_ERROR, error.to_string()).into_response(),
    }
}

async fn redeem(
    server: Arc<Server>,
    service: String,
    invite: String,
) -> Result<Value, (StatusCode, String)> {
    let _signing_up = SIGNING_UP.lock().await;
    if server.status.get().is_some() {
        return Err((
            StatusCode::CONFLICT,
            "This server is already connected.".into(),
        ));
    }
    // One that didn't start, which a new one would replace.
    match tunnel::Registration::load(&server.state_file) {
        Ok(None) => {}
        Ok(Some(_)) => {
            return Err((
                StatusCode::CONFLICT,
                format!(
                    "This server has a registration that didn't start ({}); see its log.",
                    tunnel::path(&server.state_file).display()
                ),
            ))
        }
        Err(error) => return Err((StatusCode::INTERNAL_SERVER_ERROR, error)),
    }
    let registration = signup::register(&service, &invite, &server.state_file)
        .await
        .map_err(|failure| (failure.status, failure.message))?;
    let answer = serde_json::json!({
        "hostname": registration.hostname,
        "public_url": registration.public_url,
    });
    tracing::info!(hostname = registration.hostname, "btmux connect: signed up");
    start_with(&server, registration)
        .map_err(|error| (StatusCode::INTERNAL_SERVER_ERROR, error))?;
    Ok(answer)
}

/// `btmux connect`: sign up with an invite code.
#[derive(clap::Args, Clone, Debug)]
pub struct ConnectArgs {
    /// An invite code (`btmi_…`); asked for if not given.
    invite: Option<String>,
    /// Agree to Let's Encrypt's Subscriber Agreement
    /// (https://letsencrypt.org/repository/), which issues this machine's
    /// certificate, without being asked.
    #[arg(long)]
    accept_acme_terms: bool,
}

pub async fn connect_cli(args: &ConnectArgs, profile: Option<&str>) -> i32 {
    use std::io::{BufRead, Write};
    let ask = |prompt: &str| -> Option<String> {
        eprint!("{prompt}");
        let _ = std::io::stderr().flush();
        let mut line = String::new();
        std::io::stdin().lock().read_line(&mut line).ok()?;
        Some(line.trim().to_owned())
    };
    let invite = match &args.invite {
        Some(invite) => invite.clone(),
        None => match ask("Invite code: ") {
            Some(invite) if !invite.is_empty() => invite,
            _ => {
                eprintln!("btmux: btmux connect needs an invite code");
                return 1;
            }
        },
    };
    if !args.accept_acme_terms {
        eprintln!(
            "Let's Encrypt issues this machine's certificate; its Subscriber Agreement is at {}",
            acme::LETS_ENCRYPT_TERMS
        );
        let agreed = ask("Do you agree to it? [y/N] ")
            .is_some_and(|answer| matches!(answer.to_ascii_lowercase().as_str(), "y" | "yes"));
        if !agreed {
            eprintln!("btmux: not connected");
            return 1;
        }
    }
    let registered = crate::api_client::request(
        profile,
        reqwest::Method::POST,
        "/api/connect/register",
        Some(serde_json::json!({ "invite": invite, "accept_acme_terms": true })),
        // Sign-up retries the service for a minute.
        std::time::Duration::from_secs(90),
    )
    .await;
    let url = match registered {
        Ok(body) => serde_json::from_str::<Value>(&body)
            .ok()
            .and_then(|body| body["public_url"].as_str().map(str::to_owned))
            .unwrap_or_default(),
        Err(error) => {
            eprintln!("btmux: {error}");
            return 1;
        }
    };
    eprintln!("btmux: connected; this machine is {url}");
    eprintln!(
        "Let's Encrypt issues its certificate within a minute or two; until then, browsers warn \
         about the address. Browsers sign in there with a passkey. Add the first one:"
    );
    passkeys::enroll_cli(profile).await
}

/// Replaces `path` atomically with an owner-only file holding `contents`,
/// synced before the rename so a crash leaves the old file or the new one.
pub fn write_private(path: &std::path::Path, contents: &[u8]) -> Result<(), String> {
    use std::io::Write;
    let mut temp = path.as_os_str().to_owned();
    temp.push(".tmp");
    let temp = std::path::PathBuf::from(temp);
    let _ = std::fs::remove_file(&temp);
    let error = |e: std::io::Error| format!("{}: {e}", path.display());
    let mut file = crate::discovery::create_private_file(&temp).map_err(error)?;
    file.write_all(contents).map_err(error)?;
    file.sync_all().map_err(error)?;
    drop(file);
    std::fs::rename(&temp, path).map_err(error)
}

/// Marks requests that arrived through the relay. It comes from the listener
/// that accepted the connection, never from anything a client sends.
#[derive(Clone, Copy)]
pub struct Tunneled;

/// The app as relayed connections see it.
pub fn app(app: Router) -> Router {
    app.layer(axum::middleware::from_fn(refuse_surfaces))
        .layer(Extension(Tunneled))
}

/// Binds `--connect-listen` and serves relayed connections in the background.
pub async fn listen(addr: &str, app: Router) {
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .unwrap_or_else(|e| {
            eprintln!("error: cannot bind --connect-listen {addr}: {e}");
            std::process::exit(1);
        });
    tracing::info!("serving relayed connections on {addr}");
    let app = self::app(app);
    tokio::spawn(async move {
        loop {
            match listener.accept().await {
                Ok((stream, _)) => {
                    let _ = stream.set_nodelay(true);
                    tokio::spawn(serve(stream, app.clone()));
                }
                Err(error) => {
                    tracing::warn!("relayed listener: {error}");
                    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                }
            }
        }
    });
}

/// One relayed connection, which signing out can cut (`connect::passkeys`).
/// Shared by its I/O and the requests it carries, so it lives as long as the
/// connection, including WebSockets upgraded from it.
pub struct Live {
    cut: tokio_util::sync::CancellationToken,
}

impl Live {
    pub fn cut(&self) {
        self.cut.cancel();
    }
}

/// The connection a relayed request arrived on.
#[derive(Clone)]
pub struct RelayedConnection(pub std::sync::Arc<Live>);

impl RelayedConnection {
    pub fn new() -> Self {
        Self(std::sync::Arc::new(Live {
            cut: tokio_util::sync::CancellationToken::new(),
        }))
    }

    #[cfg(test)]
    pub fn is_cut(&self) -> bool {
        self.0.cut.is_cancelled()
    }
}

/// Relayed I/O that fails once its connection is cut, so a WebSocket upgraded
/// from it ends too: a signed-out session can't keep a terminal it opened.
struct Cuttable<T> {
    io: T,
    _live: std::sync::Arc<Live>,
    cut: std::pin::Pin<Box<tokio_util::sync::WaitForCancellationFutureOwned>>,
}

fn cuttable<T>(io: T, connection: &RelayedConnection) -> Cuttable<T> {
    Cuttable {
        io,
        cut: Box::pin(connection.0.cut.clone().cancelled_owned()),
        _live: connection.0.clone(),
    }
}

impl<T> Cuttable<T> {
    fn check(&mut self, cx: &mut std::task::Context<'_>) -> std::io::Result<()> {
        use std::future::Future;
        match self.cut.as_mut().poll(cx) {
            std::task::Poll::Ready(()) => Err(std::io::Error::new(
                std::io::ErrorKind::ConnectionAborted,
                "signed out",
            )),
            std::task::Poll::Pending => Ok(()),
        }
    }
}

impl<T: tokio::io::AsyncRead + Unpin> tokio::io::AsyncRead for Cuttable<T> {
    fn poll_read(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        let this = self.get_mut();
        if let Err(error) = this.check(cx) {
            return std::task::Poll::Ready(Err(error));
        }
        std::pin::Pin::new(&mut this.io).poll_read(cx, buf)
    }
}

impl<T: tokio::io::AsyncWrite + Unpin> tokio::io::AsyncWrite for Cuttable<T> {
    fn poll_write(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &[u8],
    ) -> std::task::Poll<std::io::Result<usize>> {
        let this = self.get_mut();
        if let Err(error) = this.check(cx) {
            return std::task::Poll::Ready(Err(error));
        }
        std::pin::Pin::new(&mut this.io).poll_write(cx, buf)
    }

    fn poll_flush(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        let this = self.get_mut();
        if let Err(error) = this.check(cx) {
            return std::task::Poll::Ready(Err(error));
        }
        std::pin::Pin::new(&mut this.io).poll_flush(cx)
    }

    fn poll_shutdown(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        std::pin::Pin::new(&mut self.get_mut().io).poll_shutdown(cx)
    }
}

/// Serves one relayed connection (the tunnel's decrypted stream, or a
/// `--connect-listen` socket) as HTTP/1.1 with upgrades for WebSockets. Each
/// request carries the connection, so a sign-in can register it to be cut.
pub async fn serve<T>(io: T, app: Router)
where
    T: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static,
{
    let connection = RelayedConnection::new();
    let io = cuttable(io, &connection);
    let service = hyper::service::service_fn(
        move |mut request: axum::extract::Request<hyper::body::Incoming>| {
            request.extensions_mut().insert(connection.clone());
            tower::ServiceExt::oneshot(app.clone(), request)
        },
    );
    if let Err(error) = hyper::server::conn::http1::Builder::new()
        .timer(hyper_util::rt::TokioTimer::new())
        .header_read_timeout(std::time::Duration::from_secs(30))
        .serve_connection(hyper_util::rt::TokioIo::new(io), service)
        .with_upgrades()
        .await
    {
        tracing::debug!("relayed connection ended: {error}");
    }
}

/// Surfaces that would act on the server's machine for a relayed tab, and
/// port forwards, which would carry arbitrary TCP through the relay.
fn refused(path: &str) -> bool {
    path == "/ws/browser"
        || path.starts_with("/ws/forward/")
        || path == "/api/vscode"
        || path.starts_with("/api/vscode/")
        || path == "/vscode"
        || path.starts_with("/vscode/")
}

async fn refuse_surfaces(request: Request, next: Next) -> Response {
    if refused(request.uri().path()) {
        return (StatusCode::FORBIDDEN, "Not available over btmux connect").into_response();
    }
    next.run(request).await
}

/// Announces compressed output on a relayed pane socket; it precedes every
/// other frame.
pub const PANE_COMPRESSION_FRAME: &str = r#"{"type":"compression","format":"deflate-raw"}"#;

/// Compresses a relayed pane socket's output frames. Terminal output is mostly
/// escape sequences repeated across small messages, so one deflate stream per
/// socket, sync-flushed after each frame, shrinks it about 12x, where
/// compressing frames separately gets about 2x. Each frame starts with its
/// uncompressed length as a big-endian u32, so the browser knows when a
/// frame's output is complete and can keep text frames in order with it.
pub struct PaneDeflater(flate2::Compress);

impl PaneDeflater {
    pub fn new() -> Self {
        Self(flate2::Compress::new(flate2::Compression::new(6), false))
    }

    pub fn frame(&mut self, data: &[u8]) -> Vec<u8> {
        let mut out = Vec::with_capacity(4 + data.len() / 2 + 64);
        out.extend_from_slice(&(data.len() as u32).to_be_bytes());
        let mut consumed = 0;
        loop {
            if out.capacity() - out.len() < 64 {
                out.reserve(out.capacity());
            }
            let before = self.0.total_in();
            self.0
                .compress_vec(&data[consumed..], &mut out, flate2::FlushCompress::Sync)
                .expect("deflate on an in-memory buffer");
            consumed += (self.0.total_in() - before) as usize;
            // Spare room left means the sync flush was written in full.
            if consumed == data.len() && out.len() < out.capacity() {
                return out;
            }
        }
    }
}

/// serde writes an internally tagged enum's tag first.
const CONFIG_PREFIX: &str = r#"{"type":"config","#;

/// Rewrites a pre-serialized control event for a relayed tab; others pass
/// through untouched, so only config events are parsed.
pub fn relay_event(json: String) -> String {
    if !json.starts_with(CONFIG_PREFIX) {
        return json;
    }
    let Ok(mut message) = serde_json::from_str::<Value>(&json) else {
        return json;
    };
    let Some(config) = message.get_mut("config").and_then(Value::as_object_mut) else {
        return json;
    };
    config.insert("relayed".into(), true.into());
    config.insert("browser".into(), false.into());
    if let Some(editors) = config.get_mut("editors").and_then(Value::as_array_mut) {
        editors.retain(|editor| editor != "vscode");
    }
    if config
        .get("editor")
        .is_some_and(|editor| editor == "vscode")
    {
        config.insert("editor".into(), "native".into());
    }
    message.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A client that leaves after the service redeemed its invite doesn't
    /// lose the machine: the registration is saved and connect started.
    #[tokio::test]
    async fn sign_up_finishes_when_the_client_leaves() {
        use std::sync::Arc;
        use tokio::sync::Notify;
        let (redeemed, release) = (Arc::new(Notify::new()), Arc::new(Notify::new()));
        let app = {
            let (redeemed, release) = (redeemed.clone(), release.clone());
            Router::new().route(
                "/v1/machines",
                axum::routing::post(move || async move {
                    redeemed.notify_one();
                    release.notified().await;
                    (
                        StatusCode::CREATED,
                        axum::Json(signup::test_answer("abcd1234.btmux.test")),
                    )
                }),
            )
        };
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let service = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, app).await });

        let dir = std::env::temp_dir().join(format!("btmux-sign-up-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let state_file = dir.join("state.json");
        let server = Arc::new(Server {
            app: Router::new(),
            auth: Arc::new(crate::auth::Auth::new("t".repeat(32), "127.0.0.1", 8004, &[]).unwrap()),
            events: tokio::sync::broadcast::channel(16).0,
            state_file: state_file.clone(),
            status: OnceLock::new(),
        });
        let mut events = server.events.subscribe();
        let client = tokio::spawn(sign_up(server.clone(), service.clone(), "good".into()));
        redeemed.notified().await;
        client.abort();
        release.notify_one();
        let mut saved = None;
        for _ in 0..100 {
            saved = tunnel::Registration::load(&state_file).unwrap();
            if saved.is_some() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        assert_eq!(saved.expect("not saved").hostname, "abcd1234.btmux.test");
        // And started: relayed sign-in now knows the machine's origin.
        for _ in 0..100 {
            if server.auth.passkeys().is_some() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        assert!(server.auth.passkeys().is_some(), "not started");
        // Settings' form gets the status at once, not when the tunnel is up.
        let event = tokio::time::timeout(std::time::Duration::from_secs(5), events.recv())
            .await
            .unwrap()
            .unwrap();
        assert!(event.contains("abcd1234.btmux.test"), "{event}");
        // Running, so another sign-up is refused, even without the file.
        std::fs::remove_file(tunnel::path(&state_file)).unwrap();
        let again = redeem(server.clone(), service, "good".into()).await;
        assert_eq!(again.err().unwrap().0, StatusCode::CONFLICT);
        std::fs::remove_dir_all(dir).unwrap();
    }
    use crate::config::{ColorMode, Editor, FileConfig};
    use crate::ws::control::ServerMessage;

    fn config_event(editor: Editor, editors: Vec<Editor>) -> String {
        let mut config = crate::config::resolve_binds(&FileConfig::default(), ColorMode::Dark);
        config.editor = editor;
        config.editors = editors;
        config.browser = true;
        serde_json::to_string(&ServerMessage::Config {
            config: Box::new(config),
        })
        .unwrap()
    }

    #[test]
    fn relayed_config_drops_the_heavy_surfaces() {
        let json = config_event(
            Editor::Vscode,
            vec![Editor::Native, Editor::Neovim, Editor::Vscode],
        );
        assert!(json.starts_with(CONFIG_PREFIX));
        let relayed: Value = serde_json::from_str(&relay_event(json)).unwrap();
        let config = &relayed["config"];
        assert_eq!(config["relayed"], true);
        assert_eq!(config["browser"], false);
        assert_eq!(config["editors"], serde_json::json!(["native", "neovim"]));
        assert_eq!(config["editor"], "native");

        let state = r#"{"type":"state","sessions":[]}"#.to_string();
        assert_eq!(relay_event(state.clone()), state);
    }

    #[test]
    fn pane_frames_inflate_in_order_from_one_stream() {
        let mut deflater = PaneDeflater::new();
        let mut inflater = flate2::Decompress::new(false);
        let prompt = b"\x1b[32m~/repos/btmux\x1b[0m main \xe2\x9d\xaf ".repeat(4);
        let big: Vec<u8> = (0..200_000u32).map(|i| (i * 7919 % 251) as u8).collect();
        let mut sizes = Vec::new();
        for message in [&b"x"[..], &prompt, &prompt, &big, &prompt] {
            let frame = deflater.frame(message);
            sizes.push(frame.len());
            let size = u32::from_be_bytes(frame[..4].try_into().unwrap()) as usize;
            assert_eq!(size, message.len());
            // Everything a frame encodes is available without the next one.
            let mut out = Vec::with_capacity(size + 1);
            inflater
                .decompress_vec(&frame[4..], &mut out, flate2::FlushDecompress::Sync)
                .unwrap();
            assert_eq!(out, message);
        }
        // A repeat compresses against the shared window.
        assert!(sizes[2] < prompt.len() / 4, "{sizes:?}");
    }

    /// A terminal's socket waits in a read; cutting its connection must end it.
    #[tokio::test]
    async fn cutting_a_connection_fails_its_pending_io() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let (near, mut far) = tokio::io::duplex(64);
        let connection = RelayedConnection::new();
        let mut io = cuttable(near, &connection);
        far.write_all(b"before").await.unwrap();
        let mut buf = [0; 6];
        io.read_exact(&mut buf).await.unwrap();
        let reader = tokio::spawn(async move {
            let mut buf = [0; 1];
            let read = io.read(&mut buf).await;
            (read, io)
        });
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        connection.0.cut();
        assert!(connection.is_cut());
        let (read, mut io) = tokio::time::timeout(std::time::Duration::from_secs(1), reader)
            .await
            .expect("the pending read wasn't woken")
            .unwrap();
        assert_eq!(
            read.unwrap_err().kind(),
            std::io::ErrorKind::ConnectionAborted
        );
        assert!(io.write_all(b"after").await.is_err());
        drop(far);
    }

    #[tokio::test]
    async fn relayed_requests_cannot_reach_the_surfaces() {
        use axum::{body::Body, http::Request, routing::get};
        use tower::ServiceExt;
        let app = app(Router::new().fallback(get(|| async { "ok" })));
        for (path, status) in [
            ("/ws/browser", StatusCode::FORBIDDEN),
            ("/api/vscode", StatusCode::FORBIDDEN),
            ("/api/vscode/license", StatusCode::FORBIDDEN),
            ("/vscode/", StatusCode::FORBIDDEN),
            ("/vscode/static/x.js", StatusCode::FORBIDDEN),
            ("/ws/forward/id?port=80", StatusCode::FORBIDDEN),
            ("/api/sessions", StatusCode::OK),
            ("/vscodex", StatusCode::OK),
        ] {
            let request = Request::get(path).body(Body::empty()).unwrap();
            let response = app.clone().oneshot(request).await.unwrap();
            assert_eq!(response.status(), status, "{path}");
        }
    }
}
