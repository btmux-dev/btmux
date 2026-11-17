//! `workspace/didChangeWatchedFiles`: the globs a server registers, matched
//! against changes under its root.

use std::path::{Path, PathBuf};
use std::time::Duration;

use notify::{EventKind, RecursiveMode, Watcher as _};
use regex::Regex;
use serde_json::{json, Value};
use tokio::sync::mpsc;

/// LSP `FileChangeType`.
pub const CREATED: u8 = 1;
pub const CHANGED: u8 = 2;
pub const DELETED: u8 = 3;

/// A registered watcher: its glob, relative to `base` if given.
pub struct FileWatcher {
    base: Option<PathBuf>,
    glob: Regex,
    /// `WatchKind` bits: 1 create, 2 change, 4 delete.
    kind: u8,
}

/// A regex for an LSP glob: `*`, `**`, `?`, `{a,b}`, and `[…]`.
pub fn glob_regex(glob: &str) -> Option<Regex> {
    glob_regex_with(glob, false)
}

pub fn glob_regex_with(glob: &str, ignore_case: bool) -> Option<Regex> {
    let mut re = String::from(if ignore_case { "(?i)^" } else { "^" });
    let mut chars = glob.chars().peekable();
    let mut braces = 0;
    while let Some(c) = chars.next() {
        match c {
            '*' if chars.peek() == Some(&'*') => {
                chars.next();
                // `**/` matches zero or more directories.
                if chars.peek() == Some(&'/') {
                    chars.next();
                    re.push_str("(?:.*/)?");
                } else {
                    re.push_str(".*");
                }
            }
            '*' => re.push_str("[^/]*"),
            '?' => re.push_str("[^/]"),
            '{' => {
                braces += 1;
                re.push_str("(?:");
            }
            '}' if braces > 0 => {
                braces -= 1;
                re.push(')');
            }
            ',' if braces > 0 => re.push('|'),
            '[' => {
                re.push('[');
                if chars.peek() == Some(&'!') {
                    chars.next();
                    re.push('^');
                }
                for c in chars.by_ref() {
                    if c == ']' {
                        break;
                    }
                    if c == '\\' {
                        re.push('\\');
                    }
                    re.push(c);
                }
                re.push(']');
            }
            c => re.push_str(&regex::escape(&c.to_string())),
        }
    }
    re.push('$');
    Regex::new(&re).ok()
}

impl FileWatcher {
    /// From a `FileSystemWatcher`.
    pub fn parse(value: &Value) -> Option<Self> {
        let pattern = &value["globPattern"];
        let (base, glob) = match pattern.as_str() {
            Some(glob) => (None, glob),
            None => {
                let base = &pattern["baseUri"];
                let uri = base.as_str().or_else(|| base["uri"].as_str())?;
                (
                    Some(super::config::path(uri)?),
                    pattern["pattern"].as_str()?,
                )
            }
        };
        Some(Self {
            base,
            glob: glob_regex(glob)?,
            kind: value["kind"].as_u64().map_or(7, |k| k as u8),
        })
    }

    pub fn matches(&self, path: &Path, change: u8) -> bool {
        let bit = match change {
            CREATED => 1,
            CHANGED => 2,
            _ => 4,
        };
        if self.kind & bit == 0 {
            return false;
        }
        let candidate = match &self.base {
            Some(base) => match path.strip_prefix(base) {
                Ok(relative) => relative,
                Err(_) => return false,
            },
            None => path,
        };
        let candidate = crate::fs_ops::client_path(candidate);
        // Unanchored globs (`**/*.rs`) match anywhere, so try the whole
        // path and the path without its leading `/`.
        self.glob.is_match(&candidate) || self.glob.is_match(candidate.trim_start_matches('/'))
    }
}

/// Directories watched under one root on Linux, where each takes one of the
/// user's inotify watches.
#[cfg(target_os = "linux")]
const MAX_DIRS: usize = 4096;

/// Watches a server's root while it has watchers; changes go to `on_change`.
pub struct RootWatch {
    task: tokio::task::AbortHandle,
}

impl Drop for RootWatch {
    fn drop(&mut self) {
        self.task.abort();
    }
}

struct Watching {
    root: PathBuf,
    watcher: notify::RecommendedWatcher,
    /// The root's `.gitignore`: build output changes constantly.
    ignored: ignore::gitignore::Gitignore,
    #[cfg(target_os = "linux")]
    dirs: usize,
}

impl Watching {
    /// Blocks: on Linux it walks the tree.
    fn start(root: PathBuf, tx: mpsc::UnboundedSender<(PathBuf, u8)>) -> Option<Self> {
        let watcher = notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
            let Ok(event) = event else { return };
            let change = match event.kind {
                EventKind::Create(_) => CREATED,
                EventKind::Remove(_) => DELETED,
                EventKind::Modify(notify::event::ModifyKind::Metadata(_)) => return,
                // A rename reports both names; which exists now decides.
                EventKind::Modify(notify::event::ModifyKind::Name(_)) => 0,
                EventKind::Modify(_) => CHANGED,
                _ => return,
            };
            for path in event.paths {
                // Git's internals churn on every command.
                if path.components().any(|c| c.as_os_str() == ".git") {
                    continue;
                }
                let change = match change {
                    0 if path.exists() => CREATED,
                    0 => DELETED,
                    c => c,
                };
                let _ = tx.send((path, change));
            }
        })
        .inspect_err(|e| tracing::warn!("cannot watch {}: {e}", root.display()))
        .ok()?;
        let (ignored, _) = ignore::gitignore::Gitignore::new(root.join(".gitignore"));
        let mut watching = Self {
            root,
            watcher,
            ignored,
            #[cfg(target_os = "linux")]
            dirs: 0,
        };
        #[cfg(target_os = "linux")]
        watching.watch_tree(&watching.root.clone());
        #[cfg(not(target_os = "linux"))]
        if let Err(e) = watching
            .watcher
            .watch(&watching.root, RecursiveMode::Recursive)
        {
            tracing::warn!("cannot watch {}: {e}", watching.root.display());
            return None;
        }
        Some(watching)
    }

    fn ignores(&self, path: &Path) -> bool {
        path.starts_with(&self.root)
            && self
                .ignored
                .matched_path_or_any_parents(path, false)
                .is_ignore()
    }

    /// Watch `dir` and the directories under it that Git doesn't ignore:
    /// inotify isn't recursive. Returns the files found.
    #[cfg(target_os = "linux")]
    fn watch_tree(&mut self, dir: &Path) -> Vec<PathBuf> {
        let walk = ignore::WalkBuilder::new(dir)
            .hidden(false)
            .filter_entry(|e| e.file_name() != ".git")
            .build();
        let mut files = Vec::new();
        for entry in walk.flatten() {
            if !entry.file_type().is_some_and(|t| t.is_dir()) {
                files.push(entry.into_path());
                continue;
            }
            if self.dirs == MAX_DIRS {
                tracing::warn!(
                    "watching only {MAX_DIRS} directories under {}",
                    self.root.display()
                );
                self.dirs += 1;
            }
            if self.dirs > MAX_DIRS {
                break;
            }
            match self
                .watcher
                .watch(entry.path(), RecursiveMode::NonRecursive)
            {
                Ok(()) => self.dirs += 1,
                Err(e) => tracing::warn!("cannot watch {}: {e}", entry.path().display()),
            }
        }
        files
    }
}

/// Home, or above it: no project, and far too much to watch.
fn too_broad(root: &Path) -> bool {
    root.parent().is_none() || dirs::home_dir().is_some_and(|home| home.starts_with(root))
}

impl RootWatch {
    /// Starts watching in the background.
    pub fn start(
        root: &Path,
        on_change: impl Fn(Vec<(PathBuf, u8)>) + Send + 'static,
    ) -> Option<Self> {
        if too_broad(root) {
            tracing::info!("not watching {}", root.display());
            return None;
        }
        let root = root.to_path_buf();
        let task = tokio::spawn(async move {
            let (tx, mut rx) = mpsc::unbounded_channel::<(PathBuf, u8)>();
            let started = tokio::task::spawn_blocking(move || Watching::start(root, tx)).await;
            #[cfg_attr(not(target_os = "linux"), allow(unused_mut))]
            let Ok(Some(mut watching)) = started
            else {
                return;
            };
            while let Some(first) = rx.recv().await {
                tokio::time::sleep(Duration::from_millis(100)).await;
                let mut changes = vec![first];
                while let Ok(change) = rx.try_recv() {
                    match changes.iter_mut().find(|(p, _)| *p == change.0) {
                        Some(existing) => existing.1 = change.1,
                        None => changes.push(change),
                    }
                }
                changes.retain(|(path, _)| !watching.ignores(path));
                // A new directory's contents may predate its watch.
                #[cfg(target_os = "linux")]
                {
                    let mut found = Vec::new();
                    for (path, change) in &changes {
                        if *change == CREATED && path.is_dir() {
                            found.extend(watching.watch_tree(path));
                        }
                    }
                    for file in found {
                        if !changes.iter().any(|(p, _)| *p == file) {
                            changes.push((file, CREATED));
                        }
                    }
                }
                if !changes.is_empty() {
                    on_change(changes);
                }
            }
        });
        Some(Self {
            task: task.abort_handle(),
        })
    }
}

/// `didChangeWatchedFiles` params for the changes `watchers` want, if any.
pub fn changes(watchers: &[&FileWatcher], changes: &[(PathBuf, u8)]) -> Option<Value> {
    let list: Vec<_> = changes
        .iter()
        .filter(|(path, change)| watchers.iter().any(|w| w.matches(path, *change)))
        .map(|(path, change)| json!({ "uri": super::config::uri(path), "type": change }))
        .collect();
    (!list.is_empty()).then(|| json!({ "changes": list }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn globs() {
        let re = glob_regex("**/*.{rs,toml}").unwrap();
        assert!(re.is_match("src/main.rs"));
        assert!(re.is_match("Cargo.toml"));
        assert!(!re.is_match("src/main.rsx"));
        assert!(glob_regex("src/?.[!x]s").unwrap().is_match("src/a.ts"));
        assert!(!glob_regex("src/*.ts").unwrap().is_match("src/a/b.ts"));
    }

    #[test]
    fn watchers_match_paths() {
        let any = FileWatcher::parse(&json!({ "globPattern": "**/Cargo.toml" })).unwrap();
        assert!(any.matches(Path::new("/w/crate/Cargo.toml"), CHANGED));
        let root = Path::new(if cfg!(windows) { r"C:\w" } else { "/w" });
        let relative = FileWatcher::parse(&json!({
            "globPattern": { "baseUri": super::super::config::uri(root), "pattern": "*.json" },
            "kind": 1,
        }))
        .unwrap();
        assert!(relative.matches(&root.join("a.json"), CREATED));
        assert!(!relative.matches(&root.join("a.json"), CHANGED));
        assert!(!relative.matches(&root.join("x").join("a.json"), CREATED));
        assert!(!relative.matches(&root.with_file_name("v").join("a.json"), CREATED));
    }
}
