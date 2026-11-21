//! Pane programs' exit statuses: `new-window --wait`, `btmux wait-exit`,
//! `GET /api/panes/{id}/exit`, and the `wait_for_exit` MCP tool.
//!
//! A pane in the tree reports its program's status once it was reaped, and
//! a kept-open pane keeps it. A closed pane's status is kept for a while, so
//! a waiter that starts late, or reconnects after an upgrade, still gets it.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde::{Deserialize, Serialize};
use tokio::sync::watch;
use uuid::Uuid;

use crate::automation::Error;
use crate::popup::{PopupCommand, Spawn};
use crate::session::manager::SessionManager;
use crate::AppState;

/// How long, and how many, closed panes' statuses are kept.
const ENDED_KEPT: Duration = Duration::from_secs(300);
const ENDED_MAX: usize = 256;

/// `wait-exit` exit status when the wait times out, as for `timeout(1)`.
pub const EXIT_TIMED_OUT: i32 = 124;

struct Ended {
    pane_id: Uuid,
    status: Option<i32>,
    at: Instant,
}

/// Closed panes' statuses, and a signal for waiters.
pub struct PaneExits {
    ended: VecDeque<Ended>,
    changed: watch::Sender<u64>,
}

impl Default for PaneExits {
    fn default() -> Self {
        Self {
            ended: VecDeque::new(),
            changed: watch::channel(0).0,
        }
    }
}

/// A closed pane's status, for an upgraded server.
#[derive(Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct EndedHandoff {
    pane_id: Uuid,
    status: Option<i32>,
    /// How long ago it closed.
    age_ms: u64,
}

/// Exit statuses, and kept panes whose programs exited, for an upgraded server.
#[cfg(unix)]
#[derive(Default)]
pub struct ExitsHandoff {
    pub ended: Vec<EndedHandoff>,
    pub dead: Vec<(Uuid, crate::pty::Exited)>,
}

impl PaneExits {
    /// A pane left the tree; `status` is `None` if its program hadn't reported one.
    pub fn closed(&mut self, pane_id: Uuid, status: Option<i32>) {
        let now = Instant::now();
        self.ended.retain(|ended| ended.pane_id != pane_id);
        while self
            .ended
            .front()
            .is_some_and(|ended| now.duration_since(ended.at) > ENDED_KEPT)
            || self.ended.len() >= ENDED_MAX
        {
            self.ended.pop_front();
        }
        self.ended.push_back(Ended {
            pane_id,
            status,
            at: now,
        });
        self.touch();
    }

    /// Wake waiters, as when a kept pane's program exits.
    pub fn touch(&self) {
        self.changed.send_modify(|n| *n += 1);
    }

    fn ended(&self, pane_id: Uuid) -> Option<Option<i32>> {
        self.ended
            .iter()
            .find(|ended| ended.pane_id == pane_id && ended.at.elapsed() <= ENDED_KEPT)
            .map(|ended| ended.status)
    }

    pub fn export(&self) -> Vec<EndedHandoff> {
        self.ended
            .iter()
            .filter(|ended| ended.at.elapsed() <= ENDED_KEPT)
            .map(|ended| EndedHandoff {
                pane_id: ended.pane_id,
                status: ended.status,
                age_ms: ended
                    .at
                    .elapsed()
                    .as_millis()
                    .try_into()
                    .unwrap_or(u64::MAX),
            })
            .collect()
    }

    pub fn restore(&mut self, ended: Vec<EndedHandoff>) {
        let now = Instant::now();
        for ended in ended {
            let age = Duration::from_millis(ended.age_ms);
            let Some(at) = now.checked_sub(age).filter(|_| age <= ENDED_KEPT) else {
                continue;
            };
            if self.ended.len() >= ENDED_MAX {
                self.ended.pop_front();
            }
            self.ended.push_back(Ended {
                pane_id: ended.pane_id,
                status: ended.status,
                at,
            });
        }
    }
}

#[derive(Debug, PartialEq)]
enum Progress {
    Running,
    /// The status, if the program reported one before its pane closed.
    Exited(Option<i32>),
    Unknown,
}

fn progress(mgr: &SessionManager, pane_id: Uuid) -> Progress {
    match mgr.find_pane(pane_id) {
        Some(pane) if pane.pty.exited() => Progress::Exited(pane.pty.exit_status()),
        Some(_) => Progress::Running,
        None => mgr
            .exits
            .ended(pane_id)
            .map_or(Progress::Unknown, Progress::Exited),
    }
}

#[derive(Serialize, Debug, PartialEq)]
#[cfg_attr(test, derive(utoipa::ToSchema))]
pub struct PaneExit {
    /// The program's exit status, 128 + N if signal N ended it; null if it
    /// hasn't exited, or its pane closed before it reported one.
    pub status: Option<i32>,
    /// The wait ended before the program exited.
    pub timed_out: bool,
}

/// Waits until the pane's program exits, or `timeout`.
pub async fn wait(state: &AppState, pane_id: Uuid, timeout: Duration) -> Result<PaneExit, Error> {
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        let mut changed = {
            let mgr = state.read().await;
            match progress(&mgr, pane_id) {
                Progress::Exited(status) => {
                    return Ok(PaneExit {
                        status,
                        timed_out: false,
                    })
                }
                Progress::Unknown => return Err(Error::PaneNotFound(pane_id)),
                Progress::Running => mgr.exits.changed.subscribe(),
            }
        };
        tokio::select! {
            changed = changed.changed() => if changed.is_err() {
                return Err("the server is shutting down".into());
            },
            _ = tokio::time::sleep_until(deadline) => {
                return Ok(PaneExit { status: None, timed_out: true });
            }
        }
    }
}

#[derive(Deserialize)]
#[cfg_attr(test, derive(utoipa::IntoParams))]
#[cfg_attr(test, into_params(parameter_in = Query))]
pub(crate) struct ExitQuery {
    /// Milliseconds: 10 minutes by default, at most an hour.
    timeout_ms: Option<u64>,
}

/// Long-polls until the pane's program exits.
#[cfg_attr(test, utoipa::path(
    get,
    path = "/api/panes/{pane_id}/exit",
    tag = "Panes",
    operation_id = "wait-for-a-pane-to-exit",
    summary = "Wait for a pane's program to exit",
    description = "Waits until the pane's program (its command, or its shell) exits and returns its status. A pane that closed in the last 5 minutes reports the status it closed with.",
    params(
        ("pane_id", description = "Pane ID"),
        ExitQuery,
    ),
    responses(
        (status = 200, description = "The exit status once the program exits, or the wait times out", body = PaneExit),
        (status = 404, description = "No such pane, or it closed too long ago", body = String, content_type = "text/plain"),
        (status = 500, description = "The server is shutting down", body = String, content_type = "text/plain"),
    ),
))]
pub(crate) async fn api_wait_exit(
    State(state): State<AppState>,
    Path(pane_id): Path<Uuid>,
    Query(query): Query<ExitQuery>,
) -> Response {
    let timeout = crate::automation::wait_duration(query.timeout_ms);
    match wait(&state, pane_id, timeout).await {
        Ok(exit) => Json(exit).into_response(),
        Err(Error::PaneNotFound(_)) => (
            StatusCode::NOT_FOUND,
            "no such pane, or it closed too long ago",
        )
            .into_response(),
        Err(error) => (StatusCode::INTERNAL_SERVER_ERROR, error.to_string()).into_response(),
    }
}

/// What a new pane runs instead of the shell, checked before it is made.
pub fn spawn_of(
    command: Option<PopupCommand>,
    env: Option<Vec<(String, String)>>,
    cwd: Option<&str>,
) -> Result<Option<Spawn>, String> {
    let Some(command) = command else {
        if env.is_some() {
            return Err("env needs a command".into());
        }
        return Ok(None);
    };
    command.validate()?;
    if let Some(cwd) = cwd.filter(|cwd| !std::path::Path::new(cwd).is_dir()) {
        return Err(format!("{cwd}: no such directory"));
    }
    Ok(Some(Spawn { command, env }))
}

/// A status as a process exit code: modulo 256, 1 if unknown.
pub fn exit_code(status: Option<i32>) -> i32 {
    status.map_or(1, |status| (status as u32 % 256) as i32)
}

/// The CLI's wait: the program's exit code, `EXIT_TIMED_OUT`, or 1 if the
/// pane closed without a status. Waits across `btmux upgrade`.
pub async fn wait_cli(profile: Option<&str>, pane: Uuid, timeout: Option<Duration>) -> i32 {
    match wait_remote(profile, pane, timeout).await {
        Ok(code) => code,
        Err(error) => {
            eprintln!("btmux: {error}");
            1
        }
    }
}

async fn wait_remote(
    profile: Option<&str>,
    pane: Uuid,
    timeout: Option<Duration>,
) -> Result<i32, String> {
    let deadline = timeout.map(|timeout| Instant::now() + timeout);
    loop {
        let left = deadline
            .map_or(crate::automation::MAX_WAIT, |deadline| {
                deadline.saturating_duration_since(Instant::now())
            })
            .min(crate::automation::MAX_WAIT);
        let path = format!("/api/panes/{pane}/exit?timeout_ms={}", left.as_millis());
        let (status, reply) =
            crate::popup::request_retrying(profile, reqwest::Method::GET, &path, None).await?;
        match status {
            reqwest::StatusCode::OK => {}
            reqwest::StatusCode::NOT_FOUND => {
                eprintln!("btmux: pane {pane} closed; its exit status is unknown");
                return Ok(1);
            }
            _ => {
                return Err(if reply.is_empty() {
                    status.to_string()
                } else {
                    reply
                })
            }
        }
        #[derive(Deserialize)]
        struct Reply {
            status: Option<i32>,
            timed_out: bool,
        }
        let reply: Reply = serde_json::from_str(&reply).map_err(|e| e.to_string())?;
        if !reply.timed_out {
            if reply.status.is_none() {
                eprintln!("btmux: pane {pane} closed before its program reported a status");
            }
            return Ok(exit_code(reply.status));
        }
        if deadline.is_some_and(|deadline| Instant::now() >= deadline) {
            eprintln!("btmux: timed out waiting for pane {pane}");
            return Ok(EXIT_TIMED_OUT);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{body::Body, http::Request, Router};
    use std::sync::Arc;
    use tower::ServiceExt;

    fn manager() -> (SessionManager, tokio::sync::mpsc::UnboundedReceiver<Uuid>) {
        let (exit_tx, exit_rx) = tokio::sync::mpsc::unbounded_channel();
        let (meta_tx, _) = tokio::sync::mpsc::unbounded_channel();
        let manager = SessionManager::new(
            "/bin/sh".into(),
            crate::config::FileConfig::default(),
            exit_tx,
            meta_tx,
            8004,
            None,
        );
        (manager, exit_rx)
    }

    fn serve(
        manager: SessionManager,
        mut exit_rx: tokio::sync::mpsc::UnboundedReceiver<Uuid>,
    ) -> (AppState, Router) {
        let state: AppState = Arc::new(tokio::sync::RwLock::new(manager));
        let exits = state.clone();
        tokio::spawn(async move {
            while let Some(id) = exit_rx.recv().await {
                exits.write().await.handle_pane_exit(id).await;
            }
        });
        (state.clone(), crate::server::create_app(state))
    }

    async fn app() -> (AppState, Router, Uuid) {
        let (mut manager, exit_rx) = manager();
        let session = manager.create_session(None).await;
        let pane = manager.snapshot_by_id(session).unwrap().windows[0].panes[0].id;
        let (state, app) = serve(manager, exit_rx);
        (state, app, pane)
    }

    async fn call(app: &Router, request: Request<Body>) -> (StatusCode, serde_json::Value) {
        let response = app.clone().oneshot(request).await.unwrap();
        let status = response.status();
        let bytes = axum::body::to_bytes(response.into_body(), 1 << 20)
            .await
            .unwrap();
        let value = serde_json::from_slice(&bytes)
            .unwrap_or_else(|_| String::from_utf8_lossy(&bytes).into());
        (status, value)
    }

    async fn split(
        app: &Router,
        pane: Uuid,
        body: serde_json::Value,
    ) -> (StatusCode, serde_json::Value) {
        let mut body = body;
        body["direction"] = "right".into();
        let request = Request::post(format!("/api/panes/{pane}/split"))
            .header("content-type", "application/json")
            .body(Body::from(body.to_string()))
            .unwrap();
        call(app, request).await
    }

    async fn run(app: &Router, pane: Uuid, body: serde_json::Value) -> Uuid {
        let (status, reply) = split(app, pane, body).await;
        assert_eq!(status, StatusCode::OK, "{reply}");
        Uuid::parse_str(reply["pane_id"].as_str().unwrap()).unwrap()
    }

    async fn wait_for(app: &Router, pane: Uuid) -> (StatusCode, serde_json::Value) {
        let request = Request::get(format!("/api/panes/{pane}/exit?timeout_ms=10000"))
            .body(Body::empty())
            .unwrap();
        call(app, request).await
    }

    #[tokio::test]
    async fn commands_report_their_status() {
        let (state, app, pane) = app().await;
        let new = run(
            &app,
            pane,
            serde_json::json!({"command": {"shell": "exit 3"}}),
        )
        .await;
        let (status, reply) = wait_for(&app, new).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(reply, serde_json::json!({"status": 3, "timed_out": false}));
        // It closes, and its status stays for waiters that come later.
        for _ in 0..500 {
            if state.read().await.find_pane(new).is_none() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert!(state.read().await.find_pane(new).is_none());
        assert_eq!(wait_for(&app, new).await.1["status"], 3);

        let new = run(
            &app,
            pane,
            serde_json::json!({"command": {"exec": ["/bin/sh", "-c", "kill -9 $$"]}}),
        )
        .await;
        assert_eq!(wait_for(&app, new).await.1["status"], 128 + 9);
    }

    #[tokio::test]
    async fn kept_panes_keep_their_output() {
        let (state, app, pane) = app().await;
        let body = serde_json::json!({
            "command": {"shell": "echo hello; exit 5"},
            "keep_open": true,
        });
        let new = run(&app, pane, body).await;
        assert_eq!(wait_for(&app, new).await.1["status"], 5);
        let mut screen = String::new();
        for _ in 0..500 {
            screen = crate::automation::capture_pane(&state, new, 0, None)
                .await
                .unwrap();
            if screen.contains("[exited with status 5]") {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert!(screen.contains("hello"), "{screen}");
        assert!(screen.contains("[exited with status 5]"), "{screen}");
        let mgr = state.read().await;
        let info = mgr
            .pane_infos()
            .into_iter()
            .find(|p| p.pane_id == new)
            .unwrap();
        assert!(info.pane_dead);
        assert_eq!(info.pane_dead_status, Some(5));
    }

    #[tokio::test]
    async fn killed_panes_report_no_status() {
        let (state, app, pane) = app().await;
        let new = run(
            &app,
            pane,
            serde_json::json!({"command": {"shell": "sleep 100"}}),
        )
        .await;
        let waited = tokio::spawn({
            let app = app.clone();
            async move { wait_for(&app, new).await }
        });
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(!waited.is_finished());
        {
            let mut mgr = state.write().await;
            let session = mgr.sessions[0].id;
            mgr.kill_pane(session, new);
        }
        let (status, reply) = waited.await.unwrap();
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            reply,
            serde_json::json!({"status": null, "timed_out": false})
        );
    }

    #[tokio::test]
    async fn programs_that_cannot_start_close_their_pane() {
        let (state, app, pane) = app().await;
        let body = serde_json::json!({"command": {"exec": ["/nonexistent/btmux-run"]}});
        let (status, _) = split(&app, pane, body).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(state.read().await.pane_infos().len(), 1);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn kept_panes_survive_an_upgrade() {
        let (state, app, pane) = app().await;
        let body =
            serde_json::json!({"command": {"shell": "echo hello; exit 6"}, "keep_open": true});
        let kept = run(&app, pane, body).await;
        assert_eq!(wait_for(&app, kept).await.1["status"], 6);
        let closed = run(
            &app,
            pane,
            serde_json::json!({"command": {"shell": "exit 7"}}),
        )
        .await;
        assert_eq!(wait_for(&app, closed).await.1["status"], 7);
        for _ in 0..500 {
            let mgr = state.read().await;
            if mgr.find_pane(closed).is_none()
                && mgr
                    .find_pane(kept)
                    .is_some_and(|pane| pane.pty.journal().len() > 1)
            {
                break;
            }
            drop(mgr);
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        let (sessions, exits) = {
            let mut mgr = state.write().await;
            let (sessions, _, panes, exits) = mgr.export_handoff(Duration::from_secs(2)).unwrap();
            mgr.resume_handoff(panes);
            (sessions, exits)
        };
        assert_eq!(exits.dead.len(), 1);
        let (mut manager, exit_rx) = manager();
        manager.restore_handoff(sessions, Default::default(), Vec::new(), exits);
        let (state, app) = serve(manager, exit_rx);
        assert_eq!(wait_for(&app, kept).await.1["status"], 6);
        assert_eq!(wait_for(&app, closed).await.1["status"], 7);
        let screen = crate::automation::capture_pane(&state, kept, 0, None)
            .await
            .unwrap();
        assert!(screen.contains("hello"), "{screen}");
        assert!(screen.contains("[exited with status 6]"), "{screen}");
    }

    #[test]
    fn statuses_become_exit_codes() {
        assert_eq!(exit_code(Some(0)), 0);
        assert_eq!(exit_code(Some(3)), 3);
        assert_eq!(exit_code(Some(128 + 9)), 137);
        assert_eq!(exit_code(Some(256 + 7)), 7);
        assert_eq!(exit_code(Some(-1)), 255);
        assert_eq!(exit_code(None), 1);
    }

    #[test]
    fn closed_panes_keep_their_status() {
        let mut exits = PaneExits::default();
        let (a, b) = (Uuid::new_v4(), Uuid::new_v4());
        exits.closed(a, Some(4));
        exits.closed(b, None);
        assert_eq!(exits.ended(a), Some(Some(4)));
        assert_eq!(exits.ended(b), Some(None));
        assert_eq!(exits.ended(Uuid::new_v4()), None);

        let mut restored = PaneExits::default();
        restored.restore(exits.export());
        assert_eq!(restored.ended(a), Some(Some(4)));
        assert_eq!(restored.ended(b), Some(None));
    }

    #[test]
    fn keeps_a_bounded_number() {
        let mut exits = PaneExits::default();
        let first = Uuid::new_v4();
        exits.closed(first, Some(0));
        for _ in 0..ENDED_MAX {
            exits.closed(Uuid::new_v4(), Some(0));
        }
        assert_eq!(exits.ended.len(), ENDED_MAX);
        assert_eq!(exits.ended(first), None);
    }
}
