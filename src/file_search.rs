use nucleo::pattern::{CaseMatching, Normalization, Pattern};
use nucleo::{Config, Matcher, Utf32Str};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, Instant};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tokio::sync::watch;

const MAX_CONTENT_RESULTS: usize = 100;
const MAX_FILE_RESULTS: usize = 100;
/// Roots kept indexed, least recently searched dropped first.
const MAX_ROOTS: usize = 4;

#[derive(Debug, Serialize, Deserialize)]
pub struct FileSearchResult {
    pub path: String,
    /// Matched characters (code points, not bytes) of `path`.
    pub indices: Vec<u32>,
    /// nucleo's score plus the path's boost.
    pub score: u32,
}

#[derive(Debug, Default, Serialize)]
pub struct FileSearch {
    pub results: Vec<FileSearchResult>,
    /// The root is being listed again; a `fresh` search waits for it.
    pub refreshing: bool,
    /// The root searched, symlinks resolved, as result paths start.
    pub root: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct SearchResult {
    pub path: String,
    pub line: Option<u64>,
    pub text: Option<String>,
}

type Files = Arc<Vec<String>>;
type Listing = watch::Receiver<Option<Result<Files, String>>>;

#[derive(Clone, Copy)]
struct Timing {
    /// A search of a list this old relists its root in the background…
    refresh_after: Duration,
    /// …or this many times as long as listing it took, if longer.
    refresh_per_listing: u32,
    /// A root not searched this long is dropped…
    idle: Duration,
    /// …checked this often.
    sweep: Duration,
}

const TIMING: Timing = Timing {
    refresh_after: Duration::from_secs(5),
    refresh_per_listing: 4,
    idle: Duration::from_secs(10 * 60),
    sweep: Duration::from_secs(60),
};

/// Fuzzy path search over cached file lists of a few roots. A stale list is
/// served while its root is listed again, as `GitTracker` does.
pub struct FileIndex {
    /// Least recently searched first.
    roots: Arc<Mutex<Vec<Root>>>,
    timing: Timing,
    sweeping: AtomicBool,
}

struct Root {
    path: PathBuf,
    files: Option<Listed>,
    /// A listing in flight.
    listing: Option<Listing>,
    searched: Instant,
}

struct Listed {
    /// Paths relative to the root.
    files: Files,
    finished: Instant,
    took: Duration,
}

impl FileIndex {
    pub fn new() -> Self {
        Self::with_timing(TIMING)
    }

    fn with_timing(timing: Timing) -> Self {
        Self {
            roots: Arc::default(),
            timing,
            sweeping: AtomicBool::new(false),
        }
    }

    /// Matches of `query` under `root`, each scored up by its `boost` (keyed
    /// by result path). A stale list is searched while it is listed again
    /// (`refreshing`); `fresh` waits for that listing instead. An empty query
    /// starts listing `root` if it has no list, without waiting.
    pub async fn search(
        &self,
        query: &str,
        root: &Path,
        fresh: bool,
        boost: &HashMap<String, u32>,
    ) -> Result<FileSearch, String> {
        let root_path = crate::fs_ops::client_path(root);
        if query.trim().is_empty() {
            self.warm(root);
            return Ok(FileSearch {
                root: root_path,
                ..FileSearch::default()
            });
        }
        let (files, refreshing) = self.files(root, fresh).await?;
        let query = query.to_string();
        let prefix = path_prefix(&root_path);
        let boost: HashMap<String, u32> = boost
            .iter()
            .filter_map(|(path, &n)| Some((path.strip_prefix(&prefix)?.to_string(), n)))
            .collect();
        let results =
            tokio::task::spawn_blocking(move || match_files(&files, &query, &prefix, &boost))
                .await
                .map_err(|error| format!("File search failed: {}", error))?;
        Ok(FileSearch {
            results,
            refreshing,
            root: root_path,
        })
    }

    /// The entry for `root`, moved to most recently searched.
    fn touch<'a>(&self, roots: &'a mut Vec<Root>, root: &Path) -> &'a mut Root {
        let mut entry = match roots.iter().position(|r| r.path == root) {
            Some(at) => roots.remove(at),
            None => Root {
                path: root.to_path_buf(),
                files: None,
                listing: None,
                searched: Instant::now(),
            },
        };
        entry.searched = Instant::now();
        roots.push(entry);
        if roots.len() > MAX_ROOTS {
            roots.remove(0);
        }
        roots.last_mut().unwrap()
    }

    fn warm(&self, root: &Path) {
        let mut roots = self.roots.lock().unwrap();
        let entry = self.touch(&mut roots, root);
        if entry.files.is_none() && entry.listing.is_none() {
            entry.listing = Some(self.list(root));
        }
    }

    /// `root`'s files, listed now if it has none, and whether they are being
    /// listed again. `fresh` waits for a listing in flight.
    async fn files(&self, root: &Path, fresh: bool) -> Result<(Files, bool), String> {
        let (mut listing, stale) = {
            let mut roots = self.roots.lock().unwrap();
            let entry = self.touch(&mut roots, root);
            match &entry.files {
                Some(listed) => {
                    let files = listed.files.clone();
                    let refresh_after = self
                        .timing
                        .refresh_after
                        .max(listed.took * self.timing.refresh_per_listing);
                    if listed.finished.elapsed() >= refresh_after && entry.listing.is_none() {
                        entry.listing = Some(self.list(root));
                    }
                    match &entry.listing {
                        Some(listing) if fresh => (listing.clone(), Some(files)),
                        listing => return Ok((files, listing.is_some())),
                    }
                }
                None => (
                    entry.listing.get_or_insert_with(|| self.list(root)).clone(),
                    None,
                ),
            }
        };
        let result = match listing.wait_for(Option::is_some).await {
            Ok(result) => result.clone().unwrap(),
            Err(_) => Err("File listing stopped".to_string()),
        };
        match (result, stale) {
            (Ok(files), _) => Ok((files, false)),
            // A failed relisting keeps the last list.
            (Err(_), Some(files)) => Ok((files, false)),
            (Err(error), None) => Err(error),
        }
    }

    /// Lists `root` in a task of its own, which stores the result if `root`
    /// is still indexed. The caller stores the returned receiver.
    fn list(&self, root: &Path) -> Listing {
        if !self.sweeping.swap(true, Ordering::Relaxed) {
            tokio::spawn(sweep(Arc::downgrade(&self.roots), self.timing));
        }
        let (tx, rx) = watch::channel(None);
        let roots = self.roots.clone();
        let root = root.to_path_buf();
        let ours = rx.clone();
        tokio::spawn(async move {
            let started = Instant::now();
            let result = list_files(&root).await.map(Arc::new);
            {
                let mut roots = roots.lock().unwrap();
                if let Some(entry) = roots.iter_mut().find(|r| {
                    r.path == root && r.listing.as_ref().is_some_and(|l| l.same_channel(&ours))
                }) {
                    entry.listing = None;
                    match &result {
                        Ok(files) => {
                            entry.files = Some(Listed {
                                files: files.clone(),
                                finished: Instant::now(),
                                took: started.elapsed(),
                            })
                        }
                        // Keep the last list, and don't retry on every search.
                        Err(_) => {
                            if let Some(listed) = &mut entry.files {
                                listed.finished = Instant::now();
                            }
                        }
                    }
                }
            }
            let _ = tx.send(Some(result));
        });
        rx
    }
}

/// Drops roots not searched for `timing.idle`, until the index is dropped.
async fn sweep(roots: Weak<Mutex<Vec<Root>>>, timing: Timing) {
    loop {
        tokio::time::sleep(timing.sweep).await;
        let Some(roots) = roots.upgrade() else { return };
        roots
            .lock()
            .unwrap()
            .retain(|root| root.searched.elapsed() < timing.idle);
    }
}

async fn list_files(root: &Path) -> Result<Vec<String>, String> {
    if !tokio::fs::metadata(root)
        .await
        .is_ok_and(|metadata| metadata.is_dir())
    {
        return Err(format!("Not a directory: {}", root.display()));
    }
    let mut child = spawn_file_lister(root)?;
    let mut lines = BufReader::new(child.stdout.take().unwrap()).lines();
    let mut files = Vec::new();
    while let Ok(Some(line)) = lines.next_line().await {
        let path = line.strip_prefix("./").unwrap_or(&line);
        files.push(path.to_string());
    }
    let _ = child.wait().await;
    Ok(files)
}

fn path_prefix(root: &str) -> String {
    if root.ends_with('/') {
        root.to_string()
    } else {
        format!("{}/", root)
    }
}

/// `text` for matching, one char per code point: `Utf32Str::new` takes one
/// per grapheme cluster, or bytes when every cluster starts with ASCII,
/// which would make indices count neither.
fn haystack<'a>(text: &'a str, buf: &'a mut Vec<char>) -> Utf32Str<'a> {
    if text.is_ascii() {
        return Utf32Str::Ascii(text.as_bytes());
    }
    buf.clear();
    buf.extend(text.chars());
    Utf32Str::Unicode(buf)
}

/// The best matches of `query` among `files` (relative paths, keys of
/// `boost` too), as client paths starting with `prefix`.
fn match_files(
    files: &[String],
    query: &str,
    prefix: &str,
    boost: &HashMap<String, u32>,
) -> Vec<FileSearchResult> {
    let pattern = Pattern::parse(query, CaseMatching::Smart, Normalization::Smart);
    let mut matcher = Matcher::new(Config::DEFAULT.match_paths());
    let mut buf = Vec::new();
    let mut matches: Vec<(u32, &String)> = files
        .iter()
        .filter_map(|file| {
            let score = pattern.score(haystack(file, &mut buf), &mut matcher)?;
            Some((score + boost.get(file).copied().unwrap_or(0), file))
        })
        .collect();
    // Best first; ties go to shorter paths.
    let order = |a: &(u32, &String), b: &(u32, &String)| {
        b.0.cmp(&a.0)
            .then_with(|| a.1.len().cmp(&b.1.len()))
            .then_with(|| a.1.cmp(b.1))
    };
    if matches.len() > MAX_FILE_RESULTS {
        matches.select_nth_unstable_by(MAX_FILE_RESULTS, order);
        matches.truncate(MAX_FILE_RESULTS);
    }
    matches.sort_unstable_by(order);

    let offset = prefix.chars().count() as u32;
    let mut indices = Vec::new();
    matches
        .into_iter()
        .map(|(score, file)| {
            indices.clear();
            pattern.indices(haystack(file, &mut buf), &mut matcher, &mut indices);
            indices.sort_unstable();
            indices.dedup();
            FileSearchResult {
                path: format!("{}{}", prefix, file),
                indices: indices.iter().map(|&i| i + offset).collect(),
                score,
            }
        })
        .collect()
}

/// Lists files with `fd` (respecting ignore files), else `find`.
fn spawn_file_lister(root: &Path) -> Result<tokio::process::Child, String> {
    // Debian and Ubuntu install fd as `fdfind`.
    for program in ["fd", "fdfind"] {
        let mut fd = Command::new(program);
        fd.args([
            "--type",
            "f",
            "--hidden",
            "--exclude",
            ".git",
            "--color",
            "never",
        ]);
        #[cfg(windows)]
        fd.args(["--path-separator", "/"]);
        match spawn_lister(fd, root) {
            Ok(child) => return Ok(child),
            Err(error) if error.kind() == ErrorKind::NotFound => {}
            Err(error) => return Err(format!("Failed to run fd: {}", error)),
        }
    }

    // Windows' `find` searches text.
    #[cfg(windows)]
    return Err("File search needs fd (`winget install sharkdp.fd`)".into());
    #[cfg(unix)]
    {
        let mut find = Command::new("find");
        find.args([
            ".",
            "-type",
            "f",
            "-not",
            "-path",
            "./.git/*",
            "-not",
            "-path",
            "*/node_modules/*",
            "-not",
            "-path",
            "./target/*",
            "-not",
            "-path",
            "*/dist/*",
        ]);
        spawn_lister(find, root).map_err(|error| format!("Failed to run fd or find: {}", error))
    }
}

fn spawn_lister(mut command: Command, root: &Path) -> std::io::Result<tokio::process::Child> {
    command
        .current_dir(root)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
}

pub async fn content_search(query: &str, root: &Path) -> Result<Vec<SearchResult>, String> {
    if query.trim().is_empty() {
        return Ok(Vec::new());
    }

    let mut rg = Command::new("rg");
    rg.args([
        "--json",
        "--max-count",
        "5",
        "--max-filesize",
        "1M",
        "--max-columns",
        "200",
        "--smart-case",
    ]);
    #[cfg(windows)]
    rg.args(["--path-separator", "/"]);
    rg.arg("--")
        .arg(query)
        .arg(".")
        .current_dir(root)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());

    match rg.spawn() {
        Ok(child) => return collect_rg_results(child, root).await,
        Err(error) if error.kind() == ErrorKind::NotFound => {}
        Err(error) => return Err(format!("Failed to run ripgrep: {}", error)),
    }

    let mut grep = Command::new("grep");
    grep.args(["-R", "-n", "-H", "-I"]);
    if !has_uppercase_literal(query) {
        grep.arg("-i");
    }
    grep.arg("-e")
        .arg(query)
        .arg(".")
        .current_dir(root)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());

    let child = grep
        .spawn()
        .map_err(|error| format!("Failed to run ripgrep or grep: {}", error))?;
    collect_grep_results(child, root).await
}

/// Whether a pattern has an uppercase letter outside escapes like `\W`, as
/// ripgrep's `--smart-case` decides.
fn has_uppercase_literal(pattern: &str) -> bool {
    let mut chars = pattern.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            chars.next();
        } else if c.is_uppercase() {
            return true;
        }
    }
    false
}

async fn collect_rg_results(
    mut child: tokio::process::Child,
    root: &Path,
) -> Result<Vec<SearchResult>, String> {
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| "Failed to read ripgrep output".to_string())?;
    let mut lines = BufReader::new(stdout).lines();
    let mut results = Vec::new();

    while results.len() < MAX_CONTENT_RESULTS {
        let line = lines
            .next_line()
            .await
            .map_err(|error| format!("Failed to read ripgrep output: {}", error))?;
        let Some(line) = line else { break };
        let Ok(value) = serde_json::from_str::<serde_json::Value>(&line) else {
            continue;
        };
        if value.get("type").and_then(|t| t.as_str()) != Some("match") {
            continue;
        }
        let Some(data) = value.get("data") else {
            continue;
        };

        let rel_path = data
            .get("path")
            .and_then(|p| p.get("text"))
            .and_then(|t| t.as_str())
            .unwrap_or("");
        let line_number = data.get("line_number").and_then(|n| n.as_u64());
        let text = data
            .get("lines")
            .and_then(|l| l.get("text"))
            .and_then(|t| t.as_str())
            .map(|s| s.trim().to_string());

        results.push(SearchResult {
            path: search_path(root, rel_path),
            line: line_number,
            text,
        });
    }

    let limited = results.len() >= MAX_CONTENT_RESULTS;
    drop(lines);
    if limited {
        let _ = child.kill().await;
    }
    let status = child
        .wait()
        .await
        .map_err(|error| format!("Failed to wait for ripgrep: {}", error))?;
    if !limited && !matches!(status.code(), Some(0) | Some(1)) {
        return Err(format!("ripgrep failed with status {}", status));
    }

    Ok(results)
}

async fn collect_grep_results(
    mut child: tokio::process::Child,
    root: &Path,
) -> Result<Vec<SearchResult>, String> {
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| "Failed to read grep output".to_string())?;
    let mut lines = BufReader::new(stdout).lines();
    let mut results = Vec::new();

    while results.len() < MAX_CONTENT_RESULTS {
        let line = lines
            .next_line()
            .await
            .map_err(|error| format!("Failed to read grep output: {}", error))?;
        let Some(line) = line else { break };
        if let Some(result) = parse_grep_result(&line, root) {
            results.push(result);
        }
    }

    let limited = results.len() >= MAX_CONTENT_RESULTS;
    drop(lines);
    if limited {
        let _ = child.kill().await;
    }
    let status = child
        .wait()
        .await
        .map_err(|error| format!("Failed to wait for grep: {}", error))?;
    if !limited && !matches!(status.code(), Some(0) | Some(1)) {
        return Err(format!("grep failed with status {}", status));
    }

    Ok(results)
}

fn search_path(root: &Path, relative: &str) -> String {
    // ripgrep's JSON output ignores `--path-separator`.
    let relative = relative
        .strip_prefix("./")
        .or_else(|| relative.strip_prefix(".\\"))
        .unwrap_or(relative);
    crate::fs_ops::client_path(&root.join(relative))
}

fn parse_grep_result(line: &str, root: &Path) -> Option<SearchResult> {
    // Split at the colon followed by a numeric line number, so filenames
    // containing colons remain valid.
    for (index, character) in line.char_indices() {
        if character != ':' {
            continue;
        }
        let rest = &line[index + 1..];
        let (line_number, text) = rest.split_once(':')?;
        let Ok(line_number) = line_number.parse() else {
            continue;
        };
        return Some(SearchResult {
            path: search_path(root, &line[..index]),
            line: Some(line_number),
            text: Some(text.trim().to_string()),
        });
    }
    None
}

#[cfg(test)]
mod tests {
    use super::{content_search, match_files, FileIndex, Listed, Timing, MAX_ROOTS, TIMING};
    use std::collections::HashMap;
    use std::fs;
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    fn no_boost() -> HashMap<String, u32> {
        HashMap::new()
    }

    #[tokio::test]
    async fn file_search_respects_gitignore_with_fd() {
        if std::process::Command::new("fd")
            .arg("--version")
            .output()
            .is_err()
        {
            return;
        }
        let root = std::env::temp_dir().join(format!("btmux-file-search-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(root.join(".git")).unwrap();
        fs::create_dir_all(root.join("build")).unwrap();
        fs::write(root.join(".gitignore"), "build/\n").unwrap();
        fs::write(root.join("build/needle.txt"), "").unwrap();
        fs::write(root.join(".needle.txt"), "").unwrap();

        let results = FileIndex::new()
            .search("needle", &root, false, &no_boost())
            .await
            .unwrap()
            .results;

        let paths: Vec<_> = results.iter().map(|r| r.path.as_str()).collect();
        assert_eq!(
            paths,
            [crate::fs_ops::client_path(&root.join(".needle.txt"))]
        );
        fs::remove_dir_all(root).unwrap();
    }

    /// Whether files can be listed: Windows has no `find` fallback for fd.
    fn can_list() -> bool {
        cfg!(unix)
            || std::process::Command::new("fd")
                .arg("--version")
                .output()
                .is_ok()
    }

    fn temp_root(name: &str) -> std::path::PathBuf {
        let root = std::env::temp_dir().join(format!("btmux-{}-{}", name, uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        root.canonicalize().unwrap()
    }

    #[tokio::test]
    async fn file_search_finds_new_files_after_a_refresh() {
        if !can_list() {
            return;
        }
        let root = temp_root("file-refresh");
        fs::write(root.join("old.txt"), "").unwrap();
        let index = FileIndex::with_timing(Timing {
            refresh_after: Duration::ZERO,
            refresh_per_listing: 0,
            ..TIMING
        });
        let paths = |search: &super::FileSearch| {
            search
                .results
                .iter()
                .map(|r| r.path.clone())
                .collect::<Vec<_>>()
        };
        let new = crate::fs_ops::client_path(&root.join("new.txt"));

        let first = index
            .search("txt", &root, false, &no_boost())
            .await
            .unwrap();
        assert_eq!(paths(&first).len(), 1);
        assert!(!first.refreshing);
        fs::write(root.join("new.txt"), "").unwrap();
        // The stale list is served while the root is listed again…
        let stale = index
            .search("new", &root, false, &no_boost())
            .await
            .unwrap();
        assert!(!paths(&stale).contains(&new));
        assert!(stale.refreshing);
        // …and a fresh search waits for that listing.
        let fresh = index.search("new", &root, true, &no_boost()).await.unwrap();
        assert_eq!(paths(&fresh), [new]);
        assert!(!fresh.refreshing);
        fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn file_search_keeps_each_roots_index() {
        if !can_list() {
            return;
        }
        let a = temp_root("file-root-a");
        let b = temp_root("file-root-b");
        fs::write(a.join("one.txt"), "").unwrap();
        fs::write(b.join("two.txt"), "").unwrap();
        let index = FileIndex::new();

        let first_a = index.files(&a, false).await.unwrap().0;
        let first_b = index.files(&b, false).await.unwrap().0;
        for _ in 0..3 {
            assert!(Arc::ptr_eq(
                &first_a,
                &index.files(&a, false).await.unwrap().0
            ));
            assert!(Arc::ptr_eq(
                &first_b,
                &index.files(&b, false).await.unwrap().0
            ));
        }
        let results = index
            .search("two", &b, false, &no_boost())
            .await
            .unwrap()
            .results;
        assert_eq!(
            results.iter().map(|r| r.path.as_str()).collect::<Vec<_>>(),
            [crate::fs_ops::client_path(&b.join("two.txt"))]
        );

        // Past `MAX_ROOTS`, the least recently searched root is dropped.
        let others: Vec<_> = (0..MAX_ROOTS).map(|_| temp_root("file-root")).collect();
        for root in &others {
            index.files(root, false).await.unwrap();
        }
        assert!(!Arc::ptr_eq(
            &first_a,
            &index.files(&a, false).await.unwrap().0
        ));
        for root in others.iter().chain([&a, &b]) {
            fs::remove_dir_all(root).unwrap();
        }
    }

    fn matched(result: &super::FileSearchResult) -> String {
        let chars: Vec<char> = result.path.chars().collect();
        result.indices.iter().map(|&i| chars[i as usize]).collect()
    }

    #[test]
    fn file_search_indices_count_code_points() {
        let files = vec!["src/main.rs".to_string(), "README.md".to_string()];
        let results = match_files(&files, "main", "/tmp/ü/", &no_boost());
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].path, "/tmp/ü/src/main.rs");
        assert_eq!(matched(&results[0]), "main");

        // Decomposed (NFD) names: nucleo's own haystack counts graphemes, or bytes.
        let files = vec![
            "cafe\u{301}/main.rs".to_string(),
            "re\u{301}sume\u{301}.md".to_string(),
        ];
        let main = match_files(&files, "main", "/r/", &no_boost());
        assert_eq!(matched(&main[0]), "main");
        let resume = match_files(&files, "sum", "/r/", &no_boost());
        assert_eq!(matched(&resume[0]), "sum");
    }

    #[test]
    fn file_search_boosts_paths() {
        let files = vec!["a/x.rs".to_string(), "b/x.rs".to_string()];
        assert_eq!(
            match_files(&files, "x", "/r/", &no_boost())[0].path,
            "/r/a/x.rs"
        );
        let boost = HashMap::from([("b/x.rs".to_string(), 16)]);
        assert_eq!(match_files(&files, "x", "/r/", &boost)[0].path, "/r/b/x.rs");
    }

    #[test]
    fn file_search_uses_smart_case() {
        let files = vec!["src/Main.rs".to_string(), "src/main.c".to_string()];
        let found = |query| match_files(&files, query, "/r/", &no_boost()).len();
        assert_eq!(found("main"), 2);
        assert_eq!(found("Main"), 1);
    }

    #[test]
    fn uppercase_literals_ignore_escapes() {
        assert!(super::has_uppercase_literal("fooBar"));
        assert!(!super::has_uppercase_literal("foo\\Wbar\\S"));
        assert!(super::has_uppercase_literal("\\\\X"));
    }

    #[tokio::test]
    async fn file_search_boosts_by_result_path() {
        if !can_list() {
            return;
        }
        let root = temp_root("file-boost");
        fs::create_dir_all(root.join("a")).unwrap();
        fs::create_dir_all(root.join("b")).unwrap();
        fs::write(root.join("a/x.rs"), "").unwrap();
        fs::write(root.join("b/x.rs"), "").unwrap();
        let boosted = crate::fs_ops::client_path(&root.join("b/x.rs"));
        let boost = HashMap::from([(boosted.clone(), 16)]);
        let search = FileIndex::new()
            .search("x.rs", &root, false, &boost)
            .await
            .unwrap();
        assert_eq!(search.results[0].path, boosted);
        assert_eq!(search.root, crate::fs_ops::client_path(&root));
        fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn file_search_waits_longer_after_slow_listings() {
        if !can_list() {
            return;
        }
        let root = temp_root("file-slow");
        let index = FileIndex::with_timing(Timing {
            refresh_after: Duration::ZERO,
            ..TIMING
        });
        index.files(&root, false).await.unwrap();
        // As if listing took 10 s and just finished: fresh for 40 s.
        let set = |took: Duration| {
            let mut roots = index.roots.lock().unwrap();
            let entry = roots.last_mut().unwrap();
            let files = entry.files.take().unwrap().files;
            entry.files = Some(Listed {
                files,
                finished: Instant::now(),
                took,
            });
        };
        set(Duration::from_secs(10));
        assert!(!index.files(&root, false).await.unwrap().1);
        set(Duration::ZERO);
        assert!(index.files(&root, false).await.unwrap().1);
        fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn file_search_drops_idle_roots_and_warms_on_empty_queries() {
        let root = temp_root("file-idle");
        fs::write(root.join("one.txt"), "").unwrap();
        let index = FileIndex::with_timing(Timing {
            idle: Duration::from_millis(50),
            sweep: Duration::from_millis(10),
            ..TIMING
        });
        // An empty query lists the root without waiting, and names it.
        let search = index.search("", &root, false, &no_boost()).await.unwrap();
        assert!(search.results.is_empty());
        assert_eq!(search.root, crate::fs_ops::client_path(&root));
        assert_eq!(index.roots.lock().unwrap().len(), 1);
        let mut dropped = false;
        for _ in 0..100 {
            tokio::time::sleep(Duration::from_millis(20)).await;
            if index.roots.lock().unwrap().is_empty() {
                dropped = true;
                break;
            }
        }
        assert!(dropped, "idle root dropped");
        fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn content_search_finds_matching_lines() {
        let root =
            std::env::temp_dir().join(format!("btmux-content-search-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(root.join("nested")).unwrap();
        fs::write(
            root.join("nested/matches.txt"),
            "before\nneedle with context\nafter\n",
        )
        .unwrap();

        let results = content_search("needle", &root).await.unwrap();

        assert_eq!(results.len(), 1);
        assert_eq!(
            results[0].path,
            crate::fs_ops::client_path(&root.join("nested/matches.txt"))
        );
        assert_eq!(results[0].line, Some(2));
        assert_eq!(results[0].text.as_deref(), Some("needle with context"));

        // Smart case.
        fs::write(root.join("upper.txt"), "Needle\n").unwrap();
        assert_eq!(content_search("needle", &root).await.unwrap().len(), 2);
        assert_eq!(content_search("Needle", &root).await.unwrap().len(), 1);
        fs::remove_dir_all(root).unwrap();
    }
}
