//! `GET /api/events` and `btmux events`: server changes as newline-delimited
//! JSON. One [`Feed`] per server translates the control socket's broadcast
//! into typed events once: `State` snapshots are diffed against the last
//! (sessions, windows, panes, agent states), widget and popup lists too, and
//! notifications, config reloads, and toasts pass through. UI directives and
//! telemetry are left out. Each stream filters the feed and starts with
//! `ready`, which carries the state its events start from; `resync` marks
//! where events were lost.

use std::collections::HashMap;
use std::collections::VecDeque;
use std::convert::Infallible;
use std::io::Write;
use std::sync::{Arc, Mutex};
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

/// Batches a stream may fall behind before it resyncs.
const BACKLOG: usize = 256;

/// Set at shutdown, ending streams so they don't hold it up.
static SHUTDOWN: std::sync::LazyLock<tokio::sync::watch::Sender<bool>> =
    std::sync::LazyLock::new(|| tokio::sync::watch::channel(false).0);

pub fn shutdown() {
    SHUTDOWN.send_replace(true);
}

/// One line of the stream.
#[derive(Serialize, Clone)]
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

#[derive(Serialize, Clone)]
#[serde(tag = "type", rename_all = "snake_case")]
#[cfg_attr(test, derive(utoipa::ToSchema))]
pub enum EventKind {
    /// First on every stream: the state the events after it change, and the
    /// session `session` named, if any.
    Ready {
        version: String,
        session_id: Option<Uuid>,
        sessions: Vec<SessionSnapshot>,
    },
    /// The stream fell behind and events were lost. The events after it
    /// report the session, window, and pane changes since the last event
    /// before it, but not transitions that reversed in between, nor lost
    /// notifications, widgets, and popups.
    Resync,
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
    /// A `btmux pick`, `confirm`, or `input` opened over the pane, or over
    /// its popup `popup_id`.
    WidgetOpened {
        #[serde(flatten)]
        at: PaneRef,
        widget_id: Uuid,
        kind: WidgetKind,
        popup_id: Option<Uuid>,
    },
    WidgetClosed {
        #[serde(flatten)]
        at: PaneRef,
        widget_id: Uuid,
        kind: WidgetKind,
        popup_id: Option<Uuid>,
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
            Ready { .. } | Resync | Notification { .. } | ConfigChanged { .. } | Toast { .. } => {
                None
            }
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
                // A new agent in the pane counts even in the same state.
                if was.state != is.state
                    || was.agent != is.agent
                    || before.agent_id != pane.agent_id
                {
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

/// A widget or popup as the broadcast lists it.
#[derive(Deserialize, Clone)]
struct Listed {
    id: Uuid,
    pane_id: Uuid,
    kind: Option<WidgetKind>,
    title: Option<String>,
}

/// An open widget or popup, with where it opened, kept for its close.
#[derive(Clone)]
struct Open {
    id: Uuid,
    at: PaneRef,
    /// A widget's popup, if it opened in one.
    popup_id: Option<Uuid>,
    kind: Option<WidgetKind>,
    title: String,
}

/// The server as the feed last saw it.
struct View {
    sessions: Arc<Vec<SessionSnapshot>>,
    widgets: Vec<Open>,
    popups: Vec<Open>,
    /// The last config, without what follows the shown color mode.
    config: Value,
}

/// A config as `config_changed` compares it: a tab reporting the system's
/// light or dark mode changes only the shown variant, not the config.
fn comparable_config(mut config: Value) -> Value {
    if let Some(config) = config.as_object_mut() {
        for key in ["color_mode", "theme", "color_palette"] {
            config.remove(key);
        }
    }
    config
}

impl View {
    fn new(mgr: &SessionManager) -> Self {
        let mut view = Self {
            sessions: Arc::new(mgr.all_snapshots()),
            widgets: Vec::new(),
            popups: Vec::new(),
            config: comparable_config(serde_json::to_value(mgr.config()).unwrap_or_default()),
        };
        let (widgets, popups) = lists(mgr);
        view.popups(popups);
        view.widgets(widgets);
        view
    }

    /// Where a pane, or a popup's pane, is; and the popup.
    fn place(&self, id: Uuid) -> Option<(PaneRef, Option<Uuid>)> {
        locate(&self.sessions, id).map(|at| (at, None)).or_else(|| {
            self.popups
                .iter()
                .find(|popup| popup.id == id)
                .map(|popup| (popup.at.clone(), Some(id)))
        })
    }

    /// Everything since the last message, after a lag.
    fn resync(&mut self, mgr: &SessionManager) -> Vec<EventKind> {
        let (widgets, popups) = lists(mgr);
        let mut events = self.state(mgr.all_snapshots());
        events.extend(self.popups(popups));
        events.extend(self.widgets(widgets));
        events
    }

    fn handle(&mut self, json: &str) -> Vec<EventKind> {
        let Ok(mut message) = serde_json::from_str::<Value>(json) else {
            tracing::warn!("events: unreadable control message");
            return Vec::new();
        };
        let kind = message["type"].as_str().unwrap_or_default().to_owned();
        let parsed = match kind.as_str() {
            "state" => parse(take(&mut message, "all_sessions")).map(|s| self.state(s)),
            "widgets" => parse(take(&mut message, "widgets")).map(|w| self.widgets(w)),
            "popups" => parse(take(&mut message, "popups")).map(|p| self.popups(p)),
            "pane_notification" => {
                let pane: Option<Uuid> = parse(take(&mut message, "pane_id"));
                let level = parse(take(&mut message, "level"));
                level.map(|level| match pane.and_then(|p| self.place(p)) {
                    Some((at, _)) => vec![EventKind::PaneNotification {
                        at,
                        event: parse(take(&mut message, "event")).unwrap_or_default(),
                        level,
                        title: parse(take(&mut message, "title")),
                        body: parse(take(&mut message, "body")),
                    }],
                    None => Vec::new(),
                })
            }
            "notification" => {
                let level = parse(take(&mut message, "level"));
                let title = parse(take(&mut message, "title"));
                level.zip(title).map(|(level, title)| {
                    vec![EventKind::Notification {
                        level,
                        title,
                        body: parse(take(&mut message, "body")),
                    }]
                })
            }
            "config" => {
                let config = comparable_config(take(&mut message, "config").unwrap_or_default());
                if config == self.config {
                    Some(Vec::new())
                } else {
                    let error = parse(config.get("config_error").cloned());
                    self.config = config;
                    Some(vec![EventKind::ConfigChanged { error }])
                }
            }
            "toast" => {
                let level = parse(take(&mut message, "level"));
                let text = parse(take(&mut message, "message"));
                level
                    .zip(text)
                    .map(|(level, message)| vec![EventKind::Toast { level, message }])
            }
            _ => Some(Vec::new()),
        };
        parsed.unwrap_or_else(|| {
            tracing::warn!("events: unreadable {kind} message");
            Vec::new()
        })
    }

    fn state(&mut self, sessions: Vec<SessionSnapshot>) -> Vec<EventKind> {
        let events = diff(&self.sessions, &sessions);
        self.sessions = Arc::new(sessions);
        events
    }

    fn popups(&mut self, listed: Vec<Listed>) -> Vec<EventKind> {
        let mut events = Vec::new();
        let old = std::mem::take(&mut self.popups);
        for popup in &old {
            if !listed.iter().any(|l| l.id == popup.id) {
                events.push(EventKind::PopupClosed {
                    at: popup.at.clone(),
                    popup_id: popup.id,
                    title: popup.title.clone(),
                });
            }
        }
        for item in listed {
            if let Some(open) = old.iter().find(|p| p.id == item.id) {
                self.popups.push(open.clone());
            } else if let Some(at) = locate(&self.sessions, item.pane_id) {
                let title = item.title.unwrap_or_default();
                events.push(EventKind::PopupOpened {
                    at: at.clone(),
                    popup_id: item.id,
                    title: title.clone(),
                });
                self.popups.push(Open {
                    id: item.id,
                    at,
                    popup_id: None,
                    kind: None,
                    title,
                });
            }
        }
        events
    }

    fn widgets(&mut self, listed: Vec<Listed>) -> Vec<EventKind> {
        let mut events = Vec::new();
        let old = std::mem::take(&mut self.widgets);
        for widget in &old {
            if !listed.iter().any(|l| l.id == widget.id) {
                if let Some(kind) = widget.kind {
                    events.push(EventKind::WidgetClosed {
                        at: widget.at.clone(),
                        widget_id: widget.id,
                        kind,
                        popup_id: widget.popup_id,
                    });
                }
            }
        }
        for item in listed {
            if let Some(open) = old.iter().find(|w| w.id == item.id) {
                self.widgets.push(open.clone());
            } else if let (Some((at, popup_id)), Some(kind)) = (self.place(item.pane_id), item.kind)
            {
                events.push(EventKind::WidgetOpened {
                    at: at.clone(),
                    widget_id: item.id,
                    kind,
                    popup_id,
                });
                self.widgets.push(Open {
                    id: item.id,
                    at,
                    popup_id,
                    kind: Some(kind),
                    title: String::new(),
                });
            }
        }
        events
    }
}

fn lists(mgr: &SessionManager) -> (Vec<Listed>, Vec<Listed>) {
    (
        parse(serde_json::to_value(mgr.widgets.summaries()).ok()).unwrap_or_default(),
        parse(serde_json::to_value(mgr.popups.summaries()).ok()).unwrap_or_default(),
    )
}

fn take(message: &mut Value, key: &str) -> Option<Value> {
    message.get_mut(key).map(Value::take)
}

fn parse<T: serde::de::DeserializeOwned>(value: Option<Value>) -> Option<T> {
    value.and_then(|value| serde_json::from_value(value).ok())
}

/// The sessions at some point, shared by the feed and its streams.
type Sessions = Arc<Vec<SessionSnapshot>>;

/// One broadcast message, translated.
struct Batch {
    events: Vec<EventKind>,
    /// The sessions after it.
    sessions: Arc<Vec<SessionSnapshot>>,
    /// The feed fell behind the broadcast first, so events were lost.
    lost: bool,
}

/// The server's events, translated once for every stream.
pub struct Feed {
    /// The sessions as of the last batch, and the batches' sender, changed
    /// together so a new stream's `ready` lines up with its first batch.
    latest: Mutex<(Sessions, broadcast::Sender<Arc<Batch>>)>,
}

impl Feed {
    /// The server's feed, started on first use from `mgr`, which `state` holds.
    fn get(state: &AppState, mgr: &SessionManager) -> Arc<Feed> {
        mgr.events_feed
            .get_or_init(|| {
                // Subscribed first, so nothing falls between the two; a message
                // the view already has changes nothing.
                let messages = mgr.events().subscribe();
                let view = View::new(mgr);
                let feed = Arc::new(Feed {
                    latest: Mutex::new((view.sessions.clone(), broadcast::channel(BACKLOG).0)),
                });
                tokio::spawn(translate(feed.clone(), state.clone(), messages, view));
                feed
            })
            .clone()
    }

    fn subscribe(&self) -> (Sessions, broadcast::Receiver<Arc<Batch>>) {
        let latest = self.latest.lock().unwrap();
        (latest.0.clone(), latest.1.subscribe())
    }
}

async fn translate(
    feed: Arc<Feed>,
    state: AppState,
    mut messages: broadcast::Receiver<String>,
    mut view: View,
) {
    loop {
        let (events, lost) = match messages.recv().await {
            Ok(json) => (view.handle(&json), false),
            Err(broadcast::error::RecvError::Lagged(_)) => {
                let mgr = state.read().await;
                messages = mgr.events().subscribe();
                (view.resync(&mgr), true)
            }
            Err(broadcast::error::RecvError::Closed) => return,
        };
        let mut latest = feed.latest.lock().unwrap();
        // Kept current for `ready` even when nothing is worth an event, such as
        // a divider dragged.
        latest.0 = view.sessions.clone();
        if events.is_empty() && !lost {
            continue;
        }
        let _ = latest.1.send(Arc::new(Batch {
            events,
            sessions: view.sessions.clone(),
            lost,
        }));
    }
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

/// One client's events.
struct Stream {
    feed: Arc<Feed>,
    batches: broadcast::Receiver<Arc<Batch>>,
    /// The sessions as of the last batch sent.
    sessions: Arc<Vec<SessionSnapshot>>,
    /// Only this session's events.
    session: Option<Uuid>,
    /// The filtered session closed.
    ended: bool,
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
            if self.ended || *self.shutdown.borrow() {
                return None;
            }
            tokio::select! {
                batch = self.batches.recv() => match batch {
                    Ok(batch) => {
                        if batch.lost {
                            self.pending.push_back(EventKind::Resync);
                        }
                        self.sessions = batch.sessions.clone();
                        self.queue(batch.events.iter().cloned());
                    }
                    Err(broadcast::error::RecvError::Lagged(_)) => {
                        let (sessions, batches) = self.feed.subscribe();
                        let events = diff(&self.sessions, &sessions);
                        self.pending.push_back(EventKind::Resync);
                        self.queue(events);
                        (self.sessions, self.batches) = (sessions, batches);
                    }
                    Err(broadcast::error::RecvError::Closed) => return None,
                },
                _ = self.heartbeat.tick() => return Some("\n".into()),
                _ = self.shutdown.changed() => return None,
            }
        }
    }

    fn queue(&mut self, events: impl IntoIterator<Item = EventKind>) {
        for event in events {
            let Some(session) = self.session else {
                self.pending.push_back(event);
                continue;
            };
            if event.session_id() != Some(session) {
                continue;
            }
            self.ended |= matches!(event, EventKind::SessionClosed { .. });
            self.pending.push_back(event);
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
    description = "Newline-delimited JSON, one event per line, until the client disconnects. \
                   The first is `ready`, with the sessions the events after it change; \
                   `resync` marks where events were lost. Empty lines keep the connection \
                   alive. Each event is `{time, type, ...}`, `time` in Unix milliseconds. \
                   Guest (sandboxed) credentials can't use it.",
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
    let (feed, session) = {
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
        (Feed::get(&state, &mgr), session)
    };
    let (sessions, batches) = feed.subscribe();
    // The session can close between finding it and subscribing; if the feed
    // has also passed its close, nothing would ever end the stream.
    if let Some(id) = session {
        if !sessions.iter().any(|s| s.id == id)
            && !state
                .read()
                .await
                .all_snapshots()
                .iter()
                .any(|s| s.id == id)
        {
            let name = query.session.as_deref().unwrap_or_default();
            return (StatusCode::NOT_FOUND, format!("no session {name}")).into_response();
        }
    }
    let ready = EventKind::Ready {
        version: crate::config::VERSION.into(),
        session_id: session,
        sessions: sessions
            .iter()
            .filter(|s| session.is_none_or(|id| s.id == id))
            .cloned()
            .collect(),
    };
    let start = tokio::time::Instant::now() + HEARTBEAT;
    let stream = Stream {
        feed,
        batches,
        sessions,
        session,
        ended: false,
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
        match stream_once(profile, &mut session, &mut format, reconnecting).await {
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
                tokio::select! {
                    _ = tokio::time::sleep(Duration::from_secs(1)) => {}
                    _ = stdout_closed() => return 0,
                }
            }
        }
    }
}

async fn stream_once(
    profile: Option<&str>,
    session: &mut Option<String>,
    format: &mut Format,
    reconnecting: bool,
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
    if status == reqwest::StatusCode::FORBIDDEN {
        return Stop::Fatal(
            "the server refused to stream events; a sandboxed pane's credential can't".into(),
        );
    }
    // The filtered session closed while the connection was down.
    if status == reqwest::StatusCode::NOT_FOUND && reconnecting && session.is_some() {
        eprintln!("btmux: the session closed");
        return Stop::Done;
    }
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
        let chunk = tokio::select! {
            chunk = response.chunk() => chunk,
            _ = stdout_closed() => return Stop::Done,
        };
        let chunk = match chunk {
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
            if format.print(&event, &line).is_err() {
                return Stop::Done;
            }
        }
    }
}

/// Resolves once nothing reads stdout anymore (a pipe's reader exited), so a
/// quiet stream doesn't outlive `| head -n 1`.
async fn stdout_closed() {
    #[cfg(unix)]
    loop {
        let mut fd = libc::pollfd {
            fd: libc::STDOUT_FILENO,
            // macOS reports a closed pipe only to POLLOUT.
            events: libc::POLLOUT,
            revents: 0,
        };
        // SAFETY: polls one valid pollfd without blocking.
        let ready = unsafe { libc::poll(&mut fd, 1, 0) };
        if ready > 0 && fd.revents & (libc::POLLERR | libc::POLLHUP) != 0 {
            return;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    #[cfg(not(unix))]
    std::future::pending::<()>().await
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

    /// Prints `event`; with `--json`, `line` as the server sent it.
    fn print(&self, event: &Value, line: &[u8]) -> std::io::Result<()> {
        let mut out = std::io::stdout().lock();
        if self.json {
            out.write_all(line)?;
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

/// `text` for one terminal line: control characters, and bidi controls that
/// would reorder it, escaped.
fn printable(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        let bidi = matches!(c, '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}');
        if c.is_control() || bidi {
            out.extend(c.escape_default());
        } else {
            out.push(c);
        }
    }
    out
}

/// One line for people: time, type, where, and what changed.
fn human(event: &Value, offset: time::UtcOffset) -> String {
    let text = |key: &str| printable(event[key].as_str().unwrap_or_default());
    let time = event["time"]
        .as_u64()
        .and_then(|ms| time::OffsetDateTime::from_unix_timestamp((ms / 1000) as i64).ok())
        .map(|t| t.to_offset(offset))
        .map(|t| format!("{:02}:{:02}:{:02}", t.hour(), t.minute(), t.second()))
        .unwrap_or_default();
    let mut place = text("session");
    if let Some(index) = event["window_index"].as_u64() {
        place.push_str(&format!(":{index} ({})", text("window")));
    }
    if event["pane_id"].is_string() {
        place.push_str(&format!(" pane {}", text("pane_id")));
    }
    let message = |title: String, body: String| match (title.is_empty(), body.is_empty()) {
        (true, _) => body,
        (_, true) => title,
        _ => format!("{title}: {body}"),
    };
    let detail = match text("type").as_str() {
        "ready" => {
            let sessions = event["sessions"].as_array().map_or(0, Vec::len);
            let plural = if sessions == 1 { "" } else { "s" };
            format!("btmux {}, {sessions} session{plural}", text("version"))
        }
        "resync" => "fell behind; events were lost".into(),
        "session_renamed" | "window_renamed" => format!("was {}", text("old_name")),
        "pane_moved" => format!("from window {}", text("from_window_id")),
        "pane_zoomed" => (if event["zoomed"] == true { "on" } else { "off" }).into(),
        "pane_title_changed" => text("title"),
        "pane_cwd_changed" => text("cwd"),
        "agent_status_changed" => {
            let status = &event["status"];
            let field = |key: &str| printable(status[key].as_str().unwrap_or_default());
            let agent = field("agent");
            let mut detail = format!(
                "{} {} -> {}",
                if agent.is_empty() {
                    "agent".into()
                } else {
                    agent
                },
                text("previous"),
                field("state")
            );
            let note = field("message");
            if !note.is_empty() {
                detail.push_str(&format!(": {note}"));
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
        "widget_opened" | "widget_closed" => match event["popup_id"].as_str() {
            Some(popup) => format!("{} in popup {}", text("kind"), printable(popup)),
            None => text("kind"),
        },
        "popup_opened" | "popup_closed" => text("title"),
        "config_changed" => match event["error"].as_str() {
            Some(error) => format!("error: {}", printable(error)),
            None => "loaded".into(),
        },
        "toast" => format!("[{}] {}", text("level"), text("message")),
        _ => String::new(),
    };
    [time, text("type"), place, detail]
        .into_iter()
        .filter(|part| !part.is_empty())
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

    fn view(sessions: Vec<SessionSnapshot>) -> View {
        View {
            sessions: Arc::new(sessions),
            widgets: Vec::new(),
            popups: Vec::new(),
            config: Value::Null,
        }
    }

    #[tokio::test]
    async fn ready_starts_from_the_latest_sessions_even_without_events() {
        let [s, w, p] = ids();
        let before = vec![session(s, "a", vec![window(w, "w", &[p])])];
        let feed = Arc::new(Feed {
            latest: Mutex::new((Arc::new(before.clone()), broadcast::channel(4).0)),
        });
        let (exit_tx, _) = tokio::sync::mpsc::unbounded_channel();
        let (meta_tx, _) = tokio::sync::mpsc::unbounded_channel();
        let manager = crate::session::manager::SessionManager::new(
            "/bin/sh".into(),
            crate::config::FileConfig::default(),
            exit_tx,
            meta_tx,
            8004,
            None,
        );
        let state = Arc::new(tokio::sync::RwLock::new(manager));
        let (tx, rx) = broadcast::channel(4);
        tokio::spawn(translate(feed.clone(), state, rx, view(before.clone())));
        // Only an agent's message changes: no event, but new streams see it.
        let mut after = before.clone();
        after[0].windows[0].panes[0].agent_status.message = Some("thinking".into());
        let message = serde_json::json!({ "type": "state", "sessions": [], "all_sessions": after });
        tx.send(message.to_string()).unwrap();
        for _ in 0..50 {
            if feed.subscribe().0[0].windows[0].panes[0]
                .agent_status
                .message
                .is_some()
            {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("ready kept the old sessions");
    }

    #[test]
    fn a_new_agent_in_the_same_state_is_a_change() {
        let [s, w, p] = ids();
        let before = vec![session(s, "a", vec![window(w, "w", &[p])])];
        let mut after = before.clone();
        after[0].windows[0].panes[0].agent_id = Some(2);
        assert_eq!(types(&diff(&before, &after)), ["agent_status_changed"]);
    }

    #[test]
    fn config_changes_ignore_the_shown_color_mode() {
        let mut view = view(Vec::new());
        let config = |mode: &str, palette: &str, dark: &str| {
            serde_json::json!({ "type": "config", "config": {
                "color_mode": mode, "theme": palette, "color_palette": palette,
                "dark_theme": dark, "config_error": null,
            }})
            .to_string()
        };
        assert_eq!(
            types(&view.handle(&config("dark", "d", "d"))),
            ["config_changed"]
        );
        // A tab reports the system went light: only the shown variant changes.
        assert!(view.handle(&config("light", "l", "d")).is_empty());
        // An edited dark scheme is a change.
        assert_eq!(
            types(&view.handle(&config("light", "l", "d2"))),
            ["config_changed"]
        );
    }

    fn ids<const N: usize>() -> [Uuid; N] {
        std::array::from_fn(|_| Uuid::new_v4())
    }

    #[test]
    fn diffs_structure_in_order() {
        let [s, w1, w2, p1, p2, p3] = ids();
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
        let events = diff(&before, std::slice::from_ref(&after));
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
    fn passes_notifications_through() {
        let [s, w, p] = ids();
        let mut view = view(vec![session(s, "a", vec![window(w, "w", &[p])])]);
        let notify = serde_json::json!({
            "type": "pane_notification", "pane_id": p, "event": "Stop",
            "level": "success", "title": "Done", "body": null, "os_client": null,
        });
        assert_eq!(
            types(&view.handle(&notify.to_string())),
            ["pane_notification"]
        );
        let global = serde_json::json!({ "type": "notification", "level": "info", "title": "x" });
        assert_eq!(types(&view.handle(&global.to_string())), ["notification"]);
        let directive = serde_json::json!({ "type": "focus_pane", "pane_id": p });
        assert!(view.handle(&directive.to_string()).is_empty());
    }

    #[test]
    fn widgets_over_popups_name_the_popup_and_close_where_they_opened() {
        let [s, w, p, popup, widget] = ids();
        let mut view = view(vec![session(s, "s", vec![window(w, "w", &[p])])]);
        let list =
            |kind: &str, items: Value| serde_json::json!({ "type": kind, kind: items }).to_string();
        let popups = serde_json::json!([{ "id": popup, "pane_id": p, "title": "fzf" }]);
        assert_eq!(
            types(&view.handle(&list("popups", popups))),
            ["popup_opened"]
        );
        let widgets = serde_json::json!([{ "id": widget, "pane_id": popup, "kind": "confirm" }]);
        let opened = view.handle(&list("widgets", widgets));
        assert_eq!(types(&opened), ["widget_opened"]);
        let opened = serde_json::to_value(&opened[0]).unwrap();
        assert_eq!(opened["pane_id"], p.to_string());
        assert_eq!(opened["popup_id"], popup.to_string());

        // The pane closes before the lists empty.
        view.state(Vec::new());
        let closed = view.handle(&list("widgets", serde_json::json!([])));
        assert_eq!(types(&closed), ["widget_closed"]);
        assert_eq!(serde_json::to_value(&closed[0]).unwrap()["session"], "s");
        assert_eq!(
            types(&view.handle(&list("popups", serde_json::json!([])))),
            ["popup_closed"]
        );
    }

    #[tokio::test]
    async fn streams_filter_by_session_and_resync_after_falling_behind() {
        let [a, b, wa, wb, pa, pb] = ids();
        let both = vec![
            session(a, "a", vec![window(wa, "w", &[pa])]),
            session(b, "b", vec![window(wb, "w", &[pb])]),
        ];
        let feed = Arc::new(Feed {
            latest: Mutex::new((Arc::new(both.clone()), broadcast::channel(2).0)),
        });
        let (sessions, batches) = feed.subscribe();
        let mut stream = Stream {
            feed: feed.clone(),
            batches,
            sessions,
            session: Some(a),
            ended: false,
            pending: VecDeque::new(),
            heartbeat: tokio::time::interval_at(tokio::time::Instant::now() + HEARTBEAT, HEARTBEAT),
            shutdown: SHUTDOWN.subscribe(),
        };
        let mut v = view(both.clone());
        let send = |events: Vec<EventKind>, sessions: Arc<Vec<SessionSnapshot>>| {
            let mut latest = feed.latest.lock().unwrap();
            latest.0 = sessions.clone();
            let _ = latest.1.send(Arc::new(Batch {
                events,
                sessions,
                lost: false,
            }));
        };
        // Session b goes away, unseen by a's stream.
        let events = v.state(both[..1].to_vec());
        send(events, v.sessions.clone());
        // Then a falls three batches behind a backlog of two.
        for name in ["x", "y", "z"] {
            let mut renamed = (*v.sessions).clone();
            renamed[0].name = name.into();
            let events = v.state(renamed);
            send(events, v.sessions.clone());
        }
        let mut lines = Vec::new();
        while let Some(line) = tokio::time::timeout(Duration::from_millis(50), stream.next())
            .await
            .ok()
            .flatten()
        {
            let event: Value = serde_json::from_str(&line).unwrap();
            lines.push(event["type"].as_str().unwrap().to_owned());
        }
        assert_eq!(lines, ["resync", "session_renamed"]);
        assert!(!stream.ended);

        let events = v.state(Vec::new());
        send(events, v.sessions.clone());
        let mut closed = Vec::new();
        while let Some(line) = stream.next().await {
            let event: Value = serde_json::from_str(&line).unwrap();
            closed.push(event["type"].as_str().unwrap().to_owned());
        }
        assert_eq!(closed, ["pane_closed", "window_closed", "session_closed"]);
        assert!(stream.ended);
    }

    #[test]
    fn human_lines_name_the_place_and_escape_control_characters() {
        let event = serde_json::json!({
            "time": 0, "type": "agent_status_changed", "session": "work", "window_index": 1,
            "window": "edit", "pane_id": "p", "previous": "working",
            "status": { "state": "done", "agent": "claude", "message": null },
        });
        assert_eq!(
            human(&event, time::UtcOffset::UTC),
            "00:00:00  agent_status_changed  work:1 (edit) pane p  claude working -> done"
        );
        let event = serde_json::json!({
            "time": 0, "type": "notification", "level": "info",
            "title": "two\nlines", "body": "\u{1b}]0;title\u{7}\u{202e}",
        });
        assert_eq!(
            human(&event, time::UtcOffset::UTC),
            "00:00:00  notification  [info] two\\nlines: \\u{1b}]0;title\\u{7}\\u{202e}"
        );
    }
}
