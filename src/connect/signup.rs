//! Sign-up: an invite code from the btmux connect service becomes this
//! machine's registration, with a hostname under the relay's domain and a
//! certificate from Let's Encrypt. `btmux connect` and Settings both reach it
//! through `POST /api/connect/register`, which starts connect at once.

use std::path::Path;
use std::time::Duration;

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

/// `value` from `variable` as a URL: https, or http on loopback.
fn checked_url(variable: &str, value: &str) -> Result<String, Failure> {
    let invalid = |e: String| {
        Failure::new(
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            format!("{variable}: {e}"),
        )
    };
    let url = reqwest::Url::parse(value).map_err(|e| invalid(e.to_string()))?;
    let loopback = matches!(url.host_str(), Some("127.0.0.1" | "localhost" | "[::1]"));
    if url.scheme() != "https" && !(url.scheme() == "http" && loopback) {
        return Err(invalid(
            "must be an https URL (http only on loopback)".into(),
        ));
    }
    Ok(value.trim_end_matches('/').to_owned())
}

/// The service to sign up with: `BTMUX_CONNECT_SERVICE` overrides it for
/// development.
pub fn service() -> Result<String, Failure> {
    match std::env::var("BTMUX_CONNECT_SERVICE") {
        Ok(service) => checked_url("BTMUX_CONNECT_SERVICE", &service),
        Err(_) => Ok(SERVICE.into()),
    }
}

/// The CA that issues certificates: Let's Encrypt, unless
/// `BTMUX_CONNECT_ACME` says `staging`, a directory URL, or `none` (keep the
/// self-signed certificate, for tests).
fn acme() -> Result<Option<String>, Failure> {
    Ok(match std::env::var("BTMUX_CONNECT_ACME").as_deref() {
        Err(_) => Some(super::acme::LETS_ENCRYPT.into()),
        Ok("none") => None,
        Ok("staging") => Some(super::acme::LETS_ENCRYPT_STAGING.into()),
        Ok(url) => Some(checked_url("BTMUX_CONNECT_ACME", url)?),
    })
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

/// Tries after the first, for an answer lost on the way: the service gives
/// a repeated request the same machine.
const RETRIES: [Duration; 2] = [Duration::from_secs(1), Duration::from_secs(5)];

/// One try at `POST /v1/machines`; `Err(true)` may succeed if repeated.
async fn post(
    client: &reqwest::Client,
    service: &str,
    body: &str,
) -> Result<Provisioned, (bool, Failure)> {
    use axum::http::StatusCode;
    let bad_gateway = |message: String| Failure::new(StatusCode::BAD_GATEWAY, message);
    let response = client
        .post(format!("{service}/v1/machines"))
        .header("content-type", "application/json")
        .body(body.to_owned())
        .send()
        .await
        .map_err(|e| {
            let message = format!("Couldn't reach the btmux connect service: {e}");
            (true, bad_gateway(message))
        })?;
    let status = response.status();
    let body = response.bytes().await.map_err(|e| {
        let message = format!("The btmux connect service's answer: {e}");
        (true, bad_gateway(message))
    })?;
    if !status.is_success() {
        let message = serde_json::from_slice::<serde_json::Value>(&body)
            .ok()
            .and_then(|body| body["error"].as_str().map(str::to_owned))
            .unwrap_or_else(|| format!("The btmux connect service answered {status}."));
        // Its refusals are the client's to fix; anything else, the service's.
        return Err(match status.as_u16() {
            400 | 403 | 429 => (
                false,
                Failure::new(StatusCode::from_u16(status.as_u16()).unwrap(), message),
            ),
            _ => (status.is_server_error(), bad_gateway(message)),
        });
    }
    serde_json::from_slice(&body).map_err(|e| {
        let message = format!("The btmux connect service's answer: {e}");
        (false, bad_gateway(message))
    })
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
    let acme = acme()?;
    let client = client()?;
    // Repeated with each try, so the service knows them for one.
    let request = uuid::Uuid::new_v4().simple().to_string();
    let body = serde_json::json!({ "invite": invite, "request": request }).to_string();
    let mut retries = RETRIES.iter();
    let provisioned = loop {
        match post(&client, service, &body).await {
            Ok(provisioned) => break provisioned,
            Err((true, failure)) => match retries.next() {
                Some(wait) => {
                    tracing::warn!("btmux connect: sign-up: {}; retrying", failure.message);
                    tokio::time::sleep(*wait).await;
                }
                None => return Err(failure),
            },
            Err((false, failure)) => return Err(failure),
        }
    };
    let bad = |e: String| {
        Failure::new(
            StatusCode::BAD_GATEWAY,
            format!("The btmux connect service's answer: {e}"),
        )
    };
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
        acme,
        issued_to: None,
    };
    // Saved only if it can start: a registration that can't would block
    // signing up again.
    registration.check().map_err(bad)?;
    let internal = |e: String| Failure::new(StatusCode::INTERNAL_SERVER_ERROR, e);
    if let Some(dir) = state_file.parent() {
        std::fs::create_dir_all(dir).map_err(|e| internal(e.to_string()))?;
    }
    registration.save(state_file).map_err(internal)?;
    Ok(registration)
}

/// What a relay answers, for tests.
#[cfg(test)]
pub(crate) fn test_answer(hostname: &str) -> serde_json::Value {
    use base64::Engine;
    let ca = rcgen::generate_simple_self_signed(vec!["relay.btmux.test".into()]).unwrap();
    serde_json::json!({
        "hostname": hostname,
        "token": "btmr_token",
        "relay": "127.0.0.1:9",
        "relay_name": "relay.btmux.test",
        "relay_ca": base64::engine::general_purpose::STANDARD.encode(ca.cert.der()),
        "public_url": format!("https://{hostname}"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{http::StatusCode, routing::post, Json, Router};
    use std::sync::{Arc, Mutex};

    /// A sign-up service in process: one invite, `good`, whose first answer
    /// is lost; `bad answer` gets an answer that can't start. Records each
    /// request's ID.
    async fn service() -> (String, Arc<Mutex<Vec<String>>>) {
        let requests = Arc::new(Mutex::new(Vec::new()));
        let seen = requests.clone();
        let app = Router::new().route(
            "/v1/machines",
            post(move |Json(body): Json<serde_json::Value>| {
                let seen = seen.clone();
                async move {
                    let tries = {
                        let mut seen = seen.lock().unwrap();
                        seen.push(body["request"].as_str().unwrap_or_default().to_owned());
                        seen.len()
                    };
                    match body["invite"].as_str() {
                        Some("good") if tries == 1 => (
                            StatusCode::SERVICE_UNAVAILABLE,
                            Json(serde_json::json!({ "error": "Lost." })),
                        ),
                        Some("good") => (
                            StatusCode::CREATED,
                            Json(test_answer("abcd1234.btmux.test")),
                        ),
                        Some("bad answer") => {
                            let mut answer = test_answer("abcd1234.btmux.test");
                            answer["relay_ca"] = "Y2E=".into();
                            (StatusCode::CREATED, Json(answer))
                        }
                        _ => (
                            StatusCode::FORBIDDEN,
                            Json(serde_json::json!({ "error": "This invite code isn't valid." })),
                        ),
                    }
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await });
        (format!("http://{addr}"), requests)
    }

    #[test]
    fn urls_are_https_but_on_loopback() {
        assert!(checked_url("V", "https://ca.example/dir").is_ok());
        assert!(checked_url("V", "http://127.0.0.1:14000/dir").is_ok());
        assert!(checked_url("V", "http://ca.example/dir").is_err());
        assert!(checked_url("V", "stagign").is_err());
    }

    #[tokio::test]
    async fn an_invite_becomes_a_registration() {
        let (service, requests) = service().await;
        let dir = std::env::temp_dir().join(format!("btmux-signup-{}", uuid::Uuid::new_v4()));
        let state = dir.join("state.json");

        let refused = register(&service, "bad", &state).await.err().unwrap();
        assert_eq!(refused.status, StatusCode::FORBIDDEN);
        assert_eq!(refused.message, "This invite code isn't valid.");
        assert!(Registration::load(&state).unwrap().is_none());
        let empty = register(&service, "  ", &state).await.err().unwrap();
        assert_eq!(empty.status, StatusCode::BAD_REQUEST);
        // Not saved: it would refuse the next sign-up, and never start.
        let unusable = register(&service, "bad answer", &state)
            .await
            .err()
            .unwrap();
        assert_eq!(unusable.status, StatusCode::BAD_GATEWAY);
        assert!(Registration::load(&state).unwrap().is_none());

        requests.lock().unwrap().clear();
        let registration = register(&service, " good\n", &state).await.unwrap();
        // The lost answer was retried as the same request.
        let tries = requests.lock().unwrap().clone();
        assert_eq!(tries.len(), 2);
        assert_eq!(tries[0], tries[1]);
        assert!(tries[0].len() >= 16);
        let saved = Registration::load(&state).unwrap().unwrap();
        assert_eq!(saved.hostname, "abcd1234.btmux.test");
        assert_eq!(saved.token, "btmr_token");
        assert_eq!(saved.relay, "127.0.0.1:9");
        assert_eq!(saved.public_url, registration.public_url);
        // A placeholder until Let's Encrypt issues one.
        assert!(super::super::acme::certified_key(&saved.cert, &saved.key).is_ok());
        std::fs::remove_dir_all(dir).unwrap();
    }
}
