//! TCP ports that a pane's programs listen on, for `btmux forward`.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, System};
use uuid::Uuid;

use crate::ws::processes::{sample_ports, AncestorPanes};

/// A dev server opens many connections at once; each would sample otherwise.
const CACHE_FOR: Duration = Duration::from_secs(2);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(utoipa::ToSchema))]
pub struct Listener {
    pub port: u16,
    /// The address it listens on, such as `127.0.0.1` or `0.0.0.0`.
    pub address: String,
    pub pid: Option<u32>,
    /// The listening process's name.
    pub process: Option<String>,
    /// A wildcard listener whose loopback port another program also holds,
    /// so connecting over loopback would reach that one instead.
    #[serde(skip)]
    pub shadowed: bool,
}

impl Listener {
    /// Where the server connects: loopback for a wildcard listener.
    pub fn connect_address(&self) -> std::net::SocketAddr {
        use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
        let ip = match self.address.parse::<IpAddr>() {
            Ok(IpAddr::V4(ip)) if ip.is_unspecified() => IpAddr::V4(Ipv4Addr::LOCALHOST),
            Ok(IpAddr::V6(ip)) if ip.is_unspecified() => IpAddr::V6(Ipv6Addr::LOCALHOST),
            Ok(ip) => ip,
            Err(_) => IpAddr::V4(Ipv4Addr::LOCALHOST),
        };
        std::net::SocketAddr::new(ip, self.port)
    }
}

type Sample = HashMap<Uuid, Vec<Listener>>;

static CACHE: Mutex<Option<(Instant, Sample)>> = Mutex::new(None);

/// Each pane's TCP listeners, from a sample at most `CACHE_FOR` old unless
/// `fresh`. `shells` maps shell pids to panes, as `shell_pids` gives them.
pub async fn listeners(shells: HashMap<u32, Uuid>, fresh: bool) -> Result<Sample, String> {
    if !fresh {
        if let Some((at, sample)) = CACHE.lock().unwrap().as_ref() {
            if at.elapsed() < CACHE_FOR {
                return Ok(sample.clone());
            }
        }
    }
    let sample = tokio::task::spawn_blocking(move || sample_listeners(&shells))
        .await
        .map_err(|e| e.to_string())??;
    *CACHE.lock().unwrap() = Some((Instant::now(), sample.clone()));
    Ok(sample)
}

/// Why a pane's listeners can't be listed.
#[derive(Debug)]
pub enum Unavailable {
    NoPane,
    Guest,
    Failed(String),
}

impl axum::response::IntoResponse for Unavailable {
    fn into_response(self) -> axum::response::Response {
        use axum::http::StatusCode;
        match self {
            Unavailable::NoPane => StatusCode::NOT_FOUND.into_response(),
            Unavailable::Guest => (
                StatusCode::CONFLICT,
                "ports aren't available for panes of sandboxed sessions",
            )
                .into_response(),
            Unavailable::Failed(error) => {
                (StatusCode::INTERNAL_SERVER_ERROR, error).into_response()
            }
        }
    }
}

/// The TCP listeners of `pane`'s programs, including its popups'.
pub async fn pane_listeners(
    state: &crate::AppState,
    pane: Uuid,
    fresh: bool,
) -> Result<Vec<Listener>, Unavailable> {
    let shells = {
        let manager = state.read().await;
        if manager.find_pane(pane).is_none() {
            return Err(Unavailable::NoPane);
        }
        if manager.is_guest_pane(pane) {
            return Err(Unavailable::Guest);
        }
        manager
            .shell_pids()
            .into_iter()
            .map(|(pane, shell, _)| (shell, pane))
            .collect()
    };
    let mut sample = listeners(shells, fresh)
        .await
        .map_err(Unavailable::Failed)?;
    Ok(sample.remove(&pane).unwrap_or_default())
}

fn sample_listeners(shells: &HashMap<u32, Uuid>) -> Result<Sample, String> {
    let mut system = System::new().map_err(|e| e.to_string())?;
    system.refresh_processes_specifics(ProcessesToUpdate::All, true, ProcessRefreshKind::nothing());
    let processes = system.processes();
    let mut panes = AncestorPanes::new(
        processes
            .values()
            .filter_map(|p| Some((p.pid().as_u32(), p.parent()?.as_u32())))
            .collect(),
        shells,
    );
    let mut sample = Sample::new();
    // Every listener's address, port, and pane, to find shadowed ones.
    let mut all = Vec::new();
    for port in sample_ports()? {
        if port.protocol != "TCP" || port.state != "LISTEN" {
            continue;
        }
        let Some(pid) = port.pid else { continue };
        let process = processes.get(&sysinfo::Pid::from_u32(pid));
        let session = || {
            process
                .and_then(|p| p.session_id())
                .filter(|_| cfg!(unix))
                .map(|s| s.as_u32())
        };
        let owner = panes.owner(pid, session);
        all.push((port.local_address.clone(), port.local_port, owner));
        let Some(pane) = owner else {
            continue;
        };
        let listeners = sample.entry(pane).or_default();
        let listener = Listener {
            port: port.local_port,
            address: port.local_address,
            pid: Some(pid),
            process: process.map(|p| p.name().to_string_lossy().into_owned()),
            shadowed: false,
        };
        if !listeners.contains(&listener) {
            listeners.push(listener);
        }
    }
    mark_shadowed(&mut sample, &all);
    for listeners in sample.values_mut() {
        listeners.sort_by(|a, b| (a.port, &a.address).cmp(&(b.port, &b.address)));
    }
    Ok(sample)
}

/// Marks wildcard listeners whose loopback address and port another pane's
/// program, or one in no pane, also listens on. `all` holds every listener's
/// address, port, and pane.
fn mark_shadowed(sample: &mut Sample, all: &[(String, u16, Option<Uuid>)]) {
    for (pane, listeners) in sample.iter_mut() {
        for listener in listeners.iter_mut() {
            let loopback = listener.connect_address().ip().to_string();
            listener.shadowed = listener.address != loopback
                && all.iter().any(|(address, port, owner)| {
                    *port == listener.port && *address == loopback && *owner != Some(*pane)
                });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn listeners_belong_to_the_pane_of_their_process() {
        let mine = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = mine.local_addr().unwrap().port();
        let pane = Uuid::new_v4();
        // This test process stands in for the pane's shell.
        let shells = HashMap::from([(std::process::id(), pane)]);
        let sample = listeners(shells, true).await.unwrap();
        let found = sample[&pane].iter().find(|l| l.port == port).unwrap();
        assert_eq!(found.address, "127.0.0.1");
        assert_eq!(found.pid, Some(std::process::id()));

        // A shell that isn't an ancestor doesn't get it.
        let mut child = std::process::Command::new("sleep")
            .arg("30")
            .spawn()
            .unwrap();
        let other = Uuid::new_v4();
        let sample = listeners(HashMap::from([(child.id(), other)]), true)
            .await
            .unwrap();
        child.kill().unwrap();
        child.wait().unwrap();
        assert!(!sample.contains_key(&other));
    }

    #[test]
    fn wildcard_listeners_are_shadowed_by_another_loopback_listener() {
        let pane = Uuid::new_v4();
        let listener = |address: &str| Listener {
            port: 8125,
            address: address.into(),
            pid: Some(1),
            process: None,
            shadowed: false,
        };
        let check = |mine: &[&str], other: Option<Uuid>| {
            let mut sample = Sample::from([(pane, mine.iter().map(|a| listener(a)).collect())]);
            let mut all: Vec<_> = mine
                .iter()
                .map(|a| (a.to_string(), 8125, Some(pane)))
                .collect();
            all.push(("127.0.0.1".into(), 8125, other));
            mark_shadowed(&mut sample, &all);
            sample[&pane].iter().map(|l| l.shadowed).collect::<Vec<_>>()
        };
        // A `btmux forward` (in no pane) on the same machine.
        assert_eq!(check(&["0.0.0.0"], None), [true]);
        assert_eq!(check(&["0.0.0.0"], Some(Uuid::new_v4())), [true]);
        // The pane's own loopback listener, and a listener on loopback itself.
        assert_eq!(check(&["0.0.0.0"], Some(pane)), [false]);
        assert_eq!(check(&["127.0.0.1"], None), [false]);
        // IPv6 wildcards connect over ::1, which nothing else holds.
        assert_eq!(check(&["::"], None), [false]);
    }

    #[test]
    fn wildcard_listeners_connect_over_loopback() {
        let listener = |address: &str| Listener {
            port: 80,
            address: address.into(),
            pid: None,
            process: None,
            shadowed: false,
        };
        assert_eq!(
            listener("0.0.0.0").connect_address().to_string(),
            "127.0.0.1:80"
        );
        assert_eq!(listener("::").connect_address().to_string(), "[::1]:80");
        assert_eq!(
            listener("10.0.0.2").connect_address().to_string(),
            "10.0.0.2:80"
        );
    }
}
