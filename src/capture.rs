//! `capture-pane` (`prefix + [`): a pane's text in a file, opened over the
//! pane in the tab's editor. Nothing is typed into the pane, where an agent
//! or another program may be running.

use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::config::Editor;
use crate::popup::{PopupCommand, Spawn};
use crate::session::EditorViewOp;
use crate::AppState;

const DIR_PREFIX: &str = "btmux-capture-";

/// The captures this server made, so that only they are ever deleted.
/// Handed off on upgrade.
#[derive(Serialize, Deserialize, Default, Clone, Debug, PartialEq)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct Captures {
    /// Made for captures; removed on shutdown once empty.
    dirs: Vec<PathBuf>,
    /// Captures on the host, as editors name them.
    files: Vec<String>,
    /// Captures popups show, removed when the popup ends.
    popups: Vec<PopupCapture>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct PopupCapture {
    popup: Uuid,
    /// The instance whose file `path` is; `None` on the host.
    guest: Option<String>,
    path: String,
}

static CAPTURES: Mutex<Captures> = Mutex::new(Captures {
    dirs: Vec::new(),
    files: Vec::new(),
    popups: Vec::new(),
});
static COUNT: AtomicU64 = AtomicU64::new(0);

/// How long shutdown waits to remove captures from guests.
const GUEST_CLEANUP: Duration = Duration::from_secs(2);

/// Save `content` as a private file; its path has symlinks resolved, as
/// editors name it.
fn save(content: &str) -> Result<PathBuf, String> {
    let mut captures = CAPTURES.lock().unwrap();
    let dir = match captures.dirs.last().filter(|dir| dir.is_dir()) {
        Some(dir) => dir.clone(),
        None => {
            let name = format!("{DIR_PREFIX}{}", Uuid::new_v4().simple());
            let dir = std::env::temp_dir().join(name);
            crate::ws::nvim::create_private_dir(&dir)?;
            let dir = crate::native_editor::file_ops::resolve_new(&dir).unwrap_or(dir);
            captures.dirs.push(dir.clone());
            dir
        }
    };
    loop {
        let n = COUNT.fetch_add(1, Ordering::Relaxed) + 1;
        let path = dir.join(format!("capture-{n}.txt"));
        let mut file = match crate::discovery::create_private_file(&path) {
            Ok(file) => file,
            // A process before an upgrade made it.
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(format!("write {}: {e}", path.display())),
        };
        if let Err(e) = file.write_all(content.as_bytes()) {
            let _ = std::fs::remove_file(&path);
            return Err(format!("write {}: {e}", path.display()));
        }
        captures.files.push(crate::fs_ops::client_path(&path));
        return Ok(path);
    }
}

/// Whether `path`, as editors name it, is a capture this server made.
pub fn is_capture(path: &str) -> bool {
    CAPTURES.lock().unwrap().files.iter().any(|f| f == path)
}

/// Delete a capture.
pub fn discard(path: &str) {
    let mut captures = CAPTURES.lock().unwrap();
    if let Some(i) = captures.files.iter().position(|f| f == path) {
        captures.files.remove(i);
        let _ = std::fs::remove_file(path);
    }
}

/// A popup ended: delete the capture it showed.
pub fn popup_ended(popup: Uuid) {
    let capture = {
        let mut captures = CAPTURES.lock().unwrap();
        let Some(i) = captures.popups.iter().position(|c| c.popup == popup) else {
            return;
        };
        captures.popups.remove(i)
    };
    match capture.guest {
        Some(instance) => {
            std::thread::spawn(move || remove_in_guest(&instance, &capture.path));
        }
        None => discard(&capture.path),
    }
}

fn remove_in_guest(instance: &str, path: &str) {
    if let Err(error) = crate::multipass::remove_in(instance, path) {
        tracing::warn!("cannot remove the capture {path} from {instance}: {error}");
    }
}

#[cfg(unix)]
pub fn export() -> Captures {
    CAPTURES.lock().unwrap().clone()
}

/// Take over the captures of the server this one was upgraded from.
#[cfg(unix)]
pub fn restore(handed: Captures) {
    let mut captures = CAPTURES.lock().unwrap();
    captures.dirs.extend(handed.dirs);
    captures.files.extend(handed.files);
    captures.popups.extend(handed.popups);
}

pub fn shutdown() {
    let captures = std::mem::take(&mut *CAPTURES.lock().unwrap());
    let (done, waiting) = std::sync::mpsc::channel();
    let guests: Vec<_> = captures
        .popups
        .into_iter()
        .filter_map(|c| Some((c.guest?, c.path)))
        .collect();
    for (instance, path) in &guests {
        let (done, instance, path) = (done.clone(), instance.clone(), path.clone());
        std::thread::spawn(move || {
            remove_in_guest(&instance, &path);
            let _ = done.send(());
        });
    }
    for file in &captures.files {
        let _ = std::fs::remove_file(file);
    }
    // Only once empty: nothing but captures is ever deleted.
    for dir in &captures.dirs {
        let _ = std::fs::remove_dir(dir);
    }
    let deadline = std::time::Instant::now() + GUEST_CLEANUP;
    for _ in &guests {
        let left = deadline.saturating_duration_since(std::time::Instant::now());
        if waiting.recv_timeout(left).is_err() {
            break;
        }
    }
}

/// The editor that can show a capture of `len` bytes: the native editor
/// loads files up to a limit, `$EDITOR` any.
fn fitting(editor: Editor, len: usize) -> Editor {
    let max = crate::native_editor::buffer::MAX_FILE_BYTES;
    if editor == Editor::Native && len as u64 > max {
        Editor::Pane
    } else {
        editor
    }
}

/// Save `content`, captured from `pane_id`, and open it over the pane in
/// `editor`, the requesting tab's; VS Code opens it in `folder`'s window.
pub async fn open(
    state: &AppState,
    pane_id: Uuid,
    content: String,
    editor: Editor,
    folder: Option<String>,
) -> Result<(), String> {
    let fitting = fitting(editor, content.len());
    let path = tokio::task::spawn_blocking(move || save(&content))
        .await
        .map_err(|e| e.to_string())??;
    let client = crate::fs_ops::client_path(&path);
    let opened = match fitting {
        Editor::Native => {
            let op = EditorViewOp::Open {
                path: client.clone(),
                line: None,
            };
            crate::native_editor::view(state, pane_id, op).await
        }
        Editor::Neovim => crate::ws::nvim::open(&client, None).await,
        Editor::Vscode => {
            crate::vscode::open(&client, None, folder.as_deref());
            Ok(())
        }
        Editor::Pane => {
            let title = if fitting == editor {
                "Pane capture"
            } else {
                "Pane capture (too large for the native editor)"
            };
            popup(state, pane_id, &client, title).await
        }
    };
    if opened.is_err() {
        discard(&client);
    }
    opened
}

/// Run the user's editor on `path` in a popup over the pane; the capture is
/// deleted when the popup ends.
async fn popup(state: &AppState, pane_id: Uuid, path: &str, title: &str) -> Result<(), String> {
    let target = {
        let mgr = state.read().await;
        if mgr.find_pane(pane_id).is_none() {
            return Err("Pane no longer exists".into());
        }
        if mgr.popups.iter().any(|popup| popup.pane_id == pane_id) {
            return Err(crate::popup::OpenError::AlreadyOpen.to_string());
        }
        mgr.popup_target(pane_id)
    };
    let shown = match &target.guest {
        // The guest can't see the host's temp dir.
        Some(instance) => {
            let (instance, host) = (instance.clone(), PathBuf::from(path));
            let guest = format!("/tmp/{DIR_PREFIX}{}.txt", Uuid::new_v4().simple());
            let target = guest.clone();
            let copied = tokio::task::spawn_blocking(move || {
                crate::multipass::copy_in(&instance, &host, &target)
                    .map_err(|e| format!("cannot copy the capture into {instance}: {e}"))
            })
            .await
            .map_err(|e| e.to_string())
            .and_then(|copied| copied);
            discard(path);
            copied?;
            guest
        }
        None => path.to_string(),
    };
    let id = Uuid::new_v4();
    // Before the popup exists, so its end always finds it.
    CAPTURES.lock().unwrap().popups.push(PopupCapture {
        popup: id,
        guest: target.guest.clone(),
        path: shown.clone(),
    });
    let opened = {
        let mut mgr = state.write().await;
        // The popup's PTY is made from the same settings, under this lock.
        if mgr.popup_target(pane_id) != target {
            Err("The pane's shell or sandbox changed; capture it again".to_string())
        } else {
            let spawn = Spawn {
                command: PopupCommand::Shell(target.dialect.edit(&shown)),
                env: None,
            };
            let title = Some(title.to_string());
            crate::popup::open(&mut mgr, pane_id, id, spawn, None, title)
                .map(|_| ())
                .map_err(|e| e.to_string())
        }
    };
    if opened.is_err() {
        popup_ended(id);
    }
    opened
}

/// Held by tests that use the captures, as some clear them.
#[cfg(test)]
pub static TEST_LOCK: Mutex<()> = Mutex::new(());

/// A new capture, as editors name it.
#[cfg(test)]
pub fn save_for_test(content: &str) -> String {
    crate::fs_ops::client_path(&save(content).unwrap())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deletes_only_the_captures_it_made() {
        let _lock = TEST_LOCK.lock().unwrap();
        let path = save("hello\n").unwrap();
        let client = crate::fs_ops::client_path(&path);
        let dir = path.parent().unwrap().to_path_buf();
        assert!(dir
            .file_name()
            .unwrap()
            .to_str()
            .unwrap()
            .starts_with(DIR_PREFIX));
        assert!(is_capture(&client));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "hello\n");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }

        // A file it didn't make, even beside its captures, stays.
        let other = dir.join("notes.txt");
        std::fs::write(&other, "mine").unwrap();
        let other_client = crate::fs_ops::client_path(&other);
        assert!(!is_capture(&other_client));
        discard(&other_client);
        assert!(other.exists());

        discard(&client);
        assert!(!path.exists() && !is_capture(&client));
        shutdown();
        assert!(other.exists(), "shutdown keeps what it didn't make");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// An upgraded server deletes its predecessor's captures as its own.
    #[cfg(unix)]
    #[test]
    fn hands_off_captures_and_their_popups() {
        let _lock = TEST_LOCK.lock().unwrap();
        let path = save_for_test("x");
        let dir = std::path::Path::new(&path).parent().unwrap().to_path_buf();
        let popup = Uuid::new_v4();
        CAPTURES.lock().unwrap().popups.push(PopupCapture {
            popup,
            guest: None,
            path: path.clone(),
        });
        let handed = serde_json::to_value(export()).unwrap();
        *CAPTURES.lock().unwrap() = Captures::default();
        assert!(!is_capture(&path));
        restore(serde_json::from_value(handed).unwrap());
        assert!(is_capture(&path));
        popup_ended(popup);
        assert!(!std::path::Path::new(&path).exists() && !is_capture(&path));
        shutdown();
        assert!(!dir.exists());
    }

    #[test]
    fn opens_large_captures_with_editor() {
        let max = crate::native_editor::buffer::MAX_FILE_BYTES as usize;
        assert_eq!(fitting(Editor::Native, max), Editor::Native);
        assert_eq!(fitting(Editor::Native, max + 1), Editor::Pane);
        assert_eq!(fitting(Editor::Neovim, max + 1), Editor::Neovim);
    }
}
