//! `btmux popup`: a program in a terminal over its pane, closed when it exits.
//!
//! A popup is a PTY outside the layout tree, owned by the pane it opened over
//! and closed with it. Tabs showing that pane's window show it in a modal and
//! attach to it through `/ws/pane/{id}`, which spawns it lazily as for panes.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde::{Deserialize, Serialize};
use tokio::sync::watch;
use uuid::Uuid;

use crate::pty::PtyHandle;
use crate::session::manager::SessionManager;
use crate::ws::control::ServerMessage;
use crate::AppState;

/// Request bodies carry the caller's environment.
pub const MAX_BYTES: usize = 1024 * 1024;

/// How long tabs have to start a popup.
#[cfg(not(test))]
const SHOW_TIMEOUT: Duration = Duration::from_secs(3);
#[cfg(test)]
const SHOW_TIMEOUT: Duration = Duration::from_millis(100);

/// How long a program has to exit after a hangup before its popup is dropped.
const CLOSE_GRACE: Duration = Duration::from_secs(3);

/// How long, and how many, ended popups' statuses are kept for waiters that
/// reconnect, as after an upgrade.
const ENDED_KEPT: Duration = Duration::from_secs(300);
const ENDED_MAX: usize = 64;

const TITLE_MAX: usize = 200;

/// The request waits for the program, however long that takes.
const NO_TIMEOUT: Duration = Duration::from_secs(10 * 365 * 24 * 3600);

const EXIT_FAILED: i32 = 2;
const EXIT_NOT_FOUND: i32 = 127;

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
#[cfg_attr(test, derive(utoipa::ToSchema))]
pub enum PopupCommand {
    /// A program and its arguments.
    Exec(Vec<String>),
    /// A command line for the pane's shell.
    Shell(String),
}

impl PopupCommand {
    fn validate(&self) -> Result<(), String> {
        let empty = match self {
            Self::Exec(argv) => argv.first().is_none_or(String::is_empty),
            Self::Shell(command) => command.trim().is_empty(),
        };
        if empty {
            return Err("no command".into());
        }
        Ok(())
    }

    fn title(&self) -> String {
        match self {
            Self::Exec(argv) => argv.join(" "),
            Self::Shell(command) => command.clone(),
        }
    }

    /// The host program and arguments, through `shell` for a command line.
    /// Without the caller's environment, an interactive shell reads the
    /// startup files panes do, for their `PATH`.
    pub fn argv(&self, shell: &str, interactive: bool) -> Vec<String> {
        use crate::shell::Dialect;
        let command = match self {
            Self::Exec(argv) => return argv.clone(),
            Self::Shell(command) => command.clone(),
        };
        let mut argv = crate::shell::argv(shell);
        let flags: &[&str] = match Dialect::of(shell) {
            Dialect::Posix if interactive => &["-i", "-c"],
            Dialect::Posix => &["-c"],
            Dialect::Wsl => &["-e", "sh", "-lc"],
            Dialect::PowerShell => &["-NoLogo", "-Command"],
            Dialect::Cmd => &["/C"],
        };
        argv.extend(flags.iter().map(|flag| flag.to_string()));
        argv.push(command);
        argv
    }

    /// `btmux guest-shell` arguments that run it instead of the guest's shell.
    pub fn guest_args(&self) -> Vec<String> {
        match self {
            Self::Exec(argv) => std::iter::once("--".to_string())
                .chain(argv.iter().cloned())
                .collect(),
            Self::Shell(command) => vec![format!("--run={command}")],
        }
    }
}

/// What a popup's PTY runs.
#[derive(Serialize, Deserialize, Clone)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct Spawn {
    pub command: PopupCommand,
    /// The caller's environment; `None` keeps the server's.
    pub env: Option<Vec<(String, String)>>,
}

#[derive(Serialize, Deserialize, Clone)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct PopupSummary {
    pub id: Uuid,
    pub pane_id: Uuid,
    pub title: String,
}

pub struct Popup {
    pub id: Uuid,
    pub pane_id: Uuid,
    pub title: String,
    pub pty: PtyHandle,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub enum Outcome {
    /// The exit status, if known.
    Exited(Option<i32>),
    /// The program never started.
    Failed(String),
    /// No tab started it, so it never ran.
    NotShown,
}

#[derive(Debug, PartialEq)]
enum Progress {
    Starting,
    Running,
    Ended(Outcome),
    Unknown,
}

struct Ended {
    id: Uuid,
    pane_id: Uuid,
    outcome: Outcome,
    at: Instant,
}

/// Popups, for an upgraded server.
#[cfg(unix)]
#[derive(Serialize, Deserialize, Default)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct PopupsHandoff {
    /// Started popups, whose PTYs are among the panes.
    pub open: Vec<PopupSummary>,
    /// Popups no tab has started yet.
    pub starting: Vec<StartingHandoff>,
    /// Ended popups' outcomes, for waiters that reconnect.
    pub ended: Vec<EndedHandoff>,
}

#[derive(Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct StartingHandoff {
    pub popup: PopupSummary,
    pub spawn: Spawn,
    pub cwd: Option<std::path::PathBuf>,
}

#[derive(Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct EndedHandoff {
    id: Uuid,
    pane_id: Uuid,
    outcome: Outcome,
    /// How long ago it ended.
    age_ms: u64,
}

/// Open popups, and how recently ended ones ended.
pub struct Popups {
    open: Vec<Popup>,
    ended: VecDeque<Ended>,
    /// Bumped on every change, for requests waiting on one popup.
    changed: watch::Sender<u64>,
}

impl Default for Popups {
    fn default() -> Self {
        Self {
            open: Vec::new(),
            ended: VecDeque::new(),
            changed: watch::channel(0).0,
        }
    }
}

impl Popups {
    pub fn summaries(&self) -> Vec<PopupSummary> {
        self.open
            .iter()
            .map(|popup| PopupSummary {
                id: popup.id,
                pane_id: popup.pane_id,
                title: popup.title.clone(),
            })
            .collect()
    }

    pub fn iter(&self) -> impl Iterator<Item = &Popup> {
        self.open.iter()
    }

    pub fn iter_mut(&mut self) -> impl Iterator<Item = &mut Popup> {
        self.open.iter_mut()
    }

    pub fn get_mut(&mut self, id: Uuid) -> Option<&mut Popup> {
        self.open.iter_mut().find(|popup| popup.id == id)
    }

    pub fn contains(&self, id: Uuid) -> bool {
        self.open.iter().any(|popup| popup.id == id)
    }

    pub fn insert(&mut self, popup: Popup) {
        self.open.push(popup);
        self.touch();
    }

    /// Wake waiters, as when a viewer starts a popup.
    pub fn touch(&self) {
        self.changed.send_modify(|n| *n += 1);
    }

    /// The pane an open or recently ended popup belongs to.
    fn owner(&self, id: Uuid) -> Option<Uuid> {
        self.open
            .iter()
            .find(|popup| popup.id == id)
            .map(|popup| popup.pane_id)
            .or_else(|| {
                self.ended
                    .iter()
                    .find(|ended| ended.id == id)
                    .map(|ended| ended.pane_id)
            })
    }

    fn progress(&self, id: Uuid, pane_id: Uuid) -> Progress {
        if let Some(popup) = self
            .open
            .iter()
            .find(|p| p.id == id && p.pane_id == pane_id)
        {
            return if popup.pty.is_spawned() {
                Progress::Running
            } else {
                Progress::Starting
            };
        }
        self.ended
            .iter()
            .find(|ended| ended.id == id && ended.pane_id == pane_id)
            .map_or(Progress::Unknown, |ended| {
                Progress::Ended(ended.outcome.clone())
            })
    }

    /// Remove a popup, dropping its PTY, which hangs up what still runs.
    fn end(&mut self, id: Uuid, outcome: Outcome) -> bool {
        let Some(index) = self.open.iter().position(|popup| popup.id == id) else {
            return false;
        };
        let popup = self.open.remove(index);
        crate::capture::popup_ended(id);
        let now = Instant::now();
        while self
            .ended
            .front()
            .is_some_and(|ended| now.duration_since(ended.at) > ENDED_KEPT)
            || self.ended.len() >= ENDED_MAX
        {
            self.ended.pop_front();
        }
        self.ended.push_back(Ended {
            id,
            pane_id: popup.pane_id,
            outcome,
            at: now,
        });
        self.touch();
        true
    }

    /// Ended outcomes still kept, for an upgraded server.
    pub fn export_ended(&self) -> Vec<EndedHandoff> {
        self.ended
            .iter()
            .filter(|ended| ended.at.elapsed() <= ENDED_KEPT)
            .map(|ended| EndedHandoff {
                id: ended.id,
                pane_id: ended.pane_id,
                outcome: ended.outcome.clone(),
                age_ms: ended
                    .at
                    .elapsed()
                    .as_millis()
                    .try_into()
                    .unwrap_or(u64::MAX),
            })
            .collect()
    }

    /// Keep an old server's ended outcomes for the rest of their time.
    pub fn restore_ended(&mut self, ended: Vec<EndedHandoff>) {
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
                id: ended.id,
                pane_id: ended.pane_id,
                outcome: ended.outcome,
                at,
            });
        }
    }

    /// The PTY's reader or waiter reported its end: `false` if it's no popup.
    /// A reader's EOF while the program runs is ignored: its waiter reports
    /// once it is reaped, with its status.
    pub fn exited(&mut self, id: Uuid) -> Option<bool> {
        let popup = self.open.iter().find(|popup| popup.id == id)?;
        if popup.pty.child_alive() {
            return Some(false);
        }
        let status = popup.pty.exit_status();
        Some(self.end(id, Outcome::Exited(status)))
    }

    /// Its program could not start.
    pub fn failed(&mut self, id: Uuid, error: String) -> bool {
        self.end(id, Outcome::Failed(error))
    }

    /// Popups no tab has started yet, for an upgraded server.
    pub fn export_starting(&self) -> Vec<StartingHandoff> {
        self.open
            .iter()
            .filter_map(|popup| {
                let (spawn, cwd) = popup.pty.popup_spawn()?;
                Some(StartingHandoff {
                    popup: PopupSummary {
                        id: popup.id,
                        pane_id: popup.pane_id,
                        title: popup.title.clone(),
                    },
                    spawn: spawn.clone(),
                    cwd: cwd.map(Into::into),
                })
            })
            .collect()
    }

    /// Close the popups open over a pane that closed.
    pub fn close_pane(&mut self, pane_id: Uuid) -> bool {
        let ids: Vec<_> = self
            .open
            .iter()
            .filter(|popup| popup.pane_id == pane_id)
            .map(|popup| popup.id)
            .collect();
        for &id in &ids {
            self.end(id, Outcome::Exited(None));
        }
        !ids.is_empty()
    }

    pub fn close_all(&mut self) -> bool {
        let ids: Vec<_> = self.open.iter().map(|popup| popup.id).collect();
        for &id in &ids {
            self.end(id, Outcome::Exited(None));
        }
        !ids.is_empty()
    }
}

pub fn broadcast(mgr: &SessionManager) {
    let msg = ServerMessage::Popups {
        popups: mgr.popups.summaries(),
    };
    let _ = mgr.events().send(serde_json::to_string(&msg).unwrap());
}

#[derive(Debug)]
pub enum OpenError {
    NoPane,
    AlreadyOpen,
    Invalid(String),
}

impl OpenError {
    fn into_response(self) -> Response {
        match self {
            Self::NoPane => (StatusCode::NOT_FOUND, "no such pane").into_response(),
            Self::AlreadyOpen => {
                (StatusCode::LOCKED, "this pane already has a popup open").into_response()
            }
            Self::Invalid(error) => (StatusCode::BAD_REQUEST, error).into_response(),
        }
    }
}

impl std::fmt::Display for OpenError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoPane => f.write_str("no such pane"),
            Self::AlreadyOpen => f.write_str("this pane already has a popup open"),
            Self::Invalid(error) => f.write_str(error),
        }
    }
}

/// Where a popup over a pane runs, as its PTY is made.
#[derive(Clone, Debug, PartialEq)]
pub struct Target {
    /// How its shell reads command lines and paths.
    pub dialect: crate::shell::Dialect,
    /// Its guest instance; `None` on the host.
    pub guest: Option<String>,
}

/// Open a popup over `pane_id`, in `cwd` or else the pane's directory, and
/// tell tabs. A guest pane's popup runs in its guest, without `env`.
pub fn open(
    mgr: &mut SessionManager,
    pane_id: Uuid,
    id: Uuid,
    mut spawn: Spawn,
    cwd: Option<String>,
    title: Option<String>,
) -> Result<Uuid, OpenError> {
    spawn.command.validate().map_err(OpenError::Invalid)?;
    if mgr.find_pane(id).is_some() || mgr.popups.owner(id).is_some() {
        return Err(OpenError::Invalid("this popup id is taken".into()));
    }
    let pane = mgr.find_pane(pane_id).ok_or(OpenError::NoPane)?;
    if mgr.popups.iter().any(|popup| popup.pane_id == pane_id) {
        return Err(OpenError::AlreadyOpen);
    }
    let guest = pane.pty.is_guest();
    if guest {
        spawn.env = None;
    }
    // A guest's directories are checked against its mounts when it starts.
    let cwd = match cwd {
        Some(cwd) if guest || std::path::Path::new(&cwd).is_dir() => Some(cwd),
        Some(cwd) => return Err(OpenError::Invalid(format!("{cwd}: no such directory"))),
        None => pane
            .pty
            .local_cwd()
            .filter(|cwd| std::path::Path::new(cwd).is_dir()),
    };
    let mut title = title
        .filter(|title| !title.trim().is_empty())
        .unwrap_or_else(|| spawn.command.title());
    if let Some((end, _)) = title.char_indices().nth(TITLE_MAX) {
        title.truncate(end);
    }
    let pty = mgr.new_popup_pty(id, pane_id, cwd.map(Into::into), Some(spawn));
    mgr.popups.insert(Popup {
        id,
        pane_id,
        title,
        pty,
    });
    broadcast(mgr);
    Ok(id)
}

/// Hang up a popup's program. One that keeps running is dropped after a grace period.
pub async fn close(state: &AppState, id: Uuid) {
    {
        let mgr = state.read().await;
        let Some(popup) = mgr.popups.iter().find(|popup| popup.id == id) else {
            return;
        };
        if !popup.pty.is_spawned() {
            drop(mgr);
            let mut mgr = state.write().await;
            if mgr.popups.end(id, Outcome::Exited(None)) {
                broadcast(&mgr);
            }
            return;
        }
        popup.pty.terminate();
    }
    let state = state.clone();
    tokio::spawn(async move {
        tokio::time::sleep(CLOSE_GRACE).await;
        let mut mgr = state.write().await;
        if mgr.popups.end(id, Outcome::Exited(None)) {
            broadcast(&mgr);
        }
    });
}

#[derive(Serialize)]
#[cfg_attr(test, derive(utoipa::ToSchema))]
pub struct PopupOpened {
    pub id: Uuid,
}

#[derive(Serialize)]
#[cfg_attr(test, derive(utoipa::ToSchema))]
pub struct PopupExited {
    /// The program's exit status, if known.
    pub status: Option<i32>,
}

#[derive(Deserialize, Serialize)]
#[cfg_attr(test, derive(utoipa::ToSchema))]
pub struct PopupRequest {
    /// Chosen by the caller, so a request retried across an upgrade finds
    /// its popup rather than running the program again.
    #[serde(default)]
    pub id: Option<Uuid>,
    pub command: PopupCommand,
    #[serde(default)]
    pub cwd: Option<String>,
    // Replaces the server's environment; see `Environment`.
    #[serde(default)]
    #[cfg_attr(test, schema(value_type = Environment, inline))]
    pub env: Option<Vec<(String, String)>>,
    #[serde(default)]
    pub title: Option<String>,
}

/// `PopupRequest::env`: utoipa's tuple schema leaves out `minItems`.
#[cfg(test)]
struct Environment;

#[cfg(test)]
impl utoipa::PartialSchema for Environment {
    fn schema() -> utoipa::openapi::RefOr<utoipa::openapi::schema::Schema> {
        serde_json::from_value(serde_json::json!({
            "type": ["array", "null"],
            "description": "Replaces the server's environment.",
            "items": {
                "type": "array",
                "prefixItems": [
                    {"type": "string", "description": "Name"},
                    {"type": "string", "description": "Value"},
                ],
                "items": false,
                "minItems": 2,
            },
            "examples": [[["NAME", "value"]]],
        }))
        .unwrap()
    }
}

#[cfg(test)]
impl utoipa::ToSchema for Environment {}

/// Opens a popup and replies `{"id": ...}` once a tab started it: 409 if no
/// tab does, 422 if its program can't start.
#[cfg_attr(test, utoipa::path(
    post,
    path = "/api/panes/{pane_id}/popup",
    tag = "Popups",
    operation_id = "open-popup",
    summary = "Open a popup",
    description = "Runs a program in a popup over the pane. Replies once a client started it; then wait for its exit status.",
    params(
        ("pane_id", description = "Pane ID"),
    ),
    responses(
        (status = 200, description = "Started in a client", body = PopupOpened),
        (status = 400, description = "An empty command, a `cwd` that isn't a directory, or an `id` already in use", body = String, content_type = "text/plain"),
        (status = 404, description = "No such pane", body = String, content_type = "text/plain"),
        (status = 409, description = "No client is showing the pane", body = String, content_type = "text/plain"),
        (status = 410, description = "The popup closed before it started", body = String, content_type = "text/plain"),
        (status = 422, description = "The program can't start", body = String, content_type = "text/plain"),
        (status = 423, description = "The pane already has a popup open", body = String, content_type = "text/plain"),
    ),
))]
pub async fn api_popup(
    State(state): State<AppState>,
    Path(pane_id): Path<Uuid>,
    Json(request): Json<PopupRequest>,
) -> Response {
    let PopupRequest {
        id,
        command,
        cwd,
        env,
        title,
    } = request;
    let (id, mut changed) = {
        let mut mgr = state.write().await;
        let retried = id.filter(|&id| match mgr.popups.progress(id, pane_id) {
            // It never ran, as when an upgrade cut the request: open it again.
            Progress::Ended(Outcome::NotShown) => {
                mgr.popups.ended.retain(|ended| ended.id != id);
                false
            }
            Progress::Unknown => false,
            _ => true,
        });
        match retried {
            Some(id) => (id, mgr.popups.changed.subscribe()),
            None => {
                let id = id.unwrap_or_else(Uuid::new_v4);
                match open(&mut mgr, pane_id, id, Spawn { command, env }, cwd, title) {
                    Ok(id) => (id, mgr.popups.changed.subscribe()),
                    Err(error) => return error.into_response(),
                }
            }
        }
    };
    let started = tokio::time::timeout(SHOW_TIMEOUT, async {
        loop {
            let progress = state.read().await.popups.progress(id, pane_id);
            if progress != Progress::Starting {
                return progress;
            }
            if changed.changed().await.is_err() {
                return Progress::Unknown;
            }
        }
    })
    .await;
    let progress = match started {
        Ok(progress) => progress,
        // A tab may have started it just now: only one still starting runs here.
        Err(_) => {
            let mut mgr = state.write().await;
            let progress = mgr.popups.progress(id, pane_id);
            if progress == Progress::Starting {
                mgr.popups.end(id, Outcome::NotShown);
                broadcast(&mgr);
            }
            progress
        }
    };
    match progress {
        Progress::Running | Progress::Ended(Outcome::Exited(_)) => {
            Json(PopupOpened { id }).into_response()
        }
        Progress::Ended(Outcome::Failed(error)) => {
            (StatusCode::UNPROCESSABLE_ENTITY, error).into_response()
        }
        Progress::Starting | Progress::Ended(Outcome::NotShown) => {
            (StatusCode::CONFLICT, "no browser tab is showing this pane").into_response()
        }
        Progress::Unknown => (StatusCode::GONE, "the popup closed").into_response(),
    }
}

/// Replies `{"status": ...}` once the popup's program exits.
#[cfg_attr(test, utoipa::path(
    get,
    path = "/api/panes/{pane_id}/popup/{id}",
    tag = "Popups",
    operation_id = "wait-for-popup",
    summary = "Wait for a popup",
    params(
        ("pane_id", description = "Pane ID"),
        ("id", description = "Popup ID"),
    ),
    responses(
        (status = 200, description = "The program exited", body = PopupExited),
        (status = 404, description = "No such popup"),
        (status = 409, description = "No client showed the popup", body = String, content_type = "text/plain"),
        (status = 410, description = "The server stopped waiting; ask again"),
        (status = 422, description = "The program failed to start", body = String, content_type = "text/plain"),
    ),
))]
pub async fn api_wait(
    State(state): State<AppState>,
    Path((pane_id, id)): Path<(Uuid, Uuid)>,
) -> Response {
    loop {
        let mut changed = {
            let mgr = state.read().await;
            match mgr.popups.progress(id, pane_id) {
                Progress::Ended(Outcome::Exited(status)) => {
                    return Json(PopupExited { status }).into_response()
                }
                Progress::Ended(Outcome::Failed(error)) => {
                    return (StatusCode::UNPROCESSABLE_ENTITY, error).into_response()
                }
                Progress::Ended(Outcome::NotShown) => {
                    return (StatusCode::CONFLICT, "no browser tab showed the popup")
                        .into_response()
                }
                Progress::Unknown => return StatusCode::NOT_FOUND.into_response(),
                Progress::Starting | Progress::Running => mgr.popups.changed.subscribe(),
            }
        };
        if changed.changed().await.is_err() {
            return StatusCode::GONE.into_response();
        }
    }
}

#[derive(clap::Args, Clone, Debug)]
pub struct PopupArgs {
    /// Working directory; the current one by default.
    #[arg(short = 'd', long = "cwd")]
    cwd: Option<std::path::PathBuf>,
    /// Title above the popup; the command by default.
    #[arg(short = 'T', long)]
    title: Option<String>,
    /// Set an environment variable, as NAME=VALUE. Repeatable.
    #[arg(short = 'e', long = "env", value_name = "NAME=VALUE")]
    env: Vec<String>,
    /// The pane to open over, as `btmux list-panes` names it; this one by default.
    #[arg(short = 't')]
    target: Option<String>,
    /// Accepted for tmux's `display-popup -E`: popups always close when the command exits.
    #[arg(short = 'E', hide = true, action = clap::ArgAction::Count)]
    close_on_exit: u8,
    // Accepted for tmux scripts: btmux sizes and draws popups itself.
    #[arg(short = 'w', hide = true)]
    _width: Option<String>,
    #[arg(short = 'h', hide = true)]
    _height: Option<String>,
    #[arg(short = 'x', hide = true)]
    _x: Option<String>,
    #[arg(short = 'y', hide = true)]
    _y: Option<String>,
    #[arg(short = 'b', hide = true)]
    _border_lines: Option<String>,
    #[arg(short = 's', hide = true)]
    _style: Option<String>,
    #[arg(short = 'S', hide = true)]
    _border_style: Option<String>,
    #[arg(short = 'c', hide = true)]
    _client: Option<String>,
    #[arg(short = 'B', hide = true)]
    _no_border: bool,
    /// Print help.
    #[arg(long, action = clap::ArgAction::Help)]
    help: Option<bool>,
    /// The program and its arguments. A single argument is a command line for the shell.
    #[arg(
        required = true,
        trailing_var_arg = true,
        allow_hyphen_values = true,
        value_name = "COMMAND"
    )]
    command: Vec<String>,
}

impl PopupArgs {
    fn command(&self) -> PopupCommand {
        match self.command.as_slice() {
            [command] => PopupCommand::Shell(command.clone()),
            argv => PopupCommand::Exec(argv.to_vec()),
        }
    }

    fn extra_env(&self) -> Result<Vec<(String, String)>, String> {
        self.env
            .iter()
            .map(|pair| {
                pair.split_once('=')
                    .filter(|(name, _)| !name.is_empty())
                    .map(|(name, value)| (name.to_string(), value.to_string()))
                    .ok_or_else(|| format!("{pair}: expected NAME=VALUE"))
            })
            .collect()
    }
}

/// `btmux popup`: its exit status is the program's.
pub async fn run(profile: Option<&str>, args: &PopupArgs) -> i32 {
    match run_popup(profile, args).await {
        Ok(code) => code,
        Err(error) => {
            eprintln!("btmux: {error}");
            EXIT_FAILED
        }
    }
}

async fn run_popup(profile: Option<&str>, args: &PopupArgs) -> Result<i32, String> {
    let command = args.command();
    command.validate()?;
    let extra = args.extra_env()?;
    let cwd = match &args.cwd {
        Some(dir) => std::path::absolute(dir).map_err(|e| format!("{}: {e}", dir.display()))?,
        None => std::env::current_dir().map_err(|e| e.to_string())?,
    };
    let pane = match crate::tmux_cli::resolve_pane(profile, args.target.as_deref()).await {
        Ok(pane) => pane,
        // Outside btmux, it runs here.
        Err(_) if args.target.is_none() && crate::widget::pane_id().is_err() => {
            return run_inline(&command, &extra, &cwd);
        }
        Err(error) => return Err(error),
    };
    let mut env: Vec<(String, String)> = std::env::vars_os()
        .filter_map(|(key, value)| Some((key.into_string().ok()?, value.into_string().ok()?)))
        .collect();
    env.retain(|(key, _)| !extra.iter().any(|(name, _)| name == key));
    env.extend(extra.iter().cloned());
    let request = PopupRequest {
        id: Some(Uuid::new_v4()),
        command: command.clone(),
        cwd: Some(cwd.to_string_lossy().into_owned()),
        env: Some(env),
        title: args.title.clone(),
    };
    let body = serde_json::to_value(&request).map_err(|e| e.to_string())?;
    let path = format!("/api/panes/{pane}/popup");
    // Retried with its id across an upgrade, it never runs twice.
    let (status, reply) =
        request_retrying(profile, reqwest::Method::POST, &path, Some(body)).await?;
    match status {
        reqwest::StatusCode::OK => {}
        // No tab shows the pane, as over ssh: it runs here.
        reqwest::StatusCode::CONFLICT => return run_inline(&command, &extra, &cwd),
        reqwest::StatusCode::UNPROCESSABLE_ENTITY => {
            eprintln!("btmux: {reply}");
            return Ok(EXIT_NOT_FOUND);
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
    struct Opened {
        id: Uuid,
    }
    let Opened { id } = serde_json::from_str(&reply).map_err(|e| e.to_string())?;
    wait(profile, pane, id).await
}

/// Wait for the popup's program to exit, reconnecting while the server
/// restarts in place (`btmux upgrade`).
async fn wait(profile: Option<&str>, pane: Uuid, id: Uuid) -> Result<i32, String> {
    let path = format!("/api/panes/{pane}/popup/{id}");
    match request_retrying(profile, reqwest::Method::GET, &path, None).await? {
        (reqwest::StatusCode::OK, reply) => {
            #[derive(Deserialize)]
            struct Exited {
                status: Option<i32>,
            }
            let Exited { status } = serde_json::from_str(&reply).map_err(|e| e.to_string())?;
            Ok(status.unwrap_or(1))
        }
        (reqwest::StatusCode::UNPROCESSABLE_ENTITY, reply) => {
            eprintln!("btmux: {reply}");
            Ok(EXIT_NOT_FOUND)
        }
        (reqwest::StatusCode::NOT_FOUND, _) => {
            Err("the popup is gone; its exit status is unknown".into())
        }
        (status, reply) => Err(if reply.is_empty() {
            status.to_string()
        } else {
            reply
        }),
    }
}

/// A request repeated while the server is unreachable or failing, as while
/// it restarts in place, for up to a minute.
async fn request_retrying(
    profile: Option<&str>,
    method: reqwest::Method,
    path: &str,
    body: Option<serde_json::Value>,
) -> Result<(reqwest::StatusCode, String), String> {
    let mut unreachable_since = None;
    loop {
        match crate::api_client::request_with_status(
            profile,
            method.clone(),
            path,
            body.clone(),
            NO_TIMEOUT,
        )
        .await
        {
            Ok((status, reply)) if !status.is_server_error() => return Ok((status, reply)),
            Ok(_) | Err(_) => {
                let since = *unreachable_since.get_or_insert_with(Instant::now);
                if since.elapsed() > Duration::from_secs(60) {
                    return Err("lost the btmux server".into());
                }
                tokio::time::sleep(Duration::from_millis(500)).await;
            }
        }
    }
}

/// Run the command in this terminal instead.
fn run_inline(
    command: &PopupCommand,
    extra: &[(String, String)],
    cwd: &std::path::Path,
) -> Result<i32, String> {
    let mut process = match command {
        PopupCommand::Exec(argv) => {
            let mut process = std::process::Command::new(&argv[0]);
            process.args(&argv[1..]);
            process
        }
        #[cfg(unix)]
        PopupCommand::Shell(line) => {
            let shell = std::env::var("SHELL")
                .ok()
                .filter(|shell| !shell.is_empty())
                .unwrap_or_else(|| "/bin/sh".into());
            let mut process = std::process::Command::new(shell);
            process.args(["-c", line]);
            process
        }
        #[cfg(windows)]
        PopupCommand::Shell(line) => {
            let mut process = std::process::Command::new("cmd");
            process.args(["/C", line]);
            process
        }
    };
    // Spawning reports a missing directory as a missing program.
    if !cwd.is_dir() {
        return Err(format!("{}: no such directory", cwd.display()));
    }
    process.current_dir(cwd).envs(extra.iter().cloned());
    let status = match process.status() {
        Ok(status) => status,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            eprintln!("btmux: {}: command not found", command.title());
            return Ok(EXIT_NOT_FOUND);
        }
        Err(error) => return Err(error.to_string()),
    };
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        Ok(status
            .code()
            .unwrap_or_else(|| 128 + status.signal().unwrap_or(0)))
    }
    #[cfg(windows)]
    Ok(status.code().unwrap_or(1))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{body::Body, http::Request, Router};
    use std::sync::Arc;
    use tower::ServiceExt;

    #[test]
    fn commands_run_directly_or_in_the_shell() {
        let exec = PopupCommand::Exec(vec!["lazygit".into(), "-p".into(), "a b".into()]);
        assert_eq!(exec.argv("/bin/zsh", true), ["lazygit", "-p", "a b"]);
        assert_eq!(exec.title(), "lazygit -p a b");
        assert_eq!(exec.guest_args(), ["--", "lazygit", "-p", "a b"]);
        let shell = PopupCommand::Shell("git log | less".into());
        assert_eq!(
            shell.argv("/bin/zsh", false),
            ["/bin/zsh", "-c", "git log | less"]
        );
        assert_eq!(
            shell.argv("/bin/zsh", true),
            ["/bin/zsh", "-i", "-c", "git log | less"]
        );
        assert_eq!(shell.guest_args(), ["--run=git log | less"]);
        assert!(PopupCommand::Exec(vec![]).validate().is_err());
        assert!(PopupCommand::Exec(vec!["".into()]).validate().is_err());
        assert!(PopupCommand::Shell(" ".into()).validate().is_err());
    }

    #[test]
    fn one_argument_is_a_command_line() {
        use clap::Parser;
        #[derive(Parser)]
        #[command(disable_help_flag = true)]
        struct Cli {
            #[command(flatten)]
            popup: PopupArgs,
        }
        let parse = |args: &[&str]| {
            Cli::try_parse_from(std::iter::once("popup").chain(args.iter().copied()))
                .map(|cli| cli.popup)
        };
        let args = parse(&["-E", "-d", "/tmp", "lazygit"]).unwrap();
        assert_eq!(args.command(), PopupCommand::Shell("lazygit".into()));
        // tmux's sizing and style flags are ignored.
        let args = parse(&["-w", "80%", "-h", "80%", "-B", "-E", "lazygit"]).unwrap();
        assert_eq!(args.command(), PopupCommand::Shell("lazygit".into()));
        let args = parse(&["-e", "A=1", "git", "log", "--oneline"]).unwrap();
        assert_eq!(
            args.command(),
            PopupCommand::Exec(vec!["git".into(), "log".into(), "--oneline".into()])
        );
        assert_eq!(args.extra_env().unwrap(), [("A".into(), "1".into())]);
        assert!(parse(&["-e", "=1", "x"]).unwrap().extra_env().is_err());
        assert!(parse(&[]).is_err());
    }

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

    fn wait_request(pane: Uuid, id: Uuid) -> Request<Body> {
        Request::get(format!("/api/panes/{pane}/popup/{id}"))
            .body(Body::empty())
            .unwrap()
    }

    fn call(
        app: &Router,
        request: Request<Body>,
    ) -> tokio::task::JoinHandle<(StatusCode, serde_json::Value)> {
        let app = app.clone();
        tokio::spawn(async move {
            let response = app.oneshot(request).await.unwrap();
            let status = response.status();
            let bytes = axum::body::to_bytes(response.into_body(), MAX_BYTES)
                .await
                .unwrap();
            (
                status,
                serde_json::from_slice(&bytes)
                    .unwrap_or_else(|_| String::from_utf8_lossy(&bytes).into()),
            )
        })
    }

    fn open_request(pane: Uuid, command: &str) -> Request<Body> {
        let body = serde_json::json!({ "command": { "shell": command } });
        Request::post(format!("/api/panes/{pane}/popup"))
            .header("content-type", "application/json")
            .body(Body::from(body.to_string()))
            .unwrap()
    }

    async fn open_id(state: &AppState) -> Uuid {
        for _ in 0..200 {
            if let Some(popup) = state.read().await.popups.summaries().first() {
                return popup.id;
            }
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
        panic!("the popup never opened");
    }

    /// What a tab's pane socket does.
    async fn show(state: &AppState, id: Uuid) -> Result<(), String> {
        let mut mgr = state.write().await;
        let result = mgr.find_pty_mut(id).unwrap().ensure_spawned(80, 24);
        if let Err(error) = &result {
            mgr.popups.failed(id, error.clone());
        }
        mgr.popups.touch();
        result
    }

    #[tokio::test]
    async fn fails_when_no_tab_shows_it() {
        let (state, app, pane) = app().await;
        let (status, _) = call(&app, open_request(pane, "true")).await.unwrap();
        assert_eq!(status, StatusCode::CONFLICT);
        assert!(state.read().await.popups.summaries().is_empty());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_popup_started_at_the_deadline_runs_only_there() {
        let (state, app, pane) = app().await;
        let opened = call(&app, open_request(pane, "sleep 30"));
        let id = open_id(&state).await;
        {
            // Started while the deadline passes.
            let mut mgr = state.write().await;
            mgr.find_pty_mut(id)
                .unwrap()
                .ensure_spawned(80, 24)
                .unwrap();
            tokio::time::sleep(SHOW_TIMEOUT * 3).await;
        }
        let (status, reply) = opened.await.unwrap();
        assert_eq!(status, StatusCode::OK);
        assert_eq!(reply["id"], id.to_string());
        close(&state, id).await;
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn retried_requests_find_their_popup() {
        let (state, app, pane) = app().await;
        let id = Uuid::new_v4();
        let request = |pane: Uuid, id: Uuid| {
            let body = serde_json::json!({ "id": id, "command": { "shell": "read line" } });
            Request::post(format!("/api/panes/{pane}/popup"))
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .unwrap()
        };
        let (status, _) = call(&app, request(pane, id)).await.unwrap();
        assert_eq!(status, StatusCode::CONFLICT);
        // It never ran, so it opens again.
        let opened = call(&app, request(pane, id));
        assert_eq!(open_id(&state).await, id);
        show(&state, id).await.unwrap();
        assert_eq!(opened.await.unwrap().0, StatusCode::OK);
        // Once started, a retry finds it rather than starting another.
        let (status, reply) = call(&app, request(pane, id)).await.unwrap();
        assert_eq!(status, StatusCode::OK);
        assert_eq!(reply["id"], id.to_string());
        assert_eq!(state.read().await.popups.summaries().len(), 1);
        // A pane's id can't name a popup.
        let (status, _) = call(&app, request(pane, pane)).await.unwrap();
        assert_eq!(status, StatusCode::BAD_REQUEST);
        close(&state, id).await;
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn unstarted_popups_survive_an_upgrade() {
        let (state, _app, pane) = app().await;
        let (sessions, popups) = {
            let mut mgr = state.write().await;
            let spawn = Spawn {
                command: PopupCommand::Shell("exit $((A + 3))".into()),
                env: Some(vec![("A".into(), "1".into())]),
            };
            open(
                &mut mgr,
                pane,
                Uuid::new_v4(),
                spawn,
                Some("/".into()),
                None,
            )
            .unwrap();
            let (sessions, popups, panes) = mgr.export_handoff(Duration::from_secs(2)).unwrap();
            assert!(panes.is_empty());
            (sessions, serde_json::to_string(&popups).unwrap())
        };
        let (mut manager, exit_rx) = manager();
        manager.restore_handoff(sessions, serde_json::from_str(&popups).unwrap(), Vec::new());
        let (state, app) = serve(manager, exit_rx);
        let id = open_id(&state).await;
        show(&state, id).await.unwrap();
        let (_, reply) =
            tokio::time::timeout(Duration::from_secs(10), call(&app, wait_request(pane, id)))
                .await
                .unwrap()
                .unwrap();
        assert_eq!(reply["status"], 4);
    }

    #[test]
    fn a_missing_directory_is_not_a_missing_program() {
        let error = run_inline(
            &PopupCommand::Exec(vec!["true".into()]),
            &[],
            std::path::Path::new("/nonexistent/btmux"),
        )
        .unwrap_err();
        assert!(error.contains("no such directory"), "{error}");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn waiters_get_the_exit_status() {
        let (state, app, pane) = app().await;
        let opened = call(&app, open_request(pane, "read line; exit 3"));
        let id = open_id(&state).await;
        let (status, _) = call(&app, open_request(pane, "true")).await.unwrap();
        assert_eq!(status, StatusCode::LOCKED, "one popup per pane");
        show(&state, id).await.unwrap();
        let (status, reply) = opened.await.unwrap();
        assert_eq!(status, StatusCode::OK);
        assert_eq!(reply["id"], id.to_string());

        let waited = call(
            &app,
            Request::get(format!("/api/panes/{pane}/popup/{id}"))
                .body(Body::empty())
                .unwrap(),
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(!waited.is_finished());
        {
            let mgr = state.read().await;
            let popup = mgr.popups.iter().next().unwrap();
            popup.pty.input_tx.send(b"\r".to_vec()).unwrap();
        }
        let (status, reply) = tokio::time::timeout(Duration::from_secs(10), waited)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(status, StatusCode::OK);
        assert_eq!(reply["status"], 3);
        assert!(state.read().await.popups.summaries().is_empty());

        // Its status stays for waiters that reconnect.
        let (_, reply) = call(
            &app,
            Request::get(format!("/api/panes/{pane}/popup/{id}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
        assert_eq!(reply["status"], 3);
        let other = Uuid::new_v4();
        let (status, _) = call(
            &app,
            Request::get(format!("/api/panes/{other}/popup/{id}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
        assert_eq!(status, StatusCode::NOT_FOUND, "only under its own pane");
    }

    #[tokio::test]
    async fn programs_that_cannot_start_fail_the_request() {
        let (state, app, pane) = app().await;
        let body = serde_json::json!({ "command": { "exec": ["/nonexistent/btmux-popup"] } });
        let opened = call(
            &app,
            Request::post(format!("/api/panes/{pane}/popup"))
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        );
        let id = open_id(&state).await;
        assert!(show(&state, id).await.is_err());
        let (status, _) = opened.await.unwrap();
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn closing_hangs_up_and_closing_the_pane_closes_it() {
        let (state, app, pane) = app().await;
        let opened = call(&app, open_request(pane, "sleep 30"));
        let id = open_id(&state).await;
        show(&state, id).await.unwrap();
        assert_eq!(opened.await.unwrap().0, StatusCode::OK);
        close(&state, id).await;
        let (_, reply) = tokio::time::timeout(
            Duration::from_secs(10),
            call(
                &app,
                Request::get(format!("/api/panes/{pane}/popup/{id}"))
                    .body(Body::empty())
                    .unwrap(),
            ),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(reply["status"], 128 + libc::SIGHUP);

        let opened = call(&app, open_request(pane, "sleep 30"));
        let id = open_id(&state).await;
        show(&state, id).await.unwrap();
        assert_eq!(opened.await.unwrap().0, StatusCode::OK);
        state.write().await.handle_pane_exit(pane).await;
        assert!(state.read().await.popups.summaries().is_empty());
    }

    #[test]
    fn ended_outcomes_are_handed_off_for_the_rest_of_their_time() {
        let ago = |secs| {
            Instant::now()
                .checked_sub(Duration::from_secs(secs))
                .unwrap()
        };
        let (id, stale, pane_id) = (Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4());
        let mut popups = Popups::default();
        popups.ended.extend([
            Ended {
                id: stale,
                pane_id,
                outcome: Outcome::Exited(Some(1)),
                at: ago(ENDED_KEPT.as_secs() + 1),
            },
            Ended {
                id,
                pane_id,
                outcome: Outcome::Exited(Some(7)),
                at: ago(10),
            },
        ]);
        let json = serde_json::to_string(&popups.export_ended()).unwrap();
        let mut restored = Popups::default();
        restored.restore_ended(serde_json::from_str(&json).unwrap());
        assert_eq!(
            restored.progress(id, pane_id),
            Progress::Ended(Outcome::Exited(Some(7)))
        );
        assert_eq!(restored.progress(stale, pane_id), Progress::Unknown);
        let age = restored.ended[0].at.elapsed();
        assert!(age >= Duration::from_secs(10) && age < ENDED_KEPT);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn widgets_and_images_reach_popups() {
        let (state, app, pane) = app().await;
        let opened = call(&app, open_request(pane, "read line"));
        let id = open_id(&state).await;
        show(&state, id).await.unwrap();
        assert_eq!(opened.await.unwrap().0, StatusCode::OK);
        let body = serde_json::json!({ "type": "confirm", "message": "ok?", "default_yes": false });
        let widget = call(
            &app,
            Request::post(format!("/api/panes/{id}/widget"))
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        );
        // No tab reports it shown.
        let (status, _) = widget.await.unwrap();
        assert_eq!(status, StatusCode::CONFLICT);
        let (status, reply) = call(
            &app,
            Request::post(format!("/api/panes/{id}/paste-image"))
                .header("content-type", "image/png")
                .body(Body::from(vec![0x89, 0x50, 0x4e, 0x47]))
                .unwrap(),
        )
        .await
        .unwrap();
        assert_eq!(status, StatusCode::OK);
        let _ = std::fs::remove_file(reply["path"].as_str().unwrap());
        close(&state, id).await;
    }

    #[tokio::test]
    async fn rejects_unknown_panes_and_directories() {
        let (_state, app, pane) = app().await;
        let (status, _) = call(&app, open_request(Uuid::new_v4(), "true"))
            .await
            .unwrap();
        assert_eq!(status, StatusCode::NOT_FOUND);
        let body =
            serde_json::json!({ "command": { "shell": "true" }, "cwd": "/nonexistent/btmux" });
        let (status, _) = call(
            &app,
            Request::post(format!("/api/panes/{pane}/popup"))
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }
}
