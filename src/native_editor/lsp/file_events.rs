//! `workspace/{will,did}{Create,Rename,Delete}Files`: files created, renamed,
//! or deleted from the file browser or by an edit, for the servers whose
//! `fileOperations` filters match them.

use std::path::{Path, PathBuf};

use regex::Regex;
use serde_json::{json, Value};

use super::config;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Kind {
    Create,
    Rename,
    Delete,
}

impl Kind {
    fn name(self) -> &'static str {
        match self {
            Kind::Create => "Create",
            Kind::Rename => "Rename",
            Kind::Delete => "Delete",
        }
    }

    /// The `fileOperations` capability: `willRename`, ….
    pub fn capability(self, will: bool) -> String {
        format!("{}{}", if will { "will" } else { "did" }, self.name())
    }

    /// `workspace/willRenameFiles`, ….
    pub fn method(self, will: bool) -> String {
        let when = if will { "will" } else { "did" };
        format!("workspace/{when}{}Files", self.name())
    }
}

/// A file or directory: its path before and after (the same unless renamed).
#[derive(Debug, Clone, PartialEq)]
pub struct FileChange {
    pub old: PathBuf,
    pub new: PathBuf,
    pub dir: bool,
}

impl FileChange {
    pub fn new(path: PathBuf, dir: bool) -> Self {
        Self {
            old: path.clone(),
            new: path,
            dir,
        }
    }
}

/// A `FileOperationFilter`.
struct Filter {
    glob: Regex,
    /// Only files (false) or folders (true).
    folder: Option<bool>,
}

impl Filter {
    fn parse(value: &Value) -> Option<Self> {
        if value["scheme"].as_str().is_some_and(|s| s != "file") {
            return None;
        }
        let pattern = &value["pattern"];
        let ignore_case = pattern["options"]["ignoreCase"] == true;
        Some(Self {
            glob: super::watch::glob_regex_with(pattern["glob"].as_str()?, ignore_case)?,
            folder: match pattern["matches"].as_str() {
                Some("folder") => Some(true),
                Some("file") => Some(false),
                _ => None,
            },
        })
    }

    fn matches(&self, path: &Path, dir: bool) -> bool {
        if self.folder.is_some_and(|folder| folder != dir) {
            return false;
        }
        let path = crate::fs_ops::client_path(path);
        self.glob.is_match(&path) || self.glob.is_match(path.trim_start_matches('/'))
    }
}

/// The params for the changes a server registered for (`init` is its
/// `initialize` result), if any.
pub fn params(init: &Value, kind: Kind, will: bool, changes: &[FileChange]) -> Option<Value> {
    let filters: Vec<Filter> = init["capabilities"]["workspace"]["fileOperations"]
        [kind.capability(will)]["filters"]
        .as_array()?
        .iter()
        .filter_map(Filter::parse)
        .collect();
    let files: Vec<Value> = changes
        .iter()
        .filter(|c| {
            filters
                .iter()
                .any(|f| f.matches(&c.old, c.dir) || f.matches(&c.new, c.dir))
        })
        .map(|c| match kind {
            Kind::Rename => json!({ "oldUri": config::uri(&c.old), "newUri": config::uri(&c.new) }),
            _ => json!({ "uri": config::uri(&c.new) }),
        })
        .collect();
    (!files.is_empty()).then(|| json!({ "files": files }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_registered_filters() {
        let init = json!({ "capabilities": { "workspace": { "fileOperations": {
            "willRename": { "filters": [
                { "scheme": "file", "pattern": { "glob": "**/*.RS", "matches": "file", "options": { "ignoreCase": true } } },
                { "pattern": { "glob": "**/src/**", "matches": "folder" } },
            ] },
            "didDelete": { "filters": [{ "scheme": "untitled", "pattern": { "glob": "**" } }] },
        } } } });
        let rename = |old: &str, new: &str, dir| FileChange {
            old: old.into(),
            new: new.into(),
            dir,
        };
        let params = params(
            &init,
            Kind::Rename,
            true,
            &[
                rename("/p/a.rs", "/p/b.rs", false),
                rename("/p/a.txt", "/p/b.txt", false),
                rename("/p/src/m", "/p/src/n", true),
                rename("/p/x.rs", "/p/y.rs", true),
            ],
        )
        .unwrap();
        assert_eq!(
            params,
            json!({ "files": [
                { "oldUri": "file:///p/a.rs", "newUri": "file:///p/b.rs" },
                { "oldUri": "file:///p/src/m", "newUri": "file:///p/src/n" },
            ] })
        );
        assert!(super::params(
            &init,
            Kind::Rename,
            false,
            &[rename("/p/a.rs", "/p/b.rs", false)]
        )
        .is_none());
        let deleted = FileChange::new("/p/a.rs".into(), false);
        assert!(super::params(&init, Kind::Delete, false, &[deleted]).is_none());
        assert_eq!(Kind::Delete.method(false), "workspace/didDeleteFiles");
    }
}
