//! One open file: the authoritative text, its update log, and what is on disk.

use std::collections::VecDeque;
use std::hash::Hasher;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use ropey::Rope;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use super::text::{self, Change};

/// Files larger than this aren't opened.
pub const MAX_FILE_BYTES: u64 = 16 * 1024 * 1024;
/// Updates kept for tabs catching up after a reconnect.
const LOG_LIMIT: usize = 2000;
/// The client id of changes btmux makes itself (reloads).
pub const SERVER_CLIENT: &str = "btmux";

/// A collab update: one `ChangeSet` from one editor.
#[derive(Serialize, Deserialize, Clone, Debug)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct Update {
    pub client_id: String,
    /// `ChangeSet.toJSON()`.
    #[cfg_attr(test, ts(type = "unknown"))]
    pub changes: Value,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(test, derive(ts_rs::TS))]
pub enum LineEnding {
    Lf,
    Crlf,
}

/// The file on disk compared to what was last loaded or saved.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(test, derive(ts_rs::TS))]
pub enum DiskStatus {
    Same,
    Changed,
    Deleted,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Stamp {
    mtime: Option<SystemTime>,
    len: u64,
}

impl Stamp {
    fn of(meta: &std::fs::Metadata) -> Self {
        Self {
            mtime: meta.modified().ok(),
            len: meta.len(),
        }
    }
}

/// What was last loaded from or saved to disk.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Disk {
    stamp: Option<Stamp>,
    hash: Option<u64>,
}

/// A file as read from disk, with line endings normalized to `\n`.
pub struct Loaded {
    pub text: String,
    pub line_ending: LineEnding,
    pub bom: bool,
    pub disk: Disk,
}

fn hash(bytes: &[u8]) -> u64 {
    let mut hasher = std::hash::DefaultHasher::new();
    hasher.write(bytes);
    hasher.finish()
}

/// Read `path`; a missing file is an empty new one.
pub fn load(path: &Path) -> Result<Loaded, String> {
    let meta = match std::fs::metadata(path) {
        Ok(meta) => meta,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Ok(Loaded {
                text: String::new(),
                line_ending: LineEnding::Lf,
                bom: false,
                disk: Disk::default(),
            })
        }
        Err(e) => return Err(format!("Cannot read {}: {e}", path.display())),
    };
    if meta.is_dir() {
        return Err(format!("{} is a directory", path.display()));
    }
    if meta.len() > MAX_FILE_BYTES {
        return Err(format!(
            "{} is larger than {} MiB",
            path.display(),
            MAX_FILE_BYTES >> 20
        ));
    }
    let bytes = std::fs::read(path).map_err(|e| format!("Cannot read {}: {e}", path.display()))?;
    let disk = Disk {
        stamp: Some(Stamp::of(&meta)),
        hash: Some(hash(&bytes)),
    };
    decode(&bytes, disk).map_err(|e| format!("{}: {e}", path.display()))
}

fn decode(bytes: &[u8], disk: Disk) -> Result<Loaded, String> {
    if crate::fs_ops::looks_binary(&bytes[..bytes.len().min(8192)]) {
        return Err("binary file".into());
    }
    let text = std::str::from_utf8(bytes).map_err(|_| "not UTF-8")?;
    let (text, bom) = match text.strip_prefix('\u{feff}') {
        Some(rest) => (rest, true),
        None => (text, false),
    };
    let line_ending = match text.find('\n') {
        Some(i) if i > 0 && text.as_bytes()[i - 1] == b'\r' => LineEnding::Crlf,
        _ => LineEnding::Lf,
    };
    let text = if text.contains('\r') {
        text.replace("\r\n", "\n").replace('\r', "\n")
    } else {
        text.to_owned()
    };
    Ok(Loaded {
        text,
        line_ending,
        bom,
        disk,
    })
}

fn encode(text: &str, line_ending: LineEnding, bom: bool) -> Vec<u8> {
    let mut out = Vec::with_capacity(text.len() + 3);
    if bom {
        out.extend_from_slice("\u{feff}".as_bytes());
    }
    match line_ending {
        LineEnding::Lf => out.extend_from_slice(text.as_bytes()),
        LineEnding::Crlf => out.extend_from_slice(text.replace('\n', "\r\n").as_bytes()),
    }
    out
}

/// The disk state if it differs from `known`: `None` if unchanged, else the
/// new state, with the file's text when it exists.
pub fn changed_on_disk(path: &Path, known: Disk) -> Option<Option<Loaded>> {
    let meta = match std::fs::metadata(path) {
        Ok(meta) => meta,
        Err(_) => return known.stamp.map(|_| None),
    };
    let stamp = Stamp::of(&meta);
    if known.stamp == Some(stamp) {
        return None;
    }
    let loaded = load(path).ok()?;
    if loaded.disk.hash == known.hash {
        return None;
    }
    Some(Some(loaded))
}

/// Write `text` to `path` unless the file changed since `known` (or `force`).
/// Returns the new disk state.
pub fn save(
    path: &Path,
    text: &str,
    line_ending: LineEnding,
    bom: bool,
    known: Disk,
    force: bool,
) -> Result<Disk, SaveError> {
    if !force {
        if let Some(Some(_)) = changed_on_disk(path, known) {
            return Err(SaveError::Conflict);
        }
    }
    let bytes = encode(text, line_ending, bom);
    crate::fs_ops::write_atomic(path, &bytes)
        .map_err(|e| SaveError::Io(format!("Cannot write {}: {e}", path.display())))?;
    let stamp = std::fs::metadata(path).ok().map(|m| Stamp::of(&m));
    Ok(Disk {
        stamp,
        hash: Some(hash(&bytes)),
    })
}

#[derive(Debug)]
pub enum SaveError {
    Conflict,
    Io(String),
}

pub struct Buffer {
    pub id: Uuid,
    pub path: PathBuf,
    pub rope: Rope,
    pub version: u64,
    /// Updates that led to `version`, oldest first.
    log: VecDeque<Update>,
    pub saved_version: u64,
    pub line_ending: LineEnding,
    pub bom: bool,
    pub disk: Disk,
    pub disk_status: DiskStatus,
}

impl Buffer {
    pub fn new(path: PathBuf, loaded: Loaded) -> Self {
        Self {
            id: Uuid::new_v4(),
            path,
            rope: Rope::from_str(&loaded.text),
            version: 0,
            log: VecDeque::new(),
            saved_version: 0,
            line_ending: loaded.line_ending,
            bom: loaded.bom,
            disk: loaded.disk,
            disk_status: DiskStatus::Same,
        }
    }

    pub fn dirty(&self) -> bool {
        self.version != self.saved_version
    }

    /// Apply `updates` made against `base`. `Ok(false)` if `base` is stale:
    /// the editor rebases on the updates it is missing and pushes again.
    pub fn push(&mut self, base: u64, updates: &[Update]) -> Result<bool, String> {
        if base != self.version {
            return Ok(false);
        }
        let mut rope = self.rope.clone();
        for update in updates {
            let changes = text::parse(&update.changes, text::len_utf16(&rope))?;
            text::apply(&mut rope, &changes);
        }
        self.rope = rope;
        for update in updates {
            self.log_update(update.clone());
        }
        Ok(true)
    }

    fn log_update(&mut self, update: Update) {
        self.version += 1;
        self.log.push_back(update);
        if self.log.len() > LOG_LIMIT {
            self.log.pop_front();
        }
    }

    /// The updates after `version`, if still logged.
    pub fn since(&self, version: u64) -> Option<Vec<Update>> {
        let first = self.version - self.log.len() as u64;
        if version < first || version > self.version {
            return None;
        }
        Some(
            self.log
                .iter()
                .skip((version - first) as usize)
                .cloned()
                .collect(),
        )
    }

    /// Replace the text with `loaded` (a reload) as one minimal update.
    /// Returns the update, if the text changed.
    pub fn reload(&mut self, loaded: Loaded) -> Option<Update> {
        let update = text::diff(&self.rope, &loaded.text).map(|change| {
            let changes = text::to_json(std::slice::from_ref(&change), text::len_utf16(&self.rope));
            text::apply(&mut self.rope, &[change]);
            let update = Update {
                client_id: SERVER_CLIENT.into(),
                changes,
            };
            self.log_update(update.clone());
            update
        });
        self.saved_version = self.version;
        self.line_ending = loaded.line_ending;
        self.bom = loaded.bom;
        self.disk = loaded.disk;
        self.disk_status = DiskStatus::Same;
        update
    }

    /// Apply edits btmux makes itself, as one update.
    #[allow(dead_code)]
    pub fn edit(&mut self, changes: &[Change]) -> Update {
        let json = text::to_json(changes, text::len_utf16(&self.rope));
        text::apply(&mut self.rope, changes);
        let update = Update {
            client_id: SERVER_CLIENT.into(),
            changes: json,
        };
        self.log_update(update.clone());
        update
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn update(changes: Value) -> Update {
        Update {
            client_id: "a".into(),
            changes,
        }
    }

    fn buffer(text: &str) -> Buffer {
        Buffer::new(
            PathBuf::from("/x"),
            decode(text.as_bytes(), Disk::default()).unwrap(),
        )
    }

    #[test]
    fn accepts_pushes_on_the_current_version_only() {
        let mut b = buffer("ab");
        assert!(b.push(0, &[update(json!([2, [0, "c"]]))]).unwrap());
        assert_eq!((b.rope.to_string().as_str(), b.version), ("abc", 1));
        assert!(b.dirty());
        // Stale: another editor's update came first.
        assert!(!b.push(0, &[update(json!([2, [0, "d"]]))]).unwrap());
        assert_eq!(b.rope.to_string(), "abc");
        // Invalid updates change nothing.
        assert!(b
            .push(1, &[update(json!([1, [0, "x"]])), update(json!([9]))])
            .is_err());
        assert_eq!((b.rope.to_string().as_str(), b.version), ("abc", 1));
    }

    #[test]
    fn replays_logged_updates() {
        let mut b = buffer("");
        for i in 0..3 {
            let len = b.rope.len_chars();
            let changes = if len == 0 {
                json!([[0, "x"]])
            } else {
                json!([len, [0, "x"]])
            };
            assert!(b.push(i, &[update(changes)]).unwrap());
        }
        assert_eq!(b.since(1).unwrap().len(), 2);
        assert_eq!(b.since(3).unwrap().len(), 0);
        assert!(b.since(4).is_none());
    }

    #[test]
    fn reloads_as_a_minimal_update() {
        let mut b = buffer("one\ntwo\n");
        let update = b
            .reload(decode(b"one\n2\n", Disk::default()).unwrap())
            .unwrap();
        assert_eq!(update.changes, json!([4, [3, "2"], 1]));
        assert_eq!(b.rope.to_string(), "one\n2\n");
        assert!(!b.dirty());
    }

    #[test]
    fn keeps_line_endings_and_bom() {
        let loaded = decode("\u{feff}a\r\nb\r\n".as_bytes(), Disk::default()).unwrap();
        assert_eq!(
            (loaded.text.as_str(), loaded.line_ending, loaded.bom),
            ("a\nb\n", LineEnding::Crlf, true)
        );
        assert_eq!(
            encode(&loaded.text, loaded.line_ending, loaded.bom),
            "\u{feff}a\r\nb\r\n".as_bytes()
        );
        assert!(decode(b"a\0b", Disk::default()).is_err());
    }

    #[test]
    fn saving_detects_outside_changes() {
        let dir = tempfile_dir();
        let path = dir.join("f.txt");
        std::fs::write(&path, "one\n").unwrap();
        let loaded = load(&path).unwrap();
        let disk = save(&path, "two\n", LineEnding::Lf, false, loaded.disk, false).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "two\n");
        // Unchanged since our save: nothing to report.
        assert!(changed_on_disk(&path, disk).is_none());
        std::fs::write(&path, "three!\n").unwrap();
        assert!(matches!(
            save(&path, "four\n", LineEnding::Lf, false, disk, false),
            Err(SaveError::Conflict)
        ));
        save(&path, "four\n", LineEnding::Lf, false, disk, true).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "four\n");
        std::fs::remove_dir_all(dir).unwrap();
    }

    fn tempfile_dir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("btmux-buffer-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }
}
