pub mod agent;
pub mod detection;
pub mod layout;
pub mod manager;

use serde::{Deserialize, Serialize};
use std::sync::Arc;
use uuid::Uuid;

use crate::pty::PtyHandle;
use layout::{Layout, LayoutPreset};

pub struct Session {
    pub id: Uuid,
    pub name: String,
    pub runtime: Runtime,
    pub windows: Vec<Window>,
    pub active_window: usize,
    pub prev_window: Option<usize>,
}

pub struct Window {
    pub id: Uuid,
    pub name: String,
    pub panes: Vec<Pane>,
    pub active_pane: usize,
    pub prev_pane: Option<usize>,
    pub layout: Layout,
    pub zoomed_pane: Option<Uuid>,
    /// The preset arrangement last applied via `select-layout`/`next-layout`, if
    /// any. Drives `next-layout`'s cycle position. Transient (not snapshotted):
    /// reset to `None` on restore and cleared by manual splits/kills below.
    pub last_preset: Option<LayoutPreset>,
}

impl Window {
    /// Remove `pane_id` from the layout tree and the pane list.
    ///
    /// `active_pane`/`prev_pane` are *indices* into `panes`, so a naive
    /// `retain` would silently re-point them at the wrong pane whenever an
    /// earlier element is removed (the classic "active pane ≠ highlighted
    /// pane" glitch). We capture the ids they reference first, then re-resolve
    /// the indices after the Vec shifts: the active pane keeps its identity if
    /// it survived, falling back to the clamped neighbour if it was the one
    /// removed; `prev_pane` is cleared if its target is gone.
    ///
    /// May leave `panes` empty — the caller decides whether to drop the window.
    pub fn remove_pane(&mut self, pane_id: Uuid) -> Option<Pane> {
        let active_id = self.panes.get(self.active_pane).map(|p| p.id);
        let prev_id = self.prev_pane.and_then(|i| self.panes.get(i)).map(|p| p.id);

        if let Some(new_layout) = self.layout.remove_pane(pane_id) {
            self.layout = new_layout;
        }
        let removed = self
            .panes
            .iter()
            .position(|p| p.id == pane_id)
            .map(|i| self.panes.remove(i));
        if self.panes.is_empty() {
            return removed;
        }

        self.active_pane = active_id
            .and_then(|id| self.panes.iter().position(|p| p.id == id))
            .unwrap_or_else(|| self.active_pane.min(self.panes.len() - 1));
        self.prev_pane = prev_id.and_then(|id| self.panes.iter().position(|p| p.id == id));
        if self.zoomed_pane == Some(pane_id) {
            self.zoomed_pane = None;
        }
        // The collapsed tree no longer matches the preset, so `next-layout`
        // should start its cycle fresh rather than from a stale position.
        self.last_preset = None;
        removed
    }
}

pub struct Pane {
    pub id: Uuid,
    pub pty: PtyHandle,
    /// The most severe notification since the pane was last viewed.
    pub unread: Option<PaneAlert>,
    pub prompt: Arc<PromptLock>,
    pub editor: PaneEditor,
}

/// The native editor over a pane, the same in every tab.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct PaneEditor {
    pub shown: bool,
    /// Open files (absolute paths), in tab order.
    pub files: Vec<String>,
    pub active: Option<String>,
}

/// A change to a pane's editor.
#[derive(Deserialize, Debug, Clone)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[cfg_attr(test, derive(ts_rs::TS))]
pub enum EditorViewOp {
    Show,
    Hide,
    Toggle,
    /// Add `path` if it isn't open, make it active, and show the editor.
    Open {
        path: String,
        line: Option<u32>,
    },
    Close {
        path: String,
    },
    Activate {
        path: String,
    },
    /// Move an open file to `index` in the tab order.
    Move {
        path: String,
        index: usize,
    },
}

impl PaneEditor {
    /// Files at or under `from` moved to `to`, or were deleted (closing
    /// their tabs, except `keep`'s); true if any of this editor's were.
    pub fn moved(
        &mut self,
        from: &std::path::Path,
        to: Option<&std::path::Path>,
        keep: &[String],
    ) -> bool {
        let before = self.clone();
        let active = self.active.take();
        let mut closed_at = None;
        for file in std::mem::take(&mut self.files) {
            let renamed = match std::path::Path::new(&file).strip_prefix(from) {
                Err(_) => Some(file.clone()),
                Ok(_) if to.is_none() && keep.contains(&file) => Some(file.clone()),
                Ok(rest) => to.map(|to| {
                    let path = if rest.as_os_str().is_empty() {
                        to.to_path_buf()
                    } else {
                        to.join(rest)
                    };
                    crate::fs_ops::client_path(&path)
                }),
            };
            let is_active = active.as_ref() == Some(&file);
            match renamed {
                Some(path) => {
                    if is_active {
                        self.active = Some(path.clone());
                    }
                    if !self.files.contains(&path) {
                        self.files.push(path);
                    }
                }
                None if is_active => closed_at = Some(self.files.len()),
                None => {}
            }
        }
        if let Some(i) = closed_at {
            self.active = self.files.get(i).or_else(|| self.files.last()).cloned();
        }
        if self.files.is_empty() {
            self.shown = false;
        }
        *self != before
    }

    pub fn apply(&mut self, op: EditorViewOp) {
        match op {
            EditorViewOp::Show => self.shown = true,
            EditorViewOp::Hide => self.shown = false,
            EditorViewOp::Toggle => self.shown = !self.shown,
            EditorViewOp::Open { path, .. } => {
                if !self.files.contains(&path) {
                    // After the active file, like a browser tab.
                    let at = self
                        .active
                        .as_ref()
                        .and_then(|a| self.files.iter().position(|f| f == a))
                        .map_or(self.files.len(), |i| i + 1);
                    self.files.insert(at, path.clone());
                }
                self.active = Some(path);
                self.shown = true;
            }
            EditorViewOp::Close { path } => {
                let Some(i) = self.files.iter().position(|f| *f == path) else {
                    return;
                };
                self.files.remove(i);
                if self.active.as_ref() == Some(&path) {
                    self.active = self.files.get(i).or_else(|| self.files.last()).cloned();
                }
                if self.files.is_empty() {
                    self.shown = false;
                }
            }
            EditorViewOp::Activate { path } => {
                if self.files.contains(&path) {
                    self.active = Some(path);
                }
            }
            EditorViewOp::Move { path, index } => {
                if let Some(i) = self.files.iter().position(|f| *f == path) {
                    let file = self.files.remove(i);
                    self.files.insert(index.min(self.files.len()), file);
                }
            }
        }
    }
}

/// Orders automation prompts to a pane.
#[derive(Default)]
pub struct PromptLock {
    /// Keeps a paste with its Enter.
    pub submit: tokio::sync::Mutex<()>,
    /// Shared by unwaited prompts and held exclusively by a waited one, each
    /// until its turn starts, so a waited prompt can't claim another's turn.
    pub turn: Arc<tokio::sync::RwLock<()>>,
}

impl Pane {
    pub fn new(id: Uuid, pty: PtyHandle) -> Self {
        Self {
            id,
            pty,
            unread: None,
            prompt: Arc::default(),
            editor: PaneEditor::default(),
        }
    }

    /// Keeps the more severe of the pending and incoming notifications; ties take the newer.
    pub fn notify(&mut self, alert: PaneAlert) {
        if self
            .unread
            .as_ref()
            .is_none_or(|pending| alert.level.rank() >= pending.level.rank())
        {
            self.unread = Some(alert);
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, clap::ValueEnum)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(test, derive(ts_rs::TS))]
pub enum NotificationLevel {
    Info,
    Attention,
    Success,
    Error,
}

impl NotificationLevel {
    fn rank(self) -> u8 {
        match self {
            Self::Info => 0,
            Self::Success => 1,
            Self::Attention => 2,
            Self::Error => 3,
        }
    }
}

/// A pane notification nobody has viewed yet.
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct PaneAlert {
    pub level: NotificationLevel,
    pub title: Option<String>,
    pub body: Option<String>,
}

/// Lifecycle states for an agent occupying a pane.
///
/// This intentionally follows Herdr's small semantic vocabulary. Failure and
/// permission details belong in the accompanying message/notification instead
/// of multiplying lifecycle states.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(test, derive(ts_rs::TS))]
pub enum AgentState {
    Unknown,
    Idle,
    Working,
    Blocked,
    Done,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct AgentStatus {
    pub state: AgentState,
    pub agent: Option<String>,
    pub source: Option<String>,
    pub message: Option<String>,
    /// Background work the agent's screen shows, such as "2 shells, 1 monitor".
    pub background: Option<String>,
}

impl Default for AgentStatus {
    fn default() -> Self {
        Self {
            state: AgentState::Unknown,
            agent: None,
            source: None,
            message: None,
            background: None,
        }
    }
}

#[derive(Serialize, Deserialize, Clone)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct PaneSnapshot {
    pub id: Uuid,
    pub title: Option<String>,
    pub cwd: Option<String>,
    /// `cwd` is on another host.
    #[serde(default)]
    pub cwd_remote: bool,
    #[serde(default)]
    pub agent_status: AgentStatus,
    /// Names the running agent instance, so a client can target it.
    #[serde(default)]
    #[cfg_attr(test, ts(type = "number | null"))]
    pub agent_id: Option<u64>,
    #[serde(default)]
    pub unread: Option<PaneAlert>,
    #[serde(default)]
    pub git: Option<crate::pane_git::PaneGit>,
    #[serde(default)]
    pub editor: PaneEditor,
}

/// A pane running an agent, as listed to automation.
#[derive(Serialize, Clone)]
pub struct AgentPane {
    pub pane_id: Uuid,
    pub session_id: Uuid,
    pub session_name: String,
    pub window_index: usize,
    pub window_name: String,
    pub cwd: Option<String>,
    pub status: AgentStatus,
}

/// One entry per session for the StatusBar and the session picker.
#[derive(Serialize, Clone)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct SessionSummary {
    pub id: Uuid,
    pub name: String,
    pub runtime: Runtime,
}

/// Where a session's shells run.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
#[cfg_attr(test, derive(ts_rs::TS))]
pub enum Runtime {
    #[default]
    Local,
    /// The shared Multipass instance, with these host directories mounted
    /// at the same paths.
    Multipass {
        mounts: Vec<String>,
        shell: crate::multipass::GuestShell,
    },
}

impl Runtime {
    /// [`Self::validate`] off the async runtime; it reads the filesystem.
    pub async fn validated(self) -> Result<Self, String> {
        tokio::task::spawn_blocking(move || self.validate())
            .await
            .map_err(|e| e.to_string())?
    }

    /// With canonical mounts, at least one, and the git dirs they need.
    pub fn validate(self) -> Result<Self, String> {
        match self {
            Self::Local => Ok(Self::Local),
            Self::Multipass { mounts, shell } => {
                if !crate::multipass::available() {
                    return Err("Multipass is not installed (https://multipass.run)".into());
                }
                Ok(Self::Multipass {
                    mounts: crate::multipass::mounts(&mounts)?,
                    shell,
                })
            }
        }
    }
}

/// One pane with its window and session, for `GET /api/panes` and the tmux-style
/// CLI. Fields are named like tmux format variables.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct PaneInfo {
    pub session_id: Uuid,
    pub session_name: String,
    pub window_id: Uuid,
    pub window_index: usize,
    pub window_name: String,
    pub window_active: bool,
    pub window_zoomed_flag: bool,
    pub pane_id: Uuid,
    /// Layout order, as `prefix + q` numbers panes.
    pub pane_index: usize,
    pub pane_active: bool,
    pub pane_current_path: Option<String>,
    pub pane_title: Option<String>,
}

#[cfg(test)]
mod editor_tests {
    use super::*;

    fn open(e: &mut PaneEditor, path: &str) {
        e.apply(EditorViewOp::Open {
            path: path.into(),
            line: None,
        });
    }

    #[test]
    fn opens_after_the_active_file_and_closes_to_a_neighbor() {
        let mut e = PaneEditor::default();
        open(&mut e, "/a");
        open(&mut e, "/b");
        e.apply(EditorViewOp::Activate { path: "/a".into() });
        open(&mut e, "/c");
        assert_eq!(e.files, ["/a", "/c", "/b"]);
        assert!(e.shown);
        e.apply(EditorViewOp::Close { path: "/c".into() });
        assert_eq!(e.active.as_deref(), Some("/b"));
        e.apply(EditorViewOp::Move {
            path: "/b".into(),
            index: 0,
        });
        assert_eq!(e.files, ["/b", "/a"]);
        e.apply(EditorViewOp::Close { path: "/b".into() });
        e.apply(EditorViewOp::Close { path: "/a".into() });
        assert_eq!((e.active, e.shown), (None, false));
    }

    #[test]
    fn follows_moved_and_deleted_files() {
        use std::path::Path;
        let mut e = PaneEditor::default();
        open(&mut e, "/d/a");
        open(&mut e, "/d/sub/b");
        open(&mut e, "/x");
        e.apply(EditorViewOp::Activate {
            path: "/d/sub/b".into(),
        });
        assert!(e.moved(Path::new("/d"), Some(Path::new("/e")), &[]));
        assert_eq!(e.files, ["/e/a", "/e/sub/b", "/x"]);
        assert_eq!(e.active.as_deref(), Some("/e/sub/b"));
        assert!(!e.moved(Path::new("/nope"), None, &[]));
        assert!(!e.moved(Path::new("/e/sub/b"), None, &["/e/sub/b".into()]));
        assert!(e.moved(Path::new("/e/sub/b"), None, &[]));
        assert_eq!(e.files, ["/e/a", "/x"]);
        assert_eq!(e.active.as_deref(), Some("/x"));
        assert!(e.moved(Path::new("/"), None, &[]));
        assert_eq!((e.active, e.shown), (None, false));
    }
}
