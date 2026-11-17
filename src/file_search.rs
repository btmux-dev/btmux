use nucleo::pattern::{CaseMatching, Normalization, Pattern};
use nucleo::{Config, Matcher, Utf32Str};
use serde::{Deserialize, Serialize};
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tokio::sync::watch;

const MAX_CONTENT_RESULTS: usize = 100;
const MAX_FILE_RESULTS: usize = 100;
/// Roots kept indexed, least recently searched dropped first.
const MAX_ROOTS: usize = 4;
/// A search of an older index relists its root in the background.
const REFRESH_AFTER: Duration = Duration::from_secs(5);

#[derive(Debug, Serialize, Deserialize)]
pub struct FileSearchResult {
    pub path: String,
    /// Matched characters (not bytes) of `path`.
    pub indices: Vec<u32>,
    pub score: u32,
}

#[derive(Debug, Default, Serialize)]
pub struct FileSearch {
    pub results: Vec<FileSearchResult>,
    /// The root is being listed again; a `fresh` search waits for it.
    pub refreshing: bool,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct SearchResult {
    pub path: String,
    pub line: Option<u64>,
    pub text: Option<String>,
}

type Files = Arc<Vec<String>>;
type Listing = watch::Receiver<Option<Result<Files, String>>>;

/// Fuzzy path search over cached file lists of a few roots. A stale list is
/// served while its root is listed again, as `GitTracker` does.
pub struct FileIndex {
    /// Least recently searched first.
    roots: Arc<Mutex<Vec<Root>>>,
    refresh_after: Duration,
}

struct Root {
    path: PathBuf,
    /// Paths relative to `path`, and when their listing started.
    files: Option<(Files, Instant)>,
    /// A listing in flight.
    listing: Option<Listing>,
}

impl FileIndex {
    pub fn new() -> Self {
        Self::with_refresh(REFRESH_AFTER)
    }

    fn with_refresh(refresh_after: Duration) -> Self {
        Self {
            roots: Arc::default(),
            refresh_after,
        }
    }

    /// Matches of `query` under `root`. A stale list is searched while it is
    /// listed again (`refreshing`); `fresh` waits for that listing instead.
    pub async fn search(
        &self,
        query: &str,
        root: &Path,
        fresh: bool,
    ) -> Result<FileSearch, String> {
        if query.trim().is_empty() {
            return Ok(FileSearch::default());
        }
        let (files, refreshing) = self.files(root, fresh).await?;
        let query = query.to_string();
        let root = crate::fs_ops::client_path(root);
        let results = tokio::task::spawn_blocking(move || match_files(&files, &query, &root))
            .await
            .map_err(|error| format!("File search failed: {}", error))?;
        Ok(FileSearch {
            results,
            refreshing,
        })
    }

    /// `root`'s files, listed now if it has none, and whether they are being
    /// listed again. `fresh` waits for a listing in flight.
    async fn files(&self, root: &Path, fresh: bool) -> Result<(Files, bool), String> {
        let (mut listing, stale) = {
            let mut roots = self.roots.lock().unwrap();
            let entry = match roots.iter().position(|r| r.path == root) {
                Some(at) => roots.remove(at),
                None => Root {
                    path: root.to_path_buf(),
                    files: None,
                    listing: None,
                },
            };
            roots.push(entry);
            if roots.len() > MAX_ROOTS {
                roots.remove(0);
            }
            let entry = roots.last_mut().unwrap();
            match &entry.files {
                Some((files, listed)) => {
                    let files = files.clone();
                    if listed.elapsed() >= self.refresh_after && entry.listing.is_none() {
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
                        Ok(files) => entry.files = Some((files.clone(), started)),
                        // Keep the last list, and don't retry on every search.
                        Err(_) => {
                            if let Some((_, listed)) = &mut entry.files {
                                *listed = Instant::now();
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

/// The best matches of `query` among `files`, as client paths under `root`.
fn match_files(files: &[String], query: &str, root: &str) -> Vec<FileSearchResult> {
    let pattern = Pattern::parse(query, CaseMatching::Ignore, Normalization::Smart);
    let mut matcher = Matcher::new(Config::DEFAULT.match_paths());
    let mut buf = Vec::new();
    let mut matches: Vec<(u32, &String)> = files
        .iter()
        .filter_map(|file| {
            let score = pattern.score(Utf32Str::new(file, &mut buf), &mut matcher)?;
            Some((score, file))
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

    let prefix = if root.ends_with('/') {
        root.to_string()
    } else {
        format!("{}/", root)
    };
    let offset = prefix.chars().count() as u32;
    let mut indices = Vec::new();
    matches
        .into_iter()
        .map(|(score, file)| {
            indices.clear();
            pattern.indices(Utf32Str::new(file, &mut buf), &mut matcher, &mut indices);
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
    grep.args(["-R", "-n", "-H", "-I", "-e"])
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
    use super::{content_search, match_files, FileIndex, MAX_ROOTS};
    use std::fs;
    use std::sync::Arc;
    use std::time::Duration;

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
            .search("needle", &root, false)
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

    fn temp_root(name: &str) -> std::path::PathBuf {
        let root = std::env::temp_dir().join(format!("btmux-{}-{}", name, uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        root.canonicalize().unwrap()
    }

    #[tokio::test]
    async fn file_search_finds_new_files_after_a_refresh() {
        let root = temp_root("file-refresh");
        fs::write(root.join("old.txt"), "").unwrap();
        let index = FileIndex::with_refresh(Duration::ZERO);
        let paths = |search: &super::FileSearch| {
            search
                .results
                .iter()
                .map(|r| r.path.clone())
                .collect::<Vec<_>>()
        };
        let new = crate::fs_ops::client_path(&root.join("new.txt"));

        let first = index.search("txt", &root, false).await.unwrap();
        assert_eq!(paths(&first).len(), 1);
        assert!(!first.refreshing);
        fs::write(root.join("new.txt"), "").unwrap();
        // The stale list is served while the root is listed again…
        let stale = index.search("new", &root, false).await.unwrap();
        assert!(!paths(&stale).contains(&new));
        assert!(stale.refreshing);
        // …and a fresh search waits for that listing.
        let fresh = index.search("new", &root, true).await.unwrap();
        assert_eq!(paths(&fresh), [new]);
        assert!(!fresh.refreshing);
        fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn file_search_keeps_each_roots_index() {
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
        let results = index.search("two", &b, false).await.unwrap().results;
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

    #[test]
    fn file_search_indices_count_characters() {
        let files = vec!["src/main.rs".to_string(), "README.md".to_string()];
        let results = match_files(&files, "main", "/tmp/ü");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].path, "/tmp/ü/src/main.rs");
        let chars: Vec<char> = results[0].path.chars().collect();
        let matched: String = results[0]
            .indices
            .iter()
            .map(|&i| chars[i as usize])
            .collect();
        assert_eq!(matched, "main");
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
        fs::remove_dir_all(root).unwrap();
    }
}
