//! Pane programs' exit statuses: `new-window --wait`, `btmux wait-exit`,
//! `GET /api/panes/{id}/exit`, and the `wait_for_exit` MCP tool.
//!
//! A pane in the tree reports its program's status once the program was
//! reaped and, for a kept-open pane, its output ended and its exit was
//! written, so a capture after the wait sees all of it. A closed pane's
//! status is kept for a while, so a waiter that starts late, or reconnects
//! after an upgrade, still gets it; one already waiting always does.

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};
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

/// How long, and how many, closed panes' statuses are kept for waiters
/// that ask after the pane closed.
const ENDED_KEPT: Duration = Duration::from_secs(300);
const ENDED_MAX: usize = 256;

/// `wait-exit` exit status when the wait times out, as for `timeout(1)`.
pub const EXIT_TIMED_OUT: i32 = 124;

/// The longest `wait-exit --timeout`, in seconds: a year.
pub const MAX_TIMEOUT_SECS: u64 = 365 * 24 * 3600;

/// How long a `wait-exit` whose timeout already passed waits for its one poll.
const FIRST_POLL: Duration = Duration::from_secs(5);

/// What a new pane runs, and whether it stays once that exits.
#[derive(Default)]
pub struct Launch {
    /// Run instead of the shell, starting at once.
    pub spawn: Option<Spawn>,
    pub keep_open: bool,
}

/// A new pane's `Launch`, checked before anything changes.
pub fn launch_of(
    command: Option<PopupCommand>,
    env: Option<Vec<(String, String)>>,
    keep_open: bool,
) -> Result<Launch, String> {
    let spawn = match command {
        Some(command) => {
            command.validate()?;
            Some(Spawn { command, env })
        }
        None if env.is_some() => return Err("env needs a command".into()),
        None => None,
    };
    Ok(Launch { spawn, keep_open })
}

struct Ended {
    pane_id: Uuid,
    status: Option<i32>,
    at: Instant,
    /// A waiter was waiting when it closed.
    awaited: bool,
}

impl Ended {
    /// Still awaited by a waiter from before it closed.
    fn held(&self, waited: &HashMap<Uuid, usize>) -> bool {
        self.awaited && waited.contains_key(&self.pane_id)
    }

    /// Recent, or held.
    fn kept(&self, now: Instant, waited: &HashMap<Uuid, usize>) -> bool {
        now.duration_since(self.at) <= ENDED_KEPT || self.held(waited)
    }
}

/// Closed panes' statuses, and a signal for waiters.
pub struct PaneExits {
    ended: VecDeque<Ended>,
    changed: watch::Sender<u64>,
    /// Panes with a waiter, whose entries are never dropped.
    waited: Arc<Mutex<HashMap<Uuid, usize>>>,
}

impl Default for PaneExits {
    fn default() -> Self {
        Self {
            ended: VecDeque::new(),
            changed: watch::channel(0).0,
            waited: Arc::default(),
        }
    }
}

/// Keeps a pane's status for its waiter until dropped.
struct Waiting {
    waited: Arc<Mutex<HashMap<Uuid, usize>>>,
    pane_id: Uuid,
}

impl Drop for Waiting {
    fn drop(&mut self) {
        let mut waited = self.waited.lock().unwrap();
        if let Some(count) = waited.get_mut(&self.pane_id) {
            *count -= 1;
            if *count == 0 {
                waited.remove(&self.pane_id);
            }
        }
    }
}

/// A closed pane's status, for an upgraded server.
#[derive(Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[cfg_attr(not(unix), allow(dead_code))]
pub struct ClosedHandoff {
    pane_id: Uuid,
    status: Option<i32>,
    /// How long ago it closed.
    age_ms: u64,
}

/// Exit statuses, and panes whose programs exited, for an upgraded server.
#[cfg(unix)]
#[derive(Default)]
pub struct ExitsHandoff {
    pub ended: Vec<ClosedHandoff>,
    /// Finished kept panes.
    pub dead: Vec<(Uuid, crate::pty::Exited)>,
    /// Panes whose programs exited, for the next server to close.
    pub closing: Vec<Uuid>,
    /// Kept panes no client started yet.
    pub keep_open: Vec<Uuid>,
}

impl PaneExits {
    /// A pane left the tree; `status` is `None` if its program hadn't
    /// reported one. A status already known for it stays.
    pub fn closed(&mut self, pane_id: Uuid, status: Option<i32>) {
        let now = Instant::now();
        let known = self
            .ended
            .iter()
            .find(|ended| ended.pane_id == pane_id)
            .and_then(|ended| ended.status);
        let waited = self.waited.lock().unwrap();
        self.ended
            .retain(|ended| ended.pane_id != pane_id && ended.kept(now, &waited));
        while self.ended.len() >= ENDED_MAX {
            let Some(oldest) = self.ended.iter().position(|ended| !ended.held(&waited)) else {
                break;
            };
            self.ended.remove(oldest);
        }
        let awaited = waited.contains_key(&pane_id);
        drop(waited);
        self.ended.push_back(Ended {
            pane_id,
            status: status.or(known),
            at: now,
            awaited,
        });
        self.touch();
    }

    /// Wake waiters, as when a kept pane finishes.
    pub fn touch(&self) {
        self.changed.send_modify(|n| *n += 1);
    }

    /// Keep `pane_id`'s status, once it closes, until the guard drops.
    fn wait_for(&self, pane_id: Uuid) -> Waiting {
        *self.waited.lock().unwrap().entry(pane_id).or_default() += 1;
        Waiting {
            waited: self.waited.clone(),
            pane_id,
        }
    }

    fn ended(&self, pane_id: Uuid) -> Option<Option<i32>> {
        let waited = self.waited.lock().unwrap();
        let now = Instant::now();
        self.ended
            .iter()
            .find(|ended| ended.pane_id == pane_id && ended.kept(now, &waited))
            .map(|ended| ended.status)
    }

    #[cfg_attr(not(unix), allow(dead_code))]
    pub fn export(&self) -> Vec<ClosedHandoff> {
        self.ended
            .iter()
            .filter(|ended| ended.at.elapsed() <= ENDED_KEPT)
            .map(|ended| ClosedHandoff {
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

    #[cfg_attr(not(unix), allow(dead_code))]
    pub fn restore(&mut self, ended: Vec<ClosedHandoff>) {
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
                awaited: false,
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
        Some(pane) if pane.pty.finished() => Progress::Exited(pane.pty.exit_status()),
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

/// Waits until the pane's program exits (a kept pane's output with it), or
/// `timeout`.
pub async fn wait(state: &AppState, pane_id: Uuid, timeout: Duration) -> Result<PaneExit, Error> {
    let deadline = tokio::time::Instant::now() + timeout;
    let mut waiting = None;
    loop {
        let mut changed = {
            let mgr = state.read().await;
            waiting.get_or_insert_with(|| mgr.exits.wait_for(pane_id));
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
    description = "Waits until the pane's program (its command, or its shell) exits and returns its status; for a pane kept open, once its output ended and its exit line was written. A pane that closed in the last 5 minutes (at most the last 256 closed) reports the status it closed with; a waiter already waiting always gets it.",
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

/// A launch's error reply: 400 for a bad request, else 422.
pub fn launch_error(error: Error) -> Response {
    let status = match error {
        Error::Invalid(_) => StatusCode::BAD_REQUEST,
        _ => StatusCode::UNPROCESSABLE_ENTITY,
    };
    (status, error.to_string()).into_response()
}

/// A status as a process exit code: modulo 256, 1 if unknown.
pub fn exit_code(status: Option<i32>) -> i32 {
    status.map_or(1, |status| (status as u32 % 256) as i32)
}

/// The CLI's wait: the program's exit code, `EXIT_TIMED_OUT`, or 1 if the
/// pane closed without a status. Waits across `btmux upgrade`.
pub async fn wait_cli(profile: Option<&str>, pane: Uuid, timeout: Option<Duration>) -> i32 {
    let deadline = match timeout {
        Some(timeout) => match tokio::time::Instant::now().checked_add(timeout) {
            Some(deadline) => Some(deadline),
            None => {
                eprintln!("btmux: the timeout is too long");
                return 1;
            }
        },
        None => None,
    };
    let waited = wait_remote(profile, pane, deadline);
    let result = match deadline {
        // Requests, replies, and retries all count, but the first poll gets
        // its reply, so a short timeout still reports an exit.
        Some(deadline) => {
            let cutoff = deadline.max(tokio::time::Instant::now() + FIRST_POLL);
            tokio::time::timeout_at(cutoff, waited)
                .await
                .unwrap_or(Ok(None))
        }
        None => waited.await,
    };
    match result {
        Ok(Some(code)) => code,
        Ok(None) => {
            eprintln!("btmux: timed out waiting for pane {pane}");
            EXIT_TIMED_OUT
        }
        Err(error) => {
            eprintln!("btmux: {error}");
            1
        }
    }
}

/// The exit code, or `None` once `deadline` passed.
async fn wait_remote(
    profile: Option<&str>,
    pane: Uuid,
    deadline: Option<tokio::time::Instant>,
) -> Result<Option<i32>, String> {
    let mut first = true;
    loop {
        let left = deadline
            .map_or(crate::automation::MAX_WAIT, |deadline| {
                deadline.saturating_duration_since(tokio::time::Instant::now())
            })
            .min(crate::automation::MAX_WAIT);
        if left.is_zero() && !first {
            return Ok(None);
        }
        first = false;
        let path = format!("/api/panes/{pane}/exit?timeout_ms={}", left.as_millis());
        // Each poll is bounded, so a server that stops answering is retried.
        let (status, reply) = crate::popup::request_retrying(
            profile,
            reqwest::Method::GET,
            &path,
            None,
            left + Duration::from_secs(30),
        )
        .await?;
        match status {
            reqwest::StatusCode::OK => {}
            reqwest::StatusCode::NOT_FOUND => {
                eprintln!("btmux: no pane {pane}, or it closed too long ago");
                return Ok(Some(1));
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
            return Ok(Some(exit_code(reply.status)));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        // Closing it again without a status keeps the known one.
        exits.closed(a, None);
        assert_eq!(exits.ended(a), Some(Some(4)));

        let mut restored = PaneExits::default();
        restored.restore(exits.export());
        assert_eq!(restored.ended(a), Some(Some(4)));
        assert_eq!(restored.ended(b), Some(None));
    }

    #[test]
    fn keeps_a_bounded_number_but_never_a_waited_one() {
        let mut exits = PaneExits::default();
        let (first, waited) = (Uuid::new_v4(), Uuid::new_v4());
        let waiting = exits.wait_for(waited);
        exits.closed(first, Some(0));
        exits.closed(waited, Some(5));
        for _ in 0..ENDED_MAX {
            exits.closed(Uuid::new_v4(), Some(0));
        }
        assert_eq!(exits.ended.len(), ENDED_MAX);
        assert_eq!(exits.ended(first), None);
        assert_eq!(exits.ended(waited), Some(Some(5)));
        drop(waiting);
        exits.closed(Uuid::new_v4(), Some(0));
        assert_eq!(exits.ended(waited), None);

        // Only a waiter from before the close outlasts the retention time.
        let mut exits = PaneExits::default();
        let (early, late) = (Uuid::new_v4(), Uuid::new_v4());
        let _early = exits.wait_for(early);
        exits.closed(early, Some(1));
        exits.closed(late, Some(2));
        let _late = exits.wait_for(late);
        for ended in &mut exits.ended {
            ended.at -= ENDED_KEPT * 2;
        }
        assert_eq!(exits.ended(early), Some(Some(1)));
        assert_eq!(exits.ended(late), None);
    }

    #[test]
    fn launches_need_a_command_for_env() {
        assert!(launch_of(None, None, true).unwrap().keep_open);
        assert!(launch_of(None, Some(Vec::new()), false).is_err());
        assert!(launch_of(Some(PopupCommand::Shell(" ".into())), None, false).is_err());
        let launch = launch_of(Some(PopupCommand::Shell("true".into())), None, false).unwrap();
        assert!(launch.spawn.is_some());
    }
}

/// Real programs in PTYs, through the REST routes.
#[cfg(all(test, unix))]
mod runs {
    use super::*;
    use axum::{body::Body, http::Request, Router};
    use serde_json::json;
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

    fn post(path: String, body: serde_json::Value) -> Request<Body> {
        Request::post(path)
            .header("content-type", "application/json")
            .body(Body::from(body.to_string()))
            .unwrap()
    }

    async fn split(
        app: &Router,
        pane: Uuid,
        mut body: serde_json::Value,
    ) -> (StatusCode, serde_json::Value) {
        body["direction"] = "right".into();
        call(app, post(format!("/api/panes/{pane}/split"), body)).await
    }

    async fn run(app: &Router, pane: Uuid, body: serde_json::Value) -> Uuid {
        let (status, reply) = split(app, pane, body).await;
        assert_eq!(status, StatusCode::OK, "{reply}");
        assert_eq!(reply["pane"]["pane_id"], reply["pane_id"]);
        Uuid::parse_str(reply["pane_id"].as_str().unwrap()).unwrap()
    }

    async fn wait_for(app: &Router, pane: Uuid) -> (StatusCode, serde_json::Value) {
        let request = Request::get(format!("/api/panes/{pane}/exit?timeout_ms=10000"))
            .body(Body::empty())
            .unwrap();
        call(app, request).await
    }

    async fn screen(state: &AppState, pane: Uuid) -> String {
        crate::automation::capture_pane(state, pane, 0, None)
            .await
            .unwrap()
    }

    async fn until(mut done: impl AsyncFnMut() -> bool) {
        for _ in 0..500 {
            if done().await {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("timed out");
    }

    #[tokio::test]
    async fn commands_report_their_status() {
        let (state, app, pane) = app().await;
        let new = run(&app, pane, json!({"command": {"shell": "exit 3"}})).await;
        let (status, reply) = wait_for(&app, new).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(reply, json!({"status": 3, "timed_out": false}));
        // It closes, and its status stays for waiters that come later.
        until(async || state.read().await.find_pane(new).is_none()).await;
        assert_eq!(wait_for(&app, new).await.1["status"], 3);

        let body = json!({"command": {"exec": ["/bin/sh", "-c", "kill -9 $$"]}});
        let new = run(&app, pane, body).await;
        assert_eq!(wait_for(&app, new).await.1["status"], 128 + 9);
    }

    #[tokio::test]
    async fn kept_panes_finish_with_their_output() {
        let (state, app, pane) = app().await;
        let body = json!({"command": {"shell": "echo hello; exit 5"}, "keep_open": true});
        let new = run(&app, pane, body).await;
        assert_eq!(wait_for(&app, new).await.1["status"], 5);
        // A capture right after the wait has all of it.
        let shown = screen(&state, new).await;
        assert!(shown.contains("hello"), "{shown}");
        assert!(shown.contains("[exited with status 5]"), "{shown}");
        let mgr = state.read().await;
        let info = mgr.pane_info(new).unwrap();
        assert!(info.pane_dead);
        assert_eq!(info.pane_dead_status, Some(5));
        // Its terminal and writer are gone; input says why.
        let pty = &mgr.find_pane(new).unwrap().pty;
        assert!(!pty.has_terminal());
        let error = pty.input_tx.send(b"x".to_vec()).unwrap_err();
        assert_eq!(error, "the pane's program has exited");
        drop(mgr);
        let error = crate::automation::send_keys(&state, new, "x", &[] as &[&str], None).await;
        assert!(matches!(error, Err(Error::Conflict(_))), "{error:?}");
        state.write().await.kill_pane_by_id(new).unwrap();
    }

    #[tokio::test]
    async fn kept_panes_wait_for_output_after_the_exit() {
        let (state, app, pane) = app().await;
        // The reader is still behind when the program is reaped.
        let body = json!({
            "command": {"exec": ["/bin/sh", "-c", "seq 1 30000; exit 3"]},
            "keep_open": true,
        });
        let new = run(&app, pane, body).await;
        assert_eq!(wait_for(&app, new).await.1["status"], 3);
        let shown = screen(&state, new).await;
        let last = shown.find("30000").expect(&shown);
        let exited = shown.find("[exited with status 3]").expect(&shown);
        assert!(last < exited, "{shown}");

        // Something holding the terminal open doesn't hold the wait.
        let body = json!({
            "command": {"exec": ["/bin/sh", "-c", "(trap '' HUP; sleep 3) & exit 4"]},
            "keep_open": true,
        });
        let new = run(&app, pane, body).await;
        let started = Instant::now();
        assert_eq!(wait_for(&app, new).await.1["status"], 4);
        assert!(started.elapsed() < Duration::from_secs(2));
        assert!(screen(&state, new).await.contains("[exited with status 4]"));
    }

    #[tokio::test]
    async fn killed_panes_report_no_status() {
        let (state, app, pane) = app().await;
        let new = run(&app, pane, json!({"command": {"shell": "sleep 100"}})).await;
        let waited = tokio::spawn({
            let app = app.clone();
            async move { wait_for(&app, new).await }
        });
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(!waited.is_finished());
        state.write().await.kill_pane_by_id(new).unwrap();
        let (status, reply) = waited.await.unwrap();
        assert_eq!(status, StatusCode::OK);
        assert_eq!(reply, json!({"status": null, "timed_out": false}));
    }

    #[tokio::test]
    async fn failed_launches_change_nothing() {
        let (state, app, pane) = app().await;
        let missing = json!({"command": {"exec": ["/nonexistent/btmux-run", "x"]}});
        let (session, first) = {
            let mut mgr = state.write().await;
            let session = mgr.sessions[0].id;
            mgr.create_window(session).await;
            mgr.create_window(session).await;
            mgr.switch_window(session, 0);
            let window = &mut mgr.sessions[0].windows[0];
            window.zoomed_pane = Some(pane);
            window.last_preset = Some(crate::session::layout::LayoutPreset::ALL[0]);
            (session, window.id)
        };
        let (status, _) = call(
            &app,
            post(format!("/api/sessions/{session}/windows"), missing.clone()),
        )
        .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        let (status, _) = split(&app, pane, missing).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        let body = json!({"command": {"shell": "true"}, "cwd": "/nonexistent/btmux"});
        assert_eq!(split(&app, pane, body).await.0, StatusCode::BAD_REQUEST);

        let mgr = state.read().await;
        let session = &mgr.sessions[0];
        assert_eq!(session.windows.len(), 3);
        assert_eq!(session.active_window, 0);
        let window = &session.windows[0];
        assert_eq!(window.id, first);
        assert_eq!(window.panes.len(), 1);
        assert_eq!(window.zoomed_pane, Some(pane));
        assert!(window.last_preset.is_some());
    }

    #[tokio::test]
    async fn kept_panes_survive_an_upgrade() {
        let (state, app, pane) = app().await;
        let remote = r"printf '\033]7;file://remote.invalid/tmp\007'; echo hello; exit 6";
        let body = json!({"command": {"shell": remote}, "keep_open": true});
        let kept = run(&app, pane, body).await;
        assert_eq!(wait_for(&app, kept).await.1["status"], 6);
        let closed = run(&app, pane, json!({"command": {"shell": "exit 7"}})).await;
        assert_eq!(wait_for(&app, closed).await.1["status"], 7);
        until(async || state.read().await.find_pane(closed).is_none()).await;
        let (sessions, exits) = {
            let mut mgr = state.write().await;
            let (sessions, _, panes, exits) = mgr.export_handoff(Duration::from_secs(2)).unwrap();
            mgr.resume_handoff(panes);
            (sessions, exits)
        };
        assert_eq!(exits.dead.len(), 1);
        let (mut manager, exit_rx) = manager();
        manager
            .restore_handoff(sessions, Default::default(), Vec::new(), exits)
            .await;
        let (state, app) = serve(manager, exit_rx);
        assert_eq!(wait_for(&app, kept).await.1["status"], 6);
        assert_eq!(wait_for(&app, closed).await.1["status"], 7);
        let shown = screen(&state, kept).await;
        assert!(shown.contains("hello"), "{shown}");
        assert!(shown.contains("[exited with status 6]"), "{shown}");
        let mgr = state.read().await;
        let snapshot = mgr.all_snapshots();
        let restored = snapshot[0].windows[0]
            .panes
            .iter()
            .find(|p| p.id == kept)
            .unwrap();
        assert_eq!(restored.cwd.as_deref(), Some("/tmp"));
        assert!(restored.cwd_remote);
    }

    /// A program that closes its terminal but keeps running closes its pane
    /// after the grace, as before; a wait then reports no status.
    #[tokio::test]
    async fn detached_programs_close_their_pane() {
        let (state, app, pane) = app().await;
        let new = run(&app, pane, json!({"command": {"shell": "sleep 5"}})).await;
        {
            let mut mgr = state.write().await;
            mgr.find_pane(new).unwrap().pty.end_output();
            mgr.handle_pane_exit(new).await;
            assert!(mgr.find_pane(new).is_some(), "the waiter may still report");
        }
        until(async || state.read().await.find_pane(new).is_none()).await;
        assert_eq!(
            wait_for(&app, new).await.1,
            json!({"status": null, "timed_out": false})
        );
    }

    /// The same, with a program that really does it: Linux ends the output
    /// once no process has the terminal open; macOS keeps a session's.
    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn programs_closing_their_terminal_close_their_pane() {
        let (state, app, pane) = app().await;
        let detach = "exec </dev/null >/dev/null 2>&1; sleep 5";
        let body = json!({"command": {"exec": ["/bin/sh", "-c", detach]}});
        let new = run(&app, pane, body).await;
        until(async || state.read().await.find_pane(new).is_none()).await;
        assert_eq!(wait_for(&app, new).await.1["status"], json!(null));
    }

    #[tokio::test]
    async fn unstarted_kept_panes_stay_kept_across_an_upgrade() {
        let (state, app, pane) = app().await;
        let kept = run(&app, pane, json!({"keep_open": true})).await;
        let (sessions, exits) = {
            let mut mgr = state.write().await;
            assert!(!mgr.find_pane(kept).unwrap().pty.is_spawned());
            let (sessions, _, panes, exits) = mgr.export_handoff(Duration::from_secs(2)).unwrap();
            mgr.resume_handoff(panes);
            (sessions, exits)
        };
        let (mut manager, _exit_rx) = manager();
        manager
            .restore_handoff(sessions, Default::default(), Vec::new(), exits)
            .await;
        assert!(manager.find_pane(kept).unwrap().pty.keep_open());
    }

    /// Programs reaped while the upgrade holds the lock, before their exits
    /// are handled, end as a closed result and a finished kept pane.
    #[tokio::test]
    async fn exits_queued_during_an_upgrade_keep_their_status() {
        let (state, app, pane) = app().await;
        let command = json!({"shell": "echo out; exit 53"});
        let closed = run(&app, pane, json!({"command": command})).await;
        let kept = run(&app, pane, json!({"command": command, "keep_open": true})).await;
        let (sessions, exits) = {
            let mut mgr = state.write().await;
            for _ in 0..500 {
                let reaped = [closed, kept]
                    .iter()
                    .all(|id| mgr.find_pane(*id).is_some_and(|p| !p.pty.child_alive()));
                if reaped {
                    break;
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            let (sessions, _, panes, exits) = mgr.export_handoff(Duration::from_secs(2)).unwrap();
            mgr.resume_handoff(panes);
            (sessions, exits)
        };
        assert_eq!(exits.closing, [closed]);
        let (mut manager, exit_rx) = manager();
        manager
            .restore_handoff(sessions, Default::default(), Vec::new(), exits)
            .await;
        assert!(manager.find_pane(closed).is_none());
        let (state, app) = serve(manager, exit_rx);
        assert_eq!(
            wait_for(&app, closed).await.1,
            json!({"status": 53, "timed_out": false})
        );
        assert_eq!(wait_for(&app, kept).await.1["status"], 53);
        let shown = screen(&state, kept).await;
        assert!(shown.contains("out"), "{shown}");
        assert!(shown.contains("[exited with status 53]"), "{shown}");
    }
}
