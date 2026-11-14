//! The tunnel client: one outbound QUIC connection to a btmux connect relay,
//! which opens a stream per browser connection. Each stream carries the
//! browser's own TLS session, terminated here with this machine's certificate,
//! so the relay never sees plaintext. Streams serve the main listener's app,
//! marked `Tunneled`.
//!
//! Wire protocol, shared with the relay: the client opens a bidirectional
//! stream and writes one JSON line, `{"hostname":…,"token":…}`, and the relay
//! answers `ok`. From then on the relay opens one stream per browser
//! connection, starting with its ClientHello. A relay closes a hostname's old
//! tunnel when it registers again, as after an upgrade's exec.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::Router;
use base64::Engine;
use rustls::pki_types::{pem::PemObject, CertificateDer, PrivateKeyDer};
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
    /// The relay's certificate (DER, base64), trusted for the tunnel only.
    pub relay_cert: String,
    /// The name browsers reach this machine at.
    pub hostname: String,
    /// The origin browsers use, allowed like `--public-url`.
    pub public_url: String,
    /// The relay's credential for this machine.
    pub token: String,
    /// This machine's certificate chain and key for `hostname` (PEM).
    pub cert: String,
    pub key: String,
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
        use std::io::Write;
        let path = path(state_file);
        let temp = path.with_extension("connect.tmp");
        let _ = std::fs::remove_file(&temp);
        let json = serde_json::to_vec_pretty(self).map_err(|e| e.to_string())?;
        let mut file = crate::discovery::create_private_file(&temp)
            .map_err(|e| format!("{}: {e}", temp.display()))?;
        file.write_all(&json).map_err(|e| e.to_string())?;
        drop(file);
        std::fs::rename(&temp, &path).map_err(|e| format!("{}: {e}", path.display()))
    }
}

/// Connects to the relay in the background and keeps reconnecting.
pub fn spawn(registration: Registration, app: Router) -> Result<(), String> {
    let tls = server_tls(&registration)?;
    let client = quic_client(&registration)?;
    tokio::spawn(run(registration, client, tls, super::app(app)));
    Ok(())
}

const MIN_BACKOFF: Duration = Duration::from_millis(500);
const MAX_BACKOFF: Duration = Duration::from_secs(30);

async fn run(
    registration: Registration,
    client: quinn::ClientConfig,
    tls: tokio_rustls::TlsAcceptor,
    app: Router,
) {
    let mut backoff = MIN_BACKOFF;
    loop {
        let started = Instant::now();
        let error = connect(&registration, &client, &tls, &app).await;
        tracing::warn!(relay = registration.relay, "btmux connect tunnel: {error}");
        if started.elapsed() > MAX_BACKOFF {
            backoff = MIN_BACKOFF;
        }
        tokio::time::sleep(backoff).await;
        backoff = (backoff * 2).min(MAX_BACKOFF);
    }
}

/// Serves one tunnel until it fails.
async fn connect(
    registration: &Registration,
    client: &quinn::ClientConfig,
    tls: &tokio_rustls::TlsAcceptor,
    app: &Router,
) -> String {
    let result = async {
        let address = tokio::net::lookup_host(&registration.relay)
            .await
            .map_err(|e| e.to_string())?
            .next()
            .ok_or("the relay's name has no address")?;
        let bind = if address.is_ipv6() {
            "[::]:0"
        } else {
            "0.0.0.0:0"
        };
        let mut endpoint =
            quinn::Endpoint::client(bind.parse().unwrap()).map_err(|e| e.to_string())?;
        endpoint.set_default_client_config(client.clone());
        let connection = endpoint
            .connect(address, &registration.relay_name)
            .map_err(|e| e.to_string())?
            .await
            .map_err(|e| e.to_string())?;
        register(&connection, registration).await?;
        tracing::info!(
            hostname = registration.hostname,
            relay = registration.relay,
            "btmux connect tunnel up"
        );
        loop {
            let (send, recv) = connection.accept_bi().await.map_err(|e| e.to_string())?;
            tokio::spawn(serve(send, recv, tls.clone(), app.clone()));
        }
    }
    .await;
    match result {
        Ok(never) => never,
        Err(error) => error,
    }
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
) {
    let stream = match tls.accept(tokio::io::join(recv, send)).await {
        Ok(stream) => stream,
        Err(error) => {
            tracing::debug!("relayed TLS handshake failed: {error}");
            return;
        }
    };
    let service = hyper_util::service::TowerToHyperService::new(app);
    if let Err(error) = hyper::server::conn::http1::Builder::new()
        .timer(hyper_util::rt::TokioTimer::new())
        .header_read_timeout(Duration::from_secs(30))
        .serve_connection(hyper_util::rt::TokioIo::new(stream), service)
        .with_upgrades()
        .await
    {
        tracing::debug!("relayed connection ended: {error}");
    }
}

/// Explicit, since other dependencies enable ring too.
fn provider() -> Arc<rustls::crypto::CryptoProvider> {
    Arc::new(rustls::crypto::aws_lc_rs::default_provider())
}

fn server_tls(registration: &Registration) -> Result<tokio_rustls::TlsAcceptor, String> {
    let certs = CertificateDer::pem_slice_iter(registration.cert.as_bytes())
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("the registration's certificate: {e}"))?;
    let key = PrivateKeyDer::from_pem_slice(registration.key.as_bytes())
        .map_err(|e| format!("the registration's key: {e}"))?;
    let mut config = rustls::ServerConfig::builder_with_provider(provider())
        .with_safe_default_protocol_versions()
        .map_err(|e| e.to_string())?
        .with_no_client_auth()
        .with_single_cert(certs, key)
        .map_err(|e| format!("the registration's certificate: {e}"))?;
    config.alpn_protocols = vec![b"http/1.1".to_vec()];
    Ok(tokio_rustls::TlsAcceptor::from(Arc::new(config)))
}

fn quic_client(registration: &Registration) -> Result<quinn::ClientConfig, String> {
    let der = base64::engine::general_purpose::STANDARD
        .decode(&registration.relay_cert)
        .map_err(|e| format!("the relay's certificate: {e}"))?;
    let mut roots = rustls::RootCertStore::empty();
    roots
        .add(CertificateDer::from(der))
        .map_err(|e| format!("the relay's certificate: {e}"))?;
    let tls = rustls::ClientConfig::builder_with_provider(provider())
        .with_protocol_versions(&[&rustls::version::TLS13])
        .map_err(|e| e.to_string())?
        .with_root_certificates(roots)
        .with_no_client_auth();
    let crypto =
        quinn::crypto::rustls::QuicClientConfig::try_from(tls).map_err(|e| e.to_string())?;
    let mut config = quinn::ClientConfig::new(Arc::new(crypto));
    let mut transport = quinn::TransportConfig::default();
    // Notices a vanished relay within seconds; idle tunnels cost a keepalive.
    transport.keep_alive_interval(Some(Duration::from_secs(5)));
    transport.max_idle_timeout(Some(Duration::from_secs(15).try_into().unwrap()));
    // The relay opens a stream per browser connection.
    transport.max_concurrent_bidi_streams(1024u32.into());
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
    /// The relay's certificate, DER.
    #[arg(long)]
    relay_cert: PathBuf,
    /// The name browsers will use for this machine.
    #[arg(long)]
    hostname: String,
    /// The origin browsers will use; defaults to `https://<hostname>`.
    #[arg(long)]
    public_url: Option<String>,
    /// The relay's credential for this machine.
    #[arg(long, env = "BTMUX_CONNECT_TOKEN", hide_env_values = true)]
    token: String,
}

pub fn dev_register(args: &DevArgs, profile: Option<&str>) -> i32 {
    let result = (|| {
        let state_file = crate::persistence::state_path(profile)?
            .ok_or("no state directory: set XDG_STATE_HOME or HOME")?;
        let relay_cert = std::fs::read(&args.relay_cert)
            .map_err(|e| format!("{}: {e}", args.relay_cert.display()))?;
        let cert = rcgen::generate_simple_self_signed(vec![args.hostname.clone()])
            .map_err(|e| e.to_string())?;
        let registration = Registration {
            relay: args.relay.clone(),
            relay_name: args.relay_name.clone(),
            relay_cert: base64::engine::general_purpose::STANDARD.encode(relay_cert),
            hostname: args.hostname.clone(),
            public_url: args
                .public_url
                .clone()
                .unwrap_or_else(|| format!("https://{}", args.hostname)),
            token: args.token.clone(),
            cert: cert.cert.pem(),
            key: cert.signing_key.serialize_pem(),
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
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    fn self_signed(name: &str) -> (String, String, CertificateDer<'static>) {
        let cert = rcgen::generate_simple_self_signed(vec![name.into()]).unwrap();
        let der = cert.cert.der().clone();
        (cert.cert.pem(), cert.signing_key.serialize_pem(), der)
    }

    /// A relay's part, in process: accept the registration, then open a
    /// stream and speak TLS and HTTP through it as a browser would.
    #[tokio::test]
    async fn serves_relayed_streams_through_the_app() {
        let (relay_pem, relay_key, relay_der) = self_signed("relay.test");
        let (machine_pem, machine_key, machine_der) = self_signed("laptop.test");

        let relay_tls = rustls::ServerConfig::builder_with_provider(provider())
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
        let relay = quinn::Endpoint::server(
            quinn::ServerConfig::with_crypto(Arc::new(
                quinn::crypto::rustls::QuicServerConfig::try_from(relay_tls).unwrap(),
            )),
            "127.0.0.1:0".parse().unwrap(),
        )
        .unwrap();

        let app = Router::new().route(
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
        );
        spawn(
            Registration {
                relay: relay.local_addr().unwrap().to_string(),
                relay_name: "relay.test".into(),
                relay_cert: base64::engine::general_purpose::STANDARD.encode(&relay_der),
                hostname: "laptop.test".into(),
                public_url: "https://laptop.test".into(),
                token: "secret-token".into(),
                cert: machine_pem,
                key: machine_key,
            },
            app,
        )
        .unwrap();

        let connection = relay.accept().await.unwrap().await.unwrap();
        let (mut send, recv) = connection.accept_bi().await.unwrap();
        let mut hello = String::new();
        BufReader::new(recv).read_line(&mut hello).await.unwrap();
        let hello: serde_json::Value = serde_json::from_str(&hello).unwrap();
        assert_eq!(hello["hostname"], "laptop.test");
        assert_eq!(hello["token"], "secret-token");
        send.write_all(b"ok\n").await.unwrap();

        let mut roots = rustls::RootCertStore::empty();
        roots.add(machine_der).unwrap();
        let browser = tokio_rustls::TlsConnector::from(Arc::new(
            rustls::ClientConfig::builder_with_provider(provider())
                .with_safe_default_protocol_versions()
                .unwrap()
                .with_root_certificates(roots)
                .with_no_client_auth(),
        ));
        for (path, expected) in [
            ("/probe", "HTTP/1.1 200 OK"),
            ("/ws/browser", "HTTP/1.1 403 Forbidden"),
        ] {
            let (send, recv) = connection.open_bi().await.unwrap();
            let mut stream = browser
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
            assert!(response.starts_with(expected), "{path}: {response}");
            if path == "/probe" {
                assert!(response.ends_with("tunneled"), "{response}");
            }
        }
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
            relay_cert: String::new(),
            hostname: "laptop.test".into(),
            public_url: "https://laptop.test".into(),
            token: "t".into(),
            cert,
            key,
        };
        registration.save(&state).unwrap();
        registration.save(&state).unwrap();
        let loaded = Registration::load(&state).unwrap().unwrap();
        assert_eq!(loaded.hostname, "laptop.test");
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
