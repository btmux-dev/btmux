use std::collections::HashSet;
use std::path::Path;
use std::time::{Duration, Instant};

use reqwest::blocking::Client;
use serde::Deserialize;

use crate::discovery::{self, RunningServer};

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
    Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_millis(500))
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
    client()
        .ok()
        .and_then(|client| probe(&client, server))
        .is_some_and(|info| info.profile == server.profile)
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

/// Moves `server` to `exe` in place, as `btmux upgrade` does.
pub fn upgrade(server: &RunningServer, exe: &Path) -> Result<(), String> {
    // The server first runs `exe` to check it, for up to 10 s.
    let client = Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(15))
        .build()
        .map_err(|_| "Could not upgrade the desktop server.".to_string())?;
    let info = || -> Option<serde_json::Value> {
        client
            .get(format!("http://{}/api/info", server.address))
            .bearer_auth(&server.token)
            .send()
            .ok()?
            .json()
            .ok()
    };
    let before = info().ok_or("The desktop server is no longer available.")?;
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
        let Some(info) = info() else { continue };
        if info["instance"] != before["instance"] {
            // An exec keeps the pid; a service manager's restart doesn't.
            if info["pid"] != before["pid"] {
                return Err(
                    "The desktop server restarted instead of upgrading, closing its shells.".into(),
                );
            }
            return Ok(());
        }
        if let Some(error) = info["upgrade_error"].as_str() {
            return Err(failed(error));
        }
    }
    Err("Timed out waiting for the upgraded desktop server.".into())
}

/// Whether release `a` is newer than `b` (`x.y.z`, ignoring any suffix).
pub fn newer(a: &str, b: &str) -> bool {
    let parse = |version: &str| -> Option<Vec<u64>> {
        let release = version.split(['-', '+']).next()?;
        release.split('.').map(|part| part.parse().ok()).collect()
    };
    matches!((parse(a), parse(b)), (Some(a), Some(b)) if a.len() == b.len() && a > b)
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
    fn new(server: RunningServer, info: ServerInfo) -> Self {
        Self {
            server,
            version: info.version.clone(),
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
    fn compares_release_versions() {
        assert!(newer("0.0.210", "0.0.209"));
        assert!(newer("0.1.0", "0.0.209"));
        assert!(newer("0.0.210", "0.0.209-dev"));
        assert!(!newer("0.0.209", "0.0.209"));
        assert!(!newer("0.0.208", "0.0.209"));
        assert!(!newer("0.0.210", "test"));
        assert!(!newer("1.0", "0.0.1"));
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
