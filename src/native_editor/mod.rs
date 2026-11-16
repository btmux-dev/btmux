//! The native editor's server side: one authoritative buffer per open file,
//! edited by CodeMirror views in any number of tabs and panes through
//! `@codemirror/collab` (`/ws/editor`).

pub mod buffer;
pub mod text;

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex};

use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;
use uuid::Uuid;

use buffer::{Buffer, DiskStatus, LineEnding, SaveError, Update};

/// Messages a socket may queue before it is dropped as lagging; the tab
/// reconnects and catches up.
const SOCKET_QUEUE: usize = 1024;

/// From a tab.
#[derive(Deserialize, Debug)]
#[serde(tag = "type", rename_all = "snake_case")]
#[cfg_attr(test, derive(ts_rs::TS))]
pub enum EditorClientMessage {
    /// Open `path` (absolute) and follow its updates. With `resume`, a tab
    /// that had the buffer at that version catches up instead.
    Open {
        req: u32,
        path: String,
        resume: Option<Resume>,
    },
    /// Stop following a buffer.
    Release { buffer: Uuid },
    /// Updates made on top of `version`.
    Push {
        buffer: Uuid,
        #[cfg_attr(test, ts(type = "number"))]
        version: u64,
        updates: Vec<Update>,
    },
    /// Write the buffer; `force` overwrites changes made on disk meanwhile.
    Save { req: u32, buffer: Uuid, force: bool },
    /// Discard unsaved changes, reloading the file.
    Revert { req: u32, buffer: Uuid },
}

#[derive(Deserialize, Debug)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct Resume {
    pub buffer: Uuid,
    #[cfg_attr(test, ts(type = "number"))]
    pub version: u64,
}

/// To a tab.
#[derive(Serialize, Debug)]
#[serde(tag = "type", rename_all = "snake_case")]
#[cfg_attr(test, derive(ts_rs::TS))]
pub enum EditorServerMessage {
    /// The buffer from scratch.
    Opened { req: u32, buffer: BufferInfo },
    /// The resumed buffer; `updates` with what was missed follow.
    Resumed { req: u32, buffer: Uuid },
    /// Updates starting at `version`, in order.
    Updates {
        buffer: Uuid,
        #[cfg_attr(test, ts(type = "number"))]
        version: u64,
        updates: Vec<Update>,
    },
    BufferState {
        buffer: Uuid,
        dirty: bool,
        disk: DiskStatus,
    },
    /// Every buffer, on connect and on change.
    Buffers { buffers: Vec<BufferSummary> },
    Result {
        req: u32,
        error: Option<String>,
        /// The file changed on disk since it was loaded or saved.
        conflict: bool,
    },
}

#[derive(Serialize, Debug)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct BufferInfo {
    pub id: Uuid,
    pub path: String,
    pub text: String,
    #[cfg_attr(test, ts(type = "number"))]
    pub version: u64,
    pub dirty: bool,
    pub disk: DiskStatus,
    pub line_ending: LineEnding,
}

#[derive(Serialize, Debug, Clone, PartialEq)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct BufferSummary {
    pub id: Uuid,
    pub path: String,
    pub dirty: bool,
}

type SocketId = u64;

#[derive(Default)]
struct Inner {
    buffers: HashMap<Uuid, Buffer>,
    by_path: HashMap<PathBuf, Uuid>,
    /// Sockets following each buffer.
    subs: HashMap<Uuid, HashSet<SocketId>>,
    sockets: HashMap<SocketId, mpsc::Sender<String>>,
    next_socket: SocketId,
    /// The summaries last sent, so unchanged lists aren't resent.
    summaries: Vec<BufferSummary>,
    watcher: Option<watch::Watcher>,
}

pub struct Hub {
    inner: Mutex<Inner>,
}

static HUB: LazyLock<Hub> = LazyLock::new(Hub::new);

pub fn hub() -> &'static Hub {
    &HUB
}

/// Refuse to drop unsaved changes (an upgrade).
pub fn check_saved() -> Result<(), String> {
    let inner = hub().inner.lock().unwrap();
    let mut dirty: Vec<_> = inner
        .buffers
        .values()
        .filter(|b| b.dirty())
        .map(|b| b.path.display().to_string())
        .collect();
    if dirty.is_empty() {
        return Ok(());
    }
    dirty.sort();
    Err(format!(
        "The editor has unsaved changes: {}",
        dirty.join(", ")
    ))
}

/// Apply `op` to the editor over `pane_id`, telling every tab.
pub async fn view(
    state: &crate::AppState,
    pane_id: Uuid,
    mut op: crate::session::EditorViewOp,
) -> Result<(), String> {
    use crate::session::EditorViewOp;
    // Symlinked paths name one buffer, so one tab.
    if let EditorViewOp::Open { path, .. }
    | EditorViewOp::Close { path }
    | EditorViewOp::Activate { path }
    | EditorViewOp::Move { path, .. } = &mut op
    {
        let raw = path.clone();
        if let Ok(Ok(resolved)) = tokio::task::spawn_blocking(move || resolve(&raw)).await {
            *path = crate::fs_ops::client_path(&resolved);
        }
    }
    let reveal = match &op {
        EditorViewOp::Open {
            path,
            line: Some(line),
        } => Some(crate::ws::control::ServerMessage::EditorReveal {
            pane_id,
            path: path.clone(),
            line: *line,
        }),
        _ => None,
    };
    let mut mgr = state.write().await;
    let pane = mgr.find_pane_mut(pane_id).ok_or("Pane no longer exists")?;
    pane.editor.apply(op);
    crate::ws::control::broadcast_state(&mgr);
    if let Some(reveal) = reveal {
        let _ = mgr.events().send(serde_json::to_string(&reveal).unwrap());
    }
    Ok(())
}

/// An absolute path with symlinks resolved; the file itself may not exist.
fn resolve(path: &str) -> Result<PathBuf, String> {
    let path = match path.strip_prefix("~/") {
        Some(rest) => dirs::home_dir().ok_or("No home directory")?.join(rest),
        None => PathBuf::from(path),
    };
    if !path.is_absolute() {
        return Err(format!("{} is not an absolute path", path.display()));
    }
    #[cfg(windows)]
    let canonical = |p: &Path| dunce::canonicalize(p);
    #[cfg(not(windows))]
    let canonical = |p: &Path| p.canonicalize();
    match canonical(&path) {
        Ok(path) => Ok(path),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            let (Some(parent), Some(name)) = (path.parent(), path.file_name()) else {
                return Err(format!("Cannot open {}", path.display()));
            };
            let parent =
                canonical(parent).map_err(|e| format!("Cannot open {}: {e}", path.display()))?;
            Ok(parent.join(name))
        }
        Err(e) => Err(format!("Cannot open {}: {e}", path.display())),
    }
}

fn to_json(msg: &EditorServerMessage) -> String {
    serde_json::to_string(msg).expect("editor messages serialize")
}

impl Hub {
    fn new() -> Self {
        Self {
            inner: Mutex::new(Inner::default()),
        }
    }

    /// Register a socket; its messages arrive on the receiver.
    pub fn connect(&self) -> (SocketId, mpsc::Receiver<String>) {
        let (tx, rx) = mpsc::channel(SOCKET_QUEUE);
        let mut inner = self.inner.lock().unwrap();
        let id = inner.next_socket;
        inner.next_socket += 1;
        let _ = tx.try_send(to_json(&EditorServerMessage::Buffers {
            buffers: inner.summaries(),
        }));
        inner.sockets.insert(id, tx);
        (id, rx)
    }

    pub fn disconnect(&self, socket: SocketId) {
        let mut inner = self.inner.lock().unwrap();
        inner.sockets.remove(&socket);
        let followed: Vec<Uuid> = inner
            .subs
            .iter()
            .filter(|(_, sockets)| sockets.contains(&socket))
            .map(|(id, _)| *id)
            .collect();
        for id in followed {
            inner.release(socket, id);
        }
        inner.send_summaries();
    }

    pub async fn handle(&'static self, socket: SocketId, msg: EditorClientMessage) {
        match msg {
            EditorClientMessage::Open { req, path, resume } => {
                if let Err(error) = self.open(socket, req, &path, resume).await {
                    self.reply(socket, req, Some(error), false);
                }
            }
            EditorClientMessage::Release { buffer } => {
                let mut inner = self.inner.lock().unwrap();
                inner.release(socket, buffer);
                inner.send_summaries();
            }
            EditorClientMessage::Push {
                buffer,
                version,
                updates,
            } => self.push(socket, buffer, version, updates),
            EditorClientMessage::Save { req, buffer, force } => {
                let (error, conflict) = match self.save(buffer, force).await {
                    Ok(()) => (None, false),
                    Err(SaveError::Conflict) => (
                        Some("The file changed on disk since it was opened".into()),
                        true,
                    ),
                    Err(SaveError::Io(e)) => (Some(e), false),
                };
                self.reply(socket, req, error, conflict);
            }
            EditorClientMessage::Revert { req, buffer } => {
                let error = self.revert(buffer).await.err();
                self.reply(socket, req, error, false);
            }
        }
    }

    fn reply(&self, socket: SocketId, req: u32, error: Option<String>, conflict: bool) {
        let inner = self.inner.lock().unwrap();
        inner.send_to(
            socket,
            &EditorServerMessage::Result {
                req,
                error,
                conflict,
            },
        );
    }

    async fn open(
        &'static self,
        socket: SocketId,
        req: u32,
        path: &str,
        resume: Option<Resume>,
    ) -> Result<(), String> {
        let path = resolve(path)?;
        let existing = self.inner.lock().unwrap().by_path.get(&path).copied();
        let loaded = match existing {
            Some(_) => None,
            None => {
                let p = path.clone();
                Some(
                    tokio::task::spawn_blocking(move || buffer::load(&p))
                        .await
                        .map_err(|e| e.to_string())??,
                )
            }
        };
        let mut inner = self.inner.lock().unwrap();
        // Another open may have loaded it meanwhile.
        let id = match inner.by_path.get(&path) {
            Some(id) => *id,
            None => {
                let Some(loaded) = loaded else {
                    return Err("The buffer closed while opening".into());
                };
                let buffer = Buffer::new(path.clone(), loaded);
                let id = buffer.id;
                inner.by_path.insert(path.clone(), id);
                inner.buffers.insert(id, buffer);
                inner.watch(&path);
                id
            }
        };
        inner.subs.entry(id).or_default().insert(socket);
        let buffer = &inner.buffers[&id];
        let missed = resume
            .filter(|r| r.buffer == id)
            .and_then(|r| Some((r.version, buffer.since(r.version)?)));
        match missed {
            Some((version, updates)) => {
                inner.send_to(socket, &EditorServerMessage::Resumed { req, buffer: id });
                inner.send_to(
                    socket,
                    &EditorServerMessage::Updates {
                        buffer: id,
                        version,
                        updates,
                    },
                );
                let buffer = &inner.buffers[&id];
                inner.send_to(socket, &state_message(buffer));
            }
            None => {
                let info = BufferInfo {
                    id,
                    path: crate::fs_ops::client_path(&buffer.path),
                    text: buffer.rope.to_string(),
                    version: buffer.version,
                    dirty: buffer.dirty(),
                    disk: buffer.disk_status,
                    line_ending: buffer.line_ending,
                };
                inner.send_to(socket, &EditorServerMessage::Opened { req, buffer: info });
            }
        }
        inner.send_summaries();
        Ok(())
    }

    fn push(&self, socket: SocketId, id: Uuid, version: u64, updates: Vec<Update>) {
        let mut inner = self.inner.lock().unwrap();
        let Some(buffer) = inner.buffers.get_mut(&id) else {
            return;
        };
        let was_dirty = buffer.dirty();
        match buffer.push(version, &updates) {
            Ok(true) => {}
            // Stale: the tab gets the updates it lacks and pushes again.
            Ok(false) => return,
            Err(error) => {
                tracing::warn!(path = %buffer.path.display(), "invalid editor update: {error}");
                // Make the tab start over from the server's text.
                inner.drop_socket(socket);
                return;
            }
        }
        let dirty = buffer.dirty();
        let state = state_message(buffer);
        inner.send_to_followers(
            id,
            &EditorServerMessage::Updates {
                buffer: id,
                version,
                updates,
            },
        );
        if dirty != was_dirty {
            inner.send_to_followers(id, &state);
            inner.send_summaries();
        }
    }

    async fn save(&self, id: Uuid, force: bool) -> Result<(), SaveError> {
        let (path, text, version, line_ending, bom, disk) = {
            let inner = self.inner.lock().unwrap();
            let b = inner
                .buffers
                .get(&id)
                .ok_or_else(|| SaveError::Io("The buffer is closed".into()))?;
            (
                b.path.clone(),
                b.rope.to_string(),
                b.version,
                b.line_ending,
                b.bom,
                b.disk,
            )
        };
        let disk = tokio::task::spawn_blocking(move || {
            buffer::save(&path, &text, line_ending, bom, disk, force)
        })
        .await
        .map_err(|e| SaveError::Io(e.to_string()))??;
        let mut inner = self.inner.lock().unwrap();
        let Some(b) = inner.buffers.get_mut(&id) else {
            return Ok(());
        };
        b.saved_version = version;
        b.disk = disk;
        b.disk_status = DiskStatus::Same;
        let state = state_message(b);
        inner.send_to_followers(id, &state);
        inner.collect(id);
        inner.send_summaries();
        Ok(())
    }

    async fn revert(&self, id: Uuid) -> Result<(), String> {
        let path = {
            let inner = self.inner.lock().unwrap();
            inner
                .buffers
                .get(&id)
                .ok_or("The buffer is closed")?
                .path
                .clone()
        };
        let loaded = tokio::task::spawn_blocking(move || buffer::load(&path))
            .await
            .map_err(|e| e.to_string())??;
        let mut inner = self.inner.lock().unwrap();
        inner.reload(id, loaded);
        inner.collect(id);
        inner.send_summaries();
        Ok(())
    }

    /// Something changed `path` on disk: reload a clean buffer, flag a dirty one.
    async fn check_disk(&self, path: PathBuf) {
        let Some((id, known)) = ({
            let inner = self.inner.lock().unwrap();
            inner
                .by_path
                .get(&path)
                .map(|id| (*id, inner.buffers[id].disk))
        }) else {
            return;
        };
        let p = path.clone();
        let Ok(Some(change)) =
            tokio::task::spawn_blocking(move || buffer::changed_on_disk(&p, known)).await
        else {
            return;
        };
        let mut inner = self.inner.lock().unwrap();
        let Some(b) = inner.buffers.get_mut(&id) else {
            return;
        };
        // A save or another check got there first.
        if b.disk != known {
            return;
        }
        match change {
            Some(loaded) if !b.dirty() => inner.reload(id, loaded),
            Some(_) => {
                b.disk_status = DiskStatus::Changed;
                let state = state_message(b);
                inner.send_to_followers(id, &state);
            }
            None => {
                b.disk_status = DiskStatus::Deleted;
                let state = state_message(b);
                inner.send_to_followers(id, &state);
            }
        }
        inner.send_summaries();
    }
}

fn state_message(b: &Buffer) -> EditorServerMessage {
    EditorServerMessage::BufferState {
        buffer: b.id,
        dirty: b.dirty(),
        disk: b.disk_status,
    }
}

impl Inner {
    fn summaries(&self) -> Vec<BufferSummary> {
        let mut list: Vec<_> = self
            .buffers
            .values()
            .map(|b| BufferSummary {
                id: b.id,
                path: crate::fs_ops::client_path(&b.path),
                dirty: b.dirty(),
            })
            .collect();
        list.sort_by(|a, b| a.path.cmp(&b.path));
        list
    }

    fn send_summaries(&mut self) {
        let summaries = self.summaries();
        if summaries == self.summaries {
            return;
        }
        let msg = to_json(&EditorServerMessage::Buffers {
            buffers: summaries.clone(),
        });
        self.summaries = summaries;
        let sockets: Vec<_> = self.sockets.keys().copied().collect();
        for socket in sockets {
            self.send_raw(socket, msg.clone());
        }
    }

    fn send_to(&self, socket: SocketId, msg: &EditorServerMessage) {
        if let Some(tx) = self.sockets.get(&socket) {
            let _ = tx.try_send(to_json(msg));
        }
    }

    fn send_raw(&mut self, socket: SocketId, msg: String) {
        let Some(tx) = self.sockets.get(&socket) else {
            return;
        };
        if let Err(mpsc::error::TrySendError::Full(_)) = tx.try_send(msg) {
            self.drop_socket(socket);
        }
    }

    fn send_to_followers(&mut self, id: Uuid, msg: &EditorServerMessage) {
        let msg = to_json(msg);
        let followers: Vec<_> = self.subs.get(&id).into_iter().flatten().copied().collect();
        for socket in followers {
            self.send_raw(socket, msg.clone());
        }
    }

    /// Close a lagging or misbehaving socket; its tab reconnects and catches up.
    fn drop_socket(&mut self, socket: SocketId) {
        self.sockets.remove(&socket);
        let followed: Vec<Uuid> = self
            .subs
            .iter()
            .filter(|(_, s)| s.contains(&socket))
            .map(|(id, _)| *id)
            .collect();
        for id in followed {
            self.release(socket, id);
        }
    }

    fn release(&mut self, socket: SocketId, id: Uuid) {
        if let Some(subs) = self.subs.get_mut(&id) {
            subs.remove(&socket);
        }
        self.collect(id);
    }

    /// Drop a buffer nobody follows, unless it has unsaved changes.
    fn collect(&mut self, id: Uuid) {
        if self.subs.get(&id).is_some_and(|s| !s.is_empty()) {
            return;
        }
        if self.buffers.get(&id).is_none_or(|b| b.dirty()) {
            return;
        }
        self.subs.remove(&id);
        if let Some(b) = self.buffers.remove(&id) {
            self.by_path.remove(&b.path);
            self.unwatch(&b.path);
        }
    }

    fn reload(&mut self, id: Uuid, loaded: buffer::Loaded) {
        let Some(b) = self.buffers.get_mut(&id) else {
            return;
        };
        let version = b.version;
        let update = b.reload(loaded);
        let state = state_message(b);
        if let Some(update) = update {
            self.send_to_followers(
                id,
                &EditorServerMessage::Updates {
                    buffer: id,
                    version,
                    updates: vec![update],
                },
            );
        }
        self.send_to_followers(id, &state);
    }

    fn watch(&mut self, path: &Path) {
        let Some(dir) = path.parent() else { return };
        self.watcher
            .get_or_insert_with(watch::Watcher::new)
            .watch(dir);
    }

    fn unwatch(&mut self, path: &Path) {
        if let (Some(dir), Some(watcher)) = (path.parent(), self.watcher.as_mut()) {
            watcher.unwatch(dir);
        }
    }
}

mod watch {
    //! Directory watches for open buffers' files, counted per directory.

    use std::collections::HashMap;
    use std::path::{Path, PathBuf};
    use std::time::Duration;

    use notify::{EventKind, RecursiveMode, Watcher as _};
    use tokio::sync::mpsc;

    pub struct Watcher {
        watcher: Option<notify::RecommendedWatcher>,
        counts: HashMap<PathBuf, usize>,
    }

    impl Watcher {
        pub fn new() -> Self {
            let (tx, mut rx) = mpsc::unbounded_channel::<PathBuf>();
            let watcher =
                notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
                    let Ok(event) = event else { return };
                    if matches!(
                        event.kind,
                        EventKind::Access(_)
                            | EventKind::Modify(notify::event::ModifyKind::Metadata(_))
                    ) {
                        return;
                    }
                    for path in event.paths {
                        let _ = tx.send(path);
                    }
                })
                .inspect_err(|e| tracing::warn!("cannot watch editor files: {e}"))
                .ok();
            tokio::spawn(async move {
                while let Some(path) = rx.recv().await {
                    // Let a burst of events (a write, then a rename) settle.
                    tokio::time::sleep(Duration::from_millis(50)).await;
                    let mut paths = vec![path];
                    while let Ok(path) = rx.try_recv() {
                        if !paths.contains(&path) {
                            paths.push(path);
                        }
                    }
                    for path in paths {
                        super::hub().check_disk(path).await;
                    }
                }
            });
            Self {
                watcher,
                counts: HashMap::new(),
            }
        }

        pub fn watch(&mut self, dir: &Path) {
            let count = self.counts.entry(dir.to_path_buf()).or_default();
            *count += 1;
            if *count == 1 {
                if let Some(watcher) = self.watcher.as_mut() {
                    if let Err(e) = watcher.watch(dir, RecursiveMode::NonRecursive) {
                        tracing::warn!(dir = %dir.display(), "cannot watch: {e}");
                    }
                }
            }
        }

        pub fn unwatch(&mut self, dir: &Path) {
            let Some(count) = self.counts.get_mut(dir) else {
                return;
            };
            *count -= 1;
            if *count == 0 {
                self.counts.remove(dir);
                if let Some(watcher) = self.watcher.as_mut() {
                    let _ = watcher.unwatch(dir);
                }
            }
        }
    }
}
