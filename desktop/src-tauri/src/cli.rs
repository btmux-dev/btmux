//! Installs the bundled `btmux` as `/usr/local/bin/btmux`, a symlink into the
//! app bundle, so it updates with the app.

use std::path::{Path, PathBuf};
use std::process::Command;

use tauri::AppHandle;
use tauri_plugin_dialog::{
    DialogExt, MessageDialogButtons, MessageDialogKind, MessageDialogResult,
};

const LINK_DIR: &str = "/usr/local/bin";
const REMOVE: &str = "Remove";

/// What the link's path holds.
#[derive(Debug, PartialEq)]
enum Existing {
    Nothing,
    /// A link to this app's binary.
    Ours,
    /// A link into another btmux app bundle (moved or another copy).
    App,
    /// Anything else, such as a standalone or Homebrew btmux.
    Other(Option<PathBuf>),
}

fn existing(link: &Path, target: &Path) -> Existing {
    match std::fs::read_link(link) {
        Ok(dest) if dest == target => Existing::Ours,
        Ok(dest) if is_app_binary(&dest) => Existing::App,
        Ok(dest) => Existing::Other(Some(dest)),
        Err(_) if link.symlink_metadata().is_ok() => Existing::Other(None),
        Err(_) => Existing::Nothing,
    }
}

fn is_app_binary(path: &Path) -> bool {
    path.ends_with("Contents/MacOS/btmux")
        && path
            .ancestors()
            .nth(3)
            .and_then(Path::extension)
            .is_some_and(|ext| ext == "app")
}

/// The bundled binary, if its path is one the link can keep.
fn bundled() -> Result<PathBuf, String> {
    let exe = std::env::current_exe()
        .and_then(std::fs::canonicalize)
        .map_err(|_| "Could not find the btmux app.".to_string())?;
    let target = exe.with_file_name("btmux");
    stable(&target)?;
    if !target.is_file() {
        return Err("Could not find the bundled btmux command.".into());
    }
    Ok(target)
}

fn stable(target: &Path) -> Result<(), String> {
    let path = target.to_string_lossy();
    if path.contains("/AppTranslocation/") || path.starts_with("/Volumes/") {
        return Err(
            "Move btmux to your Applications folder and open it from there, then install the command again."
                .into(),
        );
    }
    if path.starts_with("/nix/store/") {
        return Err(
            "Nix moves this app on every update. Install the btmux package with Nix instead."
                .into(),
        );
    }
    Ok(())
}

/// Asks whether to install, replace, or remove the command, and does it.
pub fn install(app: AppHandle) {
    std::thread::spawn(move || {
        let target = match bundled() {
            Ok(target) => target,
            Err(error) => {
                return alert(
                    &app,
                    MessageDialogKind::Error,
                    "Could not install the btmux command",
                    &error,
                );
            }
        };
        let link = Path::new(LINK_DIR).join("btmux");
        match existing(&link, &target) {
            Existing::Ours => {
                let removing = app
                    .dialog()
                    .message(format!(
                        "{} runs this app's btmux and updates with the app.",
                        link.display()
                    ))
                    .title("The btmux command is installed")
                    .buttons(MessageDialogButtons::OkCancelCustom(
                        "OK".into(),
                        REMOVE.into(),
                    ))
                    .blocking_show_with_result()
                    == MessageDialogResult::Custom(REMOVE.into());
                if removing {
                    match remove(&link) {
                        Ok(()) | Err(Failure::Cancelled) => {}
                        Err(Failure::Error(error)) => alert(
                            &app,
                            MessageDialogKind::Error,
                            "Could not remove the btmux command",
                            &error,
                        ),
                    }
                }
                return;
            }
            Existing::Other(dest) => {
                let what = match dest {
                    Some(dest) => format!("{} links to {}.", link.display(), dest.display()),
                    None => format!("{} is another btmux.", link.display()),
                };
                let replace = app
                    .dialog()
                    .message(format!("{what} Replace it with this app's btmux?"))
                    .title("Replace the btmux command?")
                    .kind(MessageDialogKind::Warning)
                    .buttons(MessageDialogButtons::OkCancelCustom(
                        "Replace".into(),
                        "Cancel".into(),
                    ))
                    .blocking_show();
                if !replace {
                    return;
                }
            }
            Existing::App | Existing::Nothing => {}
        }
        match create(&target, &link) {
            Ok(()) => {
                let mut message = format!(
                    "{} runs this app's btmux and updates with the app. Open a new terminal to use it.",
                    link.display()
                );
                let path = crate::login_shell().and_then(|shell| crate::login_path(&shell));
                if let Some(note) = path.and_then(|path| shadowed(&path, &target)) {
                    message = format!("{message}\n\n{note}");
                }
                alert(
                    &app,
                    MessageDialogKind::Info,
                    "Installed the btmux command",
                    &message,
                );
            }
            Err(Failure::Cancelled) => {}
            Err(Failure::Error(error)) => alert(
                &app,
                MessageDialogKind::Error,
                "Could not install the btmux command",
                &error,
            ),
        }
    });
}

fn alert(app: &AppHandle, kind: MessageDialogKind, title: &str, message: &str) {
    app.dialog()
        .message(message)
        .title(title)
        .kind(kind)
        .blocking_show();
}

/// Why `btmux` in `path` (a `PATH` value) isn't the bundled one, if it isn't.
fn shadowed(path: &str, target: &Path) -> Option<String> {
    let found = path
        .split(':')
        .filter(|dir| !dir.is_empty())
        .map(|dir| Path::new(dir).join("btmux"))
        .find(|candidate| is_executable(candidate));
    match found {
        None => Some(format!(
            "{LINK_DIR} isn't on your shell's PATH. Add it to use the command."
        )),
        Some(found) if std::fs::canonicalize(&found).ok().as_deref() == Some(target) => None,
        Some(found) => Some(format!(
            "Your shell runs {} first. Remove it, or put {LINK_DIR} before its folder in PATH.",
            found.display()
        )),
    }
}

fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path)
        .is_ok_and(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
}

enum Failure {
    Cancelled,
    Error(String),
}

fn create(target: &Path, link: &Path) -> Result<(), Failure> {
    // Renamed into place, so the command is never missing.
    let temp = link.with_file_name(format!(".btmux-{}", std::process::id()));
    let _ = std::fs::remove_file(&temp);
    let linked = std::os::unix::fs::symlink(target, &temp)
        .and_then(|()| std::fs::rename(&temp, link))
        .inspect_err(|_| {
            let _ = std::fs::remove_file(&temp);
        });
    match linked {
        Ok(()) => Ok(()),
        Err(error) if needs_admin(&error) => {
            let target = target
                .to_str()
                .ok_or_else(|| Failure::Error("The app's path isn't valid UTF-8.".into()))?;
            as_admin(
                &format!(
                    "/bin/mkdir -p {LINK_DIR} && /bin/ln -sfn {} {}",
                    quote(target),
                    quote(&link.to_string_lossy())
                ),
                &format!("btmux wants to install the btmux command in {LINK_DIR}."),
            )
        }
        Err(error) => Err(Failure::Error(error.to_string())),
    }
}

fn remove(link: &Path) -> Result<(), Failure> {
    match std::fs::remove_file(link) {
        Ok(()) => Ok(()),
        Err(error) if needs_admin(&error) => as_admin(
            &format!("/bin/rm -f {}", quote(&link.to_string_lossy())),
            &format!("btmux wants to remove the btmux command from {LINK_DIR}."),
        ),
        Err(error) => Err(Failure::Error(error.to_string())),
    }
}

fn needs_admin(error: &std::io::Error) -> bool {
    matches!(
        error.kind(),
        std::io::ErrorKind::PermissionDenied | std::io::ErrorKind::NotFound
    )
}

// Arguments, not script text, so nothing needs AppleScript quoting.
fn as_admin(command: &str, prompt: &str) -> Result<(), Failure> {
    let output = Command::new("/usr/bin/osascript")
        .args([
            "-e",
            "on run argv",
            "-e",
            "do shell script (item 1 of argv) with prompt (item 2 of argv) with administrator privileges",
            "-e",
            "end run",
            command,
            prompt,
        ])
        .output()
        .map_err(|error| Failure::Error(error.to_string()))?;
    if output.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    // "User canceled. (-128)"
    if stderr.contains("(-128)") {
        return Err(Failure::Cancelled);
    }
    Err(Failure::Error(stderr.trim().to_string()))
}

fn quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', r"'\''"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_existing_commands() {
        let dir = std::env::temp_dir().join(format!("btmux-cli-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&dir).unwrap();
        let link = dir.join("btmux");
        let target = Path::new("/Applications/btmux.app/Contents/MacOS/btmux");
        assert_eq!(existing(&link, target), Existing::Nothing);
        std::os::unix::fs::symlink(target, &link).unwrap();
        assert_eq!(existing(&link, target), Existing::Ours);
        let moved = Path::new("/Users/me/Applications/btmux.app/Contents/MacOS/btmux");
        assert_eq!(existing(&link, moved), Existing::App);
        std::fs::remove_file(&link).unwrap();
        std::os::unix::fs::symlink("/opt/homebrew/Cellar/btmux/1/bin/btmux", &link).unwrap();
        assert!(matches!(existing(&link, target), Existing::Other(Some(_))));
        std::fs::remove_file(&link).unwrap();
        std::fs::write(&link, "").unwrap();
        assert_eq!(existing(&link, target), Existing::Other(None));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn refuses_moving_paths() {
        assert!(stable(Path::new("/Applications/btmux.app/Contents/MacOS/btmux")).is_ok());
        for path in [
            "/private/var/folders/x/AppTranslocation/y/d/btmux.app/Contents/MacOS/btmux",
            "/Volumes/btmux/btmux.app/Contents/MacOS/btmux",
            "/nix/store/abc-btmux-desktop/Applications/btmux.app/Contents/MacOS/btmux",
        ] {
            assert!(stable(Path::new(path)).is_err(), "{path}");
        }
    }

    #[test]
    fn notes_shadowing_commands() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("btmux-cli-{}", uuid::Uuid::new_v4()));
        let (first, second) = (dir.join("first"), dir.join("second"));
        std::fs::create_dir_all(&first).unwrap();
        std::fs::create_dir_all(&second).unwrap();
        let target = dir.join("btmux");
        std::fs::write(&target, "").unwrap();
        std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o755)).unwrap();
        let target = std::fs::canonicalize(&target).unwrap();
        let path = format!("{}:{}", first.display(), second.display());
        assert!(shadowed(&path, &target).unwrap().contains("isn't on"));
        std::os::unix::fs::symlink(&target, second.join("btmux")).unwrap();
        assert_eq!(shadowed(&path, &target), None);
        std::fs::write(first.join("btmux"), "").unwrap();
        std::fs::set_permissions(first.join("btmux"), std::fs::Permissions::from_mode(0o755))
            .unwrap();
        assert!(shadowed(&path, &target).unwrap().contains("runs"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn quotes_for_sh() {
        assert_eq!(quote("/a b/it's"), r"'/a b/it'\''s'");
    }
}
