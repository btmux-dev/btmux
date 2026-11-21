//! `GET /api/events` and `btmux events`: server changes as newline-delimited
//! JSON. The control socket's broadcast carries whole `State` snapshots, so
//! each is diffed against the last into changes (sessions, windows, panes,
//! agent states); notifications, widgets, popups, config reloads, and toasts
//! pass through. UI directives and telemetry are left out.

use std::collections::{HashMap, VecDeque};
use std::convert::Infallible;
use std::io::Write;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use axum::body::Body;
use axum::extract::{Query, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::broadcast;
use uuid::Uuid;

use crate::session::manager::{SessionManager, SessionSnapshot, WindowSnapshot};
use crate::session::{AgentState, AgentStatus, NotificationLevel, PaneSnapshot};
use crate::widget::WidgetKind;
use crate::AppState;

/// An empty line this often keeps proxies from closing a quiet stream.
const HEARTBEAT: Duration = Duration::from_secs(30);

/// Set at shutdown, ending streams so they don't hold it up.
static SHUTDOWN: std::sync::LazyLock<tokio::sync::watch::Sender<bool>> =
    std::sync::LazyLock::new(|| tokio::sync::watch::channel(false).0);

pub fn shutdown() {
    SHUTDOWN.send_replace(true);
}

/// One line of the stream.
#[derive(Serialize, Clone, Debug, PartialEq)]
#[cfg_attr(test, derive(utoipa::ToSchema))]
pub struct Event {
    /// Unix milliseconds.
    pub time: u64,
    #[serde(flatten)]
    pub kind: EventKind,
}

#[derive(Serialize, Clone, Debug, PartialEq)]
#[cfg_attr(test, derive(utoipa::ToSchema))]
pub struct SessionRef {
    pub session_id: Uuid,
    /// The session's name.
    pub session: String,
}

#[derive(Serialize, Clone, Debug, PartialEq)]
#[cfg_attr(test, derive(utoipa::ToSchema))]
pub struct WindowRef {
    #[serde(flatten)]
    pub session: SessionRef,
    pub window_id: Uuid,
    /// The window's name.
    pub window: String,
    pub window_index: usize,
}

#[derive(Serialize, Clone, Debug, PartialEq)]
#[cfg_attr(test, derive(utoipa::ToSchema))]
pub struct PaneRef {
    #[serde(flatten)]
    pub window: WindowRef,
    pub pane_id: Uuid,
}

#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
#[cfg_attr(test, derive(utoipa::ToSchema))]
pub enum EventKind {
    /// First on every stream; `session_id` is the session `session` named.
    Ready {
        version: String,
        session_id: Option<Uuid>,
    },
    SessionCreated {
        #[serde(flatten)]
        at: SessionRef,
    },
    SessionRenamed {
        #[serde(flatten)]
        at: SessionRef,
        old_name: String,
    },
    SessionClosed {
        #[serde(flatten)]
        at: SessionRef,
    },
    WindowCreated {
        #[serde(flatten)]
        at: WindowRef,
    },
    WindowRenamed {
        #[serde(flatten)]
        at: WindowRef,
        old_name: String,
    },
    /// The session's active window changed.
    WindowSelected {
        #[serde(flatten)]
        at: WindowRef,
    },
    WindowClosed {
        #[serde(flatten)]
        at: WindowRef,
    },
    PaneCreated {
        #[serde(flatten)]
        at: PaneRef,
    },
    /// The pane went to another window.
    PaneMoved {
        #[serde(flatten)]
        at: PaneRef,
        from_window_id: Uuid,
    },
    /// The window's active pane changed.
    PaneSelected {
        #[serde(flatten)]
        at: PaneRef,
    },
    PaneZoomed {
        #[serde(flatten)]
        at: PaneRef,
        zoomed: bool,
    },
    PaneTitleChanged {
        #[serde(flatten)]
        at: PaneRef,
        title: Option<String>,
    },
    PaneCwdChanged {
        #[serde(flatten)]
        at: PaneRef,
        cwd: Option<String>,
    },
    PaneClosed {
        #[serde(flatten)]
        at: PaneRef,
    },
    /// The pane's agent changed state, or another agent took over.
    AgentStatusChanged {
        #[serde(flatten)]
        at: PaneRef,
        previous: AgentState,
        status: AgentStatus,
    },
    /// An agent hook or `btmux` alert for a pane.
    PaneNotification {
        #[serde(flatten)]
        at: PaneRef,
        event: String,
        level: NotificationLevel,
        title: Option<String>,
        body: Option<String>,
    },
    /// `btmux send-notification` or `POST /api/notify`.
    Notification {
        level: NotificationLevel,
        title: String,
        body: Option<String>,
    },
    /// A `btmux pick`, `confirm`, or `input` opened over the pane.
    WidgetOpened {
        #[serde(flatten)]
        at: PaneRef,
        widget_id: Uuid,
        kind: WidgetKind,
    },
    WidgetClosed {
        #[serde(flatten)]
        at: PaneRef,
        widget_id: Uuid,
        kind: WidgetKind,
    },
    PopupOpened {
        #[serde(flatten)]
        at: PaneRef,
        popup_id: Uuid,
        title: String,
    },
    PopupClosed {
        #[serde(flatten)]
        at: PaneRef,
        popup_id: Uuid,
        title: String,
    },
    /// The config was reloaded or overridden; `error` if the file failed to load.
    ConfigChanged { error: Option<String> },
    /// A server message for every tab, such as an upgrade starting.
    Toast {
        level: NotificationLevel,
        message: String,
    },
}

impl EventKind {
    /// The session the event is about; `None` for server-wide events.
    fn session_id(&self) -> Option<Uuid> {
        use EventKind::*;
        match self {
            Ready { .. } | Notification { .. } | ConfigChanged { .. } | Toast { .. } => None,
            SessionCreated { at } | SessionRenamed { at, .. } | SessionClosed { at } => {
                Some(at.session_id)
            }
            WindowCreated { at }
            | WindowRenamed { at, .. }
            | WindowSelected { at }
            | WindowClosed { at } => Some(at.session.session_id),
            PaneCreated { at }
            | PaneMoved { at, .. }
            | PaneSelected { at }
            | PaneZoomed { at, .. }
            | PaneTitleChanged { at, .. }
            | PaneCwdChanged { at, .. }
            | PaneClosed { at }
            | AgentStatusChanged { at, .. }
            | PaneNotification { at, .. }
            | WidgetOpened { at, .. }
            | WidgetClosed { at, .. }
            | PopupOpened { at, .. }
            | PopupClosed { at, .. } => Some(at.window.session.session_id),
        }
    }
}

fn session_ref(session: &SessionSnapshot) -> SessionRef {
    SessionRef {
        session_id: session.id,
        session: session.name.clone(),
    }
}

fn window_ref(session: &SessionSnapshot, index: usize, window: &WindowSnapshot) -> WindowRef {
    WindowRef {
        session: session_ref(session),
        window_id: window.id,
        window: window.name.clone(),
        window_index: index,
    }
}

fn pane_ref(
    session: &SessionSnapshot,
    index: usize,
    window: &WindowSnapshot,
    pane: Uuid,
) -> PaneRef {
    PaneRef {
        window: window_ref(session, index, window),
        pane_id: pane,
    }
}

type Windows<'a> = HashMap<Uuid, (&'a SessionSnapshot, usize, &'a WindowSnapshot)>;
type Panes<'a> = HashMap<
    Uuid,
    (
        &'a SessionSnapshot,
        usize,
        &'a WindowSnapshot,
        &'a PaneSnapshot,
    ),
>;

fn windows(sessions: &[SessionSnapshot]) -> Windows<'_> {
    sessions
        .iter()
        .flat_map(|s| {
            s.windows
                .iter()
                .enumerate()
                .map(move |(i, w)| (w.id, (s, i, w)))
        })
        .collect()
}

fn panes(sessions: &[SessionSnapshot]) -> Panes<'_> {
    sessions
        .iter()
        .flat_map(|s| {
            s.windows
                .iter()
                .enumerate()
                .flat_map(move |(i, w)| w.panes.iter().map(move |p| (p.id, (s, i, w, p))))
        })
        .collect()
}

fn locate(sessions: &[SessionSnapshot], pane: Uuid) -> Option<PaneRef> {
    sessions.iter().find_map(|s| {
        s.windows.iter().enumerate().find_map(|(i, w)| {
            w.panes
                .iter()
                .any(|p| p.id == pane)
                .then(|| pane_ref(s, i, w, pane))
        })
    })
}

/// The changes from `old` to `new`: creations first (sessions, windows, then
/// panes), then changes, then closings (panes, windows, then sessions).
pub fn diff(old: &[SessionSnapshot], new: &[SessionSnapshot]) -> Vec<EventKind> {
    let mut created = Vec::new();
    let mut changed = Vec::new();
    let mut closed = Vec::new();
    let old_sessions: HashMap<Uuid, &SessionSnapshot> = old.iter().map(|s| (s.id, s)).collect();
    let new_sessions: HashMap<Uuid, &SessionSnapshot> = new.iter().map(|s| (s.id, s)).collect();
    let (old_windows, new_windows) = (windows(old), windows(new));
    let (old_panes, new_panes) = (panes(old), panes(new));

    for session in new {
        match old_sessions.get(&session.id) {
            None => created.push(EventKind::SessionCreated {
                at: session_ref(session),
            }),
            Some(before) if before.name != session.name => {
                changed.push(EventKind::SessionRenamed {
                    at: session_ref(session),
                    old_name: before.name.clone(),
                })
            }
            Some(_) => {}
        }
    }
    for session in new {
        for (index, window) in session.windows.iter().enumerate() {
            let at = || window_ref(session, index, window);
            let Some((_, _, before)) = old_windows.get(&window.id) else {
                created.push(EventKind::WindowCreated { at: at() });
                continue;
            };
            if before.name != window.name {
                changed.push(EventKind::WindowRenamed {
                    at: at(),
                    old_name: before.name.clone(),
                });
            }
            let active = |w: &WindowSnapshot| w.panes.get(w.active_pane).map(|p| p.id);
            if let Some(pane) = active(window).filter(|&p| active(before) != Some(p)) {
                changed.push(EventKind::PaneSelected {
                    at: pane_ref(session, index, window, pane),
                });
            }
            if before.zoomed_pane != window.zoomed_pane {
                let present = |pane: &Uuid| window.panes.iter().any(|p| p.id == *pane);
                if let Some(pane) = before.zoomed_pane.filter(present) {
                    changed.push(EventKind::PaneZoomed {
                        at: pane_ref(session, index, window, pane),
                        zoomed: false,
                    });
                }
                if let Some(pane) = window.zoomed_pane {
                    changed.push(EventKind::PaneZoomed {
                        at: pane_ref(session, index, window, pane),
                        zoomed: true,
                    });
                }
            }
        }
        if let Some(before) = old_sessions.get(&session.id) {
            let active = |s: &SessionSnapshot| s.windows.get(s.active_window).map(|w| w.id);
            if active(session) != active(before) {
                if let Some(window) = session.windows.get(session.active_window) {
                    changed.push(EventKind::WindowSelected {
                        at: window_ref(session, session.active_window, window),
                    });
                }
            }
        }
    }
    for session in new {
        for (index, window) in session.windows.iter().enumerate() {
            for pane in &window.panes {
                let at = || pane_ref(session, index, window, pane.id);
                let Some((_, _, from, before)) = old_panes.get(&pane.id) else {
                    created.push(EventKind::PaneCreated { at: at() });
                    continue;
                };
                if from.id != window.id {
                    changed.push(EventKind::PaneMoved {
                        at: at(),
                        from_window_id: from.id,
                    });
                }
                if before.title != pane.title {
                    changed.push(EventKind::PaneTitleChanged {
                        at: at(),
                        title: pane.title.clone(),
                    });
                }
                if before.cwd != pane.cwd {
                    changed.push(EventKind::PaneCwdChanged {
                        at: at(),
                        cwd: pane.cwd.clone(),
                    });
                }
                let (was, is) = (&before.agent_status, &pane.agent_status);
                if was.state != is.state || was.agent != is.agent {
                    changed.push(EventKind::AgentStatusChanged {
                        at: at(),
                        previous: was.state.clone(),
                        status: is.clone(),
                    });
                }
            }
        }
    }
    for session in old {
        for (index, window) in session.windows.iter().enumerate() {
            for pane in &window.panes {
                if !new_panes.contains_key(&pane.id) {
                    closed.push(EventKind::PaneClosed {
                        at: pane_ref(session, index, window, pane.id),
                    });
                }
            }
        }
    }
    for session in old {
        for (index, window) in session.windows.iter().enumerate() {
            if !new_windows.contains_key(&window.id) {
                closed.push(EventKind::WindowClosed {
                    at: window_ref(session, index, window),
                });
            }
        }
    }
    for session in old {
        if !new_sessions.contains_key(&session.id) {
            closed.push(EventKind::SessionClosed {
                at: session_ref(session),
            });
        }
    }
    created.extend(changed);
    created.extend(closed);
    created
}

#[derive(Deserialize, Clone)]
struct Widget {
    id: Uuid,
    pane_id: Uuid,
    kind: WidgetKind,
}

#[derive(Deserialize, Clone)]
struct Popup {
    id: Uuid,
    pane_id: Uuid,
    title: String,
}

/// Turns broadcast messages into events for one stream.
struct Tracker {
    sessions: Vec<SessionSnapshot>,
    widgets: Vec<Widget>,
    popups: Vec<Popup>,
    /// Only this session's events.
    session: Option<Uuid>,
    /// The filtered session closed.
    ended: bool,
}

impl Tracker {
    fn new(mgr: &SessionManager, session: Option<Uuid>) -> Self {
        let (widgets, popups) = Self::lists(mgr);
        Self {
            sessions: mgr.all_snapshots(),
            widgets,
            popups,
            session,
            ended: false,
        }
    }

    fn lists(mgr: &SessionManager) -> (Vec<Widget>, Vec<Popup>) {
        (
            parse(serde_json::to_value(mgr.widgets.summaries()).ok()).unwrap_or_default(),
            parse(serde_json::to_value(mgr.popups.summaries()).ok()).unwrap_or_default(),
        )
    }

    /// The events since the last message, after a lag.
    fn resync(&mut self, mgr: &SessionManager) -> Vec<EventKind> {
        let (widgets, popups) = Self::lists(mgr);
        let mut events = self.state(mgr.all_snapshots());
        events.extend(self.widgets(widgets));
        events.extend(self.popups(popups));
        self.filter(events)
    }

    fn handle(&mut self, json: &str) -> Vec<EventKind> {
        let Ok(mut message) = serde_json::from_str::<Value>(json) else {
            return Vec::new();
        };
        let events = match message["type"].as_str().unwrap_or_default() {
            "state" => match parse(take(&mut message, "all_sessions")) {
                Some(sessions) => self.state(sessions),
                None => Vec::new(),
            },
            "widgets" => match parse(take(&mut message, "widgets")) {
                Some(widgets) => self.widgets(widgets),
                None => Vec::new(),
            },
            "popups" => match parse(take(&mut message, "popups")) {
                Some(popups) => self.popups(popups),
                None => Vec::new(),
            },
            "pane_notification" => {
                let pane = parse(take(&mut message, "pane_id"));
                let level = parse(take(&mut message, "level"));
                match (pane.and_then(|p| locate(&self.sessions, p)), level) {
                    (Some(at), Some(level)) => vec![EventKind::PaneNotification {
                        at,
                        event: parse(take(&mut message, "event")).unwrap_or_default(),
                        level,
                        title: parse(take(&mut message, "title")),
                        body: parse(take(&mut message, "body")),
                    }],
                    _ => Vec::new(),
                }
            }
            "notification" => match (
                parse(take(&mut message, "level")),
                parse(take(&mut message, "title")),
            ) {
                (Some(level), Some(title)) => vec![EventKind::Notification {
                    level,
                    title,
                    body: parse(take(&mut message, "body")),
                }],
                _ => Vec::new(),
            },
            "config" => vec![EventKind::ConfigChanged {
                error: parse(take(&mut message["config"], "config_error")),
            }],
            "toast" => match (
                parse(take(&mut message, "level")),
                parse(take(&mut message, "message")),
            ) {
                (Some(level), Some(text)) => vec![EventKind::Toast {
                    level,
                    message: text,
                }],
                _ => Vec::new(),
            },
            _ => Vec::new(),
        };
        self.filter(events)
    }

    fn state(&mut self, sessions: Vec<SessionSnapshot>) -> Vec<EventKind> {
        let events = diff(&self.sessions, &sessions);
        self.sessions = sessions;
        events
    }

    fn widgets(&mut self, widgets: Vec<Widget>) -> Vec<EventKind> {
        let old = std::mem::replace(&mut self.widgets, widgets);
        let mut events = Vec::new();
        for widget in &self.widgets {
            if !old.iter().any(|w| w.id == widget.id) {
                if let Some(at) = locate(&self.sessions, widget.pane_id) {
                    events.push(EventKind::WidgetOpened {
                        at,
                        widget_id: widget.id,
                        kind: widget.kind,
                    });
                }
            }
        }
        for widget in old {
            if !self.widgets.iter().any(|w| w.id == widget.id) {
                if let Some(at) = locate(&self.sessions, widget.pane_id) {
                    events.push(EventKind::WidgetClosed {
                        at,
                        widget_id: widget.id,
                        kind: widget.kind,
                    });
                }
            }
        }
        events
    }

    fn popups(&mut self, popups: Vec<Popup>) -> Vec<EventKind> {
        let old = std::mem::replace(&mut self.popups, popups);
        let mut events = Vec::new();
        for popup in &self.popups {
            if !old.iter().any(|p| p.id == popup.id) {
                if let Some(at) = locate(&self.sessions, popup.pane_id) {
                    events.push(EventKind::PopupOpened {
                        at,
                        popup_id: popup.id,
                        title: popup.title.clone(),
                    });
                }
            }
        }
        for popup in old {
            if !self.popups.iter().any(|p| p.id == popup.id) {
                if let Some(at) = locate(&self.sessions, popup.pane_id) {
                    events.push(EventKind::PopupClosed {
                        at,
                        popup_id: popup.id,
                        title: popup.title,
                    });
                }
            }
        }
        events
    }

    fn filter(&mut self, events: Vec<EventKind>) -> Vec<EventKind> {
        let Some(session) = self.session else {
            return events;
        };
        let events: Vec<EventKind> = events
            .into_iter()
            .filter(|event| event.session_id() == Some(session))
            .collect();
        if events
            .iter()
            .any(|event| matches!(event, EventKind::SessionClosed { .. }))
        {
            self.ended = true;
        }
        events
    }
}

fn take(message: &mut Value, key: &str) -> Option<Value> {
    message.get_mut(key).map(Value::take)
}

fn parse<T: serde::de::DeserializeOwned>(value: Option<Value>) -> Option<T> {
    value.and_then(|value| serde_json::from_value(value).ok())
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or_default()
}

fn line(kind: EventKind) -> String {
    let mut line = serde_json::to_string(&Event {
        time: now_ms(),
        kind,
    })
    .unwrap();
    line.push('\n');
    line
}

struct Stream {
    state: AppState,
    events: broadcast::Receiver<String>,
    tracker: Tracker,
    pending: VecDeque<EventKind>,
    heartbeat: tokio::time::Interval,
    shutdown: tokio::sync::watch::Receiver<bool>,
}

impl Stream {
    async fn next(&mut self) -> Option<String> {
        loop {
            if let Some(kind) = self.pending.pop_front() {
                return Some(line(kind));
            }
            if self.tracker.ended || *self.shutdown.borrow() {
                return None;
            }
            tokio::select! {
                event = self.events.recv() => match event {
                    Ok(json) => self.pending.extend(self.tracker.handle(&json)),
                    Err(broadcast::error::RecvError::Lagged(_)) => {
                        let mgr = self.state.read().await;
                        self.events = mgr.events().subscribe();
                        self.pending.extend(self.tracker.resync(&mgr));
                    }
                    Err(broadcast::error::RecvError::Closed) => return None,
                },
                _ = self.heartbeat.tick() => return Some("\n".into()),
                _ = self.shutdown.changed() => return None,
            }
        }
    }
}

/// A session by ID, else by name.
fn find_session(mgr: &SessionManager, session: &str) -> Option<Uuid> {
    let id = Uuid::parse_str(session).ok();
    mgr.sessions
        .iter()
        .find(|s| Some(s.id) == id)
        .or_else(|| mgr.sessions.iter().find(|s| s.name == session))
        .map(|s| s.id)
}

#[derive(Deserialize)]
#[cfg_attr(test, derive(utoipa::IntoParams))]
#[cfg_attr(test, into_params(parameter_in = Query))]
pub(crate) struct EventsQuery {
    /// A session ID or name: only that session's events, ending once it closes.
    session: Option<String>,
}

/// Streams events until the client disconnects.
#[cfg_attr(test, utoipa::path(
    get,
    path = "/api/events",
    tag = "Events",
    operation_id = "stream-events",
    summary = "Stream events",
    description = "Newline-delimited JSON, one event per line, starting with `ready`, until the \
                   client disconnects. Empty lines keep the connection alive. Each event is \
                   `{time, type, ...}`, `time` in Unix milliseconds.",
    params(EventsQuery),
    responses(
        (status = 200, description = "One event per line", body = Event, content_type = "application/x-ndjson"),
        (status = 404, description = "No such session", body = String, content_type = "text/plain"),
    ),
))]
pub(crate) async fn api_events(
    State(state): State<AppState>,
    Query(query): Query<EventsQuery>,
) -> Response {
    let (events, tracker) = {
        let mgr = state.read().await;
        let session = match query.session.as_deref() {
            Some(name) => match find_session(&mgr, name) {
                Some(id) => Some(id),
                None => {
                    return (StatusCode::NOT_FOUND, format!("no session {name}")).into_response()
                }
            },
            None => None,
        };
        (mgr.events().subscribe(), Tracker::new(&mgr, session))
    };
    let ready = EventKind::Ready {
        version: crate::config::VERSION.into(),
        session_id: tracker.session,
    };
    let start = tokio::time::Instant::now() + HEARTBEAT;
    let stream = Stream {
        state,
        events,
        tracker,
        pending: VecDeque::from([ready]),
        heartbeat: tokio::time::interval_at(start, HEARTBEAT),
        shutdown: SHUTDOWN.subscribe(),
    };
    let body = futures_util::stream::unfold(stream, |mut stream| async move {
        let line = stream.next().await?;
        Some((Ok::<_, Infallible>(line), stream))
    });
    (
        [
            (header::CONTENT_TYPE, "application/x-ndjson"),
            (header::CACHE_CONTROL, "no-cache"),
        ],
        Body::from_stream(body),
    )
        .into_response()
}

/// Why a stream stopped.
enum Stop {
    /// The filtered session closed, or stdout went away.
    Done,
    /// Retrying won't help.
    Fatal(String),
    /// The connection dropped; `connected` if it got as far as `ready`.
    Lost { error: String, connected: bool },
}

/// `btmux events`: prints events until interrupted, reconnecting after drops
/// (an upgrade or restart). Returns the exit code.
pub async fn run(profile: Option<&str>, session: Option<String>, json: bool) -> i32 {
    let mut session = session;
    let mut format = Format::new(json);
    let mut reconnecting = false;
    loop {
        match stream_once(profile, &mut session, &mut format).await {
            Stop::Done => return 0,
            Stop::Fatal(error) => {
                eprintln!("btmux: {error}");
                return 1;
            }
            Stop::Lost { error, connected } => {
                // A server that never answered isn't one to wait for.
                if !connected && !reconnecting {
                    eprintln!("btmux: {error}");
                    return 1;
                }
                if connected {
                    eprintln!("btmux: lost the server ({error}); reconnecting");
                }
                reconnecting = true;
                tokio::time::sleep(Duration::from_secs(1)).await;
            }
        }
    }
}

async fn stream_once(
    profile: Option<&str>,
    session: &mut Option<String>,
    format: &mut Format,
) -> Stop {
    let lost = |error: String| Stop::Lost {
        error,
        connected: false,
    };
    let (url, token) = match crate::send_notification::server(profile) {
        Ok(server) => server,
        Err(error) => return lost(error),
    };
    let client = match reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(5))
        .read_timeout(HEARTBEAT * 3)
        .build()
    {
        Ok(client) => client,
        Err(error) => return Stop::Fatal(error.to_string()),
    };
    let url = format!("{url}/api/events");
    let url = match session.as_deref() {
        Some(session) => reqwest::Url::parse_with_params(&url, [("session", session)]),
        None => reqwest::Url::parse(&url),
    };
    let url = match url {
        Ok(url) => url,
        Err(error) => return Stop::Fatal(error.to_string()),
    };
    let mut response = match client.get(url).bearer_auth(token).send().await {
        Ok(response) => response,
        Err(error) => return lost(chain(&error)),
    };
    let status = response.status();
    if !status.is_success() {
        let body = response.text().await.unwrap_or_default();
        let error = if body.is_empty() {
            status.to_string()
        } else {
            body
        };
        return if status.is_server_error() {
            lost(error)
        } else {
            Stop::Fatal(error)
        };
    }
    let mut connected = false;
    let mut buffer = Vec::new();
    loop {
        let chunk = match response.chunk().await {
            Ok(Some(chunk)) => chunk,
            Ok(None) if connected && session.is_some() && format.session_closed => {
                return Stop::Done
            }
            Ok(None) => {
                return Stop::Lost {
                    error: "the stream ended".into(),
                    connected,
                }
            }
            Err(error) => {
                return Stop::Lost {
                    error: chain(&error),
                    connected,
                }
            }
        };
        buffer.extend_from_slice(&chunk);
        while let Some(end) = buffer.iter().position(|&b| b == b'\n') {
            let line: Vec<u8> = buffer.drain(..=end).collect();
            let Ok(event) = serde_json::from_slice::<Value>(&line) else {
                continue;
            };
            if event["type"] == "ready" {
                connected = true;
                // Reconnect by ID, which outlives a rename.
                if let Some(id) = event["session_id"].as_str() {
                    *session = Some(id.to_owned());
                }
            }
            if event["type"] == "session_closed"
                && session.as_deref() == event["session_id"].as_str()
            {
                format.session_closed = true;
            }
            if format.print(&event).is_err() {
                return Stop::Done;
            }
        }
    }
}

/// An error with its causes, which reqwest keeps out of its message.
fn chain(error: &dyn std::error::Error) -> String {
    let mut text = error.to_string();
    let mut source = error.source();
    while let Some(cause) = source {
        text.push_str(&format!(": {cause}"));
        source = cause.source();
    }
    text
}

struct Format {
    json: bool,
    offset: time::UtcOffset,
    session_closed: bool,
}

impl Format {
    fn new(json: bool) -> Self {
        Self {
            json,
            offset: local_offset(),
            session_closed: false,
        }
    }

    fn print(&self, event: &Value) -> std::io::Result<()> {
        let mut out = std::io::stdout().lock();
        if self.json {
            writeln!(out, "{event}")?;
        } else {
            writeln!(out, "{}", human(event, self.offset))?;
        }
        out.flush()
    }
}

/// The local UTC offset; `time` refuses to read it once threads run.
fn local_offset() -> time::UtcOffset {
    #[cfg(unix)]
    // SAFETY: `localtime_r` writes only to `tm`.
    unsafe {
        let now = libc::time(std::ptr::null_mut());
        let mut tm: libc::tm = std::mem::zeroed();
        if !libc::localtime_r(&now, &mut tm).is_null() {
            if let Ok(offset) = time::UtcOffset::from_whole_seconds(tm.tm_gmtoff as i32) {
                return offset;
            }
        }
    }
    time::UtcOffset::current_local_offset().unwrap_or(time::UtcOffset::UTC)
}

/// One line for people: time, type, where, and what changed.
fn human(event: &Value, offset: time::UtcOffset) -> String {
    let text = |key: &str| event[key].as_str().unwrap_or_default();
    let time = event["time"]
        .as_u64()
        .and_then(|ms| time::OffsetDateTime::from_unix_timestamp((ms / 1000) as i64).ok())
        .map(|t| t.to_offset(offset))
        .map(|t| format!("{:02}:{:02}:{:02}", t.hour(), t.minute(), t.second()))
        .unwrap_or_default();
    let mut place = text("session").to_owned();
    if let Some(index) = event["window_index"].as_u64() {
        place.push_str(&format!(":{index} ({})", text("window")));
    }
    if let Some(pane) = event["pane_id"].as_str() {
        place.push_str(&format!(" pane {pane}"));
    }
    let message = |title: &str, body: &str| match (title, body) {
        ("", body) => body.to_owned(),
        (title, "") => title.to_owned(),
        (title, body) => format!("{title}: {body}"),
    };
    let detail = match text("type") {
        "ready" => format!("btmux {}", text("version")),
        "session_renamed" | "window_renamed" => format!("was {}", text("old_name")),
        "pane_moved" => format!("from window {}", text("from_window_id")),
        "pane_zoomed" => (if event["zoomed"] == true { "on" } else { "off" }).into(),
        "pane_title_changed" => text("title").into(),
        "pane_cwd_changed" => text("cwd").into(),
        "agent_status_changed" => {
            let status = &event["status"];
            let mut detail = format!(
                "{} {} -> {}",
                status["agent"].as_str().unwrap_or("agent"),
                text("previous"),
                status["state"].as_str().unwrap_or_default()
            );
            if let Some(message) = status["message"].as_str() {
                detail.push_str(&format!(": {message}"));
            }
            detail
        }
        "pane_notification" => format!(
            "[{}] {} {}",
            text("level"),
            text("event"),
            message(text("title"), text("body"))
        ),
        "notification" => format!(
            "[{}] {}",
            text("level"),
            message(text("title"), text("body"))
        ),
        "widget_opened" | "widget_closed" => text("kind").into(),
        "popup_opened" | "popup_closed" => text("title").into(),
        "config_changed" => match event["error"].as_str() {
            Some(error) => format!("error: {error}"),
            None => "loaded".into(),
        },
        "toast" => format!("[{}] {}", text("level"), text("message")),
        _ => String::new(),
    };
    [time.as_str(), text("type"), place.as_str(), detail.as_str()]
        .iter()
        .filter(|part| !part.is_empty())
        .copied()
        .collect::<Vec<_>>()
        .join("  ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::layout::Layout;

    fn pane(id: Uuid) -> PaneSnapshot {
        serde_json::from_value(serde_json::json!({ "id": id, "title": null, "cwd": null })).unwrap()
    }

    fn window(id: Uuid, name: &str, panes: &[Uuid]) -> WindowSnapshot {
        WindowSnapshot {
            id,
            name: name.into(),
            panes: panes.iter().map(|&p| pane(p)).collect(),
            active_pane: 0,
            layout: Layout::Leaf { pane_id: panes[0] },
            zoomed_pane: None,
        }
    }

    fn session(id: Uuid, name: &str, windows: Vec<WindowSnapshot>) -> SessionSnapshot {
        SessionSnapshot {
            id,
            name: name.into(),
            runtime: Default::default(),
            windows,
            active_window: 0,
        }
    }

    fn types(events: &[EventKind]) -> Vec<String> {
        events
            .iter()
            .map(|e| {
                serde_json::to_value(e).unwrap()["type"]
                    .as_str()
                    .unwrap()
                    .to_owned()
            })
            .collect()
    }

    #[test]
    fn diffs_structure_in_order() {
        let (s, w1, w2, p1, p2, p3) = (
            Uuid::new_v4(),
            Uuid::new_v4(),
            Uuid::new_v4(),
            Uuid::new_v4(),
            Uuid::new_v4(),
            Uuid::new_v4(),
        );
        let before = vec![session(s, "work", vec![window(w1, "one", &[p1, p2])])];
        assert_eq!(
            types(&diff(&[], &before)),
            [
                "session_created",
                "window_created",
                "pane_created",
                "pane_created"
            ]
        );

        let mut after = session(
            s,
            "play",
            vec![window(w1, "one", &[p1]), window(w2, "two", &[p2, p3])],
        );
        after.active_window = 1;
        after.windows[0].panes[0].agent_status.state = AgentState::Working;
        let events = diff(&before, &[after.clone()]);
        assert_eq!(
            types(&events),
            [
                "window_created",
                "pane_created",
                "session_renamed",
                "window_selected",
                "agent_status_changed",
                "pane_moved",
            ]
        );
        let moved = serde_json::to_value(&events[5]).unwrap();
        assert_eq!(moved["pane_id"], p2.to_string());
        assert_eq!(moved["window_index"], 1);
        assert_eq!(moved["from_window_id"], w1.to_string());
        assert_eq!(moved["session"], "play");

        assert_eq!(
            types(&diff(&[after], &[])),
            [
                "pane_closed",
                "pane_closed",
                "pane_closed",
                "window_closed",
                "window_closed",
                "session_closed",
            ]
        );
    }

    #[test]
    fn filters_by_session_and_ends_when_it_closes() {
        let (a, b) = (Uuid::new_v4(), Uuid::new_v4());
        let (wa, wb, pa, pb) = (
            Uuid::new_v4(),
            Uuid::new_v4(),
            Uuid::new_v4(),
            Uuid::new_v4(),
        );
        let both = vec![
            session(a, "a", vec![window(wa, "w", &[pa])]),
            session(b, "b", vec![window(wb, "w", &[pb])]),
        ];
        let mut tracker = Tracker {
            sessions: both.clone(),
            widgets: Vec::new(),
            popups: Vec::new(),
            session: Some(a),
            ended: false,
        };
        let state = |sessions: &[SessionSnapshot]| {
            serde_json::json!({ "type": "state", "sessions": [], "all_sessions": sessions, "agent_panes": [] })
                .to_string()
        };
        let notify = serde_json::json!({
            "type": "pane_notification", "pane_id": pa, "event": "Stop",
            "level": "success", "title": "Done", "body": null, "os_client": null,
        });
        assert_eq!(
            types(&tracker.handle(&notify.to_string())),
            ["pane_notification"]
        );
        let global = serde_json::json!({ "type": "notification", "level": "info", "title": "x" });
        assert!(tracker.handle(&global.to_string()).is_empty());

        assert!(tracker.handle(&state(&both[..1])).is_empty());
        assert!(!tracker.ended);
        assert_eq!(
            types(&tracker.handle(&state(&[]))),
            ["pane_closed", "window_closed", "session_closed"]
        );
        assert!(tracker.ended);
    }

    #[test]
    fn widgets_open_and_close_over_their_pane() {
        let (s, w, p, id) = (
            Uuid::new_v4(),
            Uuid::new_v4(),
            Uuid::new_v4(),
            Uuid::new_v4(),
        );
        let mut tracker = Tracker {
            sessions: vec![session(s, "s", vec![window(w, "w", &[p])])],
            widgets: Vec::new(),
            popups: Vec::new(),
            session: None,
            ended: false,
        };
        let widgets =
            |list: Value| serde_json::json!({ "type": "widgets", "widgets": list }).to_string();
        let opened = tracker.handle(&widgets(
            serde_json::json!([{ "id": id, "pane_id": p, "kind": "confirm" }]),
        ));
        assert_eq!(types(&opened), ["widget_opened"]);
        assert_eq!(
            types(&tracker.handle(&widgets(serde_json::json!([])))),
            ["widget_closed"]
        );
    }

    #[test]
    fn human_lines_name_the_place() {
        let event = serde_json::json!({
            "time": 0, "type": "agent_status_changed", "session": "work", "window_index": 1,
            "window": "edit", "pane_id": "p", "previous": "working",
            "status": { "state": "done", "agent": "claude", "message": null },
        });
        assert_eq!(
            human(&event, time::UtcOffset::UTC),
            "00:00:00  agent_status_changed  work:1 (edit) pane p  claude working -> done"
        );
    }
}
