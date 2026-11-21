use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{Map, Value};

/// Ready-to-paste hook configurations for agent harnesses that can run a
/// command with the lifecycle event JSON on stdin.
pub fn claude_code() -> &'static str {
    include_str!("../extras/claude-code/hooks.json")
}

pub fn codex() -> &'static str {
    include_str!("../extras/codex/hooks.json")
}

pub fn gemini_cli() -> &'static str {
    include_str!("../extras/gemini-cli/hooks.json")
}

#[derive(Clone, Copy)]
pub enum Target {
    ClaudeCode,
    Codex,
    GeminiCli,
}

impl Target {
    fn snippet(self) -> &'static str {
        match self {
            Self::ClaudeCode => claude_code(),
            Self::Codex => codex(),
            Self::GeminiCli => gemini_cli(),
        }
    }

    /// The status hooks, parsed.
    pub fn generated(self) -> Value {
        serde_json::from_str(self.snippet()).expect("embedded hooks are JSON")
    }

    pub fn config_path(self) -> Result<PathBuf, String> {
        let home = dirs::home_dir().ok_or("cannot resolve the home directory")?;
        Ok(match self {
            Self::ClaudeCode => claude_dir(&home).join("settings.json"),
            Self::Codex => codex_dir(&home).join("hooks.json"),
            Self::GeminiCli => home.join(".gemini").join("settings.json"),
        })
    }
}

/// Claude Code's config directory: `CLAUDE_CONFIG_DIR`, else `~/.claude`.
pub fn claude_dir(home: &Path) -> PathBuf {
    std::env::var_os("CLAUDE_CONFIG_DIR")
        .filter(|path| !path.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".claude"))
}

/// Codex's config directory: `CODEX_HOME`, else `~/.codex`.
pub fn codex_dir(home: &Path) -> PathBuf {
    std::env::var_os("CODEX_HOME")
        .filter(|path| !path.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".codex"))
}

/// Ends every status hook's command, marking it as btmux's.
const NOTIFY_MARKER: &str = " # btmux-notify";

/// Status hook commands from before they were marked: today's without the
/// marker, and every earlier release's.
static UNMARKED: std::sync::LazyLock<Vec<String>> = std::sync::LazyLock::new(|| {
    [Target::ClaudeCode, Target::Codex, Target::GeminiCli]
        .into_iter()
        .flat_map(|target| commands(&target.generated()))
        .filter_map(|command| command.strip_suffix(NOTIFY_MARKER).map(str::to_owned))
        .chain(EARLIER.iter().map(|command| command.to_string()))
        .collect()
});

/// Status hook commands earlier releases installed, before the agent pid
/// and timeouts. Exact, so a hook someone wrote to `/notify` stays theirs.
const EARLIER: &[&str] = &[
    r#"curl -s -X POST -H 'Content-Type: application/json' --data-binary @- "${BTMUX_API_URL}/api/panes/${BTMUX_PANE_ID}/notify" >/dev/null"#,
    r#"curl -sS --fail -X POST -H 'Content-Type: application/json' --data-binary @- "${BTMUX_API_URL}/api/panes/${BTMUX_PANE_ID}/notify" >/dev/null 2>&1 || true; printf '{}'"#,
    r#"curl -H "Authorization: Bearer ${BTMUX_AUTH_TOKEN}" -sS --fail -X POST -H 'Content-Type: application/json' --data-binary @- "${BTMUX_API_URL}/api/panes/${BTMUX_PANE_ID}/notify" >/dev/null 2>&1 || true"#,
    r#"curl -H "Authorization: Bearer ${BTMUX_AUTH_TOKEN}" -sS --fail -X POST -H 'Content-Type: application/json' --data-binary @- "${BTMUX_API_URL}/api/panes/${BTMUX_PANE_ID}/notify" >/dev/null 2>&1 || true; printf '{}'"#,
];

fn commands(config: &Value) -> Vec<String> {
    let Some(hooks) = config.get("hooks").and_then(Value::as_object) else {
        return Vec::new();
    };
    hooks
        .values()
        .filter_map(Value::as_array)
        .flatten()
        .filter_map(|group| group.get("hooks").and_then(Value::as_array))
        .flatten()
        .filter_map(|handler| handler.get("command").and_then(Value::as_str))
        .map(str::to_owned)
        .collect()
}

/// A status hook btmux installed: marked, or exactly one it used to install.
pub fn is_btmux_hook(handler: &Value) -> bool {
    handler
        .get("command")
        .and_then(Value::as_str)
        .is_some_and(|command| {
            command.ends_with(NOTIFY_MARKER) || UNMARKED.iter().any(|old| old == command)
        })
}

/// The JSON config at `path`; `None` if there is none.
pub fn read(path: &Path) -> Result<Option<Value>, String> {
    match fs::read_to_string(path) {
        Ok(contents) => serde_json::from_str(&contents).map(Some).map_err(|error| {
            format!(
                "cannot parse {}: {error}; file was left unchanged",
                path.display()
            )
        }),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(format!("cannot read {}: {error}", path.display())),
    }
}

/// `config` (read from `path`) with `generated`'s hooks in place of the ones
/// `is_ours` picks out, which are kept where they were.
pub fn merged(
    path: &Path,
    config: Option<Value>,
    generated: &Value,
    is_ours: fn(&Value) -> bool,
) -> Result<Value, String> {
    let generated_hooks = generated
        .get("hooks")
        .and_then(Value::as_object)
        .ok_or("embedded hooks have no hooks object")?;
    let mut config = config.unwrap_or_else(|| generated.clone());
    let config_object = config.as_object_mut().ok_or_else(|| {
        format!(
            "{} must contain a JSON object; file was left unchanged",
            path.display()
        )
    })?;
    let generated_object = generated
        .as_object()
        .expect("generated hook JSON was checked as an object");
    for (key, value) in generated_object {
        if key != "hooks" {
            config_object
                .entry(key.clone())
                .or_insert_with(|| value.clone());
        }
    }

    let hooks = config_object
        .entry("hooks".to_string())
        .or_insert_with(|| Value::Object(Map::new()));
    let hooks_object = hooks.as_object_mut().ok_or_else(|| {
        format!(
            "{} has a non-object `hooks` value; file was left unchanged",
            path.display()
        )
    })?;
    for (event, generated_groups) in generated_hooks {
        merge_event_hooks(hooks_object, event, generated_groups, is_ours).map_err(|error| {
            format!(
                "cannot merge {} hooks in {}: {error}",
                event,
                path.display()
            )
        })?;
    }
    Ok(config)
}

/// `config` without the hooks `is_ours` picks out in `generated`'s events,
/// dropping groups and events they leave empty; `None` if it had none.
pub fn without(config: &Value, generated: &Value, is_ours: fn(&Value) -> bool) -> Option<Value> {
    let events = generated.get("hooks")?.as_object()?;
    let mut config = config.clone();
    let hooks = config.get_mut("hooks")?.as_object_mut()?;
    let mut found = false;
    hooks.retain(|event, groups| {
        let Some(groups) = groups.as_array_mut().filter(|_| events.contains_key(event)) else {
            return true;
        };
        groups.retain_mut(|group| {
            let Some(handlers) = group.get_mut("hooks").and_then(Value::as_array_mut) else {
                return true;
            };
            let before = handlers.len();
            handlers.retain(|handler| !is_ours(handler));
            found |= handlers.len() != before;
            !(handlers.is_empty() && before > 0)
        });
        !groups.is_empty()
    });
    found.then_some(config)
}

/// Whether `config` has a hook `is_ours` picks out in `generated`'s events.
pub fn contains(config: &Value, generated: &Value, is_ours: fn(&Value) -> bool) -> bool {
    without(config, generated, is_ours).is_some()
}

/// Writes `config` as pretty JSON.
pub fn write_json(path: &Path, config: &Value) -> Result<(), String> {
    let mut contents = serde_json::to_string_pretty(config)
        .map_err(|error| format!("cannot serialize {}: {error}", path.display()))?;
    contents.push('\n');
    write_config(path, &contents)
}

fn merge_event_hooks(
    destination: &mut Map<String, Value>,
    event: &str,
    generated_groups: &Value,
    is_ours: fn(&Value) -> bool,
) -> Result<(), String> {
    let generated_groups = generated_groups
        .as_array()
        .ok_or_else(|| "generated event hooks must be an array".to_string())?;
    let generated_handlers: Vec<Value> = generated_groups
        .iter()
        .filter_map(|group| group.get("hooks").and_then(Value::as_array))
        .flatten()
        .filter(|handler| is_ours(handler))
        .cloned()
        .collect();
    let replacement = generated_handlers
        .first()
        .ok_or_else(|| "generated event has no btmux command hook".to_string())?;

    let existing = destination
        .remove(event)
        .unwrap_or_else(|| Value::Array(Vec::new()));
    let mut groups = existing
        .as_array()
        .cloned()
        .ok_or_else(|| "configured event hooks must be an array".to_string())?;
    let mut found_btmux = false;
    let mut merged_groups = Vec::with_capacity(groups.len() + generated_groups.len());

    for mut group in groups.drain(..) {
        let mut empty_group = false;
        if let Some(group_object) = group.as_object_mut() {
            if let Some(handlers) = group_object.get_mut("hooks") {
                let handlers = handlers
                    .as_array_mut()
                    .ok_or_else(|| "each hook group's `hooks` must be an array".to_string())?;
                let old_handlers = std::mem::take(handlers);
                let mut kept_handlers = Vec::with_capacity(old_handlers.len());
                for handler in old_handlers {
                    if is_ours(&handler) {
                        if !found_btmux {
                            kept_handlers.push(replacement.clone());
                            found_btmux = true;
                        }
                    } else {
                        kept_handlers.push(handler);
                    }
                }
                empty_group = kept_handlers.is_empty();
                *handlers = kept_handlers;
            }
        }
        if !empty_group {
            merged_groups.push(group);
        }
    }

    if !found_btmux {
        merged_groups.extend(generated_groups.iter().cloned());
    }
    destination.insert(event.to_string(), Value::Array(merged_groups));
    Ok(())
}

fn write_config(path: &Path, contents: &str) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("cannot create {}: {error}", parent.display()))?;
    }

    // Resolve symlinks so dotfile-managed configs keep their link in place.
    let destination = if fs::symlink_metadata(path)
        .map(|metadata| metadata.file_type().is_symlink())
        .unwrap_or(false)
    {
        fs::canonicalize(path)
            .map_err(|error| format!("cannot resolve config symlink {}: {error}", path.display()))?
    } else {
        path.to_path_buf()
    };
    let parent = destination
        .parent()
        .unwrap_or_else(|| std::path::Path::new("."));
    let file_name = destination
        .file_name()
        .ok_or_else(|| format!("invalid config path {}", destination.display()))?;
    let mut temporary_name = file_name.to_os_string();
    temporary_name.push(format!(".btmux-{}.tmp", std::process::id()));
    let temporary = parent.join(temporary_name);
    let permissions = fs::metadata(&destination)
        .ok()
        .map(|metadata| metadata.permissions());

    fs::write(&temporary, contents)
        .map_err(|error| format!("cannot write {}: {error}", temporary.display()))?;
    if let Some(permissions) = permissions {
        if let Err(error) = fs::set_permissions(&temporary, permissions) {
            let _ = fs::remove_file(&temporary);
            return Err(format!(
                "cannot preserve permissions on {}: {error}",
                path.display()
            ));
        }
    }
    if let Err(error) = fs::rename(&temporary, &destination) {
        let _ = fs::remove_file(&temporary);
        return Err(format!("cannot update {}: {error}", path.display()));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_hook_snippets_are_valid_json() {
        for snippet in [claude_code(), codex(), gemini_cli()] {
            let parsed: serde_json::Value = serde_json::from_str(snippet).unwrap();
            let hooks = parsed["hooks"].as_object().unwrap();
            for event in ["SessionStart", "SessionEnd"] {
                assert!(hooks.contains_key(event));
            }
            for groups in hooks.values() {
                for group in groups.as_array().unwrap() {
                    for handler in group["hooks"].as_array().unwrap() {
                        let command = handler["command"].as_str().unwrap();
                        assert!(command.contains("X-Btmux-Agent-Pid"));
                        assert!(command.contains("--max-time 2"));
                        assert!(is_btmux_hook(handler));
                        let unmarked = command.strip_suffix(NOTIFY_MARKER).unwrap();
                        assert!(is_btmux_hook(&serde_json::json!({ "command": unmarked })));
                    }
                }
            }
        }
        let codex: serde_json::Value = serde_json::from_str(codex()).unwrap();
        for event in [
            "SessionStart",
            "UserPromptSubmit",
            "PermissionRequest",
            "Stop",
        ] {
            assert_ne!(codex["hooks"][event][0]["hooks"][0]["async"], true);
        }
        assert!(codex["hooks"].get("PostToolUse").is_some());
        assert!(codex["hooks"].get("Interrupt").is_some());
        let claude: serde_json::Value = serde_json::from_str(claude_code()).unwrap();
        for event in ["PostToolUse", "PostToolUseFailure", "PermissionDenied"] {
            assert!(claude["hooks"].get(event).is_some());
        }
        let own = serde_json::json!({ "command": "curl \"${BTMUX_API_URL}/api/panes/${BTMUX_PANE_ID}/notify\"" });
        assert!(!is_btmux_hook(&own));
    }

    /// An earlier release's hook is replaced, not joined by a second one.
    #[test]
    fn earlier_releases_hooks_are_replaced() {
        let generated: Value = serde_json::from_str(claude_code()).unwrap();
        for command in EARLIER {
            assert!(is_btmux_hook(&serde_json::json!({ "command": command })));
            let installed = serde_json::json!({ "hooks": { "Stop": [
                { "hooks": [{ "type": "command", "command": command }] }
            ]}});
            let merged = merged(
                Path::new("settings.json"),
                Some(installed),
                &generated,
                is_btmux_hook,
            )
            .unwrap();
            let stop: Vec<&str> = merged["hooks"]["Stop"]
                .as_array()
                .unwrap()
                .iter()
                .flat_map(|group| group["hooks"].as_array().unwrap())
                .filter_map(|handler| handler["command"].as_str())
                .collect();
            assert_eq!(stop.len(), 1, "{stop:?}");
            assert!(stop[0].ends_with(NOTIFY_MARKER));
        }
    }
}
