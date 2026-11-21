//! TCP ports that a pane's programs listen on, for `btmux forward`, and which
//! program accepted a connection the server made to one.

use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, System};
use uuid::Uuid;

use crate::ws::processes::{sample_ports, AncestorPanes};

/// How old a sample may be for listing ports. Checking a connection always
/// takes one started after it was made.
pub const LIST_AGE: Duration = Duration::from_secs(2);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(utoipa::ToSchema))]
pub struct Listener {
    pub port: u16,
    /// The address it listens on, such as `127.0.0.1` or `0.0.0.0`.
    pub address: String,
    pub pid: Option<u32>,
    /// The listening process's name.
    pub process: Option<String>,
}

impl Listener {
    fn ip(&self) -> Option<IpAddr> {
        self.address.parse().ok()
    }

    /// Where the server connects: loopback for a wildcard listener.
    pub fn connect_address(&self) -> SocketAddr {
        use std::net::{Ipv4Addr, Ipv6Addr};
        let ip = match self.ip() {
            Some(IpAddr::V4(ip)) if ip.is_unspecified() => IpAddr::V4(Ipv4Addr::LOCALHOST),
            Some(IpAddr::V6(ip)) if ip.is_unspecified() => IpAddr::V6(Ipv6Addr::LOCALHOST),
            Some(ip) => ip,
            None => IpAddr::V4(Ipv4Addr::LOCALHOST),
        };
        SocketAddr::new(ip, self.port)
    }
}

/// The listener for `port`, on `address` if given, else the pane's only
/// listening port. A port held on several addresses by different programs
/// needs `address`; one program's listeners on several addresses are one
/// service, reached over the first.
pub fn pick(
    listeners: &[Listener],
    port: Option<u16>,
    address: Option<IpAddr>,
) -> Result<Listener, String> {
    let list = |listeners: &[&Listener]| {
        listeners
            .iter()
            .map(|l| {
                let at = SocketAddr::new(l.ip().unwrap_or(IpAddr::from([0, 0, 0, 0])), l.port);
                format!("  {at:<22}  {}", l.process.as_deref().unwrap_or("?"))
            })
            .collect::<Vec<_>>()
            .join("\n")
    };
    let all: Vec<&Listener> = listeners.iter().collect();
    let Some(port) = port.or_else(|| {
        let first = listeners.first()?.port;
        listeners.iter().all(|l| l.port == first).then_some(first)
    }) else {
        return Err(if listeners.is_empty() {
            "no program in the pane listens on a TCP port".into()
        } else {
            format!(
                "the pane listens on several ports; name one:\n{}",
                list(&all)
            )
        });
    };
    let matching: Vec<&Listener> = all
        .into_iter()
        .filter(|l| l.port == port && address.is_none_or(|a| l.ip() == Some(a)))
        .collect();
    let Some(first) = matching.first() else {
        return Err(match address {
            Some(address) => format!(
                "no program in the pane listens on {}",
                SocketAddr::new(address, port)
            ),
            None => format!("no program in the pane listens on port {port}"),
        });
    };
    if matching.iter().any(|l| l.pid != first.pid) {
        return Err(format!(
            "different programs in the pane listen on port {port}; name one as ADDRESS:PORT:\n{}",
            list(&matching)
        ));
    }
    Ok((*first).clone())
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Socket {
    local: SocketAddr,
    remote: Option<SocketAddr>,
    listening: bool,
    pid: Option<u32>,
}

/// The `btmux forward` process asking, and where it listens, so the server
/// never forwards to the forward itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Forwarder {
    pub pid: u32,
    pub listener: SocketAddr,
}

/// One sample of every TCP socket and the process tree.
pub struct Snapshot {
    started: Instant,
    sockets: Vec<Socket>,
    parents: HashMap<u32, u32>,
    sessions: HashMap<u32, u32>,
    names: HashMap<u32, String>,
}

/// The latest snapshot. Holding the lock while sampling lets connections that
/// arrive together share one.
static LATEST: tokio::sync::Mutex<Option<Arc<Snapshot>>> = tokio::sync::Mutex::const_new(None);

/// A snapshot started no earlier than `not_before`.
pub async fn snapshot(not_before: Instant) -> Result<Arc<Snapshot>, String> {
    let mut latest = LATEST.lock().await;
    if let Some(snapshot) = latest.as_ref().filter(|s| s.started >= not_before) {
        return Ok(snapshot.clone());
    }
    let snapshot = Arc::new(
        tokio::task::spawn_blocking(Snapshot::take)
            .await
            .map_err(|e| e.to_string())??,
    );
    *latest = Some(snapshot.clone());
    Ok(snapshot)
}

/// Unmapped, so `::ffff:127.0.0.1` matches `127.0.0.1`.
fn canonical(address: SocketAddr) -> SocketAddr {
    SocketAddr::new(address.ip().to_canonical(), address.port())
}

impl Snapshot {
    fn take() -> Result<Self, String> {
        let started = Instant::now();
        let mut system = System::new().map_err(|e| e.to_string())?;
        system.refresh_processes_specifics(
            ProcessesToUpdate::All,
            true,
            ProcessRefreshKind::nothing(),
        );
        let mut snapshot = Snapshot {
            started,
            sockets: Vec::new(),
            parents: HashMap::new(),
            sessions: HashMap::new(),
            names: HashMap::new(),
        };
        for process in system.processes().values() {
            let pid = process.pid().as_u32();
            if let Some(parent) = process.parent() {
                snapshot.parents.insert(pid, parent.as_u32());
            }
            // On Windows this is a Terminal Services session.
            if let Some(session) = process.session_id().filter(|_| cfg!(unix)) {
                snapshot.sessions.insert(pid, session.as_u32());
            }
            snapshot
                .names
                .insert(pid, process.name().to_string_lossy().into_owned());
        }
        for port in sample_ports()? {
            if port.protocol != "TCP" {
                continue;
            }
            let Ok(ip) = port.local_address.parse::<IpAddr>() else {
                continue;
            };
            snapshot.sockets.push(Socket {
                local: canonical(SocketAddr::new(ip, port.local_port)),
                remote: port
                    .remote_address
                    .and_then(|r| r.parse().ok())
                    .map(canonical),
                listening: port.state == "LISTEN",
                pid: port.pid,
            });
        }
        Ok(snapshot)
    }

    fn owner(&self, shells: &HashMap<u32, Uuid>, pid: u32) -> Option<Uuid> {
        AncestorPanes::new(self.parents.clone(), shells)
            .owner(pid, || self.sessions.get(&pid).copied())
    }

    fn describe(&self, pid: u32) -> String {
        match self.names.get(&pid) {
            Some(name) => format!("{name} (pid {pid})"),
            None => format!("pid {pid}"),
        }
    }

    /// The TCP listeners of `pane`'s programs, by port and address.
    pub fn listeners(&self, shells: &HashMap<u32, Uuid>, pane: Uuid) -> Vec<Listener> {
        let mut panes = AncestorPanes::new(self.parents.clone(), shells);
        let mut listeners: Vec<Listener> = self
            .sockets
            .iter()
            .filter(|s| s.listening)
            .filter_map(|s| {
                let pid = s.pid?;
                (panes.owner(pid, || self.sessions.get(&pid).copied()) == Some(pane)).then(|| {
                    Listener {
                        port: s.local.port(),
                        address: s.local.ip().to_string(),
                        pid: Some(pid),
                        process: self.names.get(&pid).cloned(),
                    }
                })
            })
            .collect();
        // Workers sharing a listening socket (gunicorn, Puma) are one listener,
        // shown by the lowest pid, usually the parent's.
        listeners.sort_by(|a, b| (a.port, &a.address, a.pid).cmp(&(b.port, &b.address, b.pid)));
        listeners.dedup_by(|b, a| a.port == b.port && a.address == b.address);
        listeners
    }

    /// Checks that the connection from `ours` to `theirs` reached a program
    /// in `pane`: the one that accepted it, else every listener the kernel
    /// could have queued it on.
    pub fn check(
        &self,
        shells: &HashMap<u32, Uuid>,
        pane: Uuid,
        ours: SocketAddr,
        theirs: SocketAddr,
        forwarder: Option<Forwarder>,
    ) -> Result<(), String> {
        let (ours, theirs) = (canonical(ours), canonical(theirs));
        let accepted = self
            .sockets
            .iter()
            .find(|s| !s.listening && s.local == theirs && s.remote == Some(ours))
            .and_then(|s| s.pid);
        match accepted {
            Some(pid) if forwarder.is_some_and(|f| f.pid == pid) => Err(itself(theirs)),
            Some(pid) if self.owner(shells, pid) == Some(pane) => Ok(()),
            Some(pid) => Err(format!(
                "{theirs} is held by {}, not a program in the pane",
                self.describe(pid)
            )),
            // The route was checked before connecting. A program that accepts
            // one connection can close it and its listener at once, leaving
            // nothing to check; a program that took over is still there.
            None if self.route(theirs).is_empty() => Ok(()),
            None => self.check_route(shells, pane, theirs, forwarder),
        }
    }

    /// The listeners the kernel could queue a connection to `theirs` on: the
    /// most specific for it.
    fn route(&self, theirs: SocketAddr) -> Vec<&Socket> {
        let theirs = canonical(theirs);
        let on_port = || {
            self.sockets
                .iter()
                .filter(move |s| s.listening && s.local.port() == theirs.port())
        };
        let exact: Vec<&Socket> = on_port().filter(|s| s.local.ip() == theirs.ip()).collect();
        if !exact.is_empty() {
            return exact;
        }
        on_port()
            .filter(|s| {
                s.local.ip().is_unspecified()
                    // A dual-stack IPv6 wildcard takes IPv4 too.
                    && (s.local.is_ipv4() == theirs.is_ipv4() || s.local.is_ipv6())
            })
            .collect()
    }

    /// Checks, without connecting, that a connection to `theirs` would reach
    /// a program in `pane` and not the forwarder: every listener the kernel
    /// could queue it on, the most specific for `theirs`, must be the pane's.
    pub fn check_route(
        &self,
        shells: &HashMap<u32, Uuid>,
        pane: Uuid,
        theirs: SocketAddr,
        forwarder: Option<Forwarder>,
    ) -> Result<(), String> {
        let theirs = canonical(theirs);
        let candidates = self.route(theirs);
        if candidates.is_empty() {
            return Err(format!("nothing listens on {theirs} any more"));
        }
        for socket in candidates {
            let Some(pid) = socket.pid else {
                return Err(format!("can't tell which program listens on {theirs}"));
            };
            if forwarder.is_some_and(|f| f.pid == pid && canonical(f.listener) == socket.local) {
                return Err(itself(theirs));
            }
            if self.owner(shells, pid) != Some(pane) {
                return Err(format!(
                    "{} is held by {}, not a program in the pane",
                    socket.local,
                    self.describe(pid)
                ));
            }
        }
        Ok(())
    }
}

fn itself(at: SocketAddr) -> String {
    format!(
        "{at} is this forward's own port: on the server's machine it stands in front of the \
         pane's program"
    )
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

/// The oldest start a snapshot for listing ports may have.
pub fn listing_age() -> Instant {
    Instant::now()
        .checked_sub(LIST_AGE)
        .unwrap_or_else(Instant::now)
}

/// Shell pids by pane, for a pane whose ports can be listed.
pub async fn pane_shells(
    state: &crate::AppState,
    pane: Uuid,
) -> Result<HashMap<u32, Uuid>, Unavailable> {
    let manager = state.read().await;
    if manager.find_pane(pane).is_none() {
        return Err(Unavailable::NoPane);
    }
    if manager.is_guest_pane(pane) {
        return Err(Unavailable::Guest);
    }
    // A popup's programs count for its pane.
    Ok(manager
        .shell_pids()
        .into_iter()
        .map(|(pane, shell, _)| (shell, pane))
        .collect())
}

/// The TCP listeners of `pane`'s programs, including its popups'.
pub async fn pane_listeners(
    state: &crate::AppState,
    pane: Uuid,
) -> Result<Vec<Listener>, Unavailable> {
    let shells = pane_shells(state, pane).await?;
    let snapshot = snapshot(listing_age()).await.map_err(Unavailable::Failed)?;
    Ok(snapshot.listeners(&shells, pane))
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
        let snapshot = snapshot(Instant::now()).await.unwrap();
        let listeners = snapshot.listeners(&shells, pane);
        let found = listeners.iter().find(|l| l.port == port).unwrap();
        assert_eq!(found.address, "127.0.0.1");
        assert_eq!(found.pid, Some(std::process::id()));

        // A shell that isn't an ancestor doesn't get it.
        let mut child = std::process::Command::new("sleep")
            .arg("30")
            .spawn()
            .unwrap();
        let other = Uuid::new_v4();
        let snapshot = super::snapshot(Instant::now()).await.unwrap();
        let listeners = snapshot.listeners(&HashMap::from([(child.id(), other)]), other);
        child.kill().unwrap();
        child.wait().unwrap();
        assert!(listeners.is_empty());
    }

    #[tokio::test]
    async fn a_live_connection_is_checked_against_its_acceptor() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let theirs = listener.local_addr().unwrap();
        let client = std::net::TcpStream::connect(theirs).unwrap();
        let (_accepted, _) = listener.accept().unwrap();
        let ours = client.local_addr().unwrap();
        let pane = Uuid::new_v4();
        let snapshot = snapshot(Instant::now()).await.unwrap();
        let mine = HashMap::from([(std::process::id(), pane)]);
        assert_eq!(snapshot.check(&mine, pane, ours, theirs, None), Ok(()));
        let error = snapshot
            .check(&HashMap::new(), pane, ours, theirs, None)
            .unwrap_err();
        assert!(error.contains("not a program in the pane"), "{error}");
    }

    fn fixture(sockets: Vec<Socket>) -> Snapshot {
        Snapshot {
            started: Instant::now(),
            sockets,
            // Pid 10 is the pane's shell, 11 its child; 20 is elsewhere.
            parents: HashMap::from([(11, 10), (20, 1)]),
            sessions: HashMap::new(),
            names: HashMap::from([(20, "btmux".into())]),
        }
    }

    fn listen(address: &str, pid: Option<u32>) -> Socket {
        Socket {
            local: address.parse().unwrap(),
            remote: None,
            listening: true,
            pid,
        }
    }

    #[test]
    fn check_finds_the_listener_the_kernel_chose() {
        let pane = Uuid::new_v4();
        let shells = HashMap::from([(10, pane)]);
        let ours: SocketAddr = "127.0.0.1:50000".parse().unwrap();
        let check = |sockets: Vec<Socket>, theirs: &str| {
            fixture(sockets).check(&shells, pane, ours, theirs.parse().unwrap(), None)
        };
        // A wildcard listener in the pane takes loopback connections.
        assert_eq!(
            check(vec![listen("0.0.0.0:8125", Some(11))], "127.0.0.1:8125"),
            Ok(())
        );
        assert_eq!(
            check(vec![listen("[::]:8125", Some(11))], "127.0.0.1:8125"),
            Ok(())
        );
        // A more specific listener elsewhere shadows it.
        let shadowed = check(
            vec![
                listen("0.0.0.0:8125", Some(11)),
                listen("127.0.0.1:8125", Some(20)),
            ],
            "127.0.0.1:8125",
        )
        .unwrap_err();
        assert!(shadowed.contains("btmux (pid 20)"), "{shadowed}");
        // Unknown owners and vanished listeners are refused.
        assert!(
            check(vec![listen("127.0.0.1:8125", None)], "127.0.0.1:8125")
                .unwrap_err()
                .contains("can't tell")
        );
        // Nothing left after connecting: the program that accepted closed its
        // listener, which the route check before connecting saw as the pane's.
        assert_eq!(check(vec![], "127.0.0.1:8125"), Ok(()));
        assert!(fixture(vec![])
            .check_route(&shells, pane, "127.0.0.1:8125".parse().unwrap(), None)
            .unwrap_err()
            .contains("any more"));
        // An accepted socket decides, whatever listens.
        let accepted = Socket {
            local: "127.0.0.1:8125".parse().unwrap(),
            remote: Some(ours),
            listening: false,
            pid: Some(20),
        };
        assert!(check(
            vec![listen("127.0.0.1:8125", Some(11)), accepted],
            "127.0.0.1:8125"
        )
        .is_err());
    }

    #[test]
    fn workers_sharing_a_socket_are_one_listener() {
        let pane = Uuid::new_v4();
        let shells = HashMap::from([(10, pane)]);
        // The master, 11, and its workers, 12 and 13, share 127.0.0.1:8000.
        let mut snapshot = fixture(vec![
            listen("127.0.0.1:8000", Some(13)),
            listen("127.0.0.1:8000", Some(11)),
            listen("127.0.0.1:8000", Some(12)),
        ]);
        snapshot.parents.extend([(12, 11), (13, 11)]);
        let listeners = snapshot.listeners(&shells, pane);
        assert_eq!(listeners.len(), 1);
        assert_eq!(listeners[0].pid, Some(11));
        assert_eq!(pick(&listeners, Some(8000), None).unwrap().pid, Some(11));
        assert_eq!(
            snapshot.check_route(&shells, pane, "127.0.0.1:8000".parse().unwrap(), None),
            Ok(())
        );
    }

    #[test]
    fn the_forward_itself_is_never_a_destination() {
        let pane = Uuid::new_v4();
        let shells = HashMap::from([(10, pane)]);
        // A forward run in the pane, 11, on the loopback port of the pane's
        // wildcard listener, 12.
        let mut snapshot = fixture(vec![
            listen("0.0.0.0:5173", Some(12)),
            listen("127.0.0.1:5173", Some(11)),
        ]);
        snapshot.parents.insert(12, 10);
        let me = Forwarder {
            pid: 11,
            listener: "127.0.0.1:5173".parse().unwrap(),
        };
        let theirs: SocketAddr = "127.0.0.1:5173".parse().unwrap();
        // Without saying who it is, the forward looks like the pane's.
        assert_eq!(snapshot.check_route(&shells, pane, theirs, None), Ok(()));
        let error = snapshot
            .check_route(&shells, pane, theirs, Some(me))
            .unwrap_err();
        assert!(error.contains("own port"), "{error}");
        // Once it accepted the connection, too.
        let ours: SocketAddr = "127.0.0.1:50000".parse().unwrap();
        snapshot.sockets.push(Socket {
            local: theirs,
            remote: Some(ours),
            listening: false,
            pid: Some(11),
        });
        assert!(snapshot
            .check(&shells, pane, ours, theirs, Some(me))
            .is_err());
        // Another machine's forward with another pid isn't confused with it.
        let elsewhere = Forwarder { pid: 99, ..me };
        assert_eq!(
            snapshot
                .check_route(
                    &shells,
                    pane,
                    "127.0.0.1:5174".parse().unwrap(),
                    Some(elsewhere)
                )
                .unwrap_err(),
            "nothing listens on 127.0.0.1:5174 any more"
        );
    }

    fn listener(address: &str, port: u16, pid: u32) -> Listener {
        Listener {
            port,
            address: address.into(),
            pid: Some(pid),
            process: Some("node".into()),
        }
    }

    #[test]
    fn pick_takes_the_only_port_or_the_named_one() {
        let ip = |s: &str| s.parse::<IpAddr>().ok();
        assert_eq!(
            pick(&[listener("127.0.0.1", 5173, 1)], None, None)
                .unwrap()
                .port,
            5173
        );
        // One program on two addresses is one service.
        let one = [listener("127.0.0.1", 5173, 1), listener("::1", 5173, 1)];
        assert_eq!(pick(&one, None, None).unwrap().address, "127.0.0.1");
        // Two programs on one port need an address.
        let two = [listener("127.0.0.1", 5173, 1), listener("::1", 5173, 2)];
        assert!(pick(&two, Some(5173), None)
            .unwrap_err()
            .contains("ADDRESS:PORT"));
        assert_eq!(pick(&two, Some(5173), ip("::1")).unwrap().pid, Some(2));
        assert!(pick(&two, Some(5173), ip("10.0.0.1")).is_err());
        let ports = [
            listener("127.0.0.1", 3000, 1),
            listener("127.0.0.1", 5173, 1),
        ];
        assert!(pick(&ports, None, None)
            .unwrap_err()
            .contains("several ports"));
        assert_eq!(pick(&ports, Some(3000), None).unwrap().port, 3000);
        assert!(pick(&ports, Some(1), None).is_err());
        assert!(pick(&[], None, None).is_err());
    }

    #[test]
    fn wildcard_listeners_connect_over_loopback() {
        let at = |address: &str| listener(address, 80, 1).connect_address().to_string();
        assert_eq!(at("0.0.0.0"), "127.0.0.1:80");
        assert_eq!(at("::"), "[::1]:80");
        assert_eq!(at("10.0.0.2"), "10.0.0.2:80");
    }
}
