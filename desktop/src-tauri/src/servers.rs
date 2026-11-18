use std::collections::HashSet;
use std::path::Path;
use std::time::{Duration, Instant};

use reqwest::blocking::Client;
use serde::Deserialize;

use crate::discovery::{self, RunningServer};
use crate::upgrade_status::{Progress, is_newer, progress};

#[derive(Deserialize)]
struct ServerInfo {
    version: String,
    profile: Option<String>,
    // Parsed separately, so an unknown status doesn't hide the server.
    #[serde(default)]
    tailscale: Option<serde_json::Value>,
}

impl ServerInfo {
    fn tailnet(self) -> Option<Tailnet> {
        serde_json::from_value(self.tailscale?).ok()
    }
}

fn client() -> Result<Client, reqwest::Error> {
    client_with(Duration::from_millis(500))
}

fn client_with(timeout: Duration) -> Result<Client, reqwest::Error> {
    Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(timeout)
        .build()
}

fn probe(client: &Client, server: &RunningServer) -> Option<ServerInfo> {
    let response = client
        .get(format!("http://{}/api/info", server.address))
        .bearer_auth(&server.token)
        .send()
        .ok()?;
    if !response.status().is_success() {
        return None;
    }
    response
        .json::<ServerInfo>()
        .ok()
        .filter(|info| !info.version.is_empty())
}

/// What `--tailscale` reports in `/api/info`.
#[derive(Deserialize, serde::Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum Tailnet {
    Starting,
    Serving { url: String },
    Error { message: String },
}

pub fn ready(server: &RunningServer) -> bool {
    check(server.clone()).is_some()
}

pub fn shutdown(server: &RunningServer) -> Result<(), String> {
    let response = client()
        .and_then(|client| {
            client
                .post(format!("http://{}/api/server/shutdown", server.address))
                .bearer_auth(&server.token)
                .send()
        })
        .map_err(|_| "Could not stop this server.".to_string())?;
    match response.status() {
        status if status.is_success() => Ok(()),
        reqwest::StatusCode::NOT_FOUND | reqwest::StatusCode::METHOD_NOT_ALLOWED => {
            Err("This server's version can't be stopped from the app.".into())
        }
        _ => Err("Could not stop this server.".into()),
    }
}

/// Moves `server` to `exe`, release `target`, in place, as `btmux upgrade`
/// does; does nothing if it already runs `target` or later.
pub fn upgrade(server: &RunningServer, exe: &Path, target: &str) -> Result<(), String> {
    // The server first runs `exe` to check it, for up to 10 s.
    let client = client_with(Duration::from_secs(15))
        .map_err(|_| "Could not upgrade the desktop server.".to_string())?;
    let info = |timeout: Duration| -> Option<serde_json::Value> {
        client
            .get(format!("http://{}/api/info", server.address))
            .bearer_auth(&server.token)
            .timeout(timeout)
            .send()
            .and_then(|response| response.error_for_status())
            .ok()?
            .json()
            .ok()
    };
    let before =
        info(Duration::from_secs(15)).ok_or("The desktop server is no longer available.")?;
    // Another window may have upgraded it since the picker listed it. Even a
    // same-version handoff restarts its editors and browser.
    if !is_newer(target, before["version"].as_str().unwrap_or_default()) {
        return Ok(());
    }
    let response = client
        .post(format!("http://{}/api/server/upgrade", server.address))
        .bearer_auth(&server.token)
        .json(&serde_json::json!({ "executable": exe }))
        .send()
        .map_err(|_| "Could not upgrade the desktop server.".to_string())?;
    let failed = |reason: &str| {
        let reason = reason.trim().trim_end_matches('.');
        if reason.is_empty() {
            "Could not upgrade the desktop server.".to_string()
        } else {
            format!("Could not upgrade the desktop server: {reason}.")
        }
    };
    if !response.status().is_success() {
        return Err(failed(&response.text().unwrap_or_default()));
    }
    let deadline = Instant::now() + Duration::from_secs(30);
    while Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(250));
        let remaining = deadline.saturating_duration_since(Instant::now());
        let Some(info) = info(remaining.max(Duration::from_millis(100))) else {
            continue;
        };
        match progress(&before, &info) {
            Progress::Pending => {}
            Progress::Upgraded => return Ok(()),
            Progress::Restarted => {
                return Err(
                    "The desktop server restarted instead of upgrading, closing its shells.".into(),
                );
            }
            Progress::Failed(error) => return Err(failed(&error)),
        }
    }
    Err("Timed out waiting for the upgraded desktop server.".into())
}

/// A running server, with its version and tailnet status.
pub struct Discovered {
    pub server: RunningServer,
    pub version: String,
    pub tailnet: Option<Tailnet>,
}

/// `server`, if it answers as itself.
pub fn check(server: RunningServer) -> Option<Discovered> {
    let info = probe(&client().ok()?, &server).filter(|info| info.profile == server.profile)?;
    Some(Discovered::new(server, info))
}

impl Discovered {
    fn new(server: RunningServer, mut info: ServerInfo) -> Self {
        Self {
            server,
            version: std::mem::take(&mut info.version),
            tailnet: info.tailnet(),
        }
    }
}

fn discover_in(dir: &Path, client: &Client) -> Vec<Discovered> {
    let mut servers = Vec::new();
    let mut addresses = HashSet::new();
    for path in discovery::registration_paths(dir) {
        if let Some(server) = discovery::read_registration(&path)
            && let Some(info) = probe(client, &server)
            && info.profile == server.profile
            && addresses.insert(server.address)
        {
            servers.push(Discovered::new(server, info));
        }
    }

    // Older default-profile servers do not publish a registration.
    if !servers
        .iter()
        .any(|found| found.server.profile.is_none() && found.server.address.port() == 8004)
        && let Ok(token) =
            discovery::read_private_file(&dir.join("state.token"), discovery::SMALL_FILE)
    {
        let token = token.trim().to_string();
        if discovery::valid_token(&token) {
            for address in ["127.0.0.1:8004", "[::1]:8004"] {
                let server = RunningServer {
                    pid: 0,
                    address: address.parse().unwrap(),
                    profile: None,
                    token: token.clone(),
                };
                if !addresses.contains(&server.address)
                    && let Some(info) = probe(client, &server)
                    && info.profile.is_none()
                {
                    servers.push(Discovered::new(server, info));
                    break;
                }
            }
        }
    }
    servers.sort_by(|a, b| {
        (a.server.profile.cmp(&b.server.profile)).then(a.server.address.cmp(&b.server.address))
    });
    servers
}

pub fn discover() -> Result<Vec<Discovered>, String> {
    let Some(dir) = discovery::state_dir() else {
        return Ok(Vec::new());
    };
    let client = client().map_err(|_| "Could not check local btmux servers.".to_string())?;
    Ok(discover_in(&dir, &client))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;

    #[test]
    fn skips_servers_already_at_the_target() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let server = RunningServer {
            pid: std::process::id(),
            address: listener.local_addr().unwrap(),
            profile: Some("desktop".into()),
            token: "t".repeat(64),
        };
        let responder = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0; 4096];
            let count = stream.read(&mut request).unwrap();
            assert!(request[..count].starts_with(b"GET /api/info "));
            let body = r#"{"version":"0.0.209","profile":"desktop","instance":"a","pid":1}"#;
            write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
            drop(stream);
            listener
        });
        upgrade(&server, Path::new("/nonexistent/btmux"), "0.0.209").unwrap();
        let listener = responder.join().unwrap();
        listener.set_nonblocking(true).unwrap();
        // No upgrade request followed.
        assert!(listener.accept().is_err());
    }

    #[test]
    fn rejects_auth_failures_redirects_and_non_btmux_responses() {
        for (status, body) in [
            ("401 Unauthorized", r#"{"version":"test","profile":null}"#),
            ("302 Found", r#"{"version":"test","profile":null}"#),
            ("200 OK", r#"{"unrelated":"service"}"#),
        ] {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let server = RunningServer {
                pid: std::process::id(),
                address: listener.local_addr().unwrap(),
                profile: None,
                token: "t".repeat(64),
            };
            let responder = std::thread::spawn(move || {
                let (mut stream, _) = listener.accept().unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(2)))
                    .unwrap();
                let mut request = [0; 4096];
                assert!(stream.read(&mut request).unwrap() > 0);
                write!(stream, "HTTP/1.1 {status}\r\nContent-Length: {}\r\nLocation: http://127.0.0.1:1/\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
            });
            assert!(!ready(&server));
            responder.join().unwrap();
        }
    }

    #[test]
    fn discovers_live_authenticated_servers_and_ignores_stale_records() {
        let dir = std::env::temp_dir().join(format!("btmux-launcher-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(dir.join("stale")).unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let token = "t".repeat(64);
        let registration =
            discovery::Registration::publish(&dir.join("state.json"), address, None, token.clone())
                .unwrap();
        let stale_listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let stale_address = stale_listener.local_addr().unwrap();
        drop(stale_listener);
        let stale = discovery::Registration::publish(
            &dir.join("stale/state.json"),
            stale_address,
            Some("stale".into()),
            token.clone(),
        )
        .unwrap();
        let responder = std::thread::spawn(move || {
            for _ in 0..2 {
                let (mut stream, _) = listener.accept().unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(2)))
                    .unwrap();
                let mut request = Vec::new();
                let mut buffer = [0; 1024];
                while !request.windows(4).any(|part| part == b"\r\n\r\n") {
                    let count = stream.read(&mut buffer).unwrap();
                    assert_ne!(count, 0);
                    request.extend_from_slice(&buffer[..count]);
                }
                let request = String::from_utf8(request).unwrap().to_ascii_lowercase();
                assert!(request.contains(&format!("authorization: bearer {token}")));
                let body = r#"{"version":"test","profile":null,"tailscale":{"state":"serving","url":"https://box.ts.net:8004"}}"#;
                write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
            }
        });
        let client = client().unwrap();
        let servers = discover_in(&dir, &client);
        assert_eq!(servers.len(), 1);
        assert_eq!(servers[0].server.address, address);
        assert_eq!(servers[0].version, "test");
        assert!(
            matches!(&servers[0].tailnet, Some(Tailnet::Serving { url }) if url == "https://box.ts.net:8004")
        );
        let mut mismatched = servers[0].server.clone();
        mismatched.profile = Some("wrong-profile".into());
        assert!(!ready(&mismatched));
        responder.join().unwrap();
        drop(registration);
        drop(stale);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
