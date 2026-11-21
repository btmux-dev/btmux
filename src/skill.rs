//! `btmux skill`: installs a skill teaching agents the btmux CLI, and a
//! SessionStart hook telling them which pane they run in. The skill is a stub
//! that runs `btmux skill show`, and the hook runs `btmux skill context`, so
//! both take their text from the installed btmux when they run.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use reqwest::Method;
use serde::Serialize;
use serde_json::{json, Value};

use crate::agent_hooks::{self, Target};

/// The guide, also the skill `npx skills add btmux-dev/btmux` installs.
// TODO: document `new-window`/`split-window` `--wait`/`--keep-open` and
// `wait-exit` in it once they land.
const SKILL_SOURCE: &str = include_str!("../skills/btmux/SKILL.md");

/// Marks a skill `btmux skill setup` installed.
const MARKER: &str =
    "<!-- Installed by \"btmux skill setup\". Remove it with \"btmux skill remove\". -->";

/// Bump when the stub changes, so `context` can tell an old one.
const STUB_VERSION: &str = "1";

#[derive(clap::Subcommand, Clone)]
pub enum SkillCommand {
    /// Install the btmux skill and its SessionStart hook into the agents
    /// found here (Claude Code, Codex), or those given with --agent. With
    /// none found, the skill alone goes into ~/.agents/skills, which most
    /// agents read. Safe to run again; it updates what an older btmux
    /// installed.
    Setup {
        /// Only this agent. Repeatable.
        #[arg(long = "agent", value_enum, value_name = "AGENT")]
        agents: Vec<Agent>,
        /// Report what would change without changing anything.
        #[arg(long)]
        dry_run: bool,
        /// Replace a skill named btmux that btmux didn't install (moved to the trash).
        #[arg(long)]
        force: bool,
        /// Print the result as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Report whether the skill and hook are installed for each agent.
    Status {
        /// Print the result as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Remove the skill and hook from agents, leaving a skill btmux didn't
    /// install and every other hook alone.
    Remove {
        /// Only this agent. Repeatable.
        #[arg(long = "agent", value_enum, value_name = "AGENT")]
        agents: Vec<Agent>,
        /// Report what would change without changing anything.
        #[arg(long)]
        dry_run: bool,
        /// Print the result as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Print the guide the skill gives agents.
    Show,
    /// Print the SessionStart hook's context for this pane.
    #[command(hide = true)]
    Context {
        #[arg(long, value_enum)]
        agent: Option<Agent>,
    },
}

#[derive(clap::ValueEnum, Clone, Copy, PartialEq, Eq, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Agent {
    Claude,
    Codex,
}

const AGENTS: [Agent; 2] = [Agent::Claude, Agent::Codex];

impl Agent {
    fn label(self) -> &'static str {
        match self {
            Agent::Claude => "Claude Code",
            Agent::Codex => "Codex",
        }
    }

    fn name(self) -> &'static str {
        match self {
            Agent::Claude => "claude",
            Agent::Codex => "codex",
        }
    }

    fn hooks(self) -> Target {
        match self {
            Agent::Claude => Target::ClaudeCode,
            Agent::Codex => Target::Codex,
        }
    }

    /// How the user invokes the skill by name.
    fn invocation(self) -> &'static str {
        match self {
            Agent::Claude => "/btmux",
            Agent::Codex => "$btmux",
        }
    }

    fn skill_dir(self, home: &Path) -> PathBuf {
        match self {
            Agent::Claude => home.join(".claude").join("skills").join("btmux"),
            // Codex reads the shared directory.
            Agent::Codex => shared_dir(home),
        }
    }

    /// Its config directory exists, or it is on `PATH`.
    fn found(self, home: &Path) -> bool {
        let dir = match self {
            Agent::Claude => home.join(".claude"),
            Agent::Codex => std::env::var_os("CODEX_HOME")
                .filter(|path| !path.is_empty())
                .map(PathBuf::from)
                .unwrap_or_else(|| home.join(".codex")),
        };
        dir.is_dir() || crate::vscode::find_on_path(self.name()).is_some()
    }

    fn hook(self) -> Value {
        let command = format!(
            "\"${{BTMUX_BIN:-btmux}}\" skill context --agent {} 2>/dev/null || true",
            self.name()
        );
        json!({ "hooks": { "SessionStart": [{ "hooks": [
            { "type": "command", "command": command, "timeout": 5 }
        ] }] } })
    }
}

fn shared_dir(home: &Path) -> PathBuf {
    home.join(".agents").join("skills").join("btmux")
}

fn is_skill_hook(handler: &Value) -> bool {
    handler
        .get("command")
        .and_then(Value::as_str)
        .is_some_and(|command| command.contains(" skill context"))
}

/// `SKILL_SOURCE`'s frontmatter and body.
fn split_source() -> (&'static str, &'static str) {
    let rest = SKILL_SOURCE
        .strip_prefix("---\n")
        .expect("SKILL.md starts with frontmatter");
    let end = rest.find("\n---\n").expect("SKILL.md's frontmatter ends");
    (&rest[..end], rest[end + 5..].trim_start())
}

pub fn guide() -> &'static str {
    split_source().1
}

/// The SKILL.md `setup` installs: the guide's frontmatter, pointing at
/// `btmux skill show`.
fn stub() -> String {
    let description = split_source()
        .0
        .lines()
        .find(|line| line.starts_with("description:"))
        .expect("SKILL.md has a description");
    format!(
        "---\nname: btmux\n{description}\nmetadata:\n  btmux-stub: \"{STUB_VERSION}\"\n---\n\n\
         {MARKER}\n\n\
         Run `\"${{BTMUX_BIN:-btmux}}\" skill show` and follow the guide it prints. The guide\n\
         comes from the installed btmux, so it describes the commands that version has.\n\n\
         If the command fails, that btmux predates the guide: use `btmux --help` and\n\
         `btmux COMMAND --help`.\n"
    )
}

#[derive(Serialize, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "snake_case")]
enum Change {
    Created,
    Updated,
    Replaced,
    Unchanged,
    /// A skill btmux didn't install is in the way.
    Conflict,
    Removed,
    /// Nothing to remove.
    Absent,
    /// A skill btmux didn't install, left alone.
    Kept,
    Failed,
}

#[derive(Serialize)]
struct Step {
    agents: Vec<Agent>,
    kind: &'static str,
    path: PathBuf,
    change: Change,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

/// What is installed at a path.
#[derive(Serialize, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "snake_case")]
enum Installed {
    Missing,
    /// As this btmux would install it.
    Current,
    /// By another btmux version.
    Outdated,
    /// A skill btmux didn't install.
    Foreign,
}

fn skill_state(dir: &Path) -> Installed {
    if fs::symlink_metadata(dir).is_err() {
        return Installed::Missing;
    }
    match fs::read_to_string(dir.join("SKILL.md")) {
        Ok(text) if text == stub() => Installed::Current,
        Ok(text) if text.contains(MARKER) => Installed::Outdated,
        _ => Installed::Foreign,
    }
}

fn hook_state(agent: Agent) -> Result<Installed, String> {
    let path = agent.hooks().config_path()?;
    let Some(config) = agent_hooks::read(&path)? else {
        return Ok(Installed::Missing);
    };
    if !agent_hooks::contains(&config, is_skill_hook) {
        return Ok(Installed::Missing);
    }
    let merged = agent_hooks::merged(&path, Some(config.clone()), &agent.hook(), is_skill_hook)?;
    Ok(if merged == config {
        Installed::Current
    } else {
        Installed::Outdated
    })
}

fn home() -> Result<PathBuf, String> {
    dirs::home_dir().ok_or_else(|| "cannot resolve the home directory".into())
}

/// Skill directories with the agents reading each, in order.
fn skill_dirs(agents: &[Agent], home: &Path) -> Vec<(PathBuf, Vec<Agent>)> {
    let mut dirs: Vec<(PathBuf, Vec<Agent>)> = Vec::new();
    for &agent in agents {
        let dir = agent.skill_dir(home);
        match dirs.iter_mut().find(|(path, _)| *path == dir) {
            Some((_, readers)) => readers.push(agent),
            None => dirs.push((dir, vec![agent])),
        }
    }
    dirs
}

fn write_skill(dir: &Path, replace: bool) -> Result<(), String> {
    if replace {
        trash::delete(dir)
            .map_err(|e| format!("cannot move {} to the trash: {e}", dir.display()))?;
    }
    fs::create_dir_all(dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    let file = dir.join("SKILL.md");
    fs::write(&file, stub()).map_err(|e| format!("cannot write {}: {e}", file.display()))
}

fn remove_skill(dir: &Path) -> Result<(), String> {
    let file = dir.join("SKILL.md");
    fs::remove_file(&file).map_err(|e| format!("cannot remove {}: {e}", file.display()))?;
    // Only if nothing else was put there.
    let _ = fs::remove_dir(dir);
    Ok(())
}

fn setup(agents: &[Agent], dry_run: bool, force: bool) -> Result<Vec<Step>, String> {
    let home = home()?;
    let agents: Vec<Agent> = if agents.is_empty() {
        AGENTS.into_iter().filter(|a| a.found(&home)).collect()
    } else {
        agents.to_vec()
    };
    let mut steps = Vec::new();
    let dirs = if agents.is_empty() {
        vec![(shared_dir(&home), Vec::new())]
    } else {
        skill_dirs(&agents, &home)
    };
    for (dir, readers) in dirs {
        let (change, replace) = match skill_state(&dir) {
            Installed::Missing => (Change::Created, false),
            Installed::Current => (Change::Unchanged, false),
            Installed::Outdated => (Change::Updated, false),
            Installed::Foreign if force => (Change::Replaced, true),
            Installed::Foreign => (Change::Conflict, false),
        };
        let error = match change {
            Change::Created | Change::Updated | Change::Replaced if !dry_run => {
                write_skill(&dir, replace).err()
            }
            _ => None,
        };
        steps.push(step(readers, "skill", dir, change, error));
    }
    for agent in agents {
        let (path, change, error) = match setup_hook(agent, dry_run) {
            Ok((path, change)) => (path, change, None),
            Err((path, error)) => (path, Change::Failed, Some(error)),
        };
        steps.push(step(vec![agent], "hook", path, change, error));
    }
    Ok(steps)
}

fn setup_hook(agent: Agent, dry_run: bool) -> Result<(PathBuf, Change), (PathBuf, String)> {
    let path = agent
        .hooks()
        .config_path()
        .map_err(|e| (PathBuf::new(), e))?;
    let fail = |error| (path.clone(), error);
    let config = agent_hooks::read(&path).map_err(fail)?;
    let change = match &config {
        Some(config) if agent_hooks::contains(config, is_skill_hook) => Change::Updated,
        _ => Change::Created,
    };
    let merged =
        agent_hooks::merged(&path, config.clone(), &agent.hook(), is_skill_hook).map_err(fail)?;
    if config.as_ref() == Some(&merged) {
        return Ok((path, Change::Unchanged));
    }
    if !dry_run {
        agent_hooks::write_json(&path, &merged).map_err(fail)?;
    }
    Ok((path, change))
}

fn remove(agents: &[Agent], dry_run: bool) -> Result<Vec<Step>, String> {
    let home = home()?;
    let agents = if agents.is_empty() {
        AGENTS.to_vec()
    } else {
        agents.to_vec()
    };
    let mut steps = Vec::new();
    let mut dirs = skill_dirs(&agents, &home);
    let shared = shared_dir(&home);
    if !dirs.iter().any(|(dir, _)| *dir == shared) && agents.len() == AGENTS.len() {
        dirs.push((shared, Vec::new()));
    }
    for (dir, readers) in dirs {
        let change = match skill_state(&dir) {
            Installed::Missing => Change::Absent,
            Installed::Foreign => Change::Kept,
            Installed::Current | Installed::Outdated => Change::Removed,
        };
        let error = (change == Change::Removed && !dry_run)
            .then(|| remove_skill(&dir).err())
            .flatten();
        steps.push(step(readers, "skill", dir, change, error));
    }
    for agent in agents {
        let result = agent.hooks().config_path().and_then(|path| {
            let change = match agent_hooks::read(&path)? {
                Some(config) => match agent_hooks::without(&config, is_skill_hook) {
                    Some(config) => {
                        // A file holding only this hook goes with it.
                        let empty = config == json!({ "hooks": {} });
                        if !dry_run && empty {
                            fs::remove_file(&path)
                                .map_err(|e| format!("cannot remove {}: {e}", path.display()))?;
                        } else if !dry_run {
                            agent_hooks::write_json(&path, &config)?;
                        }
                        Change::Removed
                    }
                    None => Change::Absent,
                },
                None => Change::Absent,
            };
            Ok((path, change))
        });
        steps.push(match result {
            Ok((path, change)) => step(vec![agent], "hook", path, change, None),
            Err(error) => step(
                vec![agent],
                "hook",
                PathBuf::new(),
                Change::Failed,
                Some(error),
            ),
        });
    }
    Ok(steps)
}

fn step(
    agents: Vec<Agent>,
    kind: &'static str,
    path: PathBuf,
    change: Change,
    error: Option<String>,
) -> Step {
    let change = if error.is_some() {
        Change::Failed
    } else {
        change
    };
    Step {
        agents,
        kind,
        path,
        change,
        error,
    }
}

/// `path` with the home directory as `~`.
fn tilde(path: &Path, home: &Path) -> String {
    match path.strip_prefix(home) {
        Ok(rest) => format!("~/{}", rest.display()),
        Err(_) => path.display().to_string(),
    }
}

fn snake(value: impl Serialize) -> String {
    serde_json::to_value(value)
        .ok()
        .and_then(|v| v.as_str().map(str::to_owned))
        .unwrap_or_default()
}

fn print_steps(action: &str, steps: &[Step], dry_run: bool, json: bool) -> i32 {
    let failed = steps
        .iter()
        .any(|s| matches!(s.change, Change::Failed | Change::Conflict));
    if json {
        let report = json!({ "action": action, "dry_run": dry_run, "steps": steps });
        println!("{}", serde_json::to_string_pretty(&report).unwrap());
        return failed as i32;
    }
    let home = dirs::home_dir().unwrap_or_default();
    for step in steps {
        let agents = if step.agents.is_empty() {
            "shared".to_owned()
        } else {
            step.agents
                .iter()
                .map(|a| a.label())
                .collect::<Vec<_>>()
                .join(", ")
        };
        println!(
            "{:<6} {:<10} {}  ({agents})",
            step.kind,
            snake(step.change),
            tilde(&step.path, &home)
        );
        if let Some(error) = &step.error {
            println!("       {error}");
        }
    }
    if dry_run {
        println!("\nDry run: nothing was changed.");
    }
    if steps.iter().any(|s| s.change == Change::Conflict) {
        println!(
            "\nA skill named btmux that btmux didn't install was left alone. \
             Run again with --force to replace it."
        );
    }
    let changed_hook = steps.iter().any(|s| {
        s.kind == "hook"
            && matches!(
                s.change,
                Change::Created | Change::Updated | Change::Removed
            )
    });
    if changed_hook && !dry_run {
        println!("\nRestart running agents to load the change.");
    }
    failed as i32
}

fn status(json: bool) -> i32 {
    let home = match home() {
        Ok(home) => home,
        Err(error) => return fail(&error),
    };
    let rows: Vec<Value> = AGENTS
        .into_iter()
        .map(|agent| {
            let skill = agent.skill_dir(&home);
            let hook = agent.hooks().config_path().unwrap_or_default();
            let hook_state = hook_state(agent);
            json!({
                "agent": agent,
                "found": agent.found(&home),
                "skill": { "path": skill, "state": skill_state(&skill) },
                "hook": match &hook_state {
                    Ok(state) => json!({ "path": hook, "state": state }),
                    Err(error) => json!({ "path": hook, "state": "error", "error": error }),
                },
            })
        })
        .collect();
    if json {
        println!("{}", serde_json::to_string_pretty(&rows).unwrap());
        return 0;
    }
    let mut stale = false;
    for (agent, row) in AGENTS.into_iter().zip(&rows) {
        let found = if row["found"] == true {
            "found"
        } else {
            "not found"
        };
        println!("{} ({found})", agent.label());
        for kind in ["skill", "hook"] {
            let state = row[kind]["state"].as_str().unwrap_or_default();
            stale |= state != "current";
            let path = PathBuf::from(row[kind]["path"].as_str().unwrap_or_default());
            println!("  {kind:<6} {state:<9} {}", tilde(&path, &home));
            if let Some(error) = row[kind]["error"].as_str() {
                println!("         {error}");
            }
        }
    }
    if stale {
        println!("\nRun \"btmux skill setup\" to install or update them.");
    }
    0
}

/// The hook's text: where the agent runs, as JSON both agents read.
async fn context(profile: Option<&str>, agent: Option<Agent>) -> i32 {
    let Some(pane) = std::env::var("BTMUX_PANE_ID")
        .ok()
        .and_then(|id| uuid::Uuid::parse_str(&id).ok())
    else {
        return 0;
    };
    let panes = crate::api_client::request(
        profile,
        Method::GET,
        "/api/panes",
        None,
        Duration::from_secs(2),
    )
    .await
    .ok()
    .and_then(|body| serde_json::from_str::<Vec<Value>>(&body).ok())
    .unwrap_or_default();
    let place = panes.iter().find(|p| p["pane_id"] == pane.to_string());
    let skill_dir = agent.and_then(|agent| home().ok().map(|home| agent.skill_dir(&home)));
    let outdated = agent.is_some_and(|agent| {
        skill_dir.as_deref().map(skill_state) == Some(Installed::Outdated)
            || hook_state(agent).ok() == Some(Installed::Outdated)
    });
    let text = context_text(pane, place, agent.map(Agent::invocation), outdated);
    let output = json!({ "hookSpecificOutput": {
        "hookEventName": "SessionStart",
        "additionalContext": text,
    } });
    println!("{output}");
    0
}

fn context_text(
    pane: uuid::Uuid,
    place: Option<&Value>,
    invocation: Option<&str>,
    outdated: bool,
) -> String {
    let mut text = String::new();
    match place {
        Some(p) => text.push_str(&format!(
            "You are running in a btmux pane: session {:?} (ID {}), window {} {:?}, pane {}.\n",
            p["session_name"].as_str().unwrap_or_default(),
            p["session_id"].as_str().unwrap_or_default(),
            p["window_index"],
            p["window_name"].as_str().unwrap_or_default(),
            p["pane_index"],
        )),
        None => text.push_str("You are running in a btmux pane.\n"),
    }
    text.push_str(&format!(
        "BTMUX_PANE_ID={pane}\n\
         `BTMUX_PANE_ID` is set in your shell, and btmux commands without `-t` act on \
         that pane. The user may be looking at another session or pane.\n\
         Use the `btmux` command (or \"$BTMUX_BIN\" if btmux isn't on PATH) to open \
         windows and panes, run commands in them, read their output, ask the user, and \
         coordinate other agents. "
    ));
    match invocation {
        Some(name) => text.push_str(&format!("The btmux skill ({name}) has")),
        None => text.push_str("The btmux skill has"),
    }
    text.push_str(
        " instructions and examples; if it isn't installed, run `btmux skill show` to \
         read the same guide.\n",
    );
    if outdated {
        text.push_str(
            "\nThe btmux setup for this agent is out of date. Suggest running \
             `btmux skill setup` to update it.\n",
        );
    }
    text
}

fn fail(error: &str) -> i32 {
    eprintln!("btmux: {error}");
    1
}

/// Runs the command. Returns the exit code.
pub async fn run(profile: Option<&str>, command: &SkillCommand) -> i32 {
    match command {
        SkillCommand::Setup {
            agents,
            dry_run,
            force,
            json,
        } => match setup(agents, *dry_run, *force) {
            Ok(steps) => print_steps("setup", &steps, *dry_run, *json),
            Err(error) => fail(&error),
        },
        SkillCommand::Remove {
            agents,
            dry_run,
            json,
        } => match remove(agents, *dry_run) {
            Ok(steps) => print_steps("remove", &steps, *dry_run, *json),
            Err(error) => fail(&error),
        },
        SkillCommand::Status { json } => status(*json),
        SkillCommand::Show => {
            print!("{}", guide());
            0
        }
        SkillCommand::Context { agent } => context(profile, *agent).await,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stub_keeps_the_guide_description() {
        let stub = stub();
        assert!(stub.starts_with("---\nname: btmux\ndescription: "));
        assert!(stub.contains(MARKER));
        assert!(stub.contains("skill show"));
        assert!(guide().starts_with("# btmux"));
    }

    #[test]
    fn context_hook_merges_once_and_removes_cleanly() {
        let path = Path::new("settings.json");
        let notify: Value = serde_json::from_str(agent_hooks::claude_code()).unwrap();
        let hook = Agent::Claude.hook();
        let once = agent_hooks::merged(path, Some(notify.clone()), &hook, is_skill_hook).unwrap();
        let twice = agent_hooks::merged(path, Some(once.clone()), &hook, is_skill_hook).unwrap();
        assert_eq!(once, twice);
        let start = once["hooks"]["SessionStart"].as_array().unwrap();
        assert_eq!(start.len(), 2, "beside the notify hook: {start:?}");
        assert!(agent_hooks::contains(&once, is_skill_hook));

        let removed = agent_hooks::without(&once, is_skill_hook).unwrap();
        assert_eq!(removed, notify);
        assert!(agent_hooks::without(&removed, is_skill_hook).is_none());
        let alone = agent_hooks::merged(path, None, &hook, is_skill_hook).unwrap();
        let emptied = agent_hooks::without(&alone, is_skill_hook).unwrap();
        assert_eq!(emptied, json!({ "hooks": {} }));
    }

    #[test]
    fn skills_are_told_apart() {
        let dir = std::env::temp_dir().join(format!("btmux-skill-{}", uuid::Uuid::new_v4()));
        assert_eq!(skill_state(&dir), Installed::Missing);
        write_skill(&dir, false).unwrap();
        assert_eq!(skill_state(&dir), Installed::Current);
        fs::write(dir.join("SKILL.md"), format!("old\n{MARKER}\n")).unwrap();
        assert_eq!(skill_state(&dir), Installed::Outdated);
        fs::write(dir.join("SKILL.md"), "---\nname: btmux\n---\nmine\n").unwrap();
        assert_eq!(skill_state(&dir), Installed::Foreign);
        fs::write(dir.join("SKILL.md"), stub()).unwrap();
        remove_skill(&dir).unwrap();
        assert!(!dir.exists());
    }

    #[test]
    fn context_names_the_pane() {
        let pane = uuid::Uuid::new_v4();
        let place = json!({
            "session_name": "work", "session_id": "s", "window_index": 1,
            "window_name": "edit", "pane_index": 0,
        });
        let text = context_text(pane, Some(&place), Some("/btmux"), true);
        assert!(text.starts_with(
            "You are running in a btmux pane: session \"work\" (ID s), window 1 \"edit\", pane 0.\n"
        ));
        assert!(text.contains(&format!("BTMUX_PANE_ID={pane}")));
        assert!(text.contains("(/btmux)"));
        assert!(text.contains("out of date"));
    }
}
