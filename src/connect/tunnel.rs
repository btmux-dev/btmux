//! The tunnel client: one outbound QUIC connection to a btmux connect relay,
//! which opens a stream per browser connection. Each stream carries the
//! browser's own TLS session, terminated here with this machine's certificate,
//! so the relay never sees plaintext. Streams serve the main listener's app,
//! marked `Tunneled`.
//!
//! Wire protocol (`btmux-tunnel/1`, the ALPN), shared with the relay in
//! btmux-connect: the client verifies the relay against its pinned tunnel CA,
//! opens a bidirectional stream, and writes one JSON line,
//! `{"hostname":…,"token":…}`; the relay answers `ok`. From then on the relay
//! opens one stream per browser connection, starting with its ClientHello. A
//! relay closes a hostname's old tunnel with `REPLACED` when it registers
//! again, as after an upgrade's exec.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::Router;
use base64::Engine;
use rustls::pki_types::{pem::PemObject, CertificateDer};
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufReadExt, BufReader};

/// A machine's registration with a relay, kept owner-only beside the state
/// file. `btmux connect-dev` writes one until sign-up exists.
#[derive(Serialize, Deserialize, Clone)]
pub struct Registration {
    /// The relay's tunnel endpoint, `host:port` (UDP).
    pub relay: String,
    /// The name the relay's certificate is checked against.
    pub relay_name: String,
    /// The relay's tunnel CA (DER, base64), trusted for the tunnel only.
    pub relay_ca: String,
    /// The name browsers reach this machine at.
    pub hostname: String,
    /// The origin browsers use, allowed like `--public-url`.
    pub public_url: String,
    /// The relay's credential for this machine.
    pub token: String,
    /// This machine's certificate chain and key for `hostname` (PEM).
    pub cert: String,
    pub key: String,
    /// The ACME directory that issues and renews `cert` (`acme.rs`); set
    /// only with the owner's agreement to its terms. Unset, `cert` is used
    /// as is.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub acme: Option<String>,
    /// The ACME account (its URL) `cert` was issued to, saved with it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub issued_to: Option<String>,
}

pub fn path(state_file: &Path) -> PathBuf {
    state_file.with_extension("connect")
}

impl Registration {
    pub fn load(state_file: &Path) -> Result<Option<Self>, String> {
        let path = path(state_file);
        match std::fs::read(&path) {
            Ok(bytes) => serde_json::from_slice(&bytes)
                .map(Some)
                .map_err(|e| format!("{}: {e}", path.display())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(format!("{}: {e}", path.display())),
        }
    }

    /// Replaces the registration atomically, owner-only.
    pub fn save(&self, state_file: &Path) -> Result<(), String> {
        let json = serde_json::to_vec_pretty(self).map_err(|e| e.to_string())?;
        super::write_private(&path(state_file), &json)
    }
}

/// A tunnel's limits; tests shrink them.
#[derive(Clone, Copy)]
struct Limits {
    /// Browser connections at once: streams the relay may have open.
    streams: u32,
    /// Time a browser gets to finish its TLS handshake. A stalled one would
    /// otherwise hold a stream before `auth.rs` ever sees it; one that keeps
    /// stalling new ones is for the relay to limit.
    handshake: Duration,
    /// Time one relay address gets to connect.
    connect: Duration,
    /// Delay before also trying the next relay address (RFC 8305).
    stagger: Duration,
}

const LIMITS: Limits = Limits {
    streams: 1024,
    handshake: Duration::from_secs(10),
    connect: Duration::from_secs(5),
    stagger: Duration::from_millis(250),
};

/// Connects to the relay in the background and keeps reconnecting.
pub fn spawn(registration: Registration, state_file: &Path, app: Router) -> Result<(), String> {
    spawn_with(registration, state_file, app, LIMITS).map(drop)
}

fn spawn_with(
    registration: Registration,
    state_file: &Path,
    app: Router,
    limits: Limits,
) -> Result<Arc<super::acme::Certificates>, String> {
    let certificates = Arc::new(super::acme::Certificates::load(&registration)?);
    let tls = server_tls(certificates.clone())?;
    let client = quic_client(&registration, limits.streams)?;
    let (up, tunnel_up) = tokio::sync::watch::channel(false);
    if let Some(directory) = registration.acme.clone() {
        tokio::spawn(
            super::acme::Maintainer::new(
                directory,
                registration.hostname.clone(),
                state_file.to_owned(),
                certificates.clone(),
                tunnel_up,
            )
            .run(),
        );
    }
    tokio::spawn(run(registration, client, tls, super::app(app), limits, up));
    Ok(certificates)
}

/// The tunnel protocol, offered as ALPN.
pub const ALPN: &[u8] = b"btmux-tunnel/1";
/// The relay's close code when another connection registered this hostname.
const REPLACED: u32 = 0x10;

const MIN_BACKOFF: Duration = Duration::from_millis(500);
const MAX_BACKOFF: Duration = Duration::from_secs(30);

async fn run(
    registration: Registration,
    client: quinn::ClientConfig,
    tls: tokio_rustls::TlsAcceptor,
    app: Router,
    limits: Limits,
    up: tokio::sync::watch::Sender<bool>,
) {
    let mut backoff = MIN_BACKOFF;
    loop {
        let started = Instant::now();
        let ended = connect(&registration, &client, &tls, &app, limits, &up).await;
        up.send_replace(false);
        if started.elapsed() > MAX_BACKOFF {
            backoff = MIN_BACKOFF;
        }
        if ended.replaced {
            // Two copies of one registration would take turns forever.
            tracing::warn!(
                hostname = registration.hostname,
                "btmux connect tunnel: replaced by another connection for this \
                 hostname; is this registration in use elsewhere?"
            );
            backoff = MAX_BACKOFF;
        } else {
            tracing::warn!(
                relay = registration.relay,
                "btmux connect tunnel: {}",
                ended.error
            );
        }
        tokio::time::sleep(backoff).await;
        backoff = (backoff * 2).min(MAX_BACKOFF);
    }
}

/// Why a tunnel ended.
struct Ended {
    error: String,
    replaced: bool,
}

impl From<String> for Ended {
    fn from(error: String) -> Self {
        Self {
            error,
            replaced: false,
        }
    }
}

impl From<quinn::ConnectionError> for Ended {
    fn from(error: quinn::ConnectionError) -> Self {
        let replaced = matches!(
            &error,
            quinn::ConnectionError::ApplicationClosed(close)
                if close.error_code == quinn::VarInt::from_u32(REPLACED)
        );
        Self {
            error: error.to_string(),
            replaced,
        }
    }
}

/// Serves one tunnel until it fails.
async fn connect(
    registration: &Registration,
    client: &quinn::ClientConfig,
    tls: &tokio_rustls::TlsAcceptor,
    app: &Router,
    limits: Limits,
    up: &tokio::sync::watch::Sender<bool>,
) -> Ended {
    let result = async {
        let addresses: Vec<_> = tokio::net::lookup_host(&registration.relay)
            .await
            .map_err(|e| e.to_string())?
            .collect();
        // The endpoint must outlive the connection.
        let (_endpoint, connection) =
            connect_any(&addresses, &registration.relay_name, client, limits).await?;
        // A close during registration surfaces as a read error; its reason
        // is the connection's.
        register(&connection, registration).await.map_err(|e| {
            connection
                .close_reason()
                .map_or(Ended::from(e), Ended::from)
        })?;
        tracing::info!(
            hostname = registration.hostname,
            relay = registration.relay,
            "btmux connect tunnel up"
        );
        up.send_replace(true);
        loop {
            let (send, recv) = connection.accept_bi().await?;
            tokio::spawn(serve(
                send,
                recv,
                tls.clone(),
                app.clone(),
                limits.handshake,
            ));
        }
    }
    .await;
    match result {
        Ok(never) => never,
        Err(error) => error,
    }
}

/// Tries the relay's addresses in order, starting the next one `stagger`
/// after the last unless that already failed (RFC 8305), so an unreachable
/// first address, such as IPv6 without a route, doesn't hide a working one.
async fn connect_any(
    addresses: &[std::net::SocketAddr],
    name: &str,
    client: &quinn::ClientConfig,
    limits: Limits,
) -> Result<(quinn::Endpoint, quinn::Connection), String> {
    let mut pending = addresses.iter().copied();
    let mut attempts = tokio::task::JoinSet::new();
    let mut errors = Vec::new();
    let start = |attempts: &mut tokio::task::JoinSet<_>, address| {
        attempts.spawn(attempt(
            address,
            name.to_owned(),
            client.clone(),
            limits.connect,
        ));
    };
    loop {
        if attempts.is_empty() {
            match pending.next() {
                Some(address) => start(&mut attempts, address),
                None if errors.is_empty() => return Err("the relay's name has no address".into()),
                None => return Err(errors.join("; ")),
            }
        }
        tokio::select! {
            Some(joined) = attempts.join_next() => match joined {
                Ok(Ok(connected)) => return Ok(connected),
                Ok(Err(error)) => errors.push(error),
                Err(error) => errors.push(error.to_string()),
            },
            () = tokio::time::sleep(limits.stagger), if pending.len() > 0 => {
                if let Some(address) = pending.next() {
                    start(&mut attempts, address);
                }
            }
        }
    }
}

async fn attempt(
    address: std::net::SocketAddr,
    name: String,
    client: quinn::ClientConfig,
    timeout: Duration,
) -> Result<(quinn::Endpoint, quinn::Connection), String> {
    use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr};
    let bind = if address.is_ipv6() {
        SocketAddr::from((Ipv6Addr::UNSPECIFIED, 0))
    } else {
        SocketAddr::from((Ipv4Addr::UNSPECIFIED, 0))
    };
    let failed = |e: &dyn std::fmt::Display| format!("{address}: {e}");
    let mut endpoint = quinn::Endpoint::client(bind).map_err(|e| failed(&e))?;
    endpoint.set_default_client_config(client);
    let connecting = endpoint.connect(address, &name).map_err(|e| failed(&e))?;
    let connection = tokio::time::timeout(timeout, connecting)
        .await
        .map_err(|_| failed(&"timed out"))?
        .map_err(|e| failed(&e))?;
    Ok((endpoint, connection))
}

async fn register(
    connection: &quinn::Connection,
    registration: &Registration,
) -> Result<(), String> {
    let (mut send, recv) = connection.open_bi().await.map_err(|e| e.to_string())?;
    let hello = serde_json::json!({"hostname": registration.hostname, "token": registration.token});
    send.write_all(format!("{hello}\n").as_bytes())
        .await
        .map_err(|e| e.to_string())?;
    let mut line = String::new();
    tokio::time::timeout(
        Duration::from_secs(10),
        BufReader::new(recv).read_line(&mut line),
    )
    .await
    .map_err(|_| "the relay didn't answer the registration")?
    .map_err(|e| e.to_string())?;
    match line.trim() {
        "ok" => Ok(()),
        refusal => Err(format!("the relay refused the registration: {refusal}")),
    }
}

/// One browser connection: TLS, then HTTP/1.1 with upgrades for WebSockets.
async fn serve(
    send: quinn::SendStream,
    recv: quinn::RecvStream,
    tls: tokio_rustls::TlsAcceptor,
    app: Router,
    handshake: Duration,
) {
    let accepted = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let accept = tls.accept(RelayStream {
        send,
        recv,
        accepted: accepted.clone(),
    });
    // A failed or timed-out handshake has already dropped, and reset, its stream.
    let stream = match tokio::time::timeout(handshake, accept).await {
        Ok(Ok(stream)) => stream,
        Ok(Err(error)) => {
            tracing::debug!("relayed TLS handshake failed: {error}");
            return;
        }
        Err(timeout) => {
            tracing::debug!("relayed TLS handshake: {timeout}");
            return;
        }
    };
    accepted.store(true, std::sync::atomic::Ordering::Relaxed);
    if stream.get_ref().1.alpn_protocol() == Some(super::acme::ACME_TLS_ALPN) {
        // A CA validating the challenge certificate (RFC 8737): nothing
        // more to say.
        let mut stream = stream;
        let _ = tokio::io::AsyncWriteExt::shutdown(&mut stream).await;
        return;
    }
    super::serve(stream, app).await;
}

/// A stream the relay opened for one browser connection. A stream's slot is
/// free only once both sides close it, and a relay forwarding a silent browser
/// keeps its side open after a FIN; so a stream dropped before its handshake
/// finished is reset instead, which a relay treats as the end of that browser
/// connection.
struct RelayStream {
    send: quinn::SendStream,
    recv: quinn::RecvStream,
    accepted: Arc<std::sync::atomic::AtomicBool>,
}

impl Drop for RelayStream {
    fn drop(&mut self) {
        if !self.accepted.load(std::sync::atomic::Ordering::Relaxed) {
            let _ = self.send.reset(0u32.into());
            let _ = self.recv.stop(0u32.into());
        }
    }
}

impl tokio::io::AsyncRead for RelayStream {
    fn poll_read(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        std::pin::Pin::new(&mut self.get_mut().recv).poll_read(cx, buf)
    }
}

impl tokio::io::AsyncWrite for RelayStream {
    fn poll_write(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &[u8],
    ) -> std::task::Poll<std::io::Result<usize>> {
        tokio::io::AsyncWrite::poll_write(std::pin::Pin::new(&mut self.get_mut().send), cx, buf)
    }

    fn poll_flush(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        tokio::io::AsyncWrite::poll_flush(std::pin::Pin::new(&mut self.get_mut().send), cx)
    }

    fn poll_shutdown(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        tokio::io::AsyncWrite::poll_shutdown(std::pin::Pin::new(&mut self.get_mut().send), cx)
    }
}

/// Explicit, since other dependencies enable ring too.
pub(super) fn provider() -> Arc<rustls::crypto::CryptoProvider> {
    Arc::new(rustls::crypto::aws_lc_rs::default_provider())
}

/// TLS for browsers, and for an ACME CA validating the machine.
fn server_tls(
    certificates: Arc<super::acme::Certificates>,
) -> Result<tokio_rustls::TlsAcceptor, String> {
    let mut config = rustls::ServerConfig::builder_with_provider(provider())
        .with_safe_default_protocol_versions()
        .map_err(|e| e.to_string())?
        .with_no_client_auth()
        .with_cert_resolver(certificates);
    config.alpn_protocols = vec![b"http/1.1".to_vec(), super::acme::ACME_TLS_ALPN.to_vec()];
    Ok(tokio_rustls::TlsAcceptor::from(Arc::new(config)))
}

fn quic_client(registration: &Registration, streams: u32) -> Result<quinn::ClientConfig, String> {
    let der = base64::engine::general_purpose::STANDARD
        .decode(&registration.relay_ca)
        .map_err(|e| format!("the relay's CA: {e}"))?;
    let mut roots = rustls::RootCertStore::empty();
    roots
        .add(CertificateDer::from(der))
        .map_err(|e| format!("the relay's CA: {e}"))?;
    let mut tls = rustls::ClientConfig::builder_with_provider(provider())
        .with_protocol_versions(&[&rustls::version::TLS13])
        .map_err(|e| e.to_string())?
        .with_root_certificates(roots)
        .with_no_client_auth();
    tls.alpn_protocols = vec![ALPN.to_vec()];
    let crypto =
        quinn::crypto::rustls::QuicClientConfig::try_from(tls).map_err(|e| e.to_string())?;
    let mut config = quinn::ClientConfig::new(Arc::new(crypto));
    let mut transport = quinn::TransportConfig::default();
    // Notices a vanished relay within seconds; idle tunnels cost a keepalive.
    transport.keep_alive_interval(Some(Duration::from_secs(5)));
    transport.max_idle_timeout(Some(Duration::from_secs(15).try_into().unwrap()));
    // The relay opens a stream per browser connection.
    transport.max_concurrent_bidi_streams(streams.into());
    config.transport_config(Arc::new(transport));
    Ok(config)
}

/// `btmux connect-dev`: register with a relay using a self-signed certificate.
#[derive(clap::Args, Clone, Debug)]
pub struct DevArgs {
    /// The relay's tunnel endpoint, `host:port`.
    #[arg(long)]
    relay: String,
    /// The name in the relay's certificate.
    #[arg(long)]
    relay_name: String,
    /// The relay's tunnel CA, PEM or DER (`ca.pem` in its state directory).
    #[arg(long)]
    relay_ca: PathBuf,
    /// The name browsers will use for this machine.
    #[arg(long)]
    hostname: String,
    /// The origin browsers will use; defaults to `https://<hostname>`.
    #[arg(long)]
    public_url: Option<String>,
    /// The relay's credential for this machine.
    #[arg(long, env = "BTMUX_CONNECT_TOKEN", hide_env_values = true)]
    token: String,
    /// Get the machine's certificate from an ACME CA through the relay:
    /// `production` or `staging` (Let's Encrypt), or a directory URL.
    /// Unset, the self-signed certificate stays.
    #[arg(long)]
    acme: Option<String>,
    /// Agree to the ACME CA's terms (Let's Encrypt's Subscriber Agreement,
    /// https://letsencrypt.org/repository/).
    #[arg(long)]
    accept_acme_terms: bool,
}

pub fn dev_register(args: &DevArgs, profile: Option<&str>) -> i32 {
    let result = (|| {
        if args.acme.is_some() && !args.accept_acme_terms {
            return Err(format!(
                "--acme needs --accept-acme-terms, your agreement to the CA's terms \
                 (Let's Encrypt: {})",
                super::acme::LETS_ENCRYPT_TERMS
            ));
        }
        let state_file = crate::persistence::state_path(profile)?
            .ok_or("no state directory: set XDG_STATE_HOME or HOME")?;
        let relay_ca = std::fs::read(&args.relay_ca)
            .map_err(|e| format!("{}: {e}", args.relay_ca.display()))?;
        let relay_ca = if relay_ca.starts_with(b"-----BEGIN") {
            CertificateDer::from_pem_slice(&relay_ca)
                .map_err(|e| format!("{}: {e}", args.relay_ca.display()))?
                .to_vec()
        } else {
            relay_ca
        };
        let cert = rcgen::generate_simple_self_signed(vec![args.hostname.clone()])
            .map_err(|e| e.to_string())?;
        let registration = Registration {
            relay: args.relay.clone(),
            relay_name: args.relay_name.clone(),
            relay_ca: base64::engine::general_purpose::STANDARD.encode(relay_ca),
            hostname: args.hostname.clone(),
            public_url: args
                .public_url
                .clone()
                .unwrap_or_else(|| format!("https://{}", args.hostname)),
            token: args.token.clone(),
            cert: cert.cert.pem(),
            key: cert.signing_key.serialize_pem(),
            acme: args.acme.as_deref().map(|acme| match acme {
                "production" => super::acme::LETS_ENCRYPT.to_owned(),
                "staging" => super::acme::LETS_ENCRYPT_STAGING.to_owned(),
                url => url.to_owned(),
            }),
            issued_to: None,
        };
        if let Some(dir) = state_file.parent() {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        registration.save(&state_file)?;
        Ok::<_, String>(path(&state_file))
    })();
    match result {
        Ok(path) => {
            eprintln!(
                "btmux: registered in {}; restart or `btmux upgrade` the server to connect",
                path.display()
            );
            0
        }
        Err(error) => {
            eprintln!("btmux: {error}");
            1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{routing::get, Extension};
    use rustls::pki_types::PrivateKeyDer;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    fn self_signed(name: &str) -> (String, String, CertificateDer<'static>) {
        let cert = rcgen::generate_simple_self_signed(vec![name.into()]).unwrap();
        let der = cert.cert.der().clone();
        (cert.cert.pem(), cert.signing_key.serialize_pem(), der)
    }

    /// A relay's part, in process: a QUIC endpoint and a registration for it.
    struct TestRelay {
        endpoint: quinn::Endpoint,
        registration: Registration,
        machine: CertificateDer<'static>,
    }

    /// A tunnel CA and a relay certificate it signed, as `btmux-relay init`
    /// makes them: the CA's DER, then the leaf's PEM and key.
    fn relay_identity() -> (CertificateDer<'static>, String, String) {
        let ca_key = rcgen::KeyPair::generate().unwrap();
        let mut params = rcgen::CertificateParams::new(Vec::<String>::new()).unwrap();
        params.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Constrained(0));
        let ca = params.self_signed(&ca_key).unwrap();
        let issuer = rcgen::Issuer::new(params, ca_key);
        let leaf_key = rcgen::KeyPair::generate().unwrap();
        let leaf = rcgen::CertificateParams::new(vec!["relay.test".into()])
            .unwrap()
            .signed_by(&leaf_key, &issuer)
            .unwrap();
        (ca.der().clone(), leaf.pem(), leaf_key.serialize_pem())
    }

    fn test_relay() -> TestRelay {
        let (relay_ca, relay_pem, relay_key) = relay_identity();
        let (machine_pem, machine_key, machine) = self_signed("laptop.test");
        let mut tls = rustls::ServerConfig::builder_with_provider(provider())
            .with_protocol_versions(&[&rustls::version::TLS13])
            .unwrap()
            .with_no_client_auth()
            .with_single_cert(
                CertificateDer::pem_slice_iter(relay_pem.as_bytes())
                    .collect::<Result<Vec<_>, _>>()
                    .unwrap(),
                PrivateKeyDer::from_pem_slice(relay_key.as_bytes()).unwrap(),
            )
            .unwrap();
        // QUIC requires ALPN, so a client without it fails the handshake.
        tls.alpn_protocols = vec![ALPN.to_vec()];
        let endpoint = quinn::Endpoint::server(
            quinn::ServerConfig::with_crypto(Arc::new(
                quinn::crypto::rustls::QuicServerConfig::try_from(tls).unwrap(),
            )),
            "127.0.0.1:0".parse().unwrap(),
        )
        .unwrap();
        let registration = Registration {
            relay: endpoint.local_addr().unwrap().to_string(),
            relay_name: "relay.test".into(),
            relay_ca: base64::engine::general_purpose::STANDARD.encode(&relay_ca),
            hostname: "laptop.test".into(),
            public_url: "https://laptop.test".into(),
            token: "secret-token".into(),
            cert: machine_pem,
            key: machine_key,
            acme: None,
            issued_to: None,
        };
        TestRelay {
            endpoint,
            registration,
            machine,
        }
    }

    fn probe_app() -> Router {
        Router::new().route(
            "/probe",
            get(
                |tunneled: Option<Extension<super::super::Tunneled>>| async move {
                    if tunneled.is_some() {
                        "tunneled"
                    } else {
                        "direct"
                    }
                },
            ),
        )
    }

    /// Accepts the machine's tunnel and its registration.
    async fn accept_tunnel(relay: &quinn::Endpoint) -> quinn::Connection {
        let connection = relay.accept().await.unwrap().await.unwrap();
        let (mut send, recv) = connection.accept_bi().await.unwrap();
        let mut hello = String::new();
        BufReader::new(recv).read_line(&mut hello).await.unwrap();
        let hello: serde_json::Value = serde_json::from_str(&hello).unwrap();
        assert_eq!(hello["hostname"], "laptop.test");
        assert_eq!(hello["token"], "secret-token");
        send.write_all(b"ok\n").await.unwrap();
        connection
    }

    fn browser_config(machine: &CertificateDer<'static>) -> Arc<rustls::ClientConfig> {
        let mut roots = rustls::RootCertStore::empty();
        roots.add(machine.clone()).unwrap();
        Arc::new(
            rustls::ClientConfig::builder_with_provider(provider())
                .with_safe_default_protocol_versions()
                .unwrap()
                .with_root_certificates(roots)
                .with_no_client_auth(),
        )
    }

    /// A browser's request through a stream the relay opens.
    async fn get_through(
        connection: &quinn::Connection,
        machine: &CertificateDer<'static>,
        path: &str,
    ) -> String {
        let (send, recv) = connection.open_bi().await.unwrap();
        let mut stream = tokio_rustls::TlsConnector::from(browser_config(machine))
            .connect(
                "laptop.test".try_into().unwrap(),
                tokio::io::join(recv, send),
            )
            .await
            .unwrap();
        let request =
            format!("GET {path} HTTP/1.1\r\nHost: laptop.test\r\nConnection: close\r\n\r\n");
        stream.write_all(request.as_bytes()).await.unwrap();
        let mut response = String::new();
        stream.read_to_string(&mut response).await.unwrap();
        response
    }

    #[tokio::test]
    async fn serves_relayed_streams_through_the_app() {
        let relay = test_relay();
        spawn(relay.registration.clone(), Path::new("unused"), probe_app()).unwrap();
        let connection = accept_tunnel(&relay.endpoint).await;
        let response = get_through(&connection, &relay.machine, "/probe").await;
        assert!(response.starts_with("HTTP/1.1 200 OK"), "{response}");
        assert!(response.ends_with("tunneled"), "{response}");
        let response = get_through(&connection, &relay.machine, "/ws/browser").await;
        assert!(response.starts_with("HTTP/1.1 403 Forbidden"), "{response}");
    }

    /// A browser that sends its ClientHello, reads the reply, and goes quiet
    /// holds a stream until the handshake deadline, not forever.
    #[tokio::test]
    async fn stalled_handshakes_give_their_streams_back() {
        let relay = test_relay();
        let limits = Limits {
            streams: 2,
            handshake: Duration::from_millis(300),
            ..LIMITS
        };
        spawn_with(
            relay.registration.clone(),
            Path::new("unused"),
            probe_app(),
            limits,
        )
        .unwrap();
        let connection = accept_tunnel(&relay.endpoint).await;
        let mut stalled = Vec::new();
        for _ in 0..limits.streams {
            let (mut send, mut recv) = connection.open_bi().await.unwrap();
            let mut client = rustls::ClientConnection::new(
                browser_config(&relay.machine),
                "laptop.test".try_into().unwrap(),
            )
            .unwrap();
            let mut hello = Vec::new();
            client.write_tls(&mut hello).unwrap();
            send.write_all(&hello).await.unwrap();
            let mut reply = [0; 1];
            recv.read_exact(&mut reply).await.unwrap();
            // Forward as a relay does (`copy_bidirectional`): a FIN only
            // half-closes, waiting on the browser; a reset ends the stream.
            tokio::spawn(async move {
                let mut buf = [0; 4096];
                loop {
                    match recv.read(&mut buf).await {
                        Ok(Some(_)) => {}
                        Ok(None) => std::future::pending::<()>().await,
                        Err(_) => break,
                    }
                }
                drop(send);
            });
            // The browser never leaves.
            stalled.push(client);
        }
        // Every slot is taken...
        let open = tokio::time::timeout(Duration::from_millis(100), connection.open_bi()).await;
        assert!(open.is_err(), "a stream opened past the limit");
        // ...until the deadline frees them, though their browsers never left.
        let response = tokio::time::timeout(
            Duration::from_secs(5),
            get_through(&connection, &relay.machine, "/probe"),
        )
        .await
        .expect("no stream came back after the handshake deadline");
        assert!(response.starts_with("HTTP/1.1 200 OK"), "{response}");
        drop(stalled);
    }

    /// An address that never answers, like IPv6 without a route, doesn't
    /// keep the tunnel from a working one after it.
    #[tokio::test]
    async fn tries_the_next_relay_address() {
        let relay = test_relay();
        let silent = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
        let addresses = [
            silent.local_addr().unwrap(),
            relay.endpoint.local_addr().unwrap(),
        ];
        let endpoint = relay.endpoint.clone();
        tokio::spawn(async move { endpoint.accept().await.unwrap().await.unwrap() });
        let client = quic_client(&relay.registration, 8).unwrap();
        let started = Instant::now();
        let (_endpoint, connection) = connect_any(&addresses, "relay.test", &client, LIMITS)
            .await
            .unwrap();
        assert_eq!(connection.remote_address(), addresses[1]);
        assert!(
            started.elapsed() < LIMITS.connect,
            "{:?}",
            started.elapsed()
        );

        let limits = Limits {
            connect: Duration::from_millis(200),
            ..LIMITS
        };
        let error = connect_any(&addresses[..1], "relay.test", &client, limits)
            .await
            .err()
            .unwrap();
        assert!(error.contains(&addresses[0].to_string()), "{error}");
        assert!(connect_any(&[], "relay.test", &client, limits)
            .await
            .is_err());
    }

    /// Two copies of one registration would replace each other forever, so a
    /// replaced tunnel waits the longest backoff; other closes retry soon.
    #[tokio::test]
    async fn a_replaced_tunnel_backs_off() {
        let relay = test_relay();
        spawn(relay.registration.clone(), Path::new("unused"), probe_app()).unwrap();
        let connection = accept_tunnel(&relay.endpoint).await;
        connection.close(0u32.into(), b"restarting");
        let again = tokio::time::timeout(Duration::from_secs(3), accept_tunnel(&relay.endpoint));
        let connection = again.await.expect("didn't reconnect after a restart");
        connection.close(REPLACED.into(), b"replaced");
        let again = tokio::time::timeout(Duration::from_secs(3), relay.endpoint.accept());
        assert!(
            again.await.is_err(),
            "reconnected right after being replaced"
        );
    }

    /// A CA validating TLS-ALPN-01 through the relay gets the challenge
    /// certificate, and its connection never reaches the app.
    #[tokio::test]
    async fn answers_acme_challenges_through_the_relay() {
        use super::super::acme::{tests::AcceptAny, ACME_TLS_ALPN};
        let relay = test_relay();
        let certificates = spawn_with(
            relay.registration.clone(),
            Path::new("unused"),
            probe_app(),
            LIMITS,
        )
        .unwrap();
        let connection = accept_tunnel(&relay.endpoint).await;
        let challenge = super::super::acme::tests::challenge("laptop.test");
        certificates.set_challenge(Some(challenge.clone()));

        let mut config = rustls::ClientConfig::builder_with_provider(provider())
            .with_safe_default_protocol_versions()
            .unwrap()
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(AcceptAny))
            .with_no_client_auth();
        config.alpn_protocols = vec![ACME_TLS_ALPN.to_vec()];
        let (send, recv) = connection.open_bi().await.unwrap();
        let mut stream = tokio_rustls::TlsConnector::from(Arc::new(config))
            .connect(
                "laptop.test".try_into().unwrap(),
                tokio::io::join(recv, send),
            )
            .await
            .unwrap();
        let (_, session) = stream.get_ref();
        assert_eq!(session.alpn_protocol(), Some(ACME_TLS_ALPN));
        assert_eq!(session.peer_certificates().unwrap()[0], challenge.cert[0]);
        // An HTTP request gets no answer: the machine just closes.
        let _ = stream
            .write_all(b"GET /probe HTTP/1.1\r\nHost: laptop.test\r\n\r\n")
            .await;
        let mut response = Vec::new();
        let _ = stream.read_to_end(&mut response).await;
        assert!(
            response.is_empty(),
            "{}",
            String::from_utf8_lossy(&response)
        );

        // Browsers still get the machine's certificate.
        let response = get_through(&connection, &relay.machine, "/probe").await;
        assert!(response.ends_with("tunneled"), "{response}");
    }

    #[test]
    fn registration_is_private_and_round_trips() {
        let dir = std::env::temp_dir().join(format!("btmux-connect-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let state = dir.join("state.json");
        assert!(Registration::load(&state).unwrap().is_none());
        let (cert, key, _) = self_signed("laptop.test");
        let registration = Registration {
            relay: "relay.test:4433".into(),
            relay_name: "relay.test".into(),
            relay_ca: String::new(),
            hostname: "laptop.test".into(),
            public_url: "https://laptop.test".into(),
            token: "t".into(),
            cert,
            key,
            acme: Some(super::super::acme::LETS_ENCRYPT_STAGING.into()),
            issued_to: None,
        };
        registration.save(&state).unwrap();
        registration.save(&state).unwrap();
        let loaded = Registration::load(&state).unwrap().unwrap();
        assert_eq!(loaded.hostname, "laptop.test");
        assert_eq!(
            loaded.acme.as_deref(),
            Some(super::super::acme::LETS_ENCRYPT_STAGING)
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(path(&state))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        std::fs::remove_dir_all(dir).unwrap();
    }
}
