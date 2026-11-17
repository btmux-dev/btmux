//! The steps of an LSP `WorkspaceEdit`, in order: text edits and file
//! operations (create, rename, delete). File operations are checked as a
//! whole before any runs; deletes go to the trash.

use std::path::{Component, Path, PathBuf};

use serde_json::Value;

use super::lsp;

#[derive(Debug, Clone, PartialEq)]
pub enum Step {
    /// A file's text edits, made against `version` if given.
    Text {
        path: PathBuf,
        edits: Vec<Value>,
        version: Option<u64>,
    },
    Create {
        path: PathBuf,
        overwrite: bool,
        ignore_if_exists: bool,
    },
    Rename {
        from: PathBuf,
        to: PathBuf,
        overwrite: bool,
        ignore_if_exists: bool,
    },
    Delete {
        path: PathBuf,
        recursive: bool,
        ignore_if_not_exists: bool,
    },
}

impl Step {
    pub fn is_text(&self) -> bool {
        matches!(self, Step::Text { .. })
    }
}

/// An absolute path with symlinks resolved, as far as it exists.
pub fn resolve_new(path: &Path) -> Result<PathBuf, String> {
    if !path.is_absolute() || path.components().any(|c| c == Component::ParentDir) {
        return Err(format!("Cannot edit {}", path.display()));
    }
    #[cfg(windows)]
    let canonical = |p: &Path| dunce::canonicalize(p);
    #[cfg(not(windows))]
    let canonical = |p: &Path| p.canonicalize();
    let mut rest = Vec::new();
    for ancestor in path.ancestors() {
        if let Ok(found) = canonical(ancestor) {
            return Ok(rest.iter().rev().fold(found, |p, name| p.join(name)));
        }
        if let Some(name) = ancestor.file_name() {
            rest.push(name.to_owned());
        }
    }
    Err(format!("Cannot edit {}", path.display()))
}

fn path_of(uri: &Value) -> Result<PathBuf, String> {
    let uri = uri.as_str().unwrap_or_default();
    lsp::config::path(uri)
        .ok_or_else(|| format!("Cannot edit {uri}"))
        .and_then(|p| resolve_new(&p))
}

fn flag(change: &Value, name: &str) -> bool {
    change["options"][name] == true
}

/// The steps of `edit`; `documentChanges` is preferred when a server sends both.
pub fn parse(edit: &Value) -> Result<Vec<Step>, String> {
    let mut steps = Vec::new();
    if let Some(changes) = edit["documentChanges"].as_array() {
        for change in changes {
            steps.push(match change["kind"].as_str() {
                Some("create") => Step::Create {
                    path: path_of(&change["uri"])?,
                    overwrite: flag(change, "overwrite"),
                    ignore_if_exists: flag(change, "ignoreIfExists"),
                },
                Some("rename") => Step::Rename {
                    from: path_of(&change["oldUri"])?,
                    to: path_of(&change["newUri"])?,
                    overwrite: flag(change, "overwrite"),
                    ignore_if_exists: flag(change, "ignoreIfExists"),
                },
                Some("delete") => Step::Delete {
                    path: path_of(&change["uri"])?,
                    recursive: flag(change, "recursive"),
                    ignore_if_not_exists: flag(change, "ignoreIfNotExists"),
                },
                Some(kind) => return Err(format!("Unknown file operation {kind}")),
                None => {
                    let document = &change["textDocument"];
                    let version = match &document["version"] {
                        Value::Null => None,
                        v => Some(v.as_u64().ok_or("Invalid document version")?),
                    };
                    Step::Text {
                        path: path_of(&document["uri"])?,
                        edits: change["edits"].as_array().cloned().unwrap_or_default(),
                        version,
                    }
                }
            });
        }
    } else if let Some(changes) = edit["changes"].as_object() {
        for (uri, edits) in changes {
            steps.push(Step::Text {
                path: path_of(&Value::String(uri.clone()))?,
                edits: edits.as_array().cloned().unwrap_or_default(),
                version: None,
            });
        }
    }
    Ok(steps)
}

/// `path` moved from under `from` to under `to`.
pub fn moved(path: &Path, from: &Path, to: &Path) -> Option<PathBuf> {
    let rest = path.strip_prefix(from).ok()?;
    Some(if rest.as_os_str().is_empty() {
        to.to_path_buf()
    } else {
        to.join(rest)
    })
}

/// Whether `path` exists once the steps `before` (that run) have.
fn exists(path: &Path, before: &[&Step]) -> bool {
    let mut path = path.to_path_buf();
    for step in before.iter().rev() {
        match step {
            // With its parent directories.
            Step::Create { path: p, .. } if p.starts_with(&path) => return true,
            Step::Delete { path: p, .. } if path.starts_with(p) => return false,
            Step::Rename { from, to, .. } => {
                if let Some(original) = moved(&path, to, from) {
                    path = original;
                } else if to.starts_with(&path) {
                    return true;
                } else if path.starts_with(from) {
                    return false;
                }
            }
            _ => {}
        }
    }
    path.exists()
}

/// Check that every step can run, in order, and that file operations stay
/// in `sandbox`'s mounts (for a server running there). Reads the filesystem.
pub fn check(steps: &[Step], sandbox: Option<&str>) -> Result<(), String> {
    let home = dirs::home_dir();
    let allowed = |p: &Path| -> Result<(), String> {
        if sandbox.is_some() && lsp::config::sandbox(p).as_deref() != sandbox {
            return Err(format!("{} is outside the sandbox", p.display()));
        }
        Ok(())
    };
    let removable = |p: &Path| -> Result<(), String> {
        allowed(p)?;
        if p.parent().is_none() || home.as_ref().is_some_and(|h| h.starts_with(p)) {
            return Err(format!("Refusing to move or delete {}", p.display()));
        }
        Ok(())
    };
    let mut done: Vec<&Step> = Vec::new();
    for step in steps {
        let runs = match step {
            Step::Text { path, .. } => {
                if !exists(path, &done) {
                    return Err(format!("{} doesn't exist", path.display()));
                }
                false
            }
            Step::Create {
                path,
                overwrite,
                ignore_if_exists,
            } => {
                allowed(path)?;
                let found = exists(path, &done);
                if found && !overwrite && !ignore_if_exists {
                    return Err(format!("{} already exists", path.display()));
                }
                !found || *overwrite
            }
            Step::Rename {
                from,
                to,
                overwrite,
                ignore_if_exists,
            } => {
                removable(from)?;
                allowed(to)?;
                if !exists(from, &done) {
                    return Err(format!("{} doesn't exist", from.display()));
                }
                if to.starts_with(from) && to != from {
                    return Err(format!("Cannot move {} into itself", from.display()));
                }
                let found = exists(to, &done);
                if found && !overwrite && !ignore_if_exists {
                    return Err(format!("{} already exists", to.display()));
                }
                !found || *overwrite
            }
            Step::Delete {
                path,
                ignore_if_not_exists,
                ..
            } => {
                removable(path)?;
                let found = exists(path, &done);
                if !found && !ignore_if_not_exists {
                    return Err(format!("{} doesn't exist", path.display()));
                }
                found
            }
        };
        if runs {
            done.push(step);
        }
    }
    Ok(())
}

fn fail<'a>(what: &'a str, p: &'a Path) -> impl FnOnce(std::io::Error) -> String + 'a {
    move |e| format!("Cannot {what} {}: {e}", p.display())
}

/// Run a file operation; false if its options skip it.
pub fn run(step: &Step) -> Result<bool, String> {
    let parent = |p: &Path| match p.parent() {
        Some(dir) => std::fs::create_dir_all(dir).map_err(fail("create", dir)),
        None => Ok(()),
    };
    match step {
        Step::Text { .. } => Ok(false),
        Step::Create {
            path,
            overwrite,
            ignore_if_exists,
        } => {
            let found = path.exists();
            if found && !overwrite {
                return if *ignore_if_exists {
                    Ok(false)
                } else {
                    Err(format!("{} already exists", path.display()))
                };
            }
            parent(path)?;
            std::fs::OpenOptions::new()
                .write(true)
                .truncate(true)
                .create(true)
                .create_new(!found)
                .open(path)
                .map_err(fail("create", path))?;
            Ok(true)
        }
        Step::Rename {
            from,
            to,
            overwrite,
            ignore_if_exists,
        } => {
            if to.exists() && !overwrite {
                return if *ignore_if_exists {
                    Ok(false)
                } else {
                    Err(format!("{} already exists", to.display()))
                };
            }
            parent(to)?;
            std::fs::rename(from, to).map_err(fail("rename", from))?;
            Ok(true)
        }
        Step::Delete {
            path,
            recursive,
            ignore_if_not_exists,
        } => {
            let Ok(meta) = std::fs::symlink_metadata(path) else {
                return if *ignore_if_not_exists {
                    Ok(false)
                } else {
                    Err(format!("{} doesn't exist", path.display()))
                };
            };
            if meta.is_dir()
                && !recursive
                && std::fs::read_dir(path)
                    .map_err(fail("read", path))?
                    .next()
                    .is_some()
            {
                return Err(format!("{} isn't empty", path.display()));
            }
            trash::delete(path).map_err(|e| format!("Cannot trash {}: {e}", path.display()))?;
            Ok(true)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn dir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("btmux-file-ops-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        dir.canonicalize().unwrap()
    }

    #[test]
    fn parses_and_checks_in_order() {
        let d = dir();
        std::fs::write(d.join("a.rs"), "a").unwrap();
        let uri = |name: &str| lsp::config::uri(&d.join(name));
        let edit = json!({ "documentChanges": [
            { "kind": "create", "uri": uri("new/b.rs") },
            { "textDocument": { "uri": uri("new/b.rs"), "version": null }, "edits": [] },
            { "kind": "rename", "oldUri": uri("new"), "newUri": uri("moved") },
            { "textDocument": { "uri": uri("moved/b.rs"), "version": null }, "edits": [] },
            { "kind": "delete", "uri": uri("a.rs") },
        ] });
        let steps = parse(&edit).unwrap();
        assert_eq!(
            steps[0],
            Step::Create {
                path: d.join("new/b.rs"),
                overwrite: false,
                ignore_if_exists: false
            }
        );
        check(&steps, None).unwrap();
        // Gone after the rename, and deleted twice.
        let mut bad = steps.clone();
        bad.push(Step::Text {
            path: d.join("new/b.rs"),
            edits: vec![],
            version: None,
        });
        assert!(check(&bad, None).unwrap_err().contains("doesn't exist"));
        let mut twice = steps.clone();
        twice.push(steps[4].clone());
        assert!(check(&twice, None).is_err());
        // Creating what exists needs an option.
        let create = |overwrite, ignore_if_exists| Step::Create {
            path: d.join("a.rs"),
            overwrite,
            ignore_if_exists,
        };
        assert!(check(&[create(false, false)], None).is_err());
        check(&[create(false, true)], None).unwrap();
        check(&[create(true, false)], None).unwrap();
        // Sandboxed servers stay in their mounts.
        assert!(check(&steps, Some("btmux-sandbox")).is_err());

        // Not the delete, which would fill the trash.
        for step in &steps[..4] {
            run(step).unwrap();
        }
        assert!(d.join("moved/b.rs").exists());
        assert!(!d.join("new").exists());
        assert_eq!(run(&create(false, true)), Ok(false));
        std::fs::remove_dir_all(d).unwrap();
    }

    #[test]
    fn refuses_home_and_parents() {
        let home = dirs::home_dir().unwrap();
        let delete = |path: PathBuf| Step::Delete {
            path,
            recursive: true,
            ignore_if_not_exists: true,
        };
        assert!(check(&[delete(home.clone())], None).is_err());
        assert!(check(&[delete(PathBuf::from("/"))], None).is_err());
        assert!(resolve_new(Path::new("/tmp/../etc")).is_err());
        assert_eq!(
            moved(Path::new("/a/b/c"), Path::new("/a/b"), Path::new("/x")),
            Some(PathBuf::from("/x/c"))
        );
        assert_eq!(
            moved(Path::new("/a/b"), Path::new("/a/b"), Path::new("/x")),
            Some(PathBuf::from("/x"))
        );
    }
}
