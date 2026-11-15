//! Sign-up: an invite code from the btmux connect service becomes this
//! machine's registration, with a hostname under the relay's domain and a
//! certificate from Let's Encrypt. `btmux connect` and Settings both reach it
//! through `POST /api/connect/register`, which starts connect at once.

use std::path::Path;
use std::time::Duration;

use base64::Engine;
use serde::Deserialize;

use super::tunnel::Registration;

/// The sign-up API, served by the relay itself.
pub const SERVICE: &str = "https://connect.btmux.net";

/// What the service answers, as the relay's `api::Provisioned` sends it.
#[derive(Deserialize)]
struct Provisioned {
    hostname: String,
    token: String,
    relay: String,
    relay_name: String,
    relay_ca: String,
    public_url: String,
}

/// A refused sign-up, with the status to answer it with.
#[derive(Debug)]
pub struct Failure {
    pub status: axum::http::StatusCode,
    pub message: String,
}

impl Failure {
    fn new(status: axum::http::StatusCode, message: impl Into<String>) -> Self {
        Self {
            status,
            message: message.into(),
        }
    }
}

/// The service to sign up with: `BTMUX_CONNECT_SERVICE` overrides it for
/// development, plain HTTP only on loopback.
pub fn service() -> Result<String, Failure> {
    let service = std::env::var("BTMUX_CONNECT_SERVICE").unwrap_or_else(|_| SERVICE.into());
    let url = reqwest::Url::parse(&service).map_err(|e| {
        Failure::new(
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            format!("BTMUX_CONNECT_SERVICE: {e}"),
        )
    })?;
    let loopback = matches!(url.host_str(), Some("127.0.0.1" | "localhost" | "[::1]"));
    if url.scheme() != "https" && !(url.scheme() == "http" && loopback) {
        return Err(Failure::new(
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            "BTMUX_CONNECT_SERVICE must be https (http only on loopback)",
        ));
    }
    Ok(service.trim_end_matches('/').to_owned())
}

/// The CA that issues certificates: Let's Encrypt, unless
/// `BTMUX_CONNECT_ACME` says `staging`, a directory URL, or `none` (keep the
/// self-signed certificate, for tests).
fn acme() -> Option<String> {
    match std::env::var("BTMUX_CONNECT_ACME").as_deref() {
        Err(_) => Some(super::acme::LETS_ENCRYPT.into()),
        Ok("none") => None,
        Ok("staging") => Some(super::acme::LETS_ENCRYPT_STAGING.into()),
        Ok(url) => Some(url.into()),
    }
}

fn client() -> Result<reqwest::Client, Failure> {
    let internal = |e: String| Failure::new(axum::http::StatusCode::INTERNAL_SERVER_ERROR, e);
    let mut builder = reqwest::Client::builder().timeout(Duration::from_secs(20));
    // A development service's own CA (PEM).
    if let Ok(path) = std::env::var("BTMUX_CONNECT_SERVICE_CA") {
        let pem = std::fs::read(&path).map_err(|e| internal(format!("{path}: {e}")))?;
        let ca =
            reqwest::Certificate::from_pem(&pem).map_err(|e| internal(format!("{path}: {e}")))?;
        builder = builder.add_root_certificate(ca);
    }
    builder.build().map_err(|e| internal(e.to_string()))
}

/// Redeems `invite` with `service` and saves the registration it yields. The
/// caller has the owner's agreement to the ACME CA's terms.
pub async fn register(
    service: &str,
    invite: &str,
    state_file: &Path,
) -> Result<Registration, Failure> {
    use axum::http::StatusCode;
    let invite = invite.trim();
    if invite.is_empty() {
        return Err(Failure::new(
            StatusCode::BAD_REQUEST,
            "Enter an invite code.",
        ));
    }
    let response = client()?
        .post(format!("{service}/v1/machines"))
        .header("content-type", "application/json")
        .body(serde_json::json!({ "invite": invite }).to_string())
        .send()
        .await
        .map_err(|e| {
            Failure::new(
                StatusCode::BAD_GATEWAY,
                format!("Couldn't reach the btmux connect service: {e}"),
            )
        })?;
    let status = response.status();
    let body = response.bytes().await.map_err(|e| {
        Failure::new(
            StatusCode::BAD_GATEWAY,
            format!("The btmux connect service's answer: {e}"),
        )
    })?;
    if !status.is_success() {
        let message = serde_json::from_slice::<serde_json::Value>(&body)
            .ok()
            .and_then(|body| body["error"].as_str().map(str::to_owned))
            .unwrap_or_else(|| format!("The btmux connect service answered {status}."));
        // Its refusals are the client's to fix; anything else, the service's.
        let status = match status.as_u16() {
            400 | 403 | 429 => StatusCode::from_u16(status.as_u16()).unwrap(),
            _ => StatusCode::BAD_GATEWAY,
        };
        return Err(Failure::new(status, message));
    }
    let bad = |e: String| {
        Failure::new(
            StatusCode::BAD_GATEWAY,
            format!("The btmux connect service's answer: {e}"),
        )
    };
    let provisioned: Provisioned = serde_json::from_slice(&body).map_err(|e| bad(e.to_string()))?;
    base64::engine::general_purpose::STANDARD
        .decode(&provisioned.relay_ca)
        .map_err(|e| bad(format!("relay_ca: {e}")))?;
    let cert = rcgen::generate_simple_self_signed(vec![provisioned.hostname.clone()])
        .map_err(|e| bad(e.to_string()))?;
    // Self-signed until the CA issues one.
    let registration = Registration {
        relay: provisioned.relay,
        relay_name: provisioned.relay_name,
        relay_ca: provisioned.relay_ca,
        hostname: provisioned.hostname,
        public_url: provisioned.public_url,
        token: provisioned.token,
        cert: cert.cert.pem(),
        key: cert.signing_key.serialize_pem(),
        acme: acme(),
        issued_to: None,
    };
    let internal = |e: String| Failure::new(StatusCode::INTERNAL_SERVER_ERROR, e);
    if let Some(dir) = state_file.parent() {
        std::fs::create_dir_all(dir).map_err(|e| internal(e.to_string()))?;
    }
    registration.save(state_file).map_err(internal)?;
    Ok(registration)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{routing::post, Json, Router};

    /// A sign-up service in process: one invite, `good`.
    async fn service() -> String {
        let app = Router::new().route(
            "/v1/machines",
            post(|Json(body): Json<serde_json::Value>| async move {
                if body["invite"] == "good" {
                    (
                        axum::http::StatusCode::CREATED,
                        Json(serde_json::json!({
                            "hostname": "abcd1234.btmux.test",
                            "token": "btmr_token",
                            "relay": "relay.btmux.test:443",
                            "relay_name": "relay.btmux.test",
                            "relay_ca": base64::engine::general_purpose::STANDARD.encode(b"ca"),
                            "public_url": "https://abcd1234.btmux.test",
                        })),
                    )
                } else {
                    (
                        axum::http::StatusCode::FORBIDDEN,
                        Json(serde_json::json!({ "error": "This invite code isn't valid." })),
                    )
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await });
        format!("http://{addr}")
    }

    #[tokio::test]
    async fn an_invite_becomes_a_registration() {
        let service = service().await;
        let dir = std::env::temp_dir().join(format!("btmux-signup-{}", uuid::Uuid::new_v4()));
        let state = dir.join("state.json");

        let refused = register(&service, "bad", &state).await.err().unwrap();
        assert_eq!(refused.status, axum::http::StatusCode::FORBIDDEN);
        assert_eq!(refused.message, "This invite code isn't valid.");
        assert!(Registration::load(&state).unwrap().is_none());
        let empty = register(&service, "  ", &state).await.err().unwrap();
        assert_eq!(empty.status, axum::http::StatusCode::BAD_REQUEST);

        let registration = register(&service, " good\n", &state).await.unwrap();
        let saved = Registration::load(&state).unwrap().unwrap();
        assert_eq!(saved.hostname, "abcd1234.btmux.test");
        assert_eq!(saved.token, "btmr_token");
        assert_eq!(saved.relay, "relay.btmux.test:443");
        assert_eq!(saved.public_url, registration.public_url);
        // A placeholder until Let's Encrypt issues one.
        assert!(super::super::acme::certified_key(&saved.cert, &saved.key).is_ok());
        std::fs::remove_dir_all(dir).unwrap();
    }
}
