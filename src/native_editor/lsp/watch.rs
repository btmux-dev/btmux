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
    let mut re = String::from("^");
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

/// Watches a server's root while it has watchers; changes go to `on_change`.
pub struct RootWatch {
    _watcher: notify::RecommendedWatcher,
}

impl RootWatch {
    pub fn start(
        root: &Path,
        on_change: impl Fn(Vec<(PathBuf, u8)>) + Send + 'static,
    ) -> Option<Self> {
        let (tx, mut rx) = mpsc::unbounded_channel::<(PathBuf, u8)>();
        let mut watcher =
            notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
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
        if let Err(e) = watcher.watch(root, RecursiveMode::Recursive) {
            tracing::warn!("cannot watch {}: {e}", root.display());
            return None;
        }
        tokio::spawn(async move {
            while let Some(first) = rx.recv().await {
                tokio::time::sleep(Duration::from_millis(100)).await;
                let mut changes = vec![first];
                while let Ok(change) = rx.try_recv() {
                    match changes.iter_mut().find(|(p, _)| *p == change.0) {
                        Some(existing) => existing.1 = change.1,
                        None => changes.push(change),
                    }
                }
                on_change(changes);
            }
        });
        Some(Self { _watcher: watcher })
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
        let relative = FileWatcher::parse(&json!({
            "globPattern": { "baseUri": "file:///w", "pattern": "*.json" },
            "kind": 1,
        }))
        .unwrap();
        assert!(relative.matches(Path::new("/w/a.json"), CREATED));
        assert!(!relative.matches(Path::new("/w/a.json"), CHANGED));
        assert!(!relative.matches(Path::new("/w/x/a.json"), CREATED));
        assert!(!relative.matches(Path::new("/v/a.json"), CREATED));
    }
}
