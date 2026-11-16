//! `/vscode/`: a shared VS Code server (`code serve-web`, or opt-in
//! `openvscode-server`), started on first request and proxied (HTTP and
//! WebSocket) over a private Unix socket. It runs without a connection token,
//! so the auth middleware in front of this proxy is its only access control.
//!
//! The bundled extension (`extras/vscode`) is installed into the profile's
//! extensions and connects back over a second private socket
//! (`BTMUX_VSCODE_SOCKET`), which carries newline-delimited JSON: user
//! settings and keybindings to use, the color palette and terminal font, and
//! files to open. With `[vscode] desktop` (or explicit paths), btmux shares
//! desktop VS Code's settings, keybindings, and extensions.
//!
//! Not yet supported on Windows.
#![cfg_attr(windows, allow(dead_code))]

use std::ffi::OsString;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{LazyLock, OnceLock};
use std::time::{Duration, Instant};

use axum::{
    body::Body,
    extract::{Request, State},
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use hyper_util::rt::TokioIo;
use rust_embed::Embed;
use serde::Serialize;
#[cfg(unix)]
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
#[cfg(windows)]
use tokio::net::windows::named_pipe::NamedPipeClient as UnixStream;
#[cfg(unix)]
use tokio::net::{UnixListener, UnixStream};
use tokio::process::{Child, ChildStdin, Command};
use tokio::sync::{broadcast, watch, Mutex};
use tokio::task::JoinHandle;

use crate::agent_limits::AskShell;
use crate::config::{
    ClientConfig, VscodeConfig, VscodeServer as Kind, DEFAULT_FONT_FAMILY, DEFAULT_FONT_SIZE,
    DEFAULT_FONT_WEIGHT,
};
use crate::ws::nvim::PrivateDir;
use crate::AppState;

/// Mount point; the server is told to serve everything under it.
pub const BASE_PATH: &str = "/vscode";
/// macOS app bundle location, for when `code` isn't on `PATH`.
const MACOS_CODE: &str = "/Applications/Visual Studio Code.app/Contents/Resources/app/bin/code";
/// Marks the user's acceptance of the VS Code Server license, per profile.
const LICENSE_FILE: &str = "license-accepted";

static SERVER: LazyLock<Mutex<Option<Server>>> = LazyLock::new(|| Mutex::new(None));
/// Shut down by the user: only `/api/vscode` starts it again, not proxied
/// requests from iframes that haven't closed yet. Changed under `SERVER`.
static STOPPED: AtomicBool = AtomicBool::new(false);
/// Shut down for an exit or upgrade: nothing starts it again, as reconnecting
/// windows would, until a failed upgrade resumes.
static HALTED: AtomicBool = AtomicBool::new(false);
/// The pane shell's `PATH`, when btmux's own lacks the server program, as a
/// service's lacks what shell startup adds.
static SHELL_PATH: std::sync::RwLock<Option<OsString>> = std::sync::RwLock::new(None);
/// The last shutdown by the user, so tabs that missed it close the surface.
static LAST_SHUTDOWN: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);
/// Which server to start; from `[vscode] server`.
static KIND: std::sync::Mutex<Kind> = std::sync::Mutex::new(Kind::Code);
/// Accepted without a profile to remember it in.
static LICENSE_ACCEPTED: AtomicBool = AtomicBool::new(false);
/// Per-profile user data and extensions.
static DATA_DIR: OnceLock<PathBuf> = OnceLock::new();

/// What VS Code shares with desktop VS Code.
static SHARED: LazyLock<watch::Sender<Shared>> =
    LazyLock::new(|| watch::channel(Shared::default()).0);
/// The `style` message for the current color scheme and terminal font.
static STYLE: LazyLock<watch::Sender<Option<String>>> = LazyLock::new(|| watch::channel(None).0);
/// `open` messages for connected extension hosts.
static OPENS: LazyLock<broadcast::Sender<String>> = LazyLock::new(|| broadcast::channel(16).0);
/// The latest `open`. It's replayed to hosts that connect before a window
/// showing its folder opened it, as VS Code may still be waiting on its license,
/// starting, or reloading into that folder, and soon after, for other windows.
static LAST_OPEN: std::sync::Mutex<Option<PendingOpen>> = std::sync::Mutex::new(None);
/// How long after a window opened it.
const OPEN_REPLAY: Duration = Duration::from_secs(30);
/// How long it waits for a first one: a license prompt or first start
/// (`code serve-web` downloads the server).
const OPEN_PENDING: Duration = Duration::from_secs(300);
/// Quiet time before shared extension folders are linked again.
const RELINK_DEBOUNCE: Duration = Duration::from_secs(1);

struct PendingOpen {
    id: u64,
    message: String,
    created: Instant,
    delivered: Option<Instant>,
}

impl PendingOpen {
    fn current(&self) -> bool {
        match self.delivered {
            Some(at) => at.elapsed() < OPEN_REPLAY,
            None => self.created.elapsed() < OPEN_PENDING,
        }
    }
}

#[derive(Embed)]
#[folder = "extras/vscode"]
struct Extension;

struct Server {
    child: Child,
    directory: PrivateDir,
    extension_listener: JoinHandle<()>,
    /// Links shared extension folders again when they change.
    extension_watcher: JoinHandle<()>,
    /// Its read end tells the watchdog (`WATCHDOG`) btmux is gone: this
    /// close-on-exec write end closes when btmux exits, crashes, or upgrades.
    _lifeline: Option<ChildStdin>,
    /// Recent output, reported if it exits before it's ready.
    output: std::sync::Arc<std::sync::Mutex<std::collections::VecDeque<String>>>,
    readers: Vec<JoinHandle<()>>,
}

impl Server {
    fn socket(&self) -> PathBuf {
        self.directory.path().join("code.sock")
    }
}

impl Drop for Server {
    /// The binary may be a wrapper script, so kill its whole process group.
    fn drop(&mut self) {
        self.extension_listener.abort();
        self.extension_watcher.abort();
        #[cfg(unix)]
        if let Some(pid) = self.child.id() {
            // SAFETY: killpg has no memory-safety preconditions.
            unsafe { libc::killpg(pid as libc::pid_t, libc::SIGTERM) };
        }
        #[cfg(windows)]
        let _ = self.child.start_kill();
    }
}

/// A settings, keybindings, or extensions path, and whether it was configured
/// explicitly rather than found as desktop VS Code's (missing is then an error).
#[derive(Clone, Debug, PartialEq)]
struct Source {
    path: PathBuf,
    explicit: bool,
    /// For extensions, the registry listing them instead of
    /// `<path>/extensions.json`: a desktop profile's.
    registry: Option<PathBuf>,
}

/// Desktop VS Code's (or configured) user files, sent to each VS Code window,
/// and extension folders, linked when the server starts and when they change.
#[derive(Clone, Debug, Default, PartialEq)]
struct Shared {
    settings: Option<Source>,
    keybindings: Option<Source>,
    extensions: Vec<Source>,
}

/// A desktop VS Code profile's folder and which of the default profile's
/// resources it uses instead of its own.
#[derive(Debug, PartialEq)]
struct Profile {
    dir: PathBuf,
    default_settings: bool,
    default_keybindings: bool,
    default_extensions: bool,
}

impl Profile {
    /// The profile named `name`, from desktop VS Code's `globalStorage/storage.json` under `user`.
    fn find(user: &Path, name: &str) -> Result<Self, String> {
        let path = user.join("globalStorage").join("storage.json");
        let storage: serde_json::Value = std::fs::read(&path)
            .map_err(|e| format!("{}: {e}", path.display()))
            .and_then(|data| {
                serde_json::from_slice(&data).map_err(|e| format!("{}: {e}", path.display()))
            })?;
        let profile = storage["userDataProfiles"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|profile| profile["name"].as_str() == Some(name))
            .ok_or_else(|| format!("desktop VS Code has no profile named {name:?}"))?;
        let dir = match &profile["location"] {
            serde_json::Value::String(id) => user.join("profiles").join(id),
            location => PathBuf::from(
                location["path"]
                    .as_str()
                    .ok_or_else(|| format!("profile {name:?} has no location"))?,
            ),
        };
        let default = |resource: &str| profile["useDefaultFlags"][resource].as_bool() == Some(true);
        Ok(Self {
            dir,
            default_settings: default("settings"),
            default_keybindings: default("keybindings"),
            default_extensions: default("extensions"),
        })
    }
}

impl Shared {
    fn new(config: &VscodeConfig) -> Self {
        let explicit = |value: &String| Source {
            path: crate::config::expand_home(value),
            explicit: true,
            registry: None,
        };
        let desktop = |path: Option<PathBuf>| {
            path.filter(|_| config.desktop).map(|path| Source {
                path,
                explicit: false,
                registry: None,
            })
        };
        // `~/Library/Application Support/Code/User` on macOS, else `$XDG_CONFIG_HOME/Code/User`.
        let user = dirs::config_dir().map(|dir| dir.join("Code").join("User"));
        let profile = match (&config.profile, &user) {
            (Some(name), Some(user)) if config.desktop => Profile::find(user, name)
                .inspect_err(|error| warn_once(&format!("VS Code profile: {error}")))
                .ok(),
            _ => None,
        };
        let user_file = |name: &str, default: fn(&Profile) -> bool| match &profile {
            Some(profile) if !default(profile) => Some(profile.dir.join(name)),
            _ => user.as_ref().map(|dir| dir.join(name)),
        };
        let extensions = dirs::home_dir().map(|home| home.join(".vscode/extensions"));
        Self {
            settings: config
                .settings
                .as_ref()
                .map(explicit)
                .or_else(|| desktop(user_file("settings.json", |p| p.default_settings))),
            keybindings: config
                .keybindings
                .as_ref()
                .map(explicit)
                .or_else(|| desktop(user_file("keybindings.json", |p| p.default_keybindings))),
            extensions: match &config.extensions {
                Some(dirs) => dirs.iter().map(explicit).collect(),
                None => desktop(extensions)
                    .map(|source| Source {
                        registry: profile
                            .as_ref()
                            .filter(|profile| !profile.default_extensions)
                            .map(|profile| profile.dir.join("extensions.json")),
                        ..source
                    })
                    .into_iter()
                    .collect(),
            },
        }
    }
}

/// Log `message` unless it was the last one, as config resolves often.
fn warn_once(message: &str) {
    static LAST: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);
    let mut last = LAST.lock().unwrap();
    if last.as_deref() != Some(message) {
        tracing::warn!("{message}");
        *last = Some(message.to_owned());
    }
}

pub fn set_data_dir(dir: PathBuf) {
    let _ = DATA_DIR.set(dir);
}

/// Pick the server to start next, what to share with desktop VS Code, and show
/// the btmux color scheme and terminal font in VS Code.
pub fn configure(config: &ClientConfig) {
    *KIND.lock().unwrap() = config.vscode.server;
    let shared = Shared::new(&config.vscode);
    SHARED.send_if_modified(|current| {
        let changed = *current != shared;
        if changed {
            *current = shared;
        }
        changed
    });
    let terminal = &config.terminal;
    let message = serde_json::json!({
        "type": "style",
        "palette": config.color_palette,
        "font": {
            "family": terminal.font_family.as_deref().unwrap_or(DEFAULT_FONT_FAMILY),
            "size": terminal.font_size.unwrap_or(DEFAULT_FONT_SIZE),
            "weight": terminal.font_weight.unwrap_or(DEFAULT_FONT_WEIGHT),
        },
    })
    .to_string();
    STYLE.send_if_modified(|current| {
        let changed = current.as_ref() != Some(&message);
        if changed {
            *current = Some(message);
        }
        changed
    });
}

/// Open `path` in the VS Code windows showing `folder` (all, without one). The
/// browser shows the surface, starting VS Code if needed.
pub fn open(path: &str, line: Option<u32>, folder: Option<&str>) {
    static NEXT_ID: AtomicU64 = AtomicU64::new(1);
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    let message = serde_json::json!({
        "type": "open",
        "id": id,
        "path": path,
        "line": line,
        "folder": folder,
    })
    .to_string();
    *LAST_OPEN.lock().unwrap() = Some(PendingOpen {
        id,
        message: message.clone(),
        created: Instant::now(),
        delivered: None,
    });
    let _ = OPENS.send(message);
}

/// Install the bundled extension into `extensions` and register it in VS
/// Code's `extensions.json`, replacing other builds. The folder name includes a
/// content hash, so any change re-registers it and VS Code rescans; nothing
/// current is rewritten, which would needlessly invalidate VS Code's cache.
fn install_extension(extensions: &Path) -> Result<(), String> {
    let error = |e: std::io::Error| format!("{}: {e}", extensions.display());
    let manifest: serde_json::Value =
        serde_json::from_slice(&Extension::get("package.json").expect("manifest").data)
            .map_err(|e| format!("extras/vscode/package.json: {e}"))?;
    let id = format!(
        "{}.{}",
        manifest["publisher"].as_str().unwrap(),
        manifest["name"].as_str().unwrap()
    );
    let version = manifest["version"].as_str().unwrap();
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    for name in Extension::iter() {
        name.hash(&mut hasher);
        Extension::get(&name).unwrap().data.hash(&mut hasher);
    }
    let folder = format!("{id}-{version}-{:08x}", hasher.finish() as u32);
    let dir = extensions.join(&folder);

    std::fs::create_dir_all(extensions).map_err(error)?;
    for entry in std::fs::read_dir(extensions).map_err(error)?.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with(&format!("{id}-")) && name != folder {
            let _ = std::fs::remove_dir_all(entry.path());
        }
    }
    for name in Extension::iter() {
        let data = Extension::get(&name).expect("embedded file").data;
        let path = dir.join(name.as_ref());
        if std::fs::read(&path).is_ok_and(|current| *current == *data) {
            continue;
        }
        std::fs::create_dir_all(path.parent().unwrap())
            .and_then(|()| std::fs::write(&path, &data))
            .map_err(error)?;
    }

    let registry = extensions.join("extensions.json");
    let mut entries = read_registry(&registry);
    let ours = |entry: &serde_json::Value| {
        entry["identifier"]["id"]
            .as_str()
            .is_some_and(|other| other.eq_ignore_ascii_case(&id))
    };
    let current = entries
        .iter()
        .any(|entry| ours(entry) && entry["relativeLocation"] == folder.as_str());
    if !current {
        entries.retain(|entry| !ours(entry));
        entries.push(serde_json::json!({
            "identifier": { "id": id },
            "version": version,
            "location": { "$mid": 1, "path": dir, "scheme": "file" },
            "relativeLocation": folder,
        }));
        std::fs::write(&registry, serde_json::to_vec(&entries).unwrap()).map_err(error)?;
    }
    Ok(())
}

/// The entries of an `extensions.json`, or none.
fn read_registry(path: &Path) -> Vec<serde_json::Value> {
    std::fs::read(path)
        .ok()
        .and_then(|data| serde_json::from_slice(&data).ok())
        .unwrap_or_default()
}

/// An extension's `publisher.name` and version, from its manifest.
fn manifest_id_from(manifest: &serde_json::Value) -> Option<(String, String)> {
    Some((
        format!(
            "{}.{}",
            manifest["publisher"].as_str()?,
            manifest["name"].as_str()?
        ),
        manifest["version"].as_str()?.to_owned(),
    ))
}

fn read_manifest(dir: &Path) -> Option<serde_json::Value> {
    serde_json::from_slice(&std::fs::read(dir.join("package.json")).ok()?).ok()
}

/// Microsoft's and GitHub's closed-license extensions (Pylance, C/C++, C# Dev
/// Kit, Copilot, …) run only in Microsoft's VS Code, so not in openvscode-server.
fn microsoft_only(manifest: &serde_json::Value) -> bool {
    let publisher = manifest["publisher"]
        .as_str()
        .unwrap_or_default()
        .to_ascii_lowercase();
    let closed = manifest["license"]
        .as_str()
        .is_some_and(|license| license.to_ascii_uppercase().starts_with("SEE LICENSE"));
    closed && (publisher.starts_with("ms-") || ["microsoft", "github"].contains(&&*publisher))
}

/// The extension folders in `source`: itself if it is one, else those its
/// `extensions.json` (or `registry`, a profile's) registers (as desktop VS Code
/// and Home Manager keep them), else its subfolders. A profile without a
/// registry has none, as in VS Code.
fn find_extensions(source: &Path, registry: Option<&Path>) -> std::io::Result<Vec<PathBuf>> {
    if registry.is_some_and(|registry| !registry.is_file()) {
        return Ok(Vec::new());
    }
    if registry.is_none() && source.join("package.json").is_file() {
        return Ok(vec![source.to_owned()]);
    }
    let registry = registry.map_or_else(|| source.join("extensions.json"), Path::to_path_buf);
    let found: Vec<PathBuf> = if registry.is_file() {
        // Folders VS Code has uninstalled or replaced but not yet deleted.
        let obsolete: serde_json::Map<String, serde_json::Value> =
            std::fs::read(source.join(".obsolete"))
                .ok()
                .and_then(|data| serde_json::from_slice(&data).ok())
                .unwrap_or_default();
        read_registry(&registry)
            .iter()
            .filter_map(|entry| match &entry["relativeLocation"] {
                serde_json::Value::String(relative) => Some(source.join(relative)),
                _ => match &entry["location"] {
                    serde_json::Value::String(location) => Some(source.join(location)),
                    location => location["path"].as_str().map(PathBuf::from),
                },
            })
            .filter(|dir| {
                dir.file_name()
                    .is_none_or(|name| !obsolete.contains_key(&*name.to_string_lossy()))
            })
            .collect()
    } else {
        let mut dirs: Vec<PathBuf> = std::fs::read_dir(source)?
            .flatten()
            .filter(|entry| !entry.file_name().to_string_lossy().starts_with('.'))
            .map(|entry| entry.path())
            .collect();
        dirs.sort();
        dirs
    };
    Ok(found
        .into_iter()
        .filter(|dir| dir.join("package.json").is_file())
        .collect())
}

/// Register the extensions in `sources` in `extensions`' `extensions.json` by
/// absolute location, replacing those `linked` lists from last time and other
/// installs of the same extensions. The bundled extension stays as it is.
///
/// Entries carry no gallery metadata, so VS Code treats them as installed from
/// files and never updates them itself: desktop VS Code does, and the changed
/// folders are linked again.
fn link_extensions(
    extensions: &Path,
    linked: &Path,
    sources: &[Source],
    kind: Kind,
) -> Result<(), String> {
    let error = |path: &Path, e: std::io::Error| format!("{}: {e}", path.display());
    let bundled = Extension::get("package.json")
        .and_then(|file| serde_json::from_slice(&file.data).ok())
        .and_then(|manifest| manifest_id_from(&manifest))
        .map(|(id, _)| id.to_ascii_lowercase());
    let mut wanted: Vec<serde_json::Value> = Vec::new();
    let mut ids: Vec<String> = Vec::new();
    for source in sources {
        let dirs = match find_extensions(&source.path, source.registry.as_deref()) {
            Ok(dirs) => dirs,
            Err(e) if !source.explicit && e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => {
                tracing::warn!("VS Code extensions: {}", error(&source.path, e));
                continue;
            }
        };
        for dir in dirs {
            let manifest = read_manifest(&dir);
            let Some((id, version)) = manifest.as_ref().and_then(manifest_id_from) else {
                tracing::warn!("VS Code extension without an id: {}", dir.display());
                continue;
            };
            if kind == Kind::OpenvscodeServer && manifest.as_ref().is_some_and(microsoft_only) {
                tracing::debug!("not linking {id}: it runs only in Microsoft's VS Code");
                continue;
            }
            let key = id.to_ascii_lowercase();
            if bundled.as_ref() == Some(&key) || ids.contains(&key) {
                continue;
            }
            ids.push(key);
            wanted.push(serde_json::json!({
                "identifier": { "id": id },
                "version": version,
                "location": { "$mid": 1, "path": dir, "scheme": "file" },
            }));
        }
    }

    let previous: Vec<String> = std::fs::read(linked)
        .ok()
        .and_then(|data| serde_json::from_slice(&data).ok())
        .unwrap_or_default();
    let registry = extensions.join("extensions.json");
    let original = read_registry(&registry);
    let mut entries: Vec<serde_json::Value> = original
        .iter()
        .filter(|entry| {
            entry["identifier"]["id"].as_str().is_none_or(|id| {
                let id = id.to_ascii_lowercase();
                !previous.contains(&id) && !ids.contains(&id)
            })
        })
        .cloned()
        .collect();
    entries.extend(wanted);
    if entries != original {
        std::fs::create_dir_all(extensions).map_err(|e| error(extensions, e))?;
        std::fs::write(&registry, serde_json::to_vec(&entries).unwrap())
            .map_err(|e| error(&registry, e))?;
    }
    if previous != ids {
        std::fs::write(linked, serde_json::to_vec(&ids).unwrap()).map_err(|e| error(linked, e))?;
    }
    Ok(())
}

/// Parse JSON with comments and trailing commas, as VS Code's settings allow.
fn parse_jsonc(text: &str) -> serde_json::Result<serde_json::Value> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    // Drop comments.
    let mut plain = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    let mut in_string = false;
    while let Some(c) = chars.next() {
        if in_string {
            plain.push(c);
            match c {
                '\\' => plain.extend(chars.next()),
                '"' => in_string = false,
                _ => {}
            }
            continue;
        }
        match (c, chars.peek()) {
            ('/', Some('/')) => while chars.next_if(|&c| c != '\n').is_some() {},
            ('/', Some('*')) => {
                chars.next();
                let mut last = ' ';
                for c in chars.by_ref() {
                    if last == '*' && c == '/' {
                        break;
                    }
                    last = c;
                }
                plain.push(' ');
            }
            _ => {
                in_string = c == '"';
                plain.push(c);
            }
        }
    }
    // Drop trailing commas.
    let mut json = String::with_capacity(plain.len());
    let mut in_string = false;
    let mut escaped = false;
    for (i, c) in plain.char_indices() {
        if in_string {
            in_string = escaped || c != '"';
            escaped = !escaped && c == '\\';
        } else if c == '"' {
            in_string = true;
        } else if c == ',' && plain[i + 1..].trim_start().starts_with(['}', ']']) {
            continue;
        }
        json.push(c);
    }
    serde_json::from_str(&json)
}

/// Read a shared settings or keybindings file. Desktop VS Code's may be missing.
async fn read_user_file(source: &Source) -> Option<serde_json::Value> {
    let path = source.path.display();
    let text = match tokio::fs::read_to_string(&source.path).await {
        Ok(text) => text,
        Err(e) if !source.explicit && e.kind() == std::io::ErrorKind::NotFound => return None,
        Err(e) => {
            tracing::warn!("VS Code: {path}: {e}");
            return None;
        }
    };
    if text.trim().is_empty() {
        return None;
    }
    parse_jsonc(&text)
        .inspect_err(|e| tracing::warn!("VS Code: {path}: {e}"))
        .ok()
}

/// The `user` message: settings and keybindings for VS Code's user files, each
/// `null` to leave that file alone.
async fn user_message(shared: &Shared) -> Option<String> {
    let mut settings = None;
    if let Some(source) = &shared.settings {
        settings = read_user_file(source).await;
        if settings.as_ref().is_some_and(|value| !value.is_object()) {
            tracing::warn!("VS Code: {}: not an object", source.path.display());
            settings = None;
        }
    }
    let mut keybindings = None;
    if let Some(source) = &shared.keybindings {
        keybindings = read_user_file(source).await;
        if keybindings.as_ref().is_some_and(|value| !value.is_array()) {
            tracing::warn!("VS Code: {}: not an array", source.path.display());
            keybindings = None;
        }
    }
    (settings.is_some() || keybindings.is_some()).then(|| {
        serde_json::json!({ "type": "user", "settings": settings, "keybindings": keybindings })
            .to_string()
    })
}

/// Accept connections from the bundled extension.
#[cfg(unix)]
fn listen_for_extension(path: &Path) -> Result<JoinHandle<()>, String> {
    let listener = UnixListener::bind(path).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(tokio::spawn(async move {
        while let Ok((stream, _)) = listener.accept().await {
            tokio::spawn(serve_extension(stream));
        }
    }))
}

/// Send the user files, the style, a pending or recent open, and then every
/// change, until it disconnects.
#[cfg(unix)]
async fn serve_extension(stream: UnixStream) {
    let (reader, mut writer) = stream.into_split();
    let mut shared = SHARED.subscribe();
    let current = shared.borrow_and_update().clone();
    let user = user_message(&current).await;
    let mut style = STYLE.subscribe();
    let mut opens = OPENS.subscribe();
    let recent = LAST_OPEN
        .lock()
        .unwrap()
        .as_ref()
        .filter(|open| open.current())
        .map(|open| open.message.clone());
    let mut pending: Vec<String> = user
        .into_iter()
        .chain(style.borrow_and_update().clone())
        .chain(recent)
        .collect();
    let mut lines = BufReader::new(reader).lines();
    loop {
        for message in pending.drain(..) {
            if writer
                .write_all(format!("{message}\n").as_bytes())
                .await
                .is_err()
            {
                return;
            }
        }
        tokio::select! {
            changed = shared.changed() => {
                if changed.is_err() {
                    return;
                }
                let current = shared.borrow_and_update().clone();
                pending.extend(user_message(&current).await);
            }
            changed = style.changed() => {
                if changed.is_err() {
                    return;
                }
                pending.extend(style.borrow_and_update().clone());
            }
            open = opens.recv() => match open {
                Ok(message) => pending.push(message),
                Err(broadcast::error::RecvError::Lagged(_)) => {}
                Err(broadcast::error::RecvError::Closed) => return,
            },
            line = lines.next_line() => match line {
                Ok(Some(line)) => opened(&line),
                _ => return,
            },
        }
    }
}

/// A window showing its folder opened the `open` in `line`, an `opened` reply.
#[cfg(unix)]
fn opened(line: &str) {
    let Ok(reply) = serde_json::from_str::<serde_json::Value>(line) else {
        return;
    };
    if reply["type"] != "opened" {
        return;
    }
    if let Some(open) = LAST_OPEN.lock().unwrap().as_mut() {
        if reply["id"] == open.id {
            open.delivered.get_or_insert_with(Instant::now);
        }
    }
}

/// `code serve-web` needs the user to accept the VS Code Server license.
fn license_accepted() -> bool {
    LICENSE_ACCEPTED.load(Ordering::Relaxed)
        || DATA_DIR
            .get()
            .is_some_and(|dir| dir.join(LICENSE_FILE).exists())
}

fn accept_license() -> Result<(), String> {
    LICENSE_ACCEPTED.store(true, Ordering::Relaxed);
    if let Some(dir) = DATA_DIR.get() {
        std::fs::create_dir_all(dir)
            .and_then(|()| std::fs::write(dir.join(LICENSE_FILE), ""))
            .map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    Ok(())
}

pub(crate) fn find_on_path(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join(name))
        .find(|candidate| candidate.is_file())
}

fn program_name(kind: Kind) -> &'static str {
    match kind {
        Kind::Code => "code",
        Kind::OpenvscodeServer => "openvscode-server",
    }
}

/// The program `kind` runs, if installed: on btmux's `PATH`, the pane shell's,
/// or for `code`, in the macOS app bundle.
pub(crate) fn program(kind: Kind) -> Option<PathBuf> {
    let name = program_name(kind);
    find_on_path(name)
        .or_else(|| {
            let path = SHELL_PATH.read().unwrap().clone()?;
            crate::agent_limits::find_in(name.as_ref(), &path)
        })
        .or_else(|| {
            Some(PathBuf::from(MACOS_CODE)).filter(|path| kind == Kind::Code && path.is_file())
        })
}

/// Whether the VS Code surface can start `kind`.
pub fn available(kind: Kind) -> bool {
    cfg!(unix) && program(kind).is_some()
}

/// The pane shell's `PATH` if btmux's lacks `kind`'s program, which the server
/// then runs with too, so its extensions find what panes do.
async fn shell_path(shell: &str, kind: Kind, ask: AskShell) -> Option<OsString> {
    let [path] =
        crate::agent_limits::programs_paths(shell, [OsString::from(program_name(kind))], ask).await;
    if path.is_some() {
        *SHELL_PATH.write().unwrap() = path.clone();
    }
    path
}

/// At startup, look for a server program only the pane shell's `PATH` has, and
/// list VS Code among the editors if that finds it.
pub fn find_program(state: AppState) {
    tokio::spawn(async move {
        let (shell, kind) = {
            let mgr = state.read().await;
            (mgr.shell().to_owned(), mgr.config().vscode.server)
        };
        if !cfg!(unix) || available(kind) {
            return;
        }
        shell_path(&shell, kind, AskShell::Once).await;
        if !available(kind) {
            return;
        }
        let mut mgr = state.write().await;
        let config = mgr.refresh_config().clone();
        let json = serde_json::to_string(&crate::ws::control::ServerMessage::Config {
            config: Box::new(config),
        })
        .unwrap();
        let _ = mgr.events().send(json);
    });
}

/// Runs the server (`"$@"`) and kills its process group once stdin, a pipe
/// from btmux, closes: the server must not outlive btmux, even killed or
/// upgrading, when no `Drop` runs.
const WATCHDOG: &str = r#"exec 3<&0
"$@" 3<&- </dev/null &
server=$!
{ read -r _ <&3; kill -TERM 0; } </dev/null &
watchdog=$!
exec 3<&-
wait "$server"
status=$?
kill "$watchdog" 2>/dev/null
exit "$status""#;

/// The server command, minus the options both servers share.
fn server_command(kind: Kind, data: &Path) -> Result<Command, String> {
    let binary = program(kind).ok_or(match kind {
        Kind::Code => {
            "VS Code's `code` command was not found. Install Visual Studio Code \
            (e.g. `brew install --cask visual-studio-code`) and its `code` command."
        }
        Kind::OpenvscodeServer => {
            "`openvscode-server` ([vscode] server = \"openvscode-server\") was not found on PATH."
        }
    })?;
    let mut command = Command::new("/bin/sh");
    command.args(["-c", WATCHDOG, "btmux-vscode"]).arg(binary);
    match kind {
        Kind::Code => {
            command
                .args([
                    "serve-web",
                    "--accept-server-license-terms",
                    "--disable-telemetry",
                ])
                .arg("--server-data-dir")
                .arg(data)
                .env("VSCODE_CLI_NONINTERACTIVE", "1");
        }
        Kind::OpenvscodeServer => {
            command
                .args(["--telemetry-level", "off"])
                .arg("--user-data-dir")
                .arg(data.join("data"))
                .arg("--extensions-dir")
                .arg(data.join("extensions"));
        }
    }
    Ok(command)
}

/// Link shared extension folders again whenever they or the configured ones
/// change, as desktop VS Code installs, updates, and removes them.
#[cfg(unix)]
fn watch_extensions(
    extensions: PathBuf,
    linked: PathBuf,
    kind: Kind,
    mut shared: watch::Receiver<Shared>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        shared.borrow_and_update();
        // `spawn` has just linked them.
        let mut relink = false;
        loop {
            let sources = shared.borrow().extensions.clone();
            // A registry is replaced, not written in place: watch its folder.
            let registries = sources
                .iter()
                .filter_map(|source| source.registry.as_deref()?.parent());
            let (mut present, mut ancestors, mut missing) = (Vec::new(), Vec::new(), Vec::new());
            for path in sources.iter().map(|s| s.path.as_path()).chain(registries) {
                if path.exists() {
                    present.push(path.to_owned());
                } else if let Some((ancestor, names)) = missing_watch(path) {
                    ancestors.push(ancestor);
                    missing.extend(names);
                }
            }
            let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
            // Watched before linking, so nothing changed meanwhile is missed.
            let _watchers = (
                watch_paths(&present, tx.clone(), |_| true),
                // Only what leads to a missing folder, as one is created.
                watch_paths(&ancestors, tx, move |path| {
                    missing.iter().any(|entry| path.starts_with(entry))
                }),
            );
            if relink {
                let sources = sources.clone();
                let (extensions, linked) = (extensions.clone(), linked.clone());
                let linking = tokio::task::spawn_blocking(move || {
                    link_extensions(&extensions, &linked, &sources, kind)
                });
                if let Ok(Err(error)) = linking.await {
                    tracing::warn!("VS Code extensions: {error}");
                }
            }
            tokio::select! {
                changed = shared.changed() => if changed.is_err() {
                    return;
                },
                Some(()) = rx.recv() => {
                    while let Ok(Some(())) = tokio::time::timeout(RELINK_DEBOUNCE, rx.recv()).await {}
                }
            }
            relink = true;
        }
    })
}

/// Where to watch for `path` to be created: its nearest existing ancestor,
/// and the entry there leading to `path`, as events name it: as given and
/// with symlinks resolved (as FSEvents reports it).
#[cfg(unix)]
fn missing_watch(path: &Path) -> Option<(PathBuf, [PathBuf; 2])> {
    let ancestor = path.ancestors().skip(1).find(|dir| dir.is_dir())?;
    let next = path.strip_prefix(ancestor).ok()?.components().next()?;
    let resolved = std::fs::canonicalize(ancestor).ok()?.join(next);
    Some((ancestor.to_owned(), [ancestor.join(next), resolved]))
}

/// Watch `paths`, signaling `tx` on changes to any path `wanted` accepts.
#[cfg(unix)]
fn watch_paths(
    paths: &[PathBuf],
    tx: tokio::sync::mpsc::UnboundedSender<()>,
    wanted: impl Fn(&Path) -> bool + Send + 'static,
) -> Option<notify::RecommendedWatcher> {
    use notify::event::{EventKind, ModifyKind};
    use notify::{RecursiveMode, Watcher};
    if paths.is_empty() {
        return None;
    }
    let mut watcher = notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
        // Not reads (or atime), which linking does itself.
        if event.is_ok_and(|event| {
            matches!(
                event.kind,
                EventKind::Create(_) | EventKind::Modify(_) | EventKind::Remove(_)
            ) && !matches!(event.kind, EventKind::Modify(ModifyKind::Metadata(_)))
                && (event.paths.is_empty() || event.paths.iter().any(|path| wanted(path)))
        }) {
            let _ = tx.send(());
        }
    })
    .inspect_err(|e| tracing::warn!("VS Code extensions: cannot watch: {e}"))
    .ok()?;
    for path in paths {
        if let Err(e) = watcher.watch(path, RecursiveMode::NonRecursive) {
            tracing::debug!("VS Code extensions: cannot watch {}: {e}", path.display());
        }
    }
    Some(watcher)
}

#[cfg(windows)]
fn spawn(_kind: Kind, _path: Option<OsString>) -> Result<Server, String> {
    Err("The VS Code surface isn't supported on Windows yet.".into())
}

#[cfg(unix)]
fn spawn(kind: Kind, path: Option<OsString>) -> Result<Server, String> {
    let directory = PrivateDir::new()?;
    let data = match DATA_DIR.get() {
        Some(dir) => dir.clone(),
        None => directory.path().join("data"),
    };
    // Both servers keep extensions under `<data>/extensions`.
    let extensions = data.join("extensions");
    if let Err(error) = install_extension(&extensions) {
        tracing::warn!("btmux VS Code extension: {error}");
    }
    let linked = data.join("linked-extensions.json");
    let sources = SHARED.borrow().extensions.clone();
    if let Err(error) = link_extensions(&extensions, &linked, &sources, kind) {
        tracing::warn!("VS Code extensions: {error}");
    }
    let extension_socket = directory.path().join("btmux.sock");
    let mut command = server_command(kind, &data)?;
    // It isn't a pane, nor in the terminal btmux started from.
    for var in crate::pty::OUTER_TERMINAL_ENV
        .iter()
        .chain(&crate::pty::PANE_ENV)
    {
        command.env_remove(var);
    }
    if let Some(path) = path {
        command.env("PATH", path);
    }
    let mut child = command
        .env("BTMUX_VSCODE_SOCKET", &extension_socket)
        .arg("--socket-path")
        .arg(directory.path().join("code.sock"))
        .args([
            "--server-base-path",
            BASE_PATH,
            "--without-connection-token",
        ])
        .current_dir(dirs::home_dir().unwrap_or_else(|| "/".into()))
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .process_group(0)
        .spawn()
        .map_err(|e| format!("start {kind:?}: {e}"))?;
    let lifeline = child.stdin.take();
    let extension_listener = match listen_for_extension(&extension_socket) {
        Ok(listener) => listener,
        Err(error) => {
            // SAFETY: killpg has no memory-safety preconditions.
            if let Some(pid) = child.id() {
                unsafe { libc::killpg(pid as libc::pid_t, libc::SIGTERM) };
            }
            return Err(error);
        }
    };
    let output = std::sync::Arc::new(std::sync::Mutex::new(std::collections::VecDeque::new()));
    let mut readers = Vec::new();
    for out in [
        child
            .stdout
            .take()
            .map(|s| Box::new(s) as Box<dyn tokio::io::AsyncRead + Send + Unpin>),
        child
            .stderr
            .take()
            .map(|s| Box::new(s) as Box<dyn tokio::io::AsyncRead + Send + Unpin>),
    ]
    .into_iter()
    .flatten()
    {
        let output = output.clone();
        readers.push(tokio::spawn(async move {
            let mut lines = BufReader::new(out).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                tracing::debug!(target: "btmux::vscode", "{line}");
                let mut output = output.lock().unwrap();
                if output.len() == 20 {
                    output.pop_front();
                }
                output.push_back(line);
            }
        }));
    }
    Ok(Server {
        child,
        directory,
        extension_listener,
        extension_watcher: watch_extensions(extensions, linked, kind, SHARED.subscribe()),
        _lifeline: lifeline,
        output,
        readers,
    })
}

#[derive(Debug)]
enum StartError {
    /// The user hasn't accepted the VS Code Server license yet.
    License,
    /// Shut down by the user and not started again.
    Stopped,
    /// btmux is exiting or upgrading.
    Halted,
    Failed(String),
}

impl std::fmt::Display for StartError {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            Self::License => f.write_str("the VS Code Server license hasn't been accepted"),
            Self::Stopped => f.write_str("VS Code was shut down"),
            Self::Halted => f.write_str("btmux is restarting"),
            Self::Failed(message) => f.write_str(message),
        }
    }
}

async fn connect_socket(path: &Path) -> std::io::Result<UnixStream> {
    #[cfg(unix)]
    return UnixStream::connect(path).await;
    #[cfg(windows)]
    tokio::net::windows::named_pipe::ClientOptions::new().open(path)
}

fn is_running(server: &mut Option<Server>) -> bool {
    server
        .as_mut()
        .is_some_and(|server| matches!(server.child.try_wait(), Ok(None)))
}

/// Connect to the server, starting it if it isn't running and either `start`
/// or the user didn't shut it down. `SERVER` is held to start it, not while it
/// starts listening, so status checks and shutdowns don't wait on that.
async fn connect(state: &AppState, start: bool) -> Result<UnixStream, StartError> {
    // Not under `SERVER`: an upgrade takes the manager, then `SERVER`.
    let shell = state.read().await.shell().to_owned();
    let socket = {
        let mut server = SERVER.lock().await;
        if HALTED.load(Ordering::Relaxed) {
            return Err(StartError::Halted);
        }
        if start {
            STOPPED.store(false, Ordering::Relaxed);
        }
        if !is_running(&mut server) {
            if STOPPED.load(Ordering::Relaxed) {
                return Err(StartError::Stopped);
            }
            let kind = *KIND.lock().unwrap();
            if kind == Kind::Code && !license_accepted() {
                return Err(StartError::License);
            }
            let path = shell_path(&shell, kind, AskShell::Retry).await;
            *server = Some(spawn(kind, path).map_err(StartError::Failed)?);
        }
        server.as_ref().unwrap().socket()
    };
    // Node takes a moment to start listening.
    for _ in 0..300 {
        if let Ok(stream) = connect_socket(&socket).await {
            return Ok(stream);
        }
        let mut server = SERVER.lock().await;
        let Some(running) = server.as_mut().filter(|server| server.socket() == socket) else {
            // Shut down (or replaced) meanwhile.
            return Err(if HALTED.load(Ordering::Relaxed) {
                StartError::Halted
            } else {
                StartError::Stopped
            });
        };
        if let Ok(Some(status)) = running.child.try_wait() {
            let readers = std::mem::take(&mut running.readers);
            let output = running.output.clone();
            *server = None;
            drop(server);
            for reader in readers {
                let _ = tokio::time::timeout(Duration::from_secs(1), reader).await;
            }
            let output = Vec::from(output.lock().unwrap().clone()).join("\n");
            return Err(StartError::Failed(format!(
                "VS Code exited ({status}):\n{output}"
            )));
        }
        drop(server);
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    Err(StartError::Failed(
        "VS Code didn't start listening within 30 s".into(),
    ))
}

/// For `/api/vscode`: whether VS Code is ready to show.
#[derive(Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
#[cfg_attr(test, derive(ts_rs::TS))]
pub enum VscodeStatus {
    Ready,
    /// Ask the user to accept the VS Code Server license (`POST /api/vscode/license`).
    License,
    Error {
        message: String,
    },
}

/// `GET /api/vscode`: start VS Code if needed and report whether it's ready.
pub async fn status(State(state): State<AppState>) -> Json<VscodeStatus> {
    Json(match connect(&state, true).await {
        Ok(_) => VscodeStatus::Ready,
        Err(StartError::License) => VscodeStatus::License,
        Err(StartError::Failed(message)) => VscodeStatus::Error { message },
        Err(error @ (StartError::Stopped | StartError::Halted)) => VscodeStatus::Error {
            message: error.to_string(),
        },
    })
}

/// `POST /api/vscode/license`: the user accepted the license; start VS Code.
pub async fn accept(state: State<AppState>) -> Json<VscodeStatus> {
    if let Err(message) = accept_license() {
        return Json(VscodeStatus::Error { message });
    }
    status(state).await
}

pub async fn running() -> bool {
    is_running(&mut *SERVER.lock().await)
}

/// Stop it for an exit or upgrade, keeping reconnecting windows from starting
/// it again until `resume`.
pub async fn shutdown() {
    HALTED.store(true, Ordering::Relaxed);
    drop(SERVER.lock().await.take());
}

/// After a failed upgrade: windows may start it again.
pub fn resume() {
    HALTED.store(false, Ordering::Relaxed);
}

/// Shut it down for the user; returns the shutdown's id, or `None` if it wasn't running.
pub async fn stop() -> Option<String> {
    let mut server = SERVER.lock().await;
    let running = is_running(&mut server);
    drop(server.take());
    // Nor reopen it when it starts again.
    *LAST_OPEN.lock().unwrap() = None;
    running.then(|| {
        STOPPED.store(true, Ordering::Relaxed);
        let id = uuid::Uuid::new_v4().simple().to_string();
        *LAST_SHUTDOWN.lock().unwrap() = Some(id.clone());
        id
    })
}

pub fn last_shutdown() -> Option<String> {
    LAST_SHUTDOWN.lock().unwrap().clone()
}

/// Forward a request, including WebSocket upgrades, to the server.
pub async fn proxy(State(state): State<AppState>, mut request: Request) -> Response {
    match forward(&state, &mut request).await {
        Ok(response) => response,
        Err(error) => {
            tracing::warn!("vscode proxy: {error}");
            (StatusCode::BAD_GATEWAY, error).into_response()
        }
    }
}

async fn forward(state: &AppState, request: &mut Request) -> Result<Response, String> {
    let stream = match connect(state, false).await {
        Ok(stream) => stream,
        // A stale iframe; it closes once its tab hears of the shutdown.
        Err(error @ (StartError::Stopped | StartError::Halted)) => {
            return Ok((StatusCode::SERVICE_UNAVAILABLE, error.to_string()).into_response())
        }
        Err(error) => return Err(error.to_string()),
    };
    let (mut sender, connection) = hyper::client::conn::http1::handshake(TokioIo::new(stream))
        .await
        .map_err(|e| e.to_string())?;
    tokio::spawn(async move {
        let _ = connection.with_upgrades().await;
    });

    let upgrade = request
        .headers()
        .get(header::UPGRADE)
        .is_some()
        .then(|| hyper::upgrade::on(&mut *request));
    let mut outgoing = Request::new(std::mem::take(request.body_mut()));
    *outgoing.method_mut() = request.method().clone();
    *outgoing.uri_mut() = request
        .uri()
        .path_and_query()
        .map(|pq| pq.as_str())
        .unwrap_or(BASE_PATH)
        .parse()
        .map_err(|e: axum::http::uri::InvalidUri| e.to_string())?;
    *outgoing.headers_mut() = request.headers().clone();
    outgoing.headers_mut().remove(header::AUTHORIZATION);

    let mut response = sender
        .send_request(outgoing)
        .await
        .map_err(|e| e.to_string())?;
    if response.status() == StatusCode::SWITCHING_PROTOCOLS {
        let server = hyper::upgrade::on(&mut response);
        if let Some(client) = upgrade {
            tokio::spawn(async move {
                if let (Ok(client), Ok(server)) = tokio::join!(client, server) {
                    let _ = tokio::io::copy_bidirectional(
                        &mut TokioIo::new(client),
                        &mut TokioIo::new(server),
                    )
                    .await;
                }
            });
        }
    }
    Ok(response.map(Body::new))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn installs_and_registers_the_extension_once() {
        let dir =
            std::env::temp_dir().join(format!("btmux-vscode-{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(dir.join("btmux.btmux-0.0.0")).unwrap();
        let registry = dir.join("extensions.json");
        std::fs::write(
            &registry,
            r#"[{"identifier":{"id":"other.ext"},"version":"1.0.0","location":"a","relativeLocation":"other.ext-1.0.0"},
                {"identifier":{"id":"btmux.btmux"},"version":"0.0.0","location":"b","relativeLocation":"btmux.btmux-0.0.0"}]"#,
        )
        .unwrap();

        install_extension(&dir).unwrap();
        let entries: Vec<serde_json::Value> =
            serde_json::from_slice(&std::fs::read(&registry).unwrap()).unwrap();
        let folder = entries[1]["relativeLocation"].as_str().unwrap().to_owned();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0]["identifier"]["id"], "other.ext");
        assert!(folder.starts_with("btmux.btmux-") && folder != "btmux.btmux-0.0.0");
        assert!(dir.join(&folder).join("extension.js").is_file());
        assert!(!dir.join("btmux.btmux-0.0.0").exists());

        // A current install rewrites nothing, so VS Code's cache stays valid.
        let modified = std::fs::metadata(&registry).unwrap().modified().unwrap();
        install_extension(&dir).unwrap();
        assert_eq!(
            std::fs::metadata(&registry).unwrap().modified().unwrap(),
            modified
        );

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn parses_settings_with_comments_and_trailing_commas() {
        let value = parse_jsonc(
            "\u{feff}{\n  // line\n  \"a\": \"//not a comment\", /* block */\n  \"b\": [1, 2,],\n  \"c\": \"q\\\",\",\n}\n",
        )
        .unwrap();
        assert_eq!(
            value,
            serde_json::json!({ "a": "//not a comment", "b": [1, 2], "c": "q\"," })
        );
        assert!(parse_jsonc("{\"a\": }").is_err());
    }

    fn write_extension(dir: &Path, publisher: &str, name: &str, version: &str) {
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(
            dir.join("package.json"),
            serde_json::json!({ "publisher": publisher, "name": name, "version": version })
                .to_string(),
        )
        .unwrap();
    }

    #[test]
    fn links_shared_extensions_and_replaces_them_later() {
        let root =
            std::env::temp_dir().join(format!("btmux-vscode-{}", uuid::Uuid::new_v4().simple()));
        let extensions = root.join("data/extensions");
        let linked = root.join("data/linked-extensions.json");
        // A desktop folder registered by `extensions.json`, with an obsolete entry.
        let desktop = root.join("desktop");
        write_extension(
            &desktop.join("vscodevim.vim-1.0.0"),
            "vscodevim",
            "vim",
            "1.0.0",
        );
        write_extension(&desktop.join("old.ext-1.0.0"), "old", "ext", "1.0.0");
        write_extension(
            &root.join("store/rust"),
            "rust-lang",
            "rust-analyzer",
            "0.4.0",
        );
        std::fs::write(
            desktop.join("extensions.json"),
            serde_json::json!([
                { "identifier": { "id": "vscodevim.vim" }, "version": "1.0.0",
                  "location": { "$mid": 1, "path": "/elsewhere", "scheme": "file" },
                  "relativeLocation": "vscodevim.vim-1.0.0" },
                { "identifier": { "id": "old.ext" }, "version": "1.0.0",
                  "relativeLocation": "old.ext-1.0.0" },
                { "identifier": { "id": "rust-lang.rust-analyzer" }, "version": "0.4.0",
                  "location": { "$mid": 1, "path": root.join("store/rust"), "scheme": "file" } },
            ])
            .to_string(),
        )
        .unwrap();
        std::fs::write(desktop.join(".obsolete"), r#"{"old.ext-1.0.0":true}"#).unwrap();
        // A folder of extensions without a registry, and a single extension.
        let folder = root.join("folder");
        write_extension(&folder.join("a"), "Pub", "a", "2.0.0");
        write_extension(&folder.join("vim"), "vscodevim", "vim", "0.9.0");
        write_extension(&root.join("single"), "pub", "single", "3.0.0");
        // Installed in btmux before: one replaced by a shared one, one kept.
        std::fs::create_dir_all(&extensions).unwrap();
        std::fs::write(
            extensions.join("extensions.json"),
            r#"[{"identifier":{"id":"btmux.btmux"},"version":"0.0.1","relativeLocation":"btmux.btmux-0.0.1"},
                {"identifier":{"id":"pub.A"},"version":"1.0.0","relativeLocation":"pub.a-1.0.0"},
                {"identifier":{"id":"user.ext"},"version":"1.0.0","relativeLocation":"user.ext-1.0.0"}]"#,
        )
        .unwrap();
        let source = |path: PathBuf| Source {
            path,
            explicit: true,
            registry: None,
        };
        let registered = || -> Vec<(String, Option<String>)> {
            read_registry(&extensions.join("extensions.json"))
                .iter()
                .map(|entry| {
                    (
                        entry["identifier"]["id"].as_str().unwrap().to_owned(),
                        entry["location"]["path"].as_str().map(str::to_owned),
                    )
                })
                .collect()
        };
        let path = |p: PathBuf| Some(p.to_string_lossy().into_owned());

        link_extensions(
            &extensions,
            &linked,
            &[
                source(desktop.clone()),
                source(folder.clone()),
                source(root.join("single")),
                source(root.join("missing")),
            ],
            Kind::Code,
        )
        .unwrap();
        assert_eq!(
            registered(),
            [
                ("btmux.btmux".into(), None),
                ("user.ext".into(), None),
                (
                    "vscodevim.vim".into(),
                    path(desktop.join("vscodevim.vim-1.0.0"))
                ),
                (
                    "rust-lang.rust-analyzer".into(),
                    path(root.join("store/rust"))
                ),
                ("Pub.a".into(), path(folder.join("a"))),
                ("pub.single".into(), path(root.join("single"))),
            ]
        );

        // Unchanged sources rewrite nothing; removed ones are unlinked.
        let modified = std::fs::metadata(extensions.join("extensions.json"))
            .unwrap()
            .modified()
            .unwrap();
        let sources = [
            source(desktop.clone()),
            source(folder.clone()),
            source(root.join("single")),
        ];
        link_extensions(&extensions, &linked, &sources, Kind::Code).unwrap();
        assert_eq!(
            std::fs::metadata(extensions.join("extensions.json"))
                .unwrap()
                .modified()
                .unwrap(),
            modified
        );
        link_extensions(
            &extensions,
            &linked,
            &[source(root.join("single"))],
            Kind::Code,
        )
        .unwrap();
        assert_eq!(
            registered(),
            [
                ("btmux.btmux".into(), None),
                ("user.ext".into(), None),
                ("pub.single".into(), path(root.join("single"))),
            ]
        );

        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn shares_desktop_files_unless_paths_are_given() {
        assert_eq!(Shared::new(&VscodeConfig::default()), Shared::default());
        let config: VscodeConfig =
            toml::from_str("desktop = true\nkeybindings = \"/k.json\"\nextensions = []").unwrap();
        let shared = Shared::new(&config);
        let settings = shared.settings.unwrap();
        assert!(settings.path.ends_with("Code/User/settings.json") && !settings.explicit);
        assert_eq!(
            shared.keybindings,
            Some(Source {
                path: "/k.json".into(),
                explicit: true,
                registry: None,
            })
        );
        assert!(shared.extensions.is_empty());
    }

    #[test]
    fn finds_desktop_profiles_and_their_defaults() {
        let user =
            std::env::temp_dir().join(format!("btmux-vscode-{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(user.join("globalStorage")).unwrap();
        std::fs::write(
            user.join("globalStorage/storage.json"),
            serde_json::json!({ "userDataProfiles": [
                { "location": "1a2b", "name": "Work",
                  "useDefaultFlags": { "keybindings": true } },
            ] })
            .to_string(),
        )
        .unwrap();
        assert_eq!(
            Profile::find(&user, "Work").unwrap(),
            Profile {
                dir: user.join("profiles/1a2b"),
                default_settings: false,
                default_keybindings: true,
                default_extensions: false,
            }
        );
        assert!(Profile::find(&user, "Home").is_err());
        std::fs::remove_dir_all(&user).unwrap();
    }

    #[test]
    fn reads_a_profile_registry_relative_to_the_extensions_folder() {
        let root =
            std::env::temp_dir().join(format!("btmux-vscode-{}", uuid::Uuid::new_v4().simple()));
        let shared = root.join("extensions");
        write_extension(&shared.join("a.b-1.0.0"), "a", "b", "1.0.0");
        write_extension(&shared.join("c.d-1.0.0"), "c", "d", "1.0.0");
        let registry = root.join("profile/extensions.json");
        std::fs::create_dir_all(registry.parent().unwrap()).unwrap();
        std::fs::write(
            &registry,
            r#"[{"identifier":{"id":"a.b"},"version":"1.0.0","relativeLocation":"a.b-1.0.0"}]"#,
        )
        .unwrap();
        assert_eq!(
            find_extensions(&shared, Some(&registry)).unwrap(),
            [shared.join("a.b-1.0.0")]
        );
        // A profile without a registry has no extensions, not every shared one.
        let empty = root.join("empty/extensions.json");
        std::fs::create_dir_all(empty.parent().unwrap()).unwrap();
        assert!(find_extensions(&shared, Some(&empty)).unwrap().is_empty());
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn recognizes_extensions_only_microsoft_builds_run() {
        let manifest = |publisher: &str, license: &str| serde_json::json!({ "publisher": publisher, "license": license });
        assert!(microsoft_only(&manifest(
            "ms-python",
            "SEE LICENSE IN LICENSE.txt"
        )));
        assert!(microsoft_only(&manifest(
            "GitHub",
            "SEE LICENSE IN LICENSE.txt"
        )));
        assert!(!microsoft_only(&manifest("ms-python", "MIT")));
        assert!(!microsoft_only(&manifest(
            "vscodevim",
            "SEE LICENSE IN LICENSE"
        )));
    }

    #[test]
    fn opens_wait_for_a_first_window_then_replay_briefly() {
        let now = Instant::now();
        // Instants can't precede boot.
        let Some(past) = now.checked_sub(OPEN_PENDING) else {
            return;
        };
        let open = |created: Instant, delivered: Option<Instant>| PendingOpen {
            id: 0,
            message: String::new(),
            created,
            delivered,
        };
        assert!(open(now, None).current());
        assert!(!open(past, None).current());
        assert!(open(past, Some(now)).current());
        assert!(!open(now, Some(now - OPEN_REPLAY)).current());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn links_the_first_extension_installed_into_a_missing_folder() {
        let root =
            std::env::temp_dir().join(format!("btmux-vscode-{}", uuid::Uuid::new_v4().simple()));
        let extensions = root.join("data/extensions");
        std::fs::create_dir_all(&extensions).unwrap();
        let source = root.join("home/.vscode/extensions");
        std::fs::create_dir_all(root.join("home")).unwrap();
        let shared = Shared {
            extensions: vec![Source {
                path: source.clone(),
                explicit: false,
                registry: None,
            }],
            ..Shared::default()
        };
        let (_tx, rx) = watch::channel(shared);
        let watcher = watch_extensions(
            extensions.clone(),
            root.join("data/linked-extensions.json"),
            Kind::Code,
            rx,
        );
        // Let it watch first.
        tokio::time::sleep(Duration::from_millis(200)).await;
        write_extension(&source.join("pub.first-1.0.0"), "pub", "first", "1.0.0");
        let found = tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                let ids: Vec<_> = read_registry(&extensions.join("extensions.json"))
                    .iter()
                    .map(|entry| entry["identifier"]["id"].clone())
                    .collect();
                if ids.contains(&"pub.first".into()) {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        })
        .await;
        watcher.abort();
        std::fs::remove_dir_all(&root).unwrap();
        assert!(found.is_ok());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn opens_wait_for_a_window_showing_their_folder() {
        // An extension host that has received the open.
        async fn host() -> tokio::net::unix::OwnedWriteHalf {
            let (server, client) = UnixStream::pair().unwrap();
            tokio::spawn(serve_extension(server));
            let (read, write) = client.into_split();
            let mut lines = BufReader::new(read).lines();
            loop {
                let line = tokio::time::timeout(Duration::from_secs(5), lines.next_line())
                    .await
                    .unwrap()
                    .unwrap()
                    .unwrap();
                if line.contains(r#""type":"open""#) {
                    tokio::spawn(async move { while let Ok(Some(_)) = lines.next_line().await {} });
                    return write;
                }
            }
        }
        let delivered = || LAST_OPEN.lock().unwrap().as_ref().unwrap().delivered;

        open("/repo/src/main.rs", Some(3), Some("/repo"));
        let id = LAST_OPEN.lock().unwrap().as_ref().unwrap().id;
        // A window showing another folder ignores it.
        let _other = host().await;
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(delivered().is_none());
        // So the one showing it, starting later, still gets it.
        let mut matching = host().await;
        matching
            .write_all(format!("{{\"type\":\"opened\",\"id\":{id}}}\n").as_bytes())
            .await
            .unwrap();
        tokio::time::timeout(Duration::from_secs(5), async {
            while delivered().is_none() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
    }
}
