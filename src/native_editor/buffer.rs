//! One open file: the authoritative text, its update log, and what is on disk.

use std::collections::VecDeque;
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

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct Stamp {
    #[serde(with = "epoch")]
    #[cfg_attr(test, ts(type = "[number, number] | null"))]
    mtime: Option<SystemTime>,
    len: u64,
}

/// A time as seconds and nanoseconds since the epoch; earlier ones as none.
mod epoch {
    use serde::{Deserialize, Deserializer, Serialize, Serializer};
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    pub fn serialize<S: Serializer>(time: &Option<SystemTime>, s: S) -> Result<S::Ok, S::Error> {
        time.and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map(|d| (d.as_secs(), d.subsec_nanos()))
            .serialize(s)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Option<SystemTime>, D::Error> {
        let time = Option::<(u64, u32)>::deserialize(d)?;
        Ok(time.map(|(secs, nanos)| UNIX_EPOCH + Duration::new(secs, nanos)))
    }
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
#[derive(Serialize, Deserialize, Clone, Copy, Debug, Default, PartialEq, Eq)]
#[cfg_attr(test, derive(ts_rs::TS))]
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

/// Stable across builds, since upgrades hand it to another binary.
fn hash(bytes: &[u8]) -> u64 {
    let digest = aws_lc_rs::digest::digest(&aws_lc_rs::digest::SHA256, bytes);
    u64::from_le_bytes(digest.as_ref()[..8].try_into().unwrap())
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
    let bytes = read(path, &meta)?;
    let disk = Disk {
        stamp: Some(Stamp::of(&meta)),
        hash: Some(hash(&bytes)),
    };
    decode(&bytes, disk).map_err(|e| format!("{}: {e}", path.display()))
}

/// The bytes of the file `meta` describes, if it can be opened at all.
fn read(path: &Path, meta: &std::fs::Metadata) -> Result<Vec<u8>, String> {
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
    std::fs::read(path).map_err(|e| format!("Cannot read {}: {e}", path.display()))
}

pub(super) fn decode(bytes: &[u8], disk: Disk) -> Result<Loaded, String> {
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

pub(super) fn encode(text: &str, line_ending: LineEnding, bom: bool) -> Vec<u8> {
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

/// The file on disk, when it differs from `known`.
pub enum OnDisk {
    Deleted,
    /// Its text, or why it can't be opened (binary, too large): still a
    /// change, which a save must not overwrite unasked.
    Changed(Result<Loaded, String>),
}

/// How the file differs from `known`, if it does. Compares bytes, not
/// text, so a replacement that can't be decoded still counts.
pub fn changed_on_disk(path: &Path, known: Disk) -> Option<OnDisk> {
    let meta = match std::fs::metadata(path) {
        Ok(meta) => meta,
        Err(_) => return known.stamp.map(|_| OnDisk::Deleted),
    };
    let stamp = Stamp::of(&meta);
    if known.stamp == Some(stamp) {
        return None;
    }
    let bytes = match read(path, &meta) {
        Ok(bytes) => bytes,
        Err(e) => return Some(OnDisk::Changed(Err(e))),
    };
    let hash = hash(&bytes);
    if known.hash == Some(hash) {
        return None;
    }
    let disk = Disk {
        stamp: Some(stamp),
        hash: Some(hash),
    };
    let loaded = decode(&bytes, disk).map_err(|e| format!("{}: {e}", path.display()));
    Some(OnDisk::Changed(loaded))
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
        if let Some(OnDisk::Changed(_)) = changed_on_disk(path, known) {
            return Err(SaveError::Conflict);
        }
    }
    let bytes = encode(text, line_ending, bom);
    write_in_place(path, &bytes)
        .map_err(|e| SaveError::Io(format!("Cannot write {}: {e}", path.display())))?;
    let stamp = std::fs::metadata(path).ok().map(|m| Stamp::of(&m));
    Ok(Disk {
        stamp,
        hash: Some(hash(&bytes)),
    })
}

/// Write the file itself rather than rename a new one over it, as editors
/// do: its mode, owner, and hard links stay, a read-only file refuses, and
/// it needs no write access to its directory.
fn write_in_place(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(path)?;
    file.write_all(bytes)?;
    file.sync_all()
}

#[derive(Debug)]
pub enum SaveError {
    Conflict,
    Io(String),
}

/// One update as language servers see it: the version after it, and its
/// `contentChanges`.
#[derive(Debug)]
pub struct Applied {
    pub version: u64,
    pub changes: Vec<Value>,
}

/// An LSP position (UTF-16) for a UTF-16 offset.
pub fn position(rope: &Rope, offset: usize) -> Value {
    let char = text::char_at(rope, offset.min(text::len_utf16(rope)));
    let line = rope.char_to_line(char);
    let start = rope.char_to_utf16_cu(rope.line_to_char(line));
    serde_json::json!({ "line": line, "character": rope.char_to_utf16_cu(char) - start })
}

/// The UTF-16 offset of an LSP position, clamped to the line and document.
pub fn offset(rope: &Rope, position: &Value) -> Option<usize> {
    let line = position["line"].as_u64()? as usize;
    let character = position["character"].as_u64()? as usize;
    if line >= rope.len_lines() {
        return Some(text::len_utf16(rope));
    }
    let start = rope.char_to_utf16_cu(rope.line_to_char(line));
    let end = if line + 1 < rope.len_lines() {
        rope.char_to_utf16_cu(rope.line_to_char(line + 1)) - 1
    } else {
        text::len_utf16(rope)
    };
    Some((start + character).min(end))
}

/// `contentChanges` for `changes` to `rope`, last first so each range is
/// still valid when applied in order.
fn content_changes(rope: &Rope, changes: &[Change]) -> Vec<Value> {
    changes
        .iter()
        .rev()
        .map(|c| {
            serde_json::json!({
                "range": { "start": position(rope, c.from), "end": position(rope, c.to) },
                "text": c.insert,
            })
        })
        .collect()
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
    pub language_id: Option<String>,
    /// Language servers it is open in.
    pub servers: Vec<u64>,
}

/// An unsaved buffer as `btmux upgrade` hands it over, without its
/// language servers.
#[cfg(unix)]
#[derive(Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct HandedBuffer {
    id: Uuid,
    #[serde(with = "path_bytes")]
    #[cfg_attr(test, ts(type = "Array<number>"))]
    path: PathBuf,
    /// Cloned cheaply under the hub's lock, and written out after.
    #[serde(with = "rope_text")]
    #[cfg_attr(test, ts(type = "string"))]
    text: Rope,
    version: u64,
    /// Updates that led to `version`, so tabs resume and confirm their own.
    log: Vec<Update>,
    saved_version: u64,
    line_ending: LineEnding,
    bom: bool,
    disk: Disk,
    disk_status: DiskStatus,
    language_id: Option<String>,
}

/// A path as its bytes, which needn't be UTF-8.
#[cfg(unix)]
pub(super) mod path_bytes {
    use serde::{Deserialize, Deserializer, Serialize, Serializer};
    use std::os::unix::ffi::{OsStrExt, OsStringExt};
    use std::path::{Path, PathBuf};

    pub fn serialize<S: Serializer>(path: &Path, s: S) -> Result<S::Ok, S::Error> {
        path.as_os_str().as_bytes().serialize(s)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<PathBuf, D::Error> {
        Ok(std::ffi::OsString::from_vec(Vec::deserialize(d)?).into())
    }
}

#[cfg(unix)]
mod rope_text {
    use ropey::Rope;
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(rope: &Rope, s: S) -> Result<S::Ok, S::Error> {
        s.collect_str(rope)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Rope, D::Error> {
        Ok(Rope::from_str(&String::deserialize(d)?))
    }
}

/// Updates a push added: from `version`, with what language servers see.
pub struct Pushed {
    pub version: u64,
    pub updates: Vec<Update>,
    pub applied: Vec<Applied>,
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
            language_id: None,
            servers: Vec::new(),
        }
    }

    #[cfg(unix)]
    pub fn handoff(&self) -> HandedBuffer {
        HandedBuffer {
            id: self.id,
            path: self.path.clone(),
            text: self.rope.clone(),
            version: self.version,
            log: self.log.iter().cloned().collect(),
            saved_version: self.saved_version,
            line_ending: self.line_ending,
            bom: self.bom,
            disk: self.disk,
            disk_status: self.disk_status,
            language_id: self.language_id.clone(),
        }
    }

    /// A handed over buffer, which tabs resume.
    #[cfg(unix)]
    pub fn restored(handoff: HandedBuffer) -> Self {
        Self {
            id: handoff.id,
            path: handoff.path,
            rope: handoff.text,
            version: handoff.version,
            log: handoff.log.into(),
            saved_version: handoff.saved_version,
            line_ending: handoff.line_ending,
            bom: handoff.bom,
            disk: handoff.disk,
            disk_status: handoff.disk_status,
            language_id: handoff.language_id,
            servers: Vec::new(),
        }
    }

    pub fn dirty(&self) -> bool {
        self.version != self.saved_version
    }

    /// Apply `updates` made against `base`. An editor sends every update not
    /// yet confirmed, so those already applied since `base` are skipped.
    /// `Ok(None)` if `base` is stale: the editor rebases on the updates it is
    /// missing and pushes again.
    pub fn push(&mut self, base: u64, updates: &[Update]) -> Result<Option<Pushed>, String> {
        let Some(logged) = self.since(base) else {
            return Ok(None);
        };
        let resent = logged.len() <= updates.len()
            && logged
                .iter()
                .zip(updates)
                .all(|(a, b)| a.client_id == b.client_id && a.changes == b.changes);
        if !resent {
            return Ok(None);
        }
        let fresh = &updates[logged.len()..];
        let mut rope = self.rope.clone();
        let mut applied = Vec::with_capacity(fresh.len());
        for (i, update) in fresh.iter().enumerate() {
            let changes = text::parse(&update.changes, text::len_utf16(&rope))?;
            applied.push(Applied {
                version: self.version + i as u64 + 1,
                changes: content_changes(&rope, &changes),
            });
            text::apply(&mut rope, &changes);
        }
        let version = self.version;
        self.rope = rope;
        for update in fresh {
            self.log_update(update.clone());
        }
        Ok(Some(Pushed {
            version,
            updates: fresh.to_vec(),
            applied,
        }))
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
    pub fn reload(&mut self, loaded: Loaded) -> Option<(Update, Applied)> {
        let update = text::diff(&self.rope, &loaded.text).map(|change| self.edit(&[change]));
        self.saved_version = self.version;
        self.line_ending = loaded.line_ending;
        self.bom = loaded.bom;
        self.disk = loaded.disk;
        self.disk_status = DiskStatus::Same;
        update
    }

    /// Apply sorted edits btmux makes itself, as one update.
    pub fn edit(&mut self, changes: &[Change]) -> (Update, Applied) {
        let json = text::to_json(changes, text::len_utf16(&self.rope));
        let lsp = content_changes(&self.rope, changes);
        text::apply(&mut self.rope, changes);
        let update = Update {
            client_id: SERVER_CLIENT.into(),
            changes: json,
        };
        self.log_update(update.clone());
        let applied = Applied {
            version: self.version,
            changes: lsp,
        };
        (update, applied)
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
        let c = update(json!([2, [0, "c"]]));
        let pushed = b.push(0, std::slice::from_ref(&c)).unwrap().unwrap();
        assert_eq!((pushed.version, pushed.updates.len()), (0, 1));
        assert_eq!(
            pushed.applied[0].changes,
            [json!({ "range": {
                "start": { "line": 0, "character": 2 },
                "end": { "line": 0, "character": 2 },
            }, "text": "c" })]
        );
        assert_eq!((b.rope.to_string().as_str(), b.version), ("abc", 1));
        assert!(b.dirty());
        // Sent again with the next update: only that one is new.
        let d = update(json!([3, [0, "d"]]));
        let pushed = b.push(0, &[c.clone(), d]).unwrap().unwrap();
        assert_eq!((pushed.version, pushed.updates.len()), (1, 1));
        assert_eq!(b.rope.to_string(), "abcd");
        // Stale: another editor's update came first.
        let other = Update {
            client_id: "b".into(),
            changes: json!([2, [0, "e"]]),
        };
        assert!(b.push(0, &[other]).unwrap().is_none());
        assert_eq!(b.rope.to_string(), "abcd");
        // Invalid updates change nothing.
        assert!(b
            .push(2, &[update(json!([1, [0, "x"]])), update(json!([9]))])
            .is_err());
        assert_eq!((b.rope.to_string().as_str(), b.version), ("abcd", 2));
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
            assert!(b.push(i, &[update(changes)]).unwrap().is_some());
        }
        assert_eq!(b.since(1).unwrap().len(), 2);
        assert_eq!(b.since(3).unwrap().len(), 0);
        assert!(b.since(4).is_none());
    }

    #[test]
    fn reloads_as_a_minimal_update() {
        let mut b = buffer("one\ntwo\n");
        let (update, applied) = b
            .reload(decode(b"one\n2\n", Disk::default()).unwrap())
            .unwrap();
        assert_eq!(update.changes, json!([4, [3, "2"], 1]));
        assert_eq!(applied.version, 1);
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
        let disk = save(&path, "four\n", LineEnding::Lf, false, disk, true).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "four\n");
        // Replaced by something that isn't text: still a conflict.
        let binary = [0u8, 1, 2, 3, 4, 5, 6];
        std::fs::write(&path, binary).unwrap();
        assert!(matches!(
            changed_on_disk(&path, disk),
            Some(OnDisk::Changed(Err(_)))
        ));
        assert!(matches!(
            save(&path, "five\n", LineEnding::Lf, false, disk, false),
            Err(SaveError::Conflict)
        ));
        assert_eq!(std::fs::read(&path).unwrap(), binary);
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// Paths need not be UTF-8, which JSON strings must be.
    #[cfg(unix)]
    #[test]
    fn hands_over_any_path() {
        use std::os::unix::ffi::OsStringExt;
        let mut b = buffer("a\nb\n");
        b.path = std::ffi::OsString::from_vec(b"/tmp/\xff.txt".to_vec()).into();
        let json = serde_json::to_string(&b.handoff()).unwrap();
        let restored = Buffer::restored(serde_json::from_str(&json).unwrap());
        assert_eq!(restored.path, b.path);
        assert_eq!(restored.rope.to_string(), "a\nb\n");
    }

    fn tempfile_dir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("btmux-buffer-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }
}

#[cfg(test)]
mod position_tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn positions_count_utf16_per_line() {
        let rope = Rope::from_str("a😀b\nxy\n");
        assert_eq!(position(&rope, 3), json!({ "line": 0, "character": 3 }));
        assert_eq!(position(&rope, 6), json!({ "line": 1, "character": 1 }));
        let at = |line: u64, character: u64| {
            offset(&rope, &json!({ "line": line, "character": character }))
        };
        assert_eq!(at(1, 1), Some(6));
        // Past the line's end, clamped to before its newline.
        assert_eq!(at(0, 99), Some(4));
        assert_eq!(at(9, 0), Some(8));
    }
}
