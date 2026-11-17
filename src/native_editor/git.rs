//! Git signs: each buffer's file as the index has it, which tabs diff the
//! buffer against, plus staging their hunks and blaming lines.

use std::path::{Path, PathBuf};

use git2::{Index, IndexEntry, IndexTime, Repository};
use serde::Serialize;

use super::buffer::{self, LineEnding};

/// Where a file is in a repository's worktree.
#[derive(Clone, Debug)]
pub struct GitFile {
    pub workdir: PathBuf,
    /// The worktree's git dir, holding its `index`.
    pub git_dir: PathBuf,
    /// Relative to `workdir`.
    pub rel: PathBuf,
}

/// The file's text in the index, normalized as buffers are.
#[derive(Clone, Debug, PartialEq)]
pub struct Base {
    pub text: String,
    pub line_ending: LineEnding,
    pub bom: bool,
    /// Not in the index: every line counts as added.
    pub untracked: bool,
}

#[derive(Serialize, Debug, Clone)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct Blame {
    /// `None` for lines not committed yet.
    pub commit: Option<String>,
    pub author: String,
    /// Seconds since the epoch.
    #[cfg_attr(test, ts(type = "number"))]
    pub time: i64,
    pub summary: String,
    /// The whole commit message, when asked for.
    pub message: Option<String>,
}

fn canonical(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

/// The repository whose worktree holds `path` (canonical), if any.
pub fn locate(path: &Path) -> Option<GitFile> {
    let repo = Repository::discover(path.parent()?).ok()?;
    let workdir = canonical(repo.workdir()?);
    let rel = path.strip_prefix(&workdir).ok()?.to_path_buf();
    // Not the repository's own files.
    if rel.starts_with(".git") || rel.as_os_str().is_empty() {
        return None;
    }
    Some(GitFile {
        workdir,
        git_dir: canonical(repo.path()),
        rel,
    })
}

/// `rel` as the index names it.
fn index_path(rel: &Path) -> Vec<u8> {
    let s = rel.to_string_lossy();
    #[cfg(windows)]
    let s = s.replace('\\', "/");
    s.as_bytes().to_vec()
}

/// What git signs follow: the index text, and the commit blame starts from.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Snapshot {
    pub base: Option<Base>,
    pub head: Option<git2::Oid>,
}

pub fn read(file: &GitFile) -> Snapshot {
    let Ok(repo) = Repository::open(&file.workdir) else {
        return Snapshot::default();
    };
    Snapshot {
        base: repo
            .index()
            .ok()
            .and_then(|i| base_in(&repo, &i, &file.rel)),
        head: repo.refname_to_id("HEAD").ok(),
    }
}

/// The file's index text: `None` if it is ignored, binary, a submodule, or
/// conflicted.
#[cfg(test)]
fn read_base(file: &GitFile) -> Option<Base> {
    read(file).base
}

fn base_in(repo: &Repository, index: &Index, rel: &Path) -> Option<Base> {
    match index.get_path(rel, 0) {
        Some(entry) => {
            // Submodules and symlinks.
            if entry.mode & 0o170000 != 0o100000 {
                return None;
            }
            let blob = repo.find_blob(entry.id).ok()?;
            if blob.size() as u64 > buffer::MAX_FILE_BYTES {
                return None;
            }
            let loaded = buffer::decode(blob.content(), Default::default()).ok()?;
            Some(Base {
                text: loaded.text,
                line_ending: loaded.line_ending,
                bom: loaded.bom,
                untracked: false,
            })
        }
        None => {
            let conflicted = (1..=3).any(|stage| index.get_path(rel, stage).is_some());
            let ignored = repo.is_path_ignored(rel).unwrap_or(true);
            (!conflicted && !ignored).then(|| Base {
                text: String::new(),
                line_ending: LineEnding::Lf,
                bom: false,
                untracked: true,
            })
        }
    }
}

const CHANGED: &str = "The index changed meanwhile; try again";

/// Write `text` to the index as the file's content, if the index still has
/// `expected`. Tracked files keep the index blob's encoding; new ones are
/// encoded as on disk and cleaned as `git add` would.
pub fn stage(
    file: &GitFile,
    expected: &Base,
    text: &str,
    line_ending: LineEnding,
    bom: bool,
) -> Result<(), String> {
    let repo = Repository::open(&file.workdir).map_err(|e| e.to_string())?;
    let (id, mode) = if expected.untracked {
        let data = buffer::encode(text, line_ending, bom);
        let mode = new_mode(&file.workdir.join(&file.rel));
        (clean_blob(file, &data)?, mode)
    } else {
        let data = buffer::encode(text, expected.line_ending, expected.bom);
        (repo.blob(&data).map_err(|e| e.to_string())?, 0)
    };
    let lock = IndexLock::take(&file.git_dir)?;
    let mut index = lock.index(&repo)?;
    if base_in(&repo, &index, &file.rel).as_ref() != Some(expected) {
        return Err(CHANGED.into());
    }
    let mode = match index.get_path(&file.rel, 0) {
        Some(entry) => entry.mode,
        None => mode,
    };
    // No stat data, and a zero size, which git takes as never read: it
    // compares the worktree file's content.
    let zero = IndexTime::new(0, 0);
    let entry = IndexEntry {
        ctime: zero,
        mtime: zero,
        dev: 0,
        ino: 0,
        mode,
        uid: 0,
        gid: 0,
        file_size: 0,
        id,
        flags: 0,
        flags_extended: 0,
        path: index_path(&file.rel),
    };
    index
        .add(&entry)
        .map_err(|e| format!("Cannot stage: {}", e.message()))?;
    lock.commit(index)
}

/// `data` as a blob after the path's clean conversion (`text`, `eol`,
/// filters), as `git add` stores it.
fn clean_blob(file: &GitFile, data: &[u8]) -> Result<git2::Oid, String> {
    use std::io::Write;
    use std::process::{Command, Stdio};
    let mut path = std::ffi::OsString::from("--path=");
    path.push(&file.rel);
    let mut child = Command::new("git")
        .args(["hash-object", "-w", "--stdin"])
        .arg(path)
        .current_dir(&file.workdir)
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("Failed to run git: {e}"))?;
    let written = child.stdin.take().map(|mut pipe| pipe.write_all(data));
    let out = child
        .wait_with_output()
        .map_err(|e| format!("Failed to run git: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "Cannot stage: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    if let Some(Err(e)) = written {
        return Err(format!("Failed to write to git: {e}"));
    }
    git2::Oid::from_str(String::from_utf8_lossy(&out.stdout).trim()).map_err(|e| e.to_string())
}

/// Temporary index files [`IndexLock`] writes in the git dir.
pub const SCRATCH_PREFIX: &str = "index.btmux-";

/// Git's `index.lock`, held from reading the index to replacing it, so no
/// other writer's change is lost in between.
struct IndexLock {
    git_dir: PathBuf,
    scratch: PathBuf,
}

impl IndexLock {
    fn take(git_dir: &Path) -> Result<Self, String> {
        let path = git_dir.join("index.lock");
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(1);
        loop {
            match std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)
            {
                Ok(_) => break,
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                    if std::time::Instant::now() >= deadline {
                        return Err("Another Git process holds the index lock; try again".into());
                    }
                    std::thread::sleep(std::time::Duration::from_millis(20));
                }
                Err(e) => return Err(format!("Cannot lock the index: {e}")),
            }
        }
        Ok(Self {
            git_dir: git_dir.to_path_buf(),
            scratch: git_dir.join(format!("{SCRATCH_PREFIX}{}", uuid::Uuid::new_v4())),
        })
    }

    /// The index as it is now, in a scratch copy, owned by `repo`.
    fn index(&self, repo: &Repository) -> Result<Index, String> {
        let current = self.git_dir.join("index");
        match std::fs::read(&current) {
            Ok(bytes) => {
                std::fs::write(&self.scratch, bytes).map_err(|e| e.to_string())?;
                // Keep the index's time for racy-git checks.
                if let Ok(mtime) = std::fs::metadata(&current).and_then(|m| m.modified()) {
                    let _ = std::fs::File::options()
                        .write(true)
                        .open(&self.scratch)
                        .and_then(|f| f.set_modified(mtime));
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(format!("Cannot read the index: {e}")),
        }
        let mut index = Index::open(&self.scratch).map_err(|e| e.to_string())?;
        repo.set_index(&mut index).map_err(|e| e.to_string())?;
        Ok(index)
    }

    /// Replace the index with `index`, then release the lock.
    fn commit(self, mut index: Index) -> Result<(), String> {
        index
            .write()
            .map_err(|e| format!("Cannot write the index: {}", e.message()))?;
        std::fs::rename(&self.scratch, self.git_dir.join("index"))
            .map_err(|e| format!("Cannot write the index: {e}"))
    }
}

impl Drop for IndexLock {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.scratch);
        let _ = std::fs::remove_file(self.git_dir.join("index.lock"));
    }
}

#[cfg(unix)]
fn new_mode(path: &Path) -> u32 {
    use std::os::unix::fs::PermissionsExt;
    match std::fs::metadata(path) {
        Ok(meta) if meta.permissions().mode() & 0o111 != 0 => 0o100755,
        _ => 0o100644,
    }
}

#[cfg(not(unix))]
fn new_mode(_: &Path) -> u32 {
    0o100644
}

/// Blame 1-based `line` of `contents`, the file as the buffer has it.
pub async fn blame(
    file: &GitFile,
    contents: Vec<u8>,
    line: u32,
    full: bool,
) -> Result<Option<Blame>, String> {
    let rel = file.rel.to_string_lossy().into_owned();
    let args = [
        "--no-optional-locks".into(),
        "blame".into(),
        "--porcelain".into(),
        "-L".into(),
        format!("{line},{line}"),
        "--contents".into(),
        "-".into(),
        "--".into(),
        rel,
    ];
    // Untracked files have nothing to blame.
    let Ok(out) = crate::file_git_ops::run_git(&file.workdir, &args, Some(contents)).await else {
        return Ok(None);
    };
    let Some(mut blame) = parse_porcelain(&out) else {
        return Ok(None);
    };
    if full {
        if let Some(commit) = &blame.commit {
            let args = [
                "--no-optional-locks".into(),
                "show".into(),
                "-s".into(),
                "--format=%B".into(),
                commit.clone(),
            ];
            let message = crate::file_git_ops::run_git(&file.workdir, &args, None).await?;
            blame.message = Some(message);
        }
    }
    Ok(Some(blame))
}

fn parse_porcelain(out: &str) -> Option<Blame> {
    let mut lines = out.lines();
    let commit = lines.next()?.split(' ').next()?.to_owned();
    if commit.len() < 40 || !commit.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let mut blame = Blame {
        commit: (!commit.bytes().all(|b| b == b'0')).then_some(commit),
        author: String::new(),
        time: 0,
        summary: String::new(),
        message: None,
    };
    for line in lines {
        if line.starts_with('\t') {
            break;
        }
        let (key, value) = line.split_once(' ').unwrap_or((line, ""));
        match key {
            "author" => blame.author = value.to_owned(),
            "author-time" => blame.time = value.parse().unwrap_or(0),
            "summary" => blame.summary = value.to_owned(),
            _ => {}
        }
    }
    Some(blame)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn git(dir: &Path, args: &[&str]) {
        let out = std::process::Command::new("git")
            .args([
                "-c",
                "user.name=T",
                "-c",
                "user.email=t@t",
                "-c",
                "commit.gpgsign=false",
            ])
            .args(args)
            .current_dir(dir)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    fn repo() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("btmux-gitsigns-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let dir = dir.canonicalize().unwrap();
        git(&dir, &["init", "-q"]);
        dir
    }

    #[test]
    fn reads_the_index_and_stages_text() {
        let dir = repo();
        std::fs::write(dir.join(".gitignore"), "ignored\n").unwrap();
        std::fs::write(dir.join("f.txt"), "\u{feff}a\r\nb\r\n").unwrap();
        git(&dir, &["add", "f.txt"]);
        std::fs::write(dir.join("f.txt"), "\u{feff}a\r\nB\r\n").unwrap();
        let file = locate(&dir.join("f.txt")).unwrap();
        let base = read_base(&file).unwrap();
        assert_eq!(base.text, "a\nb\n");
        assert!(!base.untracked && base.bom && base.line_ending == LineEnding::Crlf);
        // Encoded as the index had it.
        stage(&file, &base, "a\nb\nc\n", LineEnding::Lf, false).unwrap();
        let out = std::process::Command::new("git")
            .args(["show", ":f.txt"])
            .current_dir(&dir)
            .output()
            .unwrap();
        assert_eq!(out.stdout, "\u{feff}a\r\nb\r\nc\r\n".as_bytes());
        // A stale base is refused.
        assert!(stage(&file, &base, "x\n", LineEnding::Lf, false).is_err());
        // The worktree file differs from the index, despite unchanged stat data.
        let out = std::process::Command::new("git")
            .args(["diff", "--name-only"])
            .current_dir(&dir)
            .output()
            .unwrap();
        assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "f.txt");

        std::fs::write(dir.join("new.txt"), "n\n").unwrap();
        let new = locate(&dir.join("new.txt")).unwrap();
        let base = read_base(&new).unwrap();
        assert!(base.untracked && base.text.is_empty());
        stage(&new, &base, "n\n", LineEnding::Lf, false).unwrap();
        assert!(!read_base(&new).unwrap().untracked);

        std::fs::write(dir.join("ignored"), "i\n").unwrap();
        assert!(read_base(&locate(&dir.join("ignored")).unwrap()).is_none());
        std::fs::remove_dir_all(dir).unwrap();
    }

    fn git_out(dir: &Path, args: &[&str]) -> Vec<u8> {
        std::process::Command::new("git")
            .args(args)
            .current_dir(dir)
            .output()
            .unwrap()
            .stdout
    }

    #[test]
    fn stages_new_files_as_git_add_does() {
        let dir = repo();
        std::fs::write(dir.join(".gitattributes"), "*.txt text eol=lf\n").unwrap();
        std::fs::write(dir.join("new.txt"), "a\r\nb\r\n").unwrap();
        let file = locate(&dir.join("new.txt")).unwrap();
        let base = read_base(&file).unwrap();
        stage(&file, &base, "a\nb\n", LineEnding::Crlf, false).unwrap();
        assert_eq!(git_out(&dir, &["show", ":new.txt"]), b"a\nb\n");
        // Fully staged: nothing left in the worktree.
        assert_eq!(
            git_out(&dir, &["status", "--porcelain", "new.txt"]),
            b"A  new.txt\n"
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn stages_under_the_index_lock() {
        let dir = repo();
        std::fs::write(dir.join("f.txt"), "a\n").unwrap();
        std::fs::write(dir.join("g.txt"), "g\n").unwrap();
        git(&dir, &["add", "f.txt", "g.txt"]);
        let file = locate(&dir.join("f.txt")).unwrap();
        let base = read_base(&file).unwrap();
        // Another writer holds the lock: refused, and its lock is kept.
        std::fs::write(dir.join(".git/index.lock"), "").unwrap();
        assert!(stage(&file, &base, "b\n", LineEnding::Lf, false).is_err());
        assert!(dir.join(".git/index.lock").exists());
        std::fs::remove_file(dir.join(".git/index.lock")).unwrap();
        // Other entries are kept.
        std::fs::write(dir.join("g.txt"), "G\n").unwrap();
        git(&dir, &["add", "g.txt"]);
        stage(&file, &base, "b\n", LineEnding::Lf, false).unwrap();
        assert_eq!(git_out(&dir, &["show", ":f.txt"]), b"b\n");
        assert_eq!(git_out(&dir, &["show", ":g.txt"]), b"G\n");
        let names: Vec<_> = std::fs::read_dir(dir.join(".git"))
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect();
        assert!(!names.iter().any(|n| n.starts_with("index.")), "{names:?}");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn follows_head() {
        let dir = repo();
        std::fs::write(dir.join("f.txt"), "a\n").unwrap();
        let file = locate(&dir.join("f.txt")).unwrap();
        assert!(read(&file).head.is_none());
        git(&dir, &["add", "f.txt"]);
        git(&dir, &["commit", "-q", "-m", "First"]);
        let first = read(&file);
        git(&dir, &["commit", "-q", "--allow-empty", "-m", "Second"]);
        let second = read(&file);
        assert_eq!(first.base, second.base);
        assert!(first.head.is_some() && first.head != second.head);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn blames_buffer_lines() {
        let dir = repo();
        std::fs::write(dir.join("f.txt"), "a\nb\n").unwrap();
        git(&dir, &["add", "f.txt"]);
        git(&dir, &["commit", "-q", "-m", "First\n\nBody"]);
        let file = locate(&dir.join("f.txt")).unwrap();
        let first = blame(&file, b"a\nB\n".to_vec(), 1, true)
            .await
            .unwrap()
            .unwrap();
        assert!(first.commit.is_some());
        assert_eq!(
            (first.author.as_str(), first.summary.as_str()),
            ("T", "First")
        );
        assert_eq!(first.message.as_deref(), Some("First\n\nBody"));
        let second = blame(&file, b"a\nB\n".to_vec(), 2, false)
            .await
            .unwrap()
            .unwrap();
        assert!(second.commit.is_none());
        std::fs::remove_dir_all(dir).unwrap();
    }
}
