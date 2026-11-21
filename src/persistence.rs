//! On-disk persistence of the session tree so structure survives a server
//! restart. We serialize the same `SessionSnapshot`s broadcast to the browser
//! (session/window/pane tree, layout + split ratios, names, active indices, and
//! each pane's last-known cwd). Live process state and scrollback cannot be
//! restored — restored panes get a fresh shell, spawned lazily in their saved
//! cwd (see `SessionManager::restore_from_snapshots`).
//!
//! The default state file lives under `$XDG_STATE_HOME/btmux/state.json`
//! (falling back to `$HOME/.local/state/btmux/state.json`). Named profiles use
//! `$XDG_STATE_HOME/btmux/<profile>/state.json`. State is deliberately *not* in
//! the config dir, which `main.rs` watches for live config reloads; writing there
//! on every mutation would spuriously trigger a reload.

use std::path::{Path, PathBuf};

use crate::session::manager::SessionSnapshot;
use crate::session::{EditorViewOp, PaneEditor};

/// Resolve the XDG state base dir: `$XDG_STATE_HOME/btmux/`, falling back to
/// `$HOME/.local/state/btmux/`. Returns `None` when neither env var is set.
fn state_dir() -> Option<PathBuf> {
    crate::discovery::state_dir()
}

/// Resolve the state file path. Without a profile this is
/// `$XDG_STATE_HOME/btmux/state.json`, falling back to
/// `$HOME/.local/state/btmux/state.json`. With a profile it is stored under a
/// profile-specific subdirectory. Returns `None` when neither env var is set
/// (persistence is then disabled).
pub fn state_path(profile: Option<&str>) -> Result<Option<PathBuf>, String> {
    let Some(dir) = state_dir() else {
        return Ok(None);
    };

    let path = match profile {
        None => dir.join("state.json"),
        Some(profile) => {
            if profile.is_empty()
                || profile == "."
                || profile == ".."
                || profile.contains('/')
                || profile.contains('\\')
                || profile.contains('\0')
            {
                return Err(format!(
                    "invalid profile {profile:?}: use a non-empty name without path separators"
                ));
            }
            dir.join(profile).join("state.json")
        }
    };

    Ok(Some(path))
}

/// Resolve the log directory: `$XDG_STATE_HOME/btmux/log/`.
pub fn log_dir() -> Option<PathBuf> {
    state_dir().map(|d| {
        let dir = d.join("log");
        let _ = std::fs::create_dir_all(&dir);
        dir
    })
}

/// Load and parse saved snapshots. Returns `None` if the file is absent or
/// unreadable/corrupt — the caller then starts with a fresh default session.
pub fn load(path: &Path) -> Option<Vec<SessionSnapshot>> {
    let contents = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return None,
        Err(e) => {
            tracing::warn!("could not read state file {}: {}", path.display(), e);
            return None;
        }
    };
    match serde_json::from_str::<Vec<SessionSnapshot>>(&contents) {
        Ok(snaps) => Some(snaps),
        Err(e) => {
            tracing::warn!("ignoring corrupt state file {}: {}", path.display(), e);
            None
        }
    }
}

/// Write snapshots to disk atomically (write to a sibling temp file, then
/// rename) so a crash mid-write can't leave a truncated, unparseable file.
/// Saves `snapshots`, with each of `commands` (panes that run a command)
/// under a new ID: after a restart it is an ordinary shell, so a waiter for
/// the command finds it gone rather than waiting on a shell that never ran it.
pub fn save(
    path: &Path,
    snapshots: &[SessionSnapshot],
    commands: &std::collections::HashSet<uuid::Uuid>,
) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let json = renew_ids(&serialize_snapshots(snapshots)?, commands);
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, json).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, path).map_err(|e| e.to_string())?;
    Ok(())
}

/// `json` with every one of `ids` replaced by a new ID, wherever it appears
/// (the pane, its layout leaf).
fn renew_ids(json: &str, ids: &std::collections::HashSet<uuid::Uuid>) -> String {
    ids.iter().fold(json.to_owned(), |json, id| {
        json.replace(&id.to_string(), &uuid::Uuid::new_v4().to_string())
    })
}

fn serialize_snapshots(snapshots: &[SessionSnapshot]) -> Result<String, String> {
    let mut value = serde_json::to_value(snapshots).map_err(|e| e.to_string())?;
    for session in value
        .as_array_mut()
        .expect("session snapshots serialize as an array")
    {
        for window in session["windows"]
            .as_array_mut()
            .expect("window snapshots serialize as an array")
        {
            for pane in window["panes"]
                .as_array_mut()
                .expect("pane snapshots serialize as an array")
            {
                let pane = pane
                    .as_object_mut()
                    .expect("pane snapshots serialize as an object");
                pane.remove("agent_status");
                pane.remove("agent_since");
                pane.remove("unread");
                pane.remove("git");
                pane.remove("cwd_remote");
                // Their files are removed on shutdown.
                if let Some(editor) = pane.get_mut("editor") {
                    let mut parsed: PaneEditor =
                        serde_json::from_value(editor.take()).expect("pane editors deserialize");
                    let captures: Vec<String> = parsed
                        .files
                        .iter()
                        .filter(|path| crate::capture::is_capture(path))
                        .cloned()
                        .collect();
                    for path in captures {
                        parsed.apply(EditorViewOp::Close { path });
                    }
                    *editor = serde_json::to_value(parsed).map_err(|e| e.to_string())?;
                }
            }
        }
    }
    serde_json::to_string(&value).map_err(|e| e.to_string())
}

#[cfg(test)]
mod agent_status_tests {
    use super::*;
    use crate::session::{
        layout::Layout, manager::WindowSnapshot, AgentState, AgentStatus, NotificationLevel,
        PaneAlert, PaneSnapshot,
    };
    use uuid::Uuid;

    #[test]
    fn command_panes_are_saved_under_new_ids() {
        let (kept, renewed) = (uuid::Uuid::new_v4(), uuid::Uuid::new_v4());
        let json =
            format!(r#"{{"panes":[{{"id":"{kept}"}},{{"id":"{renewed}"}}],"layout":"{renewed}"}}"#);
        let saved = renew_ids(&json, &std::collections::HashSet::from([renewed]));
        assert!(saved.contains(&kept.to_string()));
        assert!(!saved.contains(&renewed.to_string()));
        let value: serde_json::Value = serde_json::from_str(&saved).unwrap();
        // The pane and its layout leaf get the same new ID.
        assert_eq!(value["panes"][1]["id"], value["layout"]);
    }

    #[test]
    fn drops_pane_captures_from_saved_editors() {
        let _lock = crate::capture::TEST_LOCK.lock().unwrap();
        let pane_id = Uuid::new_v4();
        let capture = crate::capture::save_for_test("text\n");
        // Named like one, but not made by this server.
        let lookalike = std::path::Path::new(&capture)
            .with_file_name("capture-0.txt")
            .to_string_lossy()
            .into_owned();
        let snapshots = vec![SessionSnapshot {
            id: Uuid::new_v4(),
            name: "test".to_string(),
            runtime: Default::default(),
            windows: vec![WindowSnapshot {
                id: Uuid::new_v4(),
                name: "shell".to_string(),
                panes: vec![PaneSnapshot {
                    id: pane_id,
                    title: None,
                    cwd: None,
                    cwd_remote: false,
                    agent_status: Default::default(),
                    agent_id: None,
                    agent_since: None,
                    unread: None,
                    git: None,
                    editor: crate::session::PaneEditor {
                        shown: true,
                        files: vec!["/src/a.rs".into(), lookalike.clone(), capture.clone()],
                        active: Some(capture.clone()),
                    },
                }],
                active_pane: 0,
                layout: Layout::Leaf { pane_id },
                zoomed_pane: None,
            }],
            active_window: 0,
        }];
        let restored: Vec<SessionSnapshot> =
            serde_json::from_str(&serialize_snapshots(&snapshots).unwrap()).unwrap();
        let editor = &restored[0].windows[0].panes[0].editor;
        assert_eq!(editor.files, ["/src/a.rs".to_string(), lookalike]);
        assert!(editor.shown);
        crate::capture::discard(&capture);
    }

    #[test]
    fn excludes_live_agent_status_and_notifications_from_saved_state() {
        let pane_id = Uuid::new_v4();
        let snapshots = vec![SessionSnapshot {
            id: Uuid::new_v4(),
            name: "test".to_string(),
            runtime: Default::default(),
            windows: vec![WindowSnapshot {
                id: Uuid::new_v4(),
                name: "shell".to_string(),
                panes: vec![PaneSnapshot {
                    id: pane_id,
                    title: None,
                    cwd: None,
                    cwd_remote: false,
                    agent_status: AgentStatus {
                        state: AgentState::Blocked,
                        agent: Some("codex".to_string()),
                        source: Some("hook".to_string()),
                        message: Some("private prompt".to_string()),
                        background: None,
                    },
                    agent_id: None,
                    agent_since: Some(1),
                    unread: Some(PaneAlert {
                        level: NotificationLevel::Attention,
                        title: Some("Done".to_string()),
                        body: Some("private reply".to_string()),
                    }),
                    git: Some(crate::pane_git::PaneGit {
                        repo_id: "/src/repo/.git".to_string(),
                        repo_name: "repo".to_string(),
                        worktree_root: "/src/repo".to_string(),
                        branch: Some("main".to_string()),
                        main_worktree: true,
                    }),
                    editor: Default::default(),
                }],
                active_pane: 0,
                layout: Layout::Leaf { pane_id },
                zoomed_pane: None,
            }],
            active_window: 0,
        }];
        let json = serialize_snapshots(&snapshots).unwrap();
        assert!(!json.contains("agent_status"));
        assert!(!json.contains("agent_since"));
        assert!(!json.contains("private prompt"));
        assert!(!json.contains("private reply"));
        assert!(!json.contains("worktree_root"));
        let restored: Vec<SessionSnapshot> = serde_json::from_str(&json).unwrap();
        assert_eq!(
            restored[0].windows[0].panes[0].agent_status.state,
            AgentState::Unknown
        );
    }
}

/// Held for the lifetime of the server. The lock file must never be unlinked:
/// doing so would allow a second process to lock a different inode.
pub struct ProfileLock(std::fs::File);

impl ProfileLock {
    pub fn acquire(path: &Path) -> Result<Self, String> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        let mut options = std::fs::OpenOptions::new();
        options.read(true).write(true).create(true).truncate(false);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
        let file = options
            .open(path.with_extension("lock"))
            .map_err(|e| e.to_string())?;
        if let Err(error) = file.try_lock() {
            return Err(format!("session profile {} is already in use (or cannot be locked): {error}. Use --profile with a different name", path.display()));
        }
        Ok(Self(file))
    }

    /// A lock held by the server this one re-executed from.
    #[cfg(unix)]
    pub fn adopt(fd: std::os::fd::OwnedFd) -> Self {
        Self(fd.into())
    }

    #[cfg(unix)]
    pub fn raw_fd(&self) -> std::os::fd::RawFd {
        use std::os::fd::AsRawFd;
        self.0.as_raw_fd()
    }
}

impl Drop for ProfileLock {
    fn drop(&mut self) {
        let _ = self.0.unlock();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn excludes_concurrent_profile_owners_and_releases_on_drop() {
        let dir = std::env::temp_dir().join(format!("btmux-lock-{}", uuid::Uuid::new_v4()));
        let path = dir.join("state.json");
        let lock = ProfileLock::acquire(&path).unwrap();
        assert!(ProfileLock::acquire(&path).is_err());
        drop(lock);
        drop(ProfileLock::acquire(&path).unwrap());
        std::fs::remove_dir_all(dir).unwrap();
    }
}
