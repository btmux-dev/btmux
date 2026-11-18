//! `capture-pane` (`prefix + [`): a pane's text in a file, opened over the
//! pane in the tab's editor. Nothing is typed into the pane, where an agent
//! or another program may be running.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};

use uuid::Uuid;

use crate::config::Editor;
use crate::popup::{PopupCommand, Spawn};
use crate::session::EditorViewOp;
use crate::ws::nvim::PrivateDir;
use crate::AppState;

const DIR_PREFIX: &str = "btmux-capture-";

/// Removed on shutdown.
static DIR: Mutex<Option<PrivateDir>> = Mutex::new(None);
/// Those of processes this one was upgraded from, whose captures editors
/// still show. Removed on shutdown.
static ADOPTED: Mutex<Vec<PrivateDir>> = Mutex::new(Vec::new());
static COUNT: AtomicU64 = AtomicU64::new(0);

/// Save `content` as a private file; its path has symlinks resolved, as
/// editors name it.
fn save(content: &str) -> Result<PathBuf, String> {
    let mut dir = DIR.lock().unwrap();
    if dir.as_ref().is_none_or(|d| !d.path().is_dir()) {
        *dir = Some(PrivateDir::named(DIR_PREFIX)?);
    }
    let n = COUNT.fetch_add(1, Ordering::Relaxed) + 1;
    let path = resolve(dir.as_ref().unwrap().path()).join(format!("capture-{n}.txt"));
    crate::discovery::create_private_file(&path)
        .and_then(|mut file| file.write_all(content.as_bytes()))
        .map_err(|e| format!("write {}: {e}", path.display()))?;
    Ok(path)
}

fn resolve(path: &Path) -> PathBuf {
    crate::native_editor::file_ops::resolve_new(path).unwrap_or_else(|_| path.to_path_buf())
}

fn temp_dir() -> &'static Path {
    static DIR: OnceLock<PathBuf> = OnceLock::new();
    DIR.get_or_init(|| resolve(&std::env::temp_dir()))
}

/// The capture directory holding `path` (as editors name it), also one a
/// process before an upgrade made.
fn capture_dir(path: &str) -> Option<&Path> {
    let dir = Path::new(path).parent()?;
    let name = dir.file_name()?.to_str()?;
    (name.starts_with(DIR_PREFIX) && dir.parent() == Some(temp_dir())).then_some(dir)
}

pub fn is_capture(path: &str) -> bool {
    capture_dir(path).is_some()
}

/// Remove, on shutdown, the directories of captures restored editors show.
pub fn adopt<'a>(paths: impl IntoIterator<Item = &'a String>) {
    let mut adopted = ADOPTED.lock().unwrap();
    for dir in paths.into_iter().filter_map(|p| capture_dir(p)) {
        if dir.is_dir() && !adopted.iter().any(|d| d.path() == dir) {
            adopted.push(PrivateDir::adopt(dir.to_path_buf()));
        }
    }
}

/// Delete a capture no editor shows any more.
pub fn discard(path: &str) {
    if is_capture(path) {
        let _ = std::fs::remove_file(path);
    }
}

pub fn shutdown() {
    DIR.lock().unwrap().take();
    ADOPTED.lock().unwrap().clear();
}

/// Save `content`, captured from `pane_id`, and open it over the pane in
/// `editor`, the requesting tab's.
pub async fn open(
    state: &AppState,
    pane_id: Uuid,
    content: String,
    editor: Editor,
) -> Result<(), String> {
    let path = tokio::task::spawn_blocking(move || save(&content))
        .await
        .map_err(|e| e.to_string())??;
    let client = crate::fs_ops::client_path(&path);
    let opened = match editor {
        Editor::Native => {
            let op = EditorViewOp::Open {
                path: client,
                line: None,
            };
            crate::native_editor::view(state, pane_id, op).await
        }
        Editor::Neovim => crate::ws::nvim::open(&client, None).await,
        Editor::Vscode => {
            let folder = {
                let mgr = state.read().await;
                let pane = mgr.find_pane(pane_id).ok_or("Pane no longer exists")?;
                mgr.editor_folder(pane)
            };
            crate::vscode::open(&client, None, folder.as_deref());
            Ok(())
        }
        Editor::Pane => popup(state, pane_id, &path).await,
    };
    if opened.is_err() {
        let _ = std::fs::remove_file(&path);
    }
    opened
}

/// Run the user's editor on `path` in a popup over the pane, which removes
/// it after.
async fn popup(state: &AppState, pane_id: Uuid, path: &Path) -> Result<(), String> {
    let (dialect, guest) = {
        let mgr = state.read().await;
        let pane = mgr.find_pane(pane_id).ok_or("Pane no longer exists")?;
        (
            pane.pty.dialect(),
            pane.pty.guest_instance().map(str::to_string),
        )
    };
    let command = match guest {
        // A guest pane's popup runs in the guest, which can't see the host's temp dir.
        Some(instance) => {
            let host = path.to_path_buf();
            let guest = format!("/tmp/{DIR_PREFIX}{}.txt", Uuid::new_v4().simple());
            let target = guest.clone();
            tokio::task::spawn_blocking(move || {
                let copied = crate::multipass::copy_in(&instance, &host, &target);
                let _ = std::fs::remove_file(&host);
                copied.map_err(|e| format!("cannot copy the capture into {instance}: {e}"))
            })
            .await
            .map_err(|e| e.to_string())??;
            crate::shell::Dialect::Posix.edit(&guest)
        }
        None => dialect.edit(&path.to_string_lossy()),
    };
    let spawn = Spawn {
        command: PopupCommand::Shell(command),
        env: None,
    };
    let mut mgr = state.write().await;
    let title = Some("Pane capture".to_string());
    crate::popup::open(&mut mgr, pane_id, Uuid::new_v4(), spawn, None, title)
        .map(|_| ())
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saves_private_captures_it_recognizes() {
        let path = save("hello\n").unwrap();
        let client = crate::fs_ops::client_path(&path);
        assert!(is_capture(&client));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "hello\n");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        discard(&client);
        assert!(!path.exists());
        shutdown();
        assert!(!path.parent().unwrap().exists());
    }

    #[test]
    fn recognizes_only_files_in_capture_dirs() {
        let temp = crate::fs_ops::client_path(temp_dir());
        assert!(is_capture(&format!("{temp}/{DIR_PREFIX}ab/capture-1.txt")));
        assert!(!is_capture(&format!("{temp}/btmux-ab/capture-1.txt")));
        assert!(!is_capture(&format!(
            "{temp}/x/{DIR_PREFIX}ab/capture-1.txt"
        )));
        assert!(!is_capture(&format!("/home/{DIR_PREFIX}ab/capture-1.txt")));
    }
}
