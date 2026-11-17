//! The native editor's server side: one authoritative buffer per open file,
//! edited by CodeMirror views in any number of tabs and panes through
//! `@codemirror/collab` (`/ws/editor`).

pub mod buffer;
pub mod file_ops;
pub mod git;
pub mod lsp;
pub mod text;

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex, OnceLock};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::mpsc;
use uuid::Uuid;

use buffer::{Buffer, DiskStatus, LineEnding, OnDisk, SaveError, Update};
use file_ops::Step;
pub use lsp::file_events::{FileChange, Kind as FileKind};

/// Messages a socket may queue before it is dropped as lagging; the tab
/// reconnects and catches up.
const SOCKET_QUEUE: usize = 1024;
/// How long a buffer loaded for automation stays without a tab following it.
const UNFOLLOWED: std::time::Duration = std::time::Duration::from_secs(60);
/// How long servers get to answer `will…Files`.
const WILL_CHANGE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);
/// Why changes are refused while an upgrade hands the buffers over.
const UPGRADING: &str = "btmux is upgrading; try again in a moment";

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
    /// Answer a server's prompt (`lsp_prompt`); `action` is the chosen title.
    LspPromptAnswer {
        #[cfg_attr(test, ts(type = "number"))]
        server: u64,
        #[cfg_attr(test, ts(type = "number"))]
        token: u64,
        action: Option<String>,
    },
    /// Apply an LSP `WorkspaceEdit` from language server `server` (a rename,
    /// a code action), loading files as needed; its positions are in
    /// `buffer` at `version`.
    ApplyEdit {
        req: u32,
        #[cfg_attr(test, ts(type = "unknown"))]
        edit: Value,
        #[cfg_attr(test, ts(type = "number | null"))]
        server: Option<u64>,
        buffer: Option<Uuid>,
        #[cfg_attr(test, ts(type = "number | null"))]
        version: Option<u64>,
    },
    /// Make `text` the file's index content, if the index is still at
    /// `generation` (`git_base`).
    GitStage {
        req: u32,
        buffer: Uuid,
        #[cfg_attr(test, ts(type = "number"))]
        generation: u64,
        text: String,
    },
    /// Blame 1-based `line` of the buffer at `version`; `full` adds the
    /// commit message.
    GitBlame {
        req: u32,
        buffer: Uuid,
        #[cfg_attr(test, ts(type = "number"))]
        version: u64,
        line: u32,
        full: bool,
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
    /// A server asks the user to choose one of `actions` (`window/showMessageRequest`,
    /// or `workspace/applyEdit` with changes that need confirmation).
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
    /// The file as the Git index has it, which git signs diff against, on
    /// open and on change; `null` outside a repository or for ignored files.
    GitBase {
        buffer: Uuid,
        #[cfg_attr(test, ts(type = "number"))]
        generation: u64,
        text: Option<String>,
        untracked: bool,
    },
    GitBlame {
        req: u32,
        blame: Option<git::Blame>,
        /// The buffer wasn't at the version asked for.
        stale: bool,
        error: Option<String>,
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
    /// The buffers were handed to an upgrade: changes wait for its outcome.
    frozen: bool,
    /// Sockets whose pushes were dropped meanwhile.
    stalled: HashSet<SocketId>,
    /// Saves writing their file.
    saving: usize,
    /// Buffers in a Git worktree.
    git: HashMap<Uuid, GitState>,
}

struct GitState {
    file: git::GitFile,
    base: Option<git::Base>,
    /// The commit blame starts from.
    head: Option<git2::Oid>,
    /// Counts changes of `base` and `head`.
    generation: u64,
}

pub struct Hub {
    inner: Mutex<Inner>,
    /// Signaled when no save is in progress any more.
    saves_done: tokio::sync::Notify,
    /// For pane editors showing files that move.
    state: OnceLock<crate::AppState>,
}

/// A file browser's change of `old` (to `new`), with paths as buffers and
/// language servers know them. Reads the filesystem.
pub fn file_change(old: &Path, new: &Path, dir: bool) -> FileChange {
    let canonical = |p: &Path| file_ops::resolve_entry(p).unwrap_or_else(|_| p.to_path_buf());
    FileChange {
        old: canonical(old),
        new: canonical(new),
        dir,
    }
}

/// Let file operations update pane editors.
pub fn set_state(state: crate::AppState) {
    let _ = hub().state.set(state);
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

/// The native editor's state as `btmux upgrade` hands it over.
#[cfg(unix)]
#[derive(Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct EditorHandoff {
    /// The unsaved buffers.
    buffers: Vec<buffer::HandedBuffer>,
    servers: lsp::HandedServers,
}

/// How long an upgrade waits for saves in progress.
#[cfg(unix)]
const SAVE_WAIT: std::time::Duration = std::time::Duration::from_secs(5);

/// The unsaved buffers, for an upgrade to hand over, once saves in progress
/// finish. Changes are refused until `thaw`, so none are lost after this;
/// tabs keep unsent edits.
#[cfg(unix)]
pub async fn freeze() -> Result<Vec<buffer::HandedBuffer>, String> {
    hub().freeze().await
}

/// What the upgrade hands over, just before its exec: language server ids
/// stop being given out, so the next process gives none out twice.
#[cfg(unix)]
pub fn seal(buffers: Vec<buffer::HandedBuffer>) -> EditorHandoff {
    let servers = hub().inner.lock().unwrap().lsp.seal();
    EditorHandoff { buffers, servers }
}

/// The upgrade failed: accept changes again.
#[cfg(unix)]
pub fn thaw() {
    hub().thaw();
}

/// Take over the editor state an upgraded server handed over, before tabs
/// connect. Returns the check of the files against the disk, which runs on.
#[cfg(unix)]
pub async fn restore(handed: EditorHandoff) -> tokio::task::JoinHandle<()> {
    let buffers: Vec<Buffer> = handed.buffers.into_iter().map(Buffer::restored).collect();
    let paths: Vec<PathBuf> = buffers.iter().map(|b| b.path.clone()).collect();
    let config = lsp::config::current();
    let targets = tokio::task::spawn_blocking(move || {
        paths
            .iter()
            .map(|p| lsp::config::target(&config, p))
            .collect::<Vec<_>>()
    })
    .await
    .unwrap_or_default();
    let mut inner = hub().inner.lock().unwrap();
    inner.lsp.restore(handed.servers);
    let mut restored = Vec::new();
    let targets = targets.into_iter().chain(std::iter::repeat_with(|| None));
    for (buffer, target) in buffers.into_iter().zip(targets) {
        let (id, path) = (buffer.id, buffer.path.clone());
        if inner.by_path.contains_key(&path) {
            continue;
        }
        inner.by_path.insert(path.clone(), id);
        inner.buffers.insert(id, buffer);
        inner.watch(&path);
        if let Some(target) = target {
            inner.lsp_attach(id, target);
        }
        restored.push(path);
    }
    inner.send_summaries();
    drop(inner);
    // Nothing watched the files during the upgrade; their tabs hear of changes.
    tokio::spawn(async move {
        for path in restored {
            hub().attach_git(path.clone()).await;
            hub().check_disk(path).await;
        }
    })
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
    file_ops::resolve_new(&path)
}

fn to_json(msg: &EditorServerMessage) -> String {
    serde_json::to_string(msg).expect("editor messages serialize")
}

impl Hub {
    fn new() -> Self {
        Self {
            inner: Mutex::new(Inner::default()),
            saves_done: tokio::sync::Notify::new(),
            state: OnceLock::new(),
        }
    }

    #[cfg(unix)]
    async fn freeze(&self) -> Result<Vec<buffer::HandedBuffer>, String> {
        self.inner.lock().unwrap().frozen = true;
        // Saves in progress finish first, so the buffers match the disk.
        let settled = async {
            loop {
                let done = self.saves_done.notified();
                {
                    let inner = self.inner.lock().unwrap();
                    if inner.saving == 0 {
                        let dirty = inner.buffers.values().filter(|b| b.dirty());
                        return dirty.map(Buffer::handoff).collect();
                    }
                }
                done.await;
            }
        };
        match tokio::time::timeout(SAVE_WAIT, settled).await {
            Ok(buffers) => Ok(buffers),
            Err(_) => {
                self.thaw();
                Err("the editor is still saving a file".into())
            }
        }
    }

    #[cfg(unix)]
    fn thaw(&self) {
        let mut inner = self.inner.lock().unwrap();
        inner.frozen = false;
        inner.lsp.unseal();
        // Their tabs reconnect and resume, sending the dropped edits again.
        for socket in std::mem::take(&mut inner.stalled) {
            inner.drop_socket(socket);
        }
        inner.send_summaries();
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
                server,
                buffer,
                version,
            } => {
                let expect = buffer.zip(version);
                let error = self.apply_edit(edit, expect, server).await.err();
                self.reply(socket, req, error, false);
            }
            EditorClientMessage::GitStage {
                req,
                buffer,
                generation,
                text,
            } => self.git_stage(socket, req, buffer, generation, text),
            EditorClientMessage::GitBlame {
                req,
                buffer,
                version,
                line,
                full,
            } => self.git_blame(socket, req, buffer, version, line, full),
        }
    }

    /// Stage `text` as the buffer's index content, if the index is still at `generation`.
    fn git_stage(
        &'static self,
        socket: SocketId,
        req: u32,
        id: Uuid,
        generation: u64,
        text: String,
    ) {
        let taken = (|| {
            let inner = self.inner.lock().unwrap();
            if inner.frozen {
                return Err(UPGRADING.to_owned());
            }
            let b = inner.buffers.get(&id).ok_or("The buffer is closed")?;
            let g = inner.git.get(&id).ok_or("Not in a Git repository")?;
            let base = g.base.clone().ok_or("Git doesn't track this file")?;
            if g.generation != generation {
                return Err("The index changed meanwhile; try again".to_owned());
            }
            Ok((g.file.clone(), base, b.line_ending, b.bom))
        })();
        // Spawned: later messages, such as pushes, don't wait for git.
        tokio::spawn(async move {
            let result = match taken {
                Ok((file, base, line_ending, bom)) => tokio::task::spawn_blocking(move || {
                    git::stage(&file, &base, &text, line_ending, bom)
                })
                .await
                .unwrap_or_else(|e| Err(e.to_string())),
                Err(e) => Err(e),
            };
            // The new base reaches the tab before the reply.
            self.refresh_git(vec![id]).await;
            self.reply(socket, req, result.err(), false);
        });
    }

    fn git_blame(
        &'static self,
        socket: SocketId,
        req: u32,
        id: Uuid,
        version: u64,
        line: u32,
        full: bool,
    ) {
        let taken = {
            let inner = self.inner.lock().unwrap();
            match (inner.buffers.get(&id), inner.git.get(&id)) {
                (Some(b), Some(g)) if b.version == version => Some((
                    g.file.clone(),
                    buffer::encode(&b.rope.to_string(), b.line_ending, b.bom),
                )),
                (Some(b), _) if b.version != version => {
                    let stale = EditorServerMessage::GitBlame {
                        req,
                        blame: None,
                        stale: true,
                        error: None,
                    };
                    inner.send_to(socket, &stale);
                    return;
                }
                _ => None,
            }
        };
        tokio::spawn(async move {
            let result = match taken {
                Some((file, contents)) => git::blame(&file, contents, line, full).await,
                None => Ok(None),
            };
            let (blame, error) = match result {
                Ok(blame) => (blame, None),
                Err(e) => (None, Some(e)),
            };
            let msg = EditorServerMessage::GitBlame {
                req,
                blame,
                stale: false,
                error,
            };
            self.inner.lock().unwrap().send_to(socket, &msg);
        });
    }

    /// Reread the index and HEAD for `ids`, telling followers of changes.
    async fn refresh_git(&self, ids: Vec<Uuid>) {
        let files: Vec<(Uuid, git::GitFile)> = {
            let inner = self.inner.lock().unwrap();
            ids.into_iter()
                .filter_map(|id| Some((id, inner.git.get(&id)?.file.clone())))
                .collect()
        };
        if files.is_empty() {
            return;
        }
        let bases = tokio::task::spawn_blocking(move || {
            files
                .into_iter()
                .map(|(id, file)| (id, git::read(&file)))
                .collect::<Vec<_>>()
        })
        .await
        .unwrap_or_default();
        let mut inner = self.inner.lock().unwrap();
        for (id, read) in bases {
            let Some(g) = inner.git.get_mut(&id) else {
                continue;
            };
            if g.base == read.base && g.head == read.head {
                continue;
            }
            g.base = read.base;
            g.head = read.head;
            g.generation += 1;
            let msg = base_message(id, g);
            inner.send_to_followers(id, &msg);
        }
    }

    /// `path` changed: if it is in a watched git dir (the index, `HEAD`,
    /// `ORIG_HEAD`, …), reread the index and HEAD for its buffers.
    async fn check_git(&self, path: &Path) {
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            return;
        };
        if name.ends_with(".lock") || name.starts_with(git::SCRATCH_PREFIX) {
            return;
        }
        let ids: Vec<Uuid> = {
            let inner = self.inner.lock().unwrap();
            inner
                .git
                .iter()
                .filter(|(_, g)| Some(g.file.git_dir.as_path()) == path.parent())
                .map(|(id, _)| *id)
                .collect()
        };
        self.refresh_git(ids).await;
    }

    /// Find the repository of a buffer restored from an upgrade.
    #[cfg(unix)]
    async fn attach_git(&self, path: PathBuf) {
        let p = path.clone();
        let Ok(found) = tokio::task::spawn_blocking(move || find_git(&p)).await else {
            return;
        };
        let mut inner = self.inner.lock().unwrap();
        let Some(id) = inner.by_path.get(&path).copied() else {
            return;
        };
        if inner.git.contains_key(&id) {
            return;
        }
        inner.insert_git(id, found);
        if let Some(g) = inner.git.get(&id) {
            let msg = base_message(id, g);
            inner.send_to_followers(id, &msg);
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
        let base = match inner.git.get(&id) {
            Some(g) => base_message(id, g),
            None => EditorServerMessage::GitBase {
                buffer: id,
                generation: 0,
                text: None,
                untracked: false,
            },
        };
        inner.send_to(socket, &base);
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
        let (loaded, target, git) = tokio::task::spawn_blocking(move || {
            (
                buffer::load(&p),
                lsp::config::target(&config, &p),
                find_git(&p),
            )
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
        inner.insert_git(id, git);
        if let Some(target) = target {
            inner.lsp_attach(id, target);
        }
        Ok(id)
    }

    /// Apply an LSP `WorkspaceEdit` from language server `server`, step by
    /// step: each run of text edits all or none, file operations between.
    /// With `expect`, its positions are in that buffer at that version, which
    /// must still be current.
    async fn apply_edit(
        &'static self,
        edit: Value,
        mut expect: Option<(Uuid, u64)>,
        server: Option<u64>,
    ) -> Result<(), String> {
        let steps = file_ops::parse(&edit)?;
        if steps.iter().any(|s| !s.is_text()) {
            let sandbox = {
                let inner = self.inner.lock().unwrap();
                match server {
                    Some(id) => inner.lsp_sandbox(id).ok_or("The language server exited")?,
                    None => None,
                }
            };
            let checked = steps.clone();
            tokio::task::spawn_blocking(move || file_ops::check(&checked, sandbox.as_deref()))
                .await
                .map_err(|e| e.to_string())??;
        }
        let mut steps = steps.into_iter().peekable();
        while let Some(step) = steps.next() {
            let Step::Text {
                path,
                edits,
                version,
            } = step
            else {
                // The edit's positions are still checked by its first text edits.
                if let Some((id, version)) = expect {
                    let inner = self.inner.lock().unwrap();
                    if inner.buffers.get(&id).map(|b| b.version) != Some(version) {
                        return Err("The file changed meanwhile".into());
                    }
                }
                self.file_op(step).await?;
                continue;
            };
            let mut run = vec![(path, edits, version)];
            while let Some(Step::Text { .. }) = steps.peek() {
                let Some(Step::Text {
                    path,
                    edits,
                    version,
                }) = steps.next()
                else {
                    unreachable!()
                };
                run.push((path, edits, version));
            }
            self.apply_text(run, expect.take()).await?;
        }
        Ok(())
    }

    /// Text edits to files, all or none.
    async fn apply_text(
        &'static self,
        files: Vec<(PathBuf, Vec<Value>, Option<u64>)>,
        expect: Option<(Uuid, u64)>,
    ) -> Result<(), String> {
        let mut targets = Vec::new();
        let mut loaded = Ok(());
        for (path, edits, version) in files {
            // Symlinks as earlier steps left them.
            let id = match file_ops::resolve_new(&path) {
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

    /// Create, rename, or delete a file or directory for an edit.
    async fn file_op(&'static self, step: Step) -> Result<(), String> {
        {
            let inner = self.inner.lock().unwrap();
            if inner.frozen {
                return Err(UPGRADING.into());
            }
            inner.check_file_op(&step)?;
        }
        let op = step.clone();
        let change = tokio::task::spawn_blocking(move || -> Result<_, String> {
            // Before it's gone.
            let dir = match &op {
                Step::Delete { path, .. } => {
                    std::fs::symlink_metadata(path).is_ok_and(|m| m.is_dir())
                }
                _ => false,
            };
            if !file_ops::run(&op)? {
                return Ok(None);
            }
            Ok(Some(match &op {
                Step::Rename { from, to, .. } => FileChange {
                    old: from.clone(),
                    new: to.clone(),
                    dir: to.is_dir(),
                },
                Step::Create { path, .. } | Step::Delete { path, .. } => {
                    FileChange::new(path.clone(), dir)
                }
                Step::Text { .. } => unreachable!(),
            }))
        })
        .await
        .map_err(|e| e.to_string())??;
        if let Some(change) = change {
            if let Step::Create { path, .. } = &step {
                self.reload_clean(path).await;
            }
            let kind = match step {
                Step::Create { .. } => FileKind::Create,
                Step::Rename { .. } => FileKind::Rename,
                _ => FileKind::Delete,
            };
            self.did_change_files(kind, &[change]).await;
        }
        Ok(())
    }

    /// Refuse renames (from the file browser) onto paths open buffers have.
    pub fn check_renames(&self, changes: &[FileChange]) -> Result<(), String> {
        let inner = self.inner.lock().unwrap();
        changes
            .iter()
            .try_for_each(|c| inner.check_destination(&c.old, &c.new))
    }

    /// Reload the clean buffer at `path`, if any, from the file there now:
    /// later edits in the same workspace edit see a created file's text,
    /// not that of the one it replaced. The watcher would come too late.
    async fn reload_clean(&'static self, path: &Path) {
        let id = {
            let inner = self.inner.lock().unwrap();
            let Some(id) = inner.by_path.get(path).copied() else {
                return;
            };
            if inner.buffers.get(&id).is_none_or(|b| b.dirty()) {
                return;
            }
            id
        };
        let p = path.to_path_buf();
        let Ok(Ok(loaded)) = tokio::task::spawn_blocking(move || buffer::load(&p)).await else {
            return;
        };
        let mut inner = self.inner.lock().unwrap();
        if inner.buffers.get(&id).is_some_and(|b| !b.dirty()) {
            inner.reload(id, loaded);
            inner.send_summaries();
        }
    }

    /// Before files are created, renamed, or deleted (from the file
    /// browser): apply the edits servers ask for (`will…Files`), such as
    /// updated imports, waiting a while for their answers.
    pub async fn will_change_files(&'static self, kind: FileKind, changes: &[FileChange]) {
        let asked = self.inner.lock().unwrap().lsp_will_change(kind, changes);
        let answers =
            futures_util::future::join_all(asked.into_iter().map(|(id, rx)| async move {
                match tokio::time::timeout(WILL_CHANGE_TIMEOUT, rx).await {
                    Ok(Ok(answer)) => Some((id, answer)),
                    _ => None,
                }
            }))
            .await;
        for (id, answer) in answers.into_iter().flatten() {
            let mut edit = answer["result"].clone();
            if edit.is_null() {
                continue;
            }
            // Nobody is asked to confirm a file browser's change.
            if !lsp::annotations::confirmations(&edit).is_empty()
                && !lsp::annotations::skip_confirmations(&mut edit)
            {
                continue;
            }
            if let Err(e) = self.apply_edit(edit, None, Some(id)).await {
                tracing::warn!("{}: {e}", kind.method(true));
            }
        }
    }

    /// After files were created, renamed, or deleted: tell servers
    /// (`did…Files`). Open buffers and pane editors' files follow renames;
    /// pane editors close deleted files, unless unsaved.
    pub async fn did_change_files(&'static self, kind: FileKind, changes: &[FileChange]) {
        self.inner.lock().unwrap().lsp_did_change(kind, changes);
        let to = |c: &FileChange| match kind {
            FileKind::Rename => Some(c.new.clone()),
            _ => None,
        };
        if kind != FileKind::Create {
            let moves: Vec<_> = changes.iter().map(|c| (c.old.clone(), to(c))).collect();
            self.follow(moves).await;
        }
    }

    /// Files under each `from` moved to its `to`, or were deleted: move
    /// their buffers, then pane editors' files, broadcasting once.
    async fn follow(&'static self, moves: Vec<(PathBuf, Option<PathBuf>)>) {
        let (moving, dirty): (Vec<(Uuid, PathBuf)>, Vec<String>) = {
            let inner = self.inner.lock().unwrap();
            let mut moving = Vec::new();
            let mut dirty = Vec::new();
            for (from, to) in &moves {
                for (path, id) in inner.by_path.iter().filter(|(p, _)| p.starts_with(from)) {
                    match to {
                        Some(to) => {
                            moving.extend(file_ops::moved(path, from, to).map(|new| (*id, new)))
                        }
                        None if inner.buffers.get(id).is_some_and(|b| b.dirty()) => {
                            dirty.push(crate::fs_ops::client_path(path))
                        }
                        None => {}
                    }
                }
            }
            (moving, dirty)
        };
        if !moving.is_empty() {
            let config = lsp::config::current();
            let moved = tokio::task::spawn_blocking(move || {
                moving
                    .into_iter()
                    .map(|(id, new)| {
                        let target = lsp::config::target(&config, &new);
                        let git = find_git(&new);
                        (id, new, target, git)
                    })
                    .collect::<Vec<_>>()
            })
            .await
            .unwrap_or_default();
            let mut inner = self.inner.lock().unwrap();
            for (id, path, target, git) in moved {
                inner.repath(id, path, target, git);
            }
            inner.send_summaries();
        }
        let Some(state) = self.state.get() else {
            return;
        };
        let mut mgr = state.write().await;
        let mut changed = false;
        for pane in mgr
            .sessions
            .iter_mut()
            .flat_map(|s| &mut s.windows)
            .flat_map(|w| &mut w.panes)
        {
            for (from, to) in &moves {
                changed |= pane.editor.moved(from, to.as_deref(), &dirty);
            }
        }
        if changed {
            crate::ws::control::broadcast_state(&mgr);
        }
    }

    fn push(&self, socket: SocketId, id: Uuid, version: u64, updates: Vec<Update>) {
        let mut inner = self.inner.lock().unwrap();
        // The tab sends it again once reconnected.
        if inner.frozen {
            inner.stalled.insert(socket);
            return;
        }
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

    async fn save(&'static self, id: Uuid, force: bool) -> Result<(), SaveError> {
        let (path, text, version, line_ending, bom, disk) = {
            let mut inner = self.inner.lock().unwrap();
            if inner.frozen {
                return Err(SaveError::Io(UPGRADING.into()));
            }
            let b = inner
                .buffers
                .get(&id)
                .ok_or_else(|| SaveError::Io("The buffer is closed".into()))?;
            let taken = (
                b.path.clone(),
                b.rope.to_string(),
                b.version,
                b.line_ending,
                b.bom,
                b.disk,
            );
            inner.saving += 1;
            taken
        };
        // Spawned, so it finishes and is counted even if the tab goes away.
        tokio::spawn(async move {
            let saved = tokio::task::spawn_blocking(move || {
                buffer::save(&path, &text, line_ending, bom, disk, force)
            })
            .await
            .map_err(|e| SaveError::Io(e.to_string()))
            .and_then(|saved| saved);
            let mut inner = self.inner.lock().unwrap();
            if let Ok(disk) = saved {
                inner.saved(id, version, disk);
            }
            inner.saving -= 1;
            if inner.saving == 0 {
                self.saves_done.notify_waiters();
            }
            saved.map(drop)
        })
        .await
        .map_err(|e| SaveError::Io(e.to_string()))?
    }
}

impl Inner {
    /// Record a save of `id` at `version`.
    fn saved(&mut self, id: Uuid, version: u64, disk: buffer::Disk) {
        let Some(b) = self.buffers.get_mut(&id) else {
            return;
        };
        b.saved_version = version;
        b.disk = disk;
        b.disk_status = DiskStatus::Same;
        let state = state_message(b);
        self.send_to_followers(id, &state);
        self.lsp_saved(id);
        self.collect(id);
        self.send_summaries();
    }
}

impl Hub {
    async fn revert(&self, id: Uuid) -> Result<(), String> {
        let path = {
            let inner = self.inner.lock().unwrap();
            if inner.frozen {
                return Err(UPGRADING.into());
            }
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
        if inner.frozen {
            return Err(UPGRADING.into());
        }
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
        // A save or another check got there first, or it moved.
        if b.disk != known || b.path != path {
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

fn base_message(id: Uuid, g: &GitState) -> EditorServerMessage {
    EditorServerMessage::GitBase {
        buffer: id,
        generation: g.generation,
        text: g.base.as_ref().map(|b| b.text.clone()),
        untracked: g.base.as_ref().is_some_and(|b| b.untracked),
    }
}

/// The repository holding `path` and what git signs follow there.
fn find_git(path: &Path) -> Option<(git::GitFile, git::Snapshot)> {
    let file = git::locate(path)?;
    let read = git::read(&file);
    Some((file, read))
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
        if self.frozen {
            return Err(UPGRADING.into());
        }
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
            if let Some(g) = self.git.remove(&id) {
                self.unwatch_dir(&g.file.git_dir);
            }
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
        if let Some(dir) = path.parent() {
            self.watch_dir(dir);
        }
    }

    fn unwatch(&mut self, path: &Path) {
        if let Some(dir) = path.parent() {
            self.unwatch_dir(dir);
        }
    }

    fn watch_dir(&mut self, dir: &Path) {
        self.watcher
            .get_or_insert_with(watch::Watcher::new)
            .watch(dir);
    }

    fn unwatch_dir(&mut self, dir: &Path) {
        if let Some(watcher) = self.watcher.as_mut() {
            watcher.unwatch(dir);
        }
    }

    /// Refuse to move `from` onto a path an open buffer has (at or under
    /// `to`): two buffers would share it.
    fn check_destination(&self, from: &Path, to: &Path) -> Result<(), String> {
        if from == to {
            return Ok(());
        }
        match self.by_path.keys().find(|p| p.starts_with(to)) {
            Some(p) => Err(format!("{} is open in the editor", p.display())),
            None => Ok(()),
        }
    }

    /// Refuse a file operation that would lose or confuse open buffers:
    /// overwriting one, or deleting unsaved changes.
    fn check_file_op(&self, step: &Step) -> Result<(), String> {
        let under = |dir: &Path| {
            self.by_path
                .iter()
                .filter(|(p, _)| p.starts_with(dir))
                .map(|(p, id)| (*id, p.clone()))
                .collect::<Vec<_>>()
        };
        let open = |p: &Path| format!("{} is open in the editor", p.display());
        match step {
            Step::Text { .. } => Ok(()),
            Step::Create {
                path, overwrite, ..
            } => match self.by_path.get(path).and_then(|id| self.buffers.get(id)) {
                // A buffer of a deleted file: the new one replaces it, unless unsaved.
                Some(b) if b.dirty() => Err(format!("{} has unsaved changes", path.display())),
                Some(_) if *overwrite && path.exists() => Err(open(path)),
                _ => Ok(()),
            },
            Step::Rename { from, to, .. } => self.check_destination(from, to),
            Step::Delete { path, .. } => {
                for (id, p) in under(path) {
                    if self.buffers.get(&id).is_some_and(|b| b.dirty()) {
                        return Err(format!("{} has unsaved changes", p.display()));
                    }
                }
                Ok(())
            }
        }
    }

    /// The buffer's file moved to `path`: reopen it there in its language's
    /// servers and repository.
    fn repath(
        &mut self,
        id: Uuid,
        path: PathBuf,
        target: Option<lsp::config::Target>,
        git: Option<(git::GitFile, git::Snapshot)>,
    ) {
        // One buffer per path: a rename onto another's path is refused first.
        if self.by_path.get(&path).is_some_and(|other| *other != id) {
            tracing::warn!(path = %path.display(), "not moving a buffer onto another");
            return;
        }
        let Some(b) = self.buffers.get_mut(&id) else {
            return;
        };
        let old = std::mem::replace(&mut b.path, path.clone());
        let servers = std::mem::take(&mut b.servers);
        b.language_id = None;
        // The watcher may have seen it go before it moved here.
        if b.disk_status == DiskStatus::Deleted {
            b.disk_status = DiskStatus::Same;
            let state = state_message(b);
            self.send_to_followers(id, &state);
        }
        self.by_path.remove(&old);
        self.by_path.insert(path.clone(), id);
        self.unwatch(&old);
        self.watch(&path);
        if let Some(g) = self.git.remove(&id) {
            self.unwatch_dir(&g.file.git_dir);
        }
        self.insert_git(id, git);
        self.lsp_closed(id, &old, &servers);
        if let Some(target) = target {
            self.lsp_attach(id, target);
        }
    }

    /// Track a buffer's repository, watching its index.
    fn insert_git(&mut self, id: Uuid, found: Option<(git::GitFile, git::Snapshot)>) {
        let Some((file, read)) = found else { return };
        self.watch_dir(&file.git_dir);
        self.git.insert(
            id,
            GitState {
                file,
                base: read.base,
                head: read.head,
                generation: 1,
            },
        );
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
                        super::hub().check_git(&path).await;
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
        assert!(hub().apply_edit(edit(0), None, None).await.is_err());
        assert_eq!(text(), "Xabc\n");
        hub().apply_edit(edit(1), None, None).await.unwrap();
        assert_eq!(text(), "Aabc\n");
        let mut inner = hub().inner.lock().unwrap();
        inner.buffers.remove(&id);
        inner.by_path.remove(&path);
        drop(inner);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn renames_onto_open_buffers_are_refused() {
        let dir = std::env::temp_dir().join(format!("btmux-collide-{}", Uuid::new_v4()));
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        let (a, b) = (dir.join("a.txt"), dir.join("sub/b.txt"));
        std::fs::write(&a, "a").unwrap();
        std::fs::write(&b, "b").unwrap();
        let (a, b) = (
            resolve(&a.to_string_lossy()).unwrap(),
            resolve(&b.to_string_lossy()).unwrap(),
        );
        let ids = [
            hub().load_buffer(a.clone()).await.unwrap(),
            hub().load_buffer(b.clone()).await.unwrap(),
        ];
        let rename = |to: &Path| file_change(&a, to, false);
        // Onto the file, or the directory holding it.
        assert!(hub().check_renames(&[rename(&b)]).is_err());
        assert!(hub().check_renames(&[rename(b.parent().unwrap())]).is_err());
        hub()
            .check_renames(&[rename(&a.with_file_name("c.txt"))])
            .unwrap();
        let uri = |p: &Path| lsp::config::uri(p);
        let edit = json!({ "documentChanges": [{ "kind": "rename", "oldUri": uri(&a), "newUri": uri(&b), "options": { "overwrite": true } }] });
        assert!(hub().apply_edit(edit, None, None).await.is_err());
        assert_eq!(std::fs::read_to_string(&b).unwrap(), "b");
        let mut inner = hub().inner.lock().unwrap();
        for (id, path) in ids.iter().zip([&a, &b]) {
            inner.buffers.remove(id);
            inner.by_path.remove(path);
        }
        drop(inner);
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// Drop the buffers at `paths` a test loaded.
    fn forget(paths: &[&Path]) {
        let mut inner = hub().inner.lock().unwrap();
        for path in paths {
            if let Some(id) = inner.by_path.remove(*path) {
                inner.buffers.remove(&id);
            }
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn text_edits_follow_symlinks_as_earlier_steps_left_them() {
        let dir = std::env::temp_dir().join(format!("btmux-steps-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let dir = dir.canonicalize().unwrap();
        std::fs::write(dir.join("old.txt"), "old text\n").unwrap();
        std::fs::write(dir.join("target.txt"), "target text\n").unwrap();
        std::os::unix::fs::symlink(dir.join("target.txt"), dir.join("dest.txt")).unwrap();
        let uri = |name: &str| lsp::config::uri(&dir.join(name));
        let edit = json!({ "documentChanges": [
            { "kind": "rename", "oldUri": uri("old.txt"), "newUri": uri("dest.txt"), "options": { "overwrite": true } },
            { "textDocument": { "uri": uri("dest.txt"), "version": null }, "edits": [{ "range": {
                "start": { "line": 0, "character": 0 }, "end": { "line": 0, "character": 3 },
            }, "newText": "NEW" }] },
        ] });
        hub().apply_edit(edit, None, None).await.unwrap();
        let (dest, target) = (dir.join("dest.txt"), dir.join("target.txt"));
        {
            let inner = hub().inner.lock().unwrap();
            assert_eq!(
                inner.buffers[&inner.by_path[&dest]].rope.to_string(),
                "NEW text\n"
            );
            assert!(!inner.by_path.contains_key(&target));
        }
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "target text\n");
        forget(&[&dest]);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn a_created_file_replaces_a_clean_buffer_of_the_old_one() {
        let dir = std::env::temp_dir().join(format!("btmux-recreate-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("replaced.txt");
        std::fs::write(&path, "old content\n").unwrap();
        let path = resolve(&path.to_string_lossy()).unwrap();
        hub().load_buffer(path.clone()).await.unwrap();
        // Deleted (outside, not into the trash), then created and edited.
        std::fs::remove_file(&path).unwrap();
        let uri = lsp::config::uri(&path);
        let edit = json!({ "documentChanges": [
            { "kind": "create", "uri": uri },
            { "textDocument": { "uri": uri, "version": null }, "edits": [{ "range": {
                "start": { "line": 0, "character": 0 }, "end": { "line": 0, "character": 0 },
            }, "newText": "new content\n" }] },
        ] });
        hub().apply_edit(edit, None, None).await.unwrap();
        {
            let inner = hub().inner.lock().unwrap();
            assert_eq!(
                inner.buffers[&inner.by_path[&path]].rope.to_string(),
                "new content\n"
            );
        }
        forget(&[&path]);
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
        assert!(hub().apply_edit(reversed, None, None).await.is_err());
        assert!(!hub().inner.lock().unwrap().by_path.contains_key(&path));
        // The second edit of a file sees the first.
        let twice = json!({ "documentChanges": [change(0, 1, "AA"), change(0, 2, "B")] });
        hub().apply_edit(twice, None, None).await.unwrap();
        let mut inner = hub().inner.lock().unwrap();
        let id = inner.by_path.remove(&path).unwrap();
        assert_eq!(inner.buffers.remove(&id).unwrap().rope.to_string(), "Bbc\n");
        drop(inner);
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// An unsaved buffer survives an upgrade's handoff, still unsaved, and
    /// a tab whose update was applied but not confirmed resumes it.
    #[cfg(unix)]
    #[tokio::test]
    async fn hands_over_unsaved_buffers() {
        let dir = std::env::temp_dir().join(format!("btmux-handoff-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("f.txt");
        std::fs::write(&path, "\u{feff}one\r\ntwo\r\n").unwrap();
        let other = dir.join("g.txt");
        std::fs::write(&other, "a\n").unwrap();
        let path = resolve(&path.to_string_lossy()).unwrap();
        let other = resolve(&other.to_string_lossy()).unwrap();
        let tab = |changes: Value| Update {
            client_id: "tab".into(),
            changes,
        };
        // Applied, but the tab hasn't seen it confirmed.
        let accepted = tab(json!([3, [0, "!"], 5]));
        // What the old process hands over, and drops.
        let mut buffers = Vec::new();
        let mut ids = Vec::new();
        for (p, update) in [(&path, &accepted), (&other, &tab(json!([1, [0, "!"], 1])))] {
            let id = hub().load_buffer(p.clone()).await.unwrap();
            let mut inner = hub().inner.lock().unwrap();
            let b = inner.buffers.get_mut(&id).unwrap();
            b.push(0, std::slice::from_ref(update)).unwrap().unwrap();
            buffers.push(b.handoff());
            ids.push(id);
            inner.buffers.remove(&id);
            inner.by_path.remove(p);
            inner.unwatch(p);
        }
        // Not the shared hub's: sealing it would stop other tests' servers.
        let servers = lsp::Lsp::default().seal();
        let json = serde_json::to_string(&EditorHandoff { buffers, servers }).unwrap();
        // Changed on disk meanwhile; a new length, as mtimes may not have moved.
        std::fs::write(&other, "bb\n").unwrap();
        restore(serde_json::from_str(&json).unwrap())
            .await
            .await
            .unwrap();
        let find = |p: &Path| {
            let inner = hub().inner.lock().unwrap();
            let b = &inner.buffers[&inner.by_path[p]];
            (b.id, b.since(0), b.dirty(), b.disk_status)
        };
        let (id, missed, dirty, disk) = find(&path);
        assert_eq!((id, dirty, disk), (ids[0], true, DiskStatus::Same));
        assert_eq!(missed.unwrap()[0].changes, accepted.changes);
        let (other_id, _, dirty, disk) = find(&other);
        assert_eq!((dirty, disk), (true, DiskStatus::Changed));
        // The tab resends it with a newer one: only the newer one applies.
        let unsent = tab(json!([9, [0, "?"]]));
        hub().push(SocketId::MAX, id, 0, vec![accepted, unsent]);
        let text = hub().inner.lock().unwrap().buffers[&id].rope.to_string();
        assert_eq!(text, "one!\ntwo\n?");
        assert!(matches!(
            hub().save(other_id, false).await,
            Err(SaveError::Conflict)
        ));
        // The file as loaded: saving needs no force, and keeps its encoding.
        hub().save(id, false).await.unwrap();
        assert_eq!(
            std::fs::read(&path).unwrap(),
            "\u{feff}one!\r\ntwo\r\n?".as_bytes()
        );
        let mut inner = hub().inner.lock().unwrap();
        assert!(!inner.by_path.contains_key(&path));
        inner.buffers.remove(&other_id);
        inner.by_path.remove(&other);
        inner.unwatch(&other);
        drop(inner);
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// An upgrade hands over what a save in progress leaves, not what it found.
    #[cfg(unix)]
    #[tokio::test]
    async fn upgrades_wait_for_saves() {
        // Its own hub: a frozen one refuses other tests' edits.
        let hub: &'static Hub = Box::leak(Box::new(Hub::new()));
        let dir = std::env::temp_dir().join(format!("btmux-freeze-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("f.txt");
        std::fs::write(&path, "abc\n").unwrap();
        let path = resolve(&path.to_string_lossy()).unwrap();
        let id = hub.load_buffer(path).await.unwrap();
        let version = {
            let mut inner = hub.inner.lock().unwrap();
            let b = inner.buffers.get_mut(&id).unwrap();
            b.edit(&[text::Change {
                from: 3,
                to: 3,
                insert: "!".into(),
            }]);
            // Writing the file.
            inner.saving += 1;
            inner.buffers[&id].version
        };
        let freezing = tokio::spawn(hub.freeze());
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        assert!(!freezing.is_finished());
        // Saves starting now wait for the upgrade.
        assert!(matches!(hub.save(id, false).await, Err(SaveError::Io(_))));
        {
            let mut inner = hub.inner.lock().unwrap();
            let disk = inner.buffers[&id].disk;
            inner.saved(id, version, disk);
            inner.saving -= 1;
            hub.saves_done.notify_waiters();
        }
        assert!(freezing.await.unwrap().unwrap().is_empty());
        hub.thaw();
        std::fs::remove_dir_all(dir).unwrap();
    }
}
