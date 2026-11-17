//! The native editor's server side: one authoritative buffer per open file,
//! edited by CodeMirror views in any number of tabs and panes through
//! `@codemirror/collab` (`/ws/editor`).

pub mod buffer;
pub mod lsp;
pub mod text;

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::mpsc;
use uuid::Uuid;

use buffer::{Buffer, DiskStatus, LineEnding, OnDisk, SaveError, Update};

/// Messages a socket may queue before it is dropped as lagging; the tab
/// reconnects and catches up.
const SOCKET_QUEUE: usize = 1024;
/// How long a buffer loaded for automation stays without a tab following it.
const UNFOLLOWED: std::time::Duration = std::time::Duration::from_secs(60);

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
    /// A JSON-RPC message for language server `server`. A request's
    /// positions are in `buffer` at `version`.
    Lsp {
        #[cfg_attr(test, ts(type = "number"))]
        server: u64,
        #[cfg_attr(test, ts(type = "unknown"))]
        message: Value,
        buffer: Option<Uuid>,
        #[cfg_attr(test, ts(type = "number | null"))]
        version: Option<u64>,
    },
    /// Answer a server's `window/showMessageRequest`; `action` is the chosen title.
    LspPromptAnswer {
        #[cfg_attr(test, ts(type = "number"))]
        server: u64,
        #[cfg_attr(test, ts(type = "number"))]
        token: u64,
        action: Option<String>,
    },
    /// Apply an LSP `WorkspaceEdit` (a rename) to buffers, loading files as
    /// needed; its positions are in `buffer` at `version`.
    ApplyEdit {
        req: u32,
        #[cfg_attr(test, ts(type = "unknown"))]
        edit: Value,
        buffer: Option<Uuid>,
        #[cfg_attr(test, ts(type = "number | null"))]
        version: Option<u64>,
    },
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
    /// A JSON-RPC message from language server `server`.
    Lsp {
        #[cfg_attr(test, ts(type = "number"))]
        server: u64,
        #[cfg_attr(test, ts(type = "unknown"))]
        message: Value,
    },
    /// Every language server, on connect and on change.
    LspStatus { servers: Vec<lsp::LspServerStatus> },
    /// A server asks the user to choose one of `actions` (`window/showMessageRequest`).
    LspPrompt {
        #[cfg_attr(test, ts(type = "number"))]
        server: u64,
        #[cfg_attr(test, ts(type = "number"))]
        token: u64,
        /// LSP `MessageType`: 1 error, 2 warning, 3 info, 4 log.
        level: u8,
        message: String,
        actions: Vec<String>,
    },
    /// The prompt was answered, or its server exited.
    LspPromptDone {
        #[cfg_attr(test, ts(type = "number"))]
        server: u64,
        #[cfg_attr(test, ts(type = "number"))]
        token: u64,
    },
    /// A server asks to show a document (`window/showDocument`).
    LspShowDocument {
        uri: String,
        /// In the browser rather than the editor.
        external: bool,
        #[cfg_attr(test, ts(type = "unknown"))]
        selection: Option<Value>,
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
    pub language_id: Option<String>,
    /// The `file://` URI language servers know it by.
    pub uri: String,
    /// Its language servers.
    #[cfg_attr(test, ts(type = "Array<number>"))]
    pub servers: Vec<u64>,
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
    lsp: lsp::Lsp,
}

pub struct Hub {
    inner: Mutex<Inner>,
}

static HUB: LazyLock<Hub> = LazyLock::new(Hub::new);

pub fn hub() -> &'static Hub {
    &HUB
}

/// Load `path` and start its language servers, with no tab open (automation).
pub async fn load(path: &str) -> Result<(), String> {
    let path = resolve(path)?;
    let id = hub().load_buffer(path).await?;
    // Dropped again if no tab follows it by then.
    tokio::spawn(async move {
        tokio::time::sleep(UNFOLLOWED).await;
        let mut inner = hub().inner.lock().unwrap();
        inner.collect(id);
        inner.send_summaries();
    });
    Ok(())
}

/// Open buffers, for automation.
pub fn buffers() -> Vec<serde_json::Value> {
    let inner = hub().inner.lock().unwrap();
    let mut list: Vec<_> = inner
        .buffers
        .values()
        .map(|b| {
            let servers: Vec<_> = b
                .servers
                .iter()
                .filter_map(|id| inner.lsp_statuses().into_iter().find(|s| s.id == *id))
                .collect();
            serde_json::json!({
                "path": crate::fs_ops::client_path(&b.path),
                "unsaved": b.dirty(),
                "changed_on_disk": b.disk_status != DiskStatus::Same,
                "language": b.language_id,
                "language_servers": servers,
            })
        })
        .collect();
    list.sort_by(|a, b| a["path"].as_str().cmp(&b["path"].as_str()));
    list
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
        let _ = tx.try_send(to_json(&EditorServerMessage::LspStatus {
            servers: inner.lsp_statuses(),
        }));
        inner.sockets.insert(id, tx);
        (id, rx)
    }

    pub fn disconnect(&self, socket: SocketId) {
        let mut inner = self.inner.lock().unwrap();
        inner.sockets.remove(&socket);
        inner.lsp_disconnected(socket);
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
            EditorClientMessage::Lsp {
                server,
                message,
                buffer,
                version,
            } => {
                let mut inner = self.inner.lock().unwrap();
                inner.lsp_from_tab(socket, server, message, buffer, version);
            }
            EditorClientMessage::LspPromptAnswer {
                server,
                token,
                action,
            } => self.lsp_prompt_answer(server, token, action),
            EditorClientMessage::ApplyEdit {
                req,
                edit,
                buffer,
                version,
            } => {
                let expect = buffer.zip(version);
                let error = self.apply_edit(edit, expect).await.err();
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
        let mut tries = 0;
        let (mut inner, id) = loop {
            let id = self.load_buffer(path.clone()).await?;
            let inner = self.inner.lock().unwrap();
            // A release may have dropped it meanwhile.
            if inner.buffers.contains_key(&id) {
                break (inner, id);
            }
            tries += 1;
            if tries == 3 {
                return Err("The buffer closed while opening".into());
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
                    language_id: buffer.language_id.clone(),
                    uri: lsp::config::uri(&buffer.path),
                    servers: buffer.servers.clone(),
                };
                inner.send_to(socket, &EditorServerMessage::Opened { req, buffer: info });
            }
        }
        inner.send_summaries();
        Ok(())
    }

    /// The buffer for `path` (canonical), loading it and starting its
    /// language servers if it isn't open.
    async fn load_buffer(&'static self, path: PathBuf) -> Result<Uuid, String> {
        if let Some(id) = self.inner.lock().unwrap().by_path.get(&path) {
            return Ok(*id);
        }
        let p = path.clone();
        let config = lsp::config::current();
        let (loaded, target) = tokio::task::spawn_blocking(move || {
            (buffer::load(&p), lsp::config::target(&config, &p))
        })
        .await
        .map_err(|e| e.to_string())?;
        let loaded = loaded?;
        let mut inner = self.inner.lock().unwrap();
        // Another open may have loaded it meanwhile.
        if let Some(id) = inner.by_path.get(&path) {
            return Ok(*id);
        }
        let buffer = Buffer::new(path.clone(), loaded);
        let id = buffer.id;
        inner.by_path.insert(path.clone(), id);
        inner.buffers.insert(id, buffer);
        inner.watch(&path);
        if let Some(target) = target {
            inner.lsp_attach(id, target);
        }
        Ok(id)
    }

    /// Apply an LSP `WorkspaceEdit`. With `expect`, its positions are in
    /// that buffer at that version, which must still be current.
    async fn apply_edit(
        &'static self,
        edit: Value,
        expect: Option<(Uuid, u64)>,
    ) -> Result<(), String> {
        // Each file's edits, and the version they were made against, if given.
        let mut files: Vec<(String, Vec<Value>, Option<u64>)> = Vec::new();
        // `documentChanges` is preferred when a server sends both.
        if let Some(changes) = edit["documentChanges"].as_array() {
            for change in changes {
                if change.get("kind").is_some() {
                    return Err("Creating, renaming, or deleting files isn't supported".into());
                }
                let document = &change["textDocument"];
                let uri = document["uri"].as_str().unwrap_or_default();
                let version = match &document["version"] {
                    Value::Null => None,
                    v => Some(v.as_u64().ok_or("Invalid document version")?),
                };
                let edits = change["edits"].as_array().cloned().unwrap_or_default();
                files.push((uri.to_owned(), edits, version));
            }
        } else if let Some(changes) = edit["changes"].as_object() {
            for (uri, edits) in changes {
                let edits = edits.as_array().cloned().unwrap_or_default();
                files.push((uri.clone(), edits, None));
            }
        }
        let mut targets = Vec::new();
        let mut loaded = Ok(());
        for (uri, edits, version) in files {
            let id = match lsp::config::path(&uri)
                .ok_or_else(|| format!("Cannot edit {uri}"))
                .and_then(|path| resolve(&path.to_string_lossy()))
            {
                Ok(path) => self.load_buffer(path).await,
                Err(e) => Err(e),
            };
            match id {
                Ok(id) => targets.push((id, edits, version)),
                Err(e) => {
                    loaded = Err(e);
                    break;
                }
            }
        }
        let ids: Vec<Uuid> = targets.iter().map(|(id, ..)| *id).collect();
        let mut inner = self.inner.lock().unwrap();
        let result = loaded.and_then(|()| inner.apply_edits(targets, expect));
        // Also buffers loaded for an edit that failed.
        for id in ids {
            inner.collect(id);
        }
        inner.send_summaries();
        result
    }

    fn push(&self, socket: SocketId, id: Uuid, version: u64, updates: Vec<Update>) {
        let mut inner = self.inner.lock().unwrap();
        let Some(buffer) = inner.buffers.get_mut(&id) else {
            return;
        };
        let was_dirty = buffer.dirty();
        let pushed = match buffer.push(version, &updates) {
            Ok(Some(pushed)) => pushed,
            // Stale: the tab gets the updates it lacks and pushes again.
            Ok(None) => return,
            Err(error) => {
                tracing::warn!(path = %buffer.path.display(), "invalid editor update: {error}");
                // Make the tab start over from the server's text.
                inner.drop_socket(socket);
                return;
            }
        };
        if pushed.updates.is_empty() {
            return;
        }
        let dirty = buffer.dirty();
        let state = state_message(buffer);
        inner.send_to_followers(
            id,
            &EditorServerMessage::Updates {
                buffer: id,
                version: pushed.version,
                updates: pushed.updates,
            },
        );
        inner.lsp_changed(id, &pushed.applied);
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
        inner.lsp_saved(id);
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
            OnDisk::Changed(Ok(loaded)) if !b.dirty() => inner.reload(id, loaded),
            // Dirty, or not text any more: keep the buffer, flag the file.
            OnDisk::Changed(_) => {
                b.disk_status = DiskStatus::Changed;
                let state = state_message(b);
                inner.send_to_followers(id, &state);
            }
            OnDisk::Deleted => {
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
    /// Apply each buffer's LSP `TextEdit`s, all or none.
    fn apply_edits(
        &mut self,
        targets: Vec<(Uuid, Vec<Value>, Option<u64>)>,
        expect: Option<(Uuid, u64)>,
    ) -> Result<(), String> {
        // Edits made against an older version would land in the wrong place.
        let stale =
            |id: &Uuid, version: u64| self.buffers.get(id).map(|b| b.version) != Some(version);
        if expect.is_some_and(|(id, version)| stale(&id, version))
            || targets
                .iter()
                .any(|(id, _, version)| version.is_some_and(|v| stale(id, v)))
        {
            return Err("The file changed meanwhile".into());
        }
        // Check every file's edits before changing any. A file named again
        // is edited as the earlier edits left it.
        let mut ropes: HashMap<Uuid, ropey::Rope> = HashMap::new();
        let mut planned = Vec::new();
        for (id, edits, _) in targets {
            let b = self.buffers.get(&id).ok_or("A file closed meanwhile")?;
            let rope = ropes.entry(id).or_insert_with(|| b.rope.clone());
            let mut changes = Vec::with_capacity(edits.len());
            for edit in &edits {
                let range = &edit["range"];
                let (Some(from), Some(to), Some(insert)) = (
                    buffer::offset(rope, &range["start"]),
                    buffer::offset(rope, &range["end"]),
                    edit["newText"].as_str(),
                ) else {
                    return Err("Invalid edit".into());
                };
                if from > to {
                    return Err("Invalid edit".into());
                }
                let insert = insert.replace("\r\n", "\n").replace('\r', "\n");
                changes.push(text::Change { from, to, insert });
            }
            // Stable, so inserts at one position keep their order.
            changes.sort_by_key(|c| (c.from, c.to));
            if changes.windows(2).any(|w| w[0].to > w[1].from) {
                return Err("Overlapping edits".into());
            }
            text::apply(rope, &changes);
            planned.push((id, changes));
        }
        for (id, changes) in &planned {
            if changes.is_empty() {
                continue;
            }
            let b = self.buffers.get_mut(id).expect("checked");
            let version = b.version;
            let (update, applied) = b.edit(changes);
            let state = state_message(b);
            self.send_to_followers(
                *id,
                &EditorServerMessage::Updates {
                    buffer: *id,
                    version,
                    updates: vec![update],
                },
            );
            self.send_to_followers(*id, &state);
            self.lsp_changed(*id, &[applied]);
        }
        Ok(())
    }

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
        self.lsp_disconnected(socket);
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
            self.lsp_closed(id, &b.path, &b.servers);
        }
    }

    fn reload(&mut self, id: Uuid, loaded: buffer::Loaded) {
        let Some(b) = self.buffers.get_mut(&id) else {
            return;
        };
        let version = b.version;
        let update = b.reload(loaded);
        let state = state_message(b);
        if let Some((update, applied)) = update {
            self.send_to_followers(
                id,
                &EditorServerMessage::Updates {
                    buffer: id,
                    version,
                    updates: vec![update],
                },
            );
            self.lsp_changed(id, &[applied]);
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[tokio::test]
    async fn refuses_edits_to_older_versions() {
        let dir = std::env::temp_dir().join(format!("btmux-edit-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("f.txt");
        std::fs::write(&path, "abc\n").unwrap();
        let path = resolve(&path.to_string_lossy()).unwrap();
        let id = hub().load_buffer(path.clone()).await.unwrap();
        // Version 1, typed meanwhile.
        hub()
            .inner
            .lock()
            .unwrap()
            .buffers
            .get_mut(&id)
            .unwrap()
            .edit(&[text::Change {
                from: 0,
                to: 0,
                insert: "X".into(),
            }]);
        let uri = lsp::config::uri(&path);
        let edit = |version: u64| {
            json!({ "documentChanges": [{
                "textDocument": { "uri": uri, "version": version },
                "edits": [{ "range": {
                    "start": { "line": 0, "character": 0 },
                    "end": { "line": 0, "character": 1 },
                }, "newText": "A" }],
            }] })
        };
        let text = || hub().inner.lock().unwrap().buffers[&id].rope.to_string();
        assert!(hub().apply_edit(edit(0), None).await.is_err());
        assert_eq!(text(), "Xabc\n");
        hub().apply_edit(edit(1), None).await.unwrap();
        assert_eq!(text(), "Aabc\n");
        let mut inner = hub().inner.lock().unwrap();
        inner.buffers.remove(&id);
        inner.by_path.remove(&path);
        drop(inner);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn edits_apply_in_order_or_not_at_all() {
        let dir = std::env::temp_dir().join(format!("btmux-edit-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("f.txt");
        std::fs::write(&path, "abc\n").unwrap();
        let path = resolve(&path.to_string_lossy()).unwrap();
        let uri = lsp::config::uri(&path);
        let change = |from: u64, to: u64, text: &str| {
            json!({
                "textDocument": { "uri": uri, "version": null },
                "edits": [{ "range": {
                    "start": { "line": 0, "character": from },
                    "end": { "line": 0, "character": to },
                }, "newText": text }],
            })
        };
        // A reversed range, and the clean buffer loaded for it is dropped.
        let reversed = json!({ "documentChanges": [change(2, 1, "X")] });
        assert!(hub().apply_edit(reversed, None).await.is_err());
        assert!(!hub().inner.lock().unwrap().by_path.contains_key(&path));
        // The second edit of a file sees the first.
        let twice = json!({ "documentChanges": [change(0, 1, "AA"), change(0, 2, "B")] });
        hub().apply_edit(twice, None).await.unwrap();
        let mut inner = hub().inner.lock().unwrap();
        let id = inner.by_path.remove(&path).unwrap();
        assert_eq!(inner.buffers.remove(&id).unwrap().rope.to_string(), "Bbc\n");
        drop(inner);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
