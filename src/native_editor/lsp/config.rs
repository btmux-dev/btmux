//! Which language servers a file gets: built-in languages and servers,
//! overridden by `[[language]]` and `[language-server.NAME]`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, RwLock};

use crate::config::{FileConfig, LanguageConfig, LanguageServerConfig};

/// (name, language id, file types, roots, servers)
type Builtin = (
    &'static str,
    &'static str,
    &'static [&'static str],
    &'static [&'static str],
    &'static [&'static str],
);

const JS_ROOTS: &[&str] = &["package.json", "tsconfig.json", "jsconfig.json"];
const C_ROOTS: &[&str] = &["compile_commands.json", ".clangd", "CMakeLists.txt"];

const LANGUAGES: &[Builtin] = &[
    ("rust", "rust", &["rs"], &["Cargo.toml"], &["rust-analyzer"]),
    (
        "typescript",
        "typescript",
        &["ts", "mts", "cts"],
        JS_ROOTS,
        &["typescript-language-server"],
    ),
    (
        "tsx",
        "typescriptreact",
        &["tsx"],
        JS_ROOTS,
        &["typescript-language-server"],
    ),
    (
        "javascript",
        "javascript",
        &["js", "mjs", "cjs"],
        JS_ROOTS,
        &["typescript-language-server"],
    ),
    (
        "jsx",
        "javascriptreact",
        &["jsx"],
        JS_ROOTS,
        &["typescript-language-server"],
    ),
    (
        "python",
        "python",
        &["py", "pyi"],
        &[
            "pyproject.toml",
            "setup.py",
            "setup.cfg",
            "requirements.txt",
        ],
        &["pyright"],
    ),
    ("go", "go", &["go"], &["go.mod", "go.work"], &["gopls"]),
    ("c", "c", &["c", "h"], C_ROOTS, &["clangd"]),
    (
        "cpp",
        "cpp",
        &["cc", "cpp", "cxx", "c++", "hh", "hpp", "hxx", "h++"],
        C_ROOTS,
        &["clangd"],
    ),
    (
        "lua",
        "lua",
        &["lua"],
        &[".luarc.json", ".luarc.jsonc"],
        &["lua-language-server"],
    ),
    ("nix", "nix", &["nix"], &["flake.nix"], &["nil"]),
    (
        "bash",
        "shellscript",
        &["sh", "bash", ".bashrc", ".bash_profile"],
        &[],
        &["bash-language-server"],
    ),
    ("toml", "toml", &["toml"], &[], &["taplo"]),
    (
        "yaml",
        "yaml",
        &["yaml", "yml"],
        &[],
        &["yaml-language-server"],
    ),
    (
        "json",
        "json",
        &["json"],
        &[],
        &["vscode-json-language-server"],
    ),
    (
        "jsonc",
        "jsonc",
        &["jsonc"],
        &[],
        &["vscode-json-language-server"],
    ),
    ("css", "css", &["css"], &[], &["vscode-css-language-server"]),
    (
        "scss",
        "scss",
        &["scss"],
        &[],
        &["vscode-css-language-server"],
    ),
    (
        "less",
        "less",
        &["less"],
        &[],
        &["vscode-css-language-server"],
    ),
    (
        "html",
        "html",
        &["html", "htm"],
        &[],
        &["vscode-html-language-server"],
    ),
];

/// (name, command, args)
const SERVERS: &[(&str, &str, &[&str])] = &[
    ("rust-analyzer", "rust-analyzer", &[]),
    (
        "typescript-language-server",
        "typescript-language-server",
        &["--stdio"],
    ),
    ("pyright", "pyright-langserver", &["--stdio"]),
    ("gopls", "gopls", &[]),
    ("clangd", "clangd", &[]),
    ("lua-language-server", "lua-language-server", &[]),
    ("nil", "nil", &[]),
    ("bash-language-server", "bash-language-server", &["start"]),
    ("taplo", "taplo", &["lsp", "stdio"]),
    ("yaml-language-server", "yaml-language-server", &["--stdio"]),
    (
        "vscode-json-language-server",
        "vscode-json-language-server",
        &["--stdio"],
    ),
    (
        "vscode-css-language-server",
        "vscode-css-language-server",
        &["--stdio"],
    ),
    (
        "vscode-html-language-server",
        "vscode-html-language-server",
        &["--stdio"],
    ),
];

/// Languages and servers, resolved from the config file and the built-ins.
#[derive(Debug, Default)]
pub struct LspConfig {
    pub languages: Vec<LanguageConfig>,
    pub servers: BTreeMap<String, LanguageServerConfig>,
    /// The pane shell, whose `PATH` finds servers missing from btmux's.
    pub shell: String,
}

impl LspConfig {
    pub fn resolve(file: &FileConfig, shell: &str) -> Self {
        let mut servers: BTreeMap<String, LanguageServerConfig> = SERVERS
            .iter()
            .map(|(name, command, args)| {
                (
                    name.to_string(),
                    LanguageServerConfig {
                        command: command.to_string(),
                        args: args.iter().map(|a| a.to_string()).collect(),
                        ..Default::default()
                    },
                )
            })
            .collect();
        servers.extend(file.language_servers.clone());
        let mut languages = file.languages.clone();
        for (name, id, types, roots, names) in LANGUAGES {
            if languages.iter().any(|l| l.name == *name) {
                continue;
            }
            languages.push(LanguageConfig {
                name: name.to_string(),
                language_id: Some(id.to_string()),
                file_types: types.iter().map(|t| t.to_string()).collect(),
                roots: roots.iter().map(|r| r.to_string()).collect(),
                language_servers: names.iter().map(|n| n.to_string()).collect(),
            });
        }
        Self {
            languages,
            servers,
            shell: shell.to_owned(),
        }
    }

    /// The language covering `path`: a whole-name match, else by extension.
    pub fn language(&self, path: &Path) -> Option<&LanguageConfig> {
        let name = path.file_name()?.to_str()?;
        let ext = path.extension().and_then(|e| e.to_str());
        self.languages
            .iter()
            .find(|l| l.file_types.iter().any(|t| t == name))
            .or_else(|| {
                let ext = ext?;
                self.languages
                    .iter()
                    .find(|l| l.file_types.iter().any(|t| t == ext))
            })
    }
}

static CONFIG: LazyLock<RwLock<Arc<LspConfig>>> = LazyLock::new(Default::default);

/// Take a loaded config file; servers already running keep theirs.
pub fn configure(file: &FileConfig, shell: &str) {
    *CONFIG.write().unwrap() = Arc::new(LspConfig::resolve(file, shell));
}

pub fn current() -> Arc<LspConfig> {
    CONFIG.read().unwrap().clone()
}

/// A file's language and the servers (with their roots) to give it.
#[derive(Debug, Clone, PartialEq)]
pub struct Target {
    pub language_id: String,
    pub servers: Vec<(String, PathBuf)>,
    /// The Multipass instance to run them in: the file is in a sandboxed
    /// session's mount, where its toolchain is.
    pub sandbox: Option<String>,
}

/// Sandboxed sessions' mounts and their instance.
static SANDBOXES: LazyLock<RwLock<Vec<(PathBuf, String)>>> = LazyLock::new(Default::default);

/// Record where sandboxed sessions' files are (on every state change).
pub fn set_sandboxes(mounts: Vec<(PathBuf, String)>) {
    let mut current = SANDBOXES.write().unwrap();
    if *current != mounts {
        *current = mounts;
    }
}

fn sandbox(path: &Path) -> Option<String> {
    SANDBOXES
        .read()
        .unwrap()
        .iter()
        .find(|(mount, _)| path.starts_with(mount))
        .map(|(_, instance)| instance.clone())
}

/// Where `path`'s servers run. Reads the filesystem.
pub fn target(config: &LspConfig, path: &Path) -> Option<Target> {
    let language = config.language(path)?;
    let dir = path.parent()?;
    let root = find_root(dir, &language.roots);
    Some(Target {
        language_id: language
            .language_id
            .clone()
            .unwrap_or_else(|| language.name.clone()),
        servers: language
            .language_servers
            .iter()
            .filter(|name| config.servers.contains_key(*name))
            .map(|name| (name.clone(), root.clone()))
            .collect(),
        sandbox: sandbox(path),
    })
}

/// The nearest directory with one of `markers`, looking no higher than the
/// Git worktree; else the worktree, else `dir`.
pub fn find_root(dir: &Path, markers: &[String]) -> PathBuf {
    let mut worktree = None;
    for ancestor in dir.ancestors() {
        if markers.iter().any(|m| ancestor.join(m).exists()) {
            return ancestor.to_path_buf();
        }
        if ancestor.join(".git").exists() {
            worktree = Some(ancestor.to_path_buf());
            break;
        }
    }
    worktree.unwrap_or_else(|| dir.to_path_buf())
}

const URI_SAFE: &[u8] = b"-._~/!$&'()*+,;=:@";

/// A `file://` URI for an absolute path.
pub fn uri(path: &Path) -> String {
    let path = crate::fs_ops::client_path(path);
    let mut out = String::from("file://");
    if !path.starts_with('/') {
        out.push('/'); // C:/…
    }
    for byte in path.bytes() {
        if byte.is_ascii_alphanumeric() || URI_SAFE.contains(&byte) {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

/// The path a `file://` URI names.
pub fn path(uri: &str) -> Option<PathBuf> {
    let rest = uri.strip_prefix("file://")?;
    let rest = rest.strip_prefix("localhost").unwrap_or(rest);
    let mut bytes = Vec::with_capacity(rest.len());
    let mut iter = rest.bytes();
    while let Some(byte) = iter.next() {
        if byte == b'%' {
            let hex = [iter.next()?, iter.next()?];
            bytes.push(u8::from_str_radix(std::str::from_utf8(&hex).ok()?, 16).ok()?);
        } else {
            bytes.push(byte);
        }
    }
    let path = String::from_utf8(bytes).ok()?;
    // file:///C:/… on Windows.
    #[cfg(windows)]
    let path = path.strip_prefix('/').unwrap_or(&path).to_string();
    Some(PathBuf::from(path))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn user_languages_and_servers_override_builtins() {
        let file: FileConfig = toml::from_str(
            r#"
            [language-server.basedpyright]
            command = "basedpyright-langserver"
            args = ["--stdio"]
            [[language]]
            name = "python"
            file-types = ["py"]
            language-servers = ["basedpyright"]
            "#,
        )
        .unwrap();
        let config = LspConfig::resolve(&file, "/bin/sh");
        let python = config.language(Path::new("/x/a.py")).unwrap();
        assert_eq!(python.language_servers, ["basedpyright"]);
        assert_eq!(
            config.language(Path::new("/x/main.rs")).unwrap().name,
            "rust"
        );
        assert_eq!(
            config.language(Path::new("/x/.bashrc")).unwrap().name,
            "bash"
        );
        assert!(config.language(Path::new("/x/notes.txt")).is_none());
        assert_eq!(config.servers["pyright"].command, "pyright-langserver");
    }

    #[test]
    fn roots_stop_at_the_worktree() {
        let dir = std::env::temp_dir().join(format!("btmux-lsp-{}", uuid::Uuid::new_v4()));
        let src = dir.join("repo/crate/src");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::create_dir_all(dir.join("repo/.git")).unwrap();
        std::fs::write(dir.join("Cargo.toml"), "").unwrap();
        let markers = vec!["Cargo.toml".to_string()];
        // Above the worktree doesn't count.
        assert_eq!(find_root(&src, &markers), dir.join("repo"));
        std::fs::write(dir.join("repo/crate/Cargo.toml"), "").unwrap();
        assert_eq!(find_root(&src, &markers), dir.join("repo/crate"));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    #[cfg(unix)]
    fn uris_round_trip() {
        let p = Path::new("/a b/ü#.rs");
        assert_eq!(uri(p), "file:///a%20b/%C3%BC%23.rs");
        assert_eq!(path(&uri(p)).unwrap(), p);
    }
}
