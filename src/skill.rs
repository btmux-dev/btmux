//! `btmux skill`: sets agents up to use btmux. Each agent gets the status
//! hooks that report its state, and Claude Code, Codex, and Antigravity also
//! a skill teaching them the btmux CLI and a SessionStart hook telling them
//! which pane they run in. The skill is a stub that runs `btmux skill show`, and
//! the hook runs `btmux skill context`, so both take their text from the
//! installed btmux when they run.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use reqwest::Method;
use serde::Serialize;
use serde_json::{json, Value};

use crate::agent_hooks::{self, Target};

/// The guide, also the skill `npx skills add btmux-dev/btmux` installs.
const SKILL_SOURCE: &str = include_str!("../skills/btmux/SKILL.md");

/// Marks a skill `btmux skill setup` installed.
const MARKER: &str =
    "<!-- Installed by \"btmux skill setup\". Remove it with \"btmux skill remove\". -->";

/// Ends the context hook's command, marking it as btmux's.
const CONTEXT_MARKER: &str = " # btmux-context";

/// Bump when the stub changes, so `context` can tell an old one.
const STUB_VERSION: &str = "1";

#[derive(clap::Subcommand, Clone)]
pub enum SkillCommand {
    /// Set up the agents found here (Claude Code, Codex, Gemini CLI,
    /// Antigravity CLI), or those given with --agent: hooks that report their
    /// state to btmux, and for all but Gemini CLI the btmux skill and a hook
    /// telling them their pane. With no agent found, the skill alone goes into ~/.agents/skills,
    /// which most agents read. Safe to run again; it updates what an older
    /// btmux installed.
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
    /// Report what is installed for each agent.
    Status {
        /// Print the result as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Remove what setup installed, leaving a skill btmux didn't install and
    /// every other hook alone.
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
    Gemini,
    Agy,
}

const AGENTS: [Agent; 4] = [Agent::Claude, Agent::Codex, Agent::Gemini, Agent::Agy];

impl Agent {
    fn label(self) -> &'static str {
        match self {
            Agent::Claude => "Claude Code",
            Agent::Codex => "Codex",
            Agent::Gemini => "Gemini CLI",
            Agent::Agy => "Antigravity CLI",
        }
    }

    fn name(self) -> &'static str {
        match self {
            Agent::Claude => "claude",
            Agent::Codex => "codex",
            Agent::Gemini => "gemini",
            Agent::Agy => "agy",
        }
    }

    fn target(self) -> Target {
        match self {
            Agent::Claude => Target::ClaudeCode,
            Agent::Codex => Target::Codex,
            Agent::Gemini => Target::GeminiCli,
            Agent::Agy => Target::Antigravity,
        }
    }

    /// How the user invokes the skill by name.
    fn invocation(self) -> &'static str {
        match self {
            Agent::Claude | Agent::Agy => "/btmux",
            _ => "$btmux",
        }
    }

    /// Where it reads the skill; `None` if btmux installs none for it.
    fn skill_dir(self, home: &Path) -> Option<PathBuf> {
        match self {
            Agent::Claude => Some(agent_hooks::claude_dir(home).join("skills").join("btmux")),
            // Codex reads the shared directory.
            Agent::Codex => Some(shared_dir(home)),
            Agent::Gemini => None,
            Agent::Agy => Some(
                agent_hooks::antigravity_dir(home)
                    .join("skills")
                    .join("btmux"),
            ),
        }
    }

    /// It left its config behind, or it is on `PATH`.
    fn found(self, home: &Path) -> bool {
        self.configured(home) || crate::vscode::find_on_path(self.name()).is_some()
    }

    fn configured(self, home: &Path) -> bool {
        let gemini = home.join(".gemini");
        match self {
            Agent::Claude => agent_hooks::claude_dir(home).is_dir(),
            Agent::Codex => agent_hooks::codex_dir(home).is_dir(),
            // Antigravity uses `~/.gemini` too, and setup writes its
            // `settings.json`, so only files Gemini CLI writes itself count.
            Agent::Gemini => GEMINI_FILES.iter().any(|file| gemini.join(file).is_file()),
            Agent::Agy => gemini.join("antigravity-cli").is_dir(),
        }
    }

    /// The SessionStart hook naming the pane; `None` without a skill.
    fn context_hook(self) -> Option<Hooks> {
        self.skill_dir(Path::new("")).as_ref()?;
        // Antigravity wants a JSON object even when there is nothing to say.
        let fallback = match self {
            Agent::Agy => "printf '{}'",
            _ => "true",
        };
        let command = format!(
            "\"${{BTMUX_BIN:-btmux}}\" skill context --agent {} 2>/dev/null || {fallback}{CONTEXT_MARKER}",
            self.name()
        );
        let handler = json!({ "type": "command", "command": command, "timeout": 5 });
        Some(match self {
            Agent::Agy => Hooks::Named(json!({ "btmux-context": { "SessionStart": [handler] } })),
            _ => Hooks::Merged {
                generated: json!({ "hooks": { "SessionStart": [{ "hooks": [handler] }] } }),
                is_ours: is_context_hook,
            },
        })
    }

    /// The hooks reporting its state.
    fn status_hooks(self) -> Hooks {
        let generated = self.target().generated();
        match self {
            Agent::Agy => Hooks::Named(generated),
            _ => Hooks::Merged {
                generated,
                is_ours: agent_hooks::is_btmux_hook,
            },
        }
    }
}

/// Hooks setup puts in an agent's config file.
enum Hooks {
    /// Handlers in `hooks.<event>` groups, among the user's: `is_ours` picks
    /// out the ones btmux installed.
    Merged {
        generated: Value,
        is_ours: fn(&Value) -> bool,
    },
    /// Antigravity's: whole hooks named at the top level, btmux's under its
    /// own names.
    Named(Value),
}

impl Hooks {
    /// `config` with these hooks; whether it had them before.
    fn merged(&self, path: &Path, config: Option<Value>) -> Result<(Value, bool), String> {
        Ok(match self {
            Hooks::Merged { generated, is_ours } => {
                let had = config
                    .as_ref()
                    .is_some_and(|c| agent_hooks::contains(c, generated, *is_ours));
                (agent_hooks::merged(path, config, generated, *is_ours)?, had)
            }
            Hooks::Named(generated) => {
                let had = config
                    .as_ref()
                    .is_some_and(|c| agent_hooks::without_named(c, generated).is_some());
                (agent_hooks::with_named(path, config, generated)?, had)
            }
        })
    }

    /// `config` without these hooks; `None` if it had none.
    fn without(&self, config: &Value) -> Option<Value> {
        match self {
            Hooks::Merged { generated, is_ours } => {
                agent_hooks::without(config, generated, *is_ours)
            }
            Hooks::Named(generated) => agent_hooks::without_named(config, generated),
        }
    }
}

/// Files in `~/.gemini` that Gemini CLI writes and Antigravity doesn't.
const GEMINI_FILES: &[&str] = &[
    "installation_id",
    "oauth_creds.json",
    "google_accounts.json",
];

fn shared_dir(home: &Path) -> PathBuf {
    home.join(".agents").join("skills").join("btmux")
}

fn is_context_hook(handler: &Value) -> bool {
    handler
        .get("command")
        .and_then(Value::as_str)
        .is_some_and(|command| command.ends_with(CONTEXT_MARKER))
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

/// What setup installs.
#[derive(Serialize, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "snake_case")]
enum Part {
    Skill,
    /// The SessionStart hook naming the pane.
    ContextHook,
    /// The hooks reporting the agent's state.
    StatusHooks,
}

impl Part {
    fn label(self) -> &'static str {
        match self {
            Part::Skill => "skill",
            Part::ContextHook => "context hook",
            Part::StatusHooks => "status hooks",
        }
    }
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
    /// Not installed, since the agent's skill wasn't.
    Skipped,
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
    part: Part,
    path: PathBuf,
    change: Change,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

fn step(agents: Vec<Agent>, part: Part, path: PathBuf, result: Result<Change, String>) -> Step {
    let (change, error) = match result {
        Ok(change) => (change, None),
        Err(error) => (Change::Failed, Some(error)),
    };
    Step {
        agents,
        part,
        path,
        change,
        error,
    }
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
        // An empty directory, or a symlink whose target is gone, as a skill
        // shared through a symlink leaves after its removal.
        Err(error) if error.kind() == std::io::ErrorKind::NotFound && holds_nothing(dir) => {
            Installed::Missing
        }
        _ => Installed::Foreign,
    }
}

fn holds_nothing(dir: &Path) -> bool {
    match fs::read_dir(dir) {
        Ok(mut entries) => entries.next().is_none(),
        Err(error) => error.kind() == std::io::ErrorKind::NotFound,
    }
}

/// `dir`, or a dangling symlink's target, created.
fn create_skill_dir(dir: &Path) -> Result<(), String> {
    let target = match fs::read_link(dir) {
        Ok(target) if !dir.exists() => dir.parent().map_or(target.clone(), |p| p.join(target)),
        _ => dir.to_path_buf(),
    };
    fs::create_dir_all(&target).map_err(|e| format!("cannot create {}: {e}", target.display()))
}

fn hooks_state(path: &Path, hooks: &Hooks) -> Result<Installed, String> {
    let Some(config) = agent_hooks::read(path)? else {
        return Ok(Installed::Missing);
    };
    let (merged, had) = hooks.merged(path, Some(config.clone()))?;
    Ok(if !had {
        Installed::Missing
    } else if merged == config {
        Installed::Current
    } else {
        Installed::Outdated
    })
}

fn install_hooks(path: &Path, hooks: &Hooks, dry_run: bool) -> Result<Change, String> {
    let config = agent_hooks::read(path)?;
    let (merged, had) = hooks.merged(path, config.clone())?;
    if config.as_ref() == Some(&merged) {
        return Ok(Change::Unchanged);
    }
    if !dry_run {
        agent_hooks::write_json(path, &merged)?;
    }
    Ok(if had {
        Change::Updated
    } else {
        Change::Created
    })
}

fn remove_hooks(path: &Path, hooks: &Hooks, dry_run: bool) -> Result<Change, String> {
    let Some(config) = agent_hooks::read(path)? else {
        return Ok(Change::Absent);
    };
    let Some(config) = hooks.without(&config) else {
        return Ok(Change::Absent);
    };
    // Through a symlink too, as setup wrote it.
    if !dry_run {
        agent_hooks::write_json(path, &config)?;
    }
    Ok(Change::Removed)
}

fn home() -> Result<PathBuf, String> {
    dirs::home_dir().ok_or_else(|| "cannot resolve the home directory".into())
}

/// Skill directories with the agents reading each, in order.
fn skill_dirs(agents: &[Agent], home: &Path) -> Vec<(PathBuf, Vec<Agent>)> {
    let mut dirs: Vec<(PathBuf, Vec<Agent>)> = Vec::new();
    for &agent in agents {
        let Some(dir) = agent.skill_dir(home) else {
            continue;
        };
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
    create_skill_dir(dir)?;
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
        let result = match skill_state(&dir) {
            Installed::Missing => Ok(Change::Created),
            Installed::Current => Ok(Change::Unchanged),
            Installed::Outdated => Ok(Change::Updated),
            Installed::Foreign if force => Ok(Change::Replaced),
            Installed::Foreign => Ok(Change::Conflict),
        };
        let result = match result {
            Ok(change @ (Change::Created | Change::Updated | Change::Replaced)) if !dry_run => {
                write_skill(&dir, change == Change::Replaced).map(|()| change)
            }
            result => result,
        };
        steps.push(step(readers, Part::Skill, dir, result));
    }
    for agent in agents {
        let path = agent.target().config_path()?;
        if let (Some(hook), Some(dir)) = (agent.context_hook(), agent.skill_dir(&home)) {
            // The hook points at the skill, so it waits for one.
            let skill_ok = steps.iter().any(|s| {
                s.part == Part::Skill
                    && s.path == dir
                    && matches!(
                        s.change,
                        Change::Created | Change::Updated | Change::Replaced | Change::Unchanged
                    )
            });
            let result = if skill_ok {
                install_hooks(&path, &hook, dry_run)
            } else {
                Ok(Change::Skipped)
            };
            steps.push(step(vec![agent], Part::ContextHook, path.clone(), result));
        }
        let result = install_hooks(&path, &agent.status_hooks(), dry_run);
        steps.push(step(vec![agent], Part::StatusHooks, path, result));
    }
    Ok(steps)
}

fn remove(agents: &[Agent], dry_run: bool) -> Result<Vec<Step>, String> {
    let home = home()?;
    let all = agents.is_empty();
    let agents = if all {
        AGENTS.to_vec()
    } else {
        agents.to_vec()
    };
    let mut steps = Vec::new();
    let mut dirs = skill_dirs(&agents, &home);
    let shared = shared_dir(&home);
    if all && !dirs.iter().any(|(dir, _)| *dir == shared) {
        dirs.push((shared, Vec::new()));
    }
    for (dir, readers) in dirs {
        let result = match skill_state(&dir) {
            Installed::Missing => Ok(Change::Absent),
            Installed::Foreign => Ok(Change::Kept),
            Installed::Current | Installed::Outdated if dry_run => Ok(Change::Removed),
            Installed::Current | Installed::Outdated => {
                remove_skill(&dir).map(|()| Change::Removed)
            }
        };
        steps.push(step(readers, Part::Skill, dir, result));
    }
    for agent in agents {
        let path = agent.target().config_path()?;
        if let Some(hook) = agent.context_hook() {
            let result = remove_hooks(&path, &hook, dry_run);
            steps.push(step(vec![agent], Part::ContextHook, path.clone(), result));
        }
        let result = remove_hooks(&path, &agent.status_hooks(), dry_run);
        steps.push(step(vec![agent], Part::StatusHooks, path, result));
    }
    Ok(steps)
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
            "{:<13} {:<10} {}  ({agents})",
            step.part.label(),
            snake(step.change),
            tilde(&step.path, &home)
        );
        if let Some(error) = &step.error {
            println!("              {error}");
        }
    }
    if dry_run {
        println!("\nDry run: nothing was changed.");
    }
    if steps.iter().any(|s| s.change == Change::Conflict) {
        println!(
            "\nA skill named btmux that btmux didn't install was left alone, and the \
             context hook that points at it wasn't added. Run again with --force to \
             replace it."
        );
    }
    let hook_changed = |agent: Option<Agent>| {
        steps.iter().any(|s| {
            s.part != Part::Skill
                && agent.is_none_or(|agent| s.agents.contains(&agent))
                && matches!(
                    s.change,
                    Change::Created | Change::Updated | Change::Removed
                )
        })
    };
    if hook_changed(None) {
        let verb = if dry_run { "would need" } else { "need" };
        println!("\nRunning agents {verb} a restart to load the change.");
    }
    if action == "setup" && hook_changed(Some(Agent::Codex)) {
        println!(
            "Codex skips new and changed hooks until you trust them: run /hooks in \
             Codex and trust btmux's."
        );
    }
    failed as i32
}

fn status(json: bool) -> i32 {
    let home = match home() {
        Ok(home) => home,
        Err(error) => return fail(&error),
    };
    let state = |result: Result<Installed, String>, path: PathBuf| match result {
        Ok(state) => json!({ "path": path, "state": state }),
        Err(error) => json!({ "path": path, "state": "error", "error": error }),
    };
    let rows: Vec<Value> = AGENTS
        .into_iter()
        .map(|agent| {
            let path = agent.target().config_path().unwrap_or_default();
            let skill = agent
                .skill_dir(&home)
                .map(|dir| state(Ok(skill_state(&dir)), dir));
            let context = agent
                .context_hook()
                .map(|hook| state(hooks_state(&path, &hook), path.clone()));
            let status_hooks = state(hooks_state(&path, &agent.status_hooks()), path.clone());
            json!({
                "agent": agent,
                "found": agent.found(&home),
                "skill": skill,
                "context_hook": context,
                "status_hooks": status_hooks,
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
        for part in [Part::Skill, Part::ContextHook, Part::StatusHooks] {
            let entry = &row[snake(part)];
            if entry.is_null() {
                continue;
            }
            let state = entry["state"].as_str().unwrap_or_default();
            stale |= state != "current";
            let path = PathBuf::from(entry["path"].as_str().unwrap_or_default());
            println!("  {:<13} {state:<9} {}", part.label(), tilde(&path, &home));
            if let Some(error) = entry["error"].as_str() {
                println!("                {error}");
            }
        }
    }
    if stale {
        println!("\nRun \"btmux skill setup\" to install or update them.");
    }
    0
}

/// Whether anything setup installs for `agent` is missing or from another
/// btmux version.
fn incomplete(agent: Agent) -> bool {
    let Ok(home) = home() else {
        return false;
    };
    let Ok(path) = agent.target().config_path() else {
        return false;
    };
    let stale = |state: Installed| matches!(state, Installed::Missing | Installed::Outdated);
    let skill = agent.skill_dir(&home).map(|dir| skill_state(&dir));
    let context = agent
        .context_hook()
        .and_then(|hook| hooks_state(&path, &hook).ok());
    let status = hooks_state(&path, &agent.status_hooks()).ok();
    [skill, context, status].into_iter().flatten().any(stale)
}

/// The hook's text: where the agent runs, as JSON the agent reads.
async fn context(profile: Option<&str>, agent: Option<Agent>) -> i32 {
    let Some(pane) = std::env::var("BTMUX_PANE_ID")
        .ok()
        .and_then(|id| uuid::Uuid::parse_str(&id).ok())
    else {
        if agent == Some(Agent::Agy) {
            println!("{{}}");
        }
        return 0;
    };
    // Guest credentials can't list panes; the ID alone still helps.
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
    let outdated = agent.is_some_and(incomplete);
    let text = context_text(pane, place, agent.map(Agent::invocation), outdated);
    println!("{}", context_output(agent, text));
    0
}

/// Claude Code and Codex read `additionalContext`; Antigravity injects steps.
fn context_output(agent: Option<Agent>, text: String) -> Value {
    match agent {
        Some(Agent::Agy) => json!({ "injectSteps": [
            { "systemMessage": { "systemMessage": text } }
        ] }),
        _ => json!({ "hookSpecificOutput": {
            "hookEventName": "SessionStart",
            "additionalContext": text,
        } }),
    }
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
            "\nThe btmux setup for this agent is out of date or incomplete. Suggest \
             running `btmux skill setup` to update it.\n",
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

    fn temp() -> PathBuf {
        std::env::temp_dir().join(format!("btmux-skill-{}", uuid::Uuid::new_v4()))
    }

    #[test]
    fn stub_keeps_the_guide_description() {
        let stub = stub();
        assert!(stub.starts_with("---\nname: btmux\ndescription: "));
        assert!(stub.contains(MARKER));
        assert!(stub.contains("skill show"));
        assert!(guide().starts_with("# btmux"));
    }

    #[test]
    fn hooks_merge_once_beside_others_and_remove_only_ours() {
        let path = Path::new("settings.json");
        let hook = Agent::Claude.context_hook().unwrap();
        let status = Agent::Claude.status_hooks();
        // The user's own hooks that mention the same words.
        let own = json!({ "hooks": {
            "SessionStart": [{ "hooks": [{ "type": "command", "command": "btmux skill context --agent claude" }] }],
            "Stop": [{ "hooks": [{ "type": "command", "command": "echo skill context # btmux-context" }] }],
        } });
        let once = hook.merged(path, Some(own.clone())).unwrap().0;
        let once = status.merged(path, Some(once)).unwrap().0;
        let twice = hook.merged(path, Some(once.clone())).unwrap().0;
        let twice = status.merged(path, Some(twice)).unwrap().0;
        assert_eq!(once, twice);
        let start = once["hooks"]["SessionStart"].as_array().unwrap();
        assert_eq!(
            start.len(),
            3,
            "the user's, the context, and status: {start:?}"
        );

        let removed = hook.without(&once).unwrap();
        let removed = status.without(&removed).unwrap();
        assert_eq!(removed, own);
        assert!(hook.without(&removed).is_none());
    }

    #[test]
    fn antigravity_hooks_are_named_beside_the_users() {
        let dir = temp();
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("hooks.json");
        let own = json!({ "mine": { "Stop": [{ "command": "echo done" }] } });
        fs::write(&path, own.to_string()).unwrap();
        let (context, status) = (
            Agent::Agy.context_hook().unwrap(),
            Agent::Agy.status_hooks(),
        );
        assert_eq!(hooks_state(&path, &status), Ok(Installed::Missing));
        assert_eq!(install_hooks(&path, &context, false), Ok(Change::Created));
        assert_eq!(install_hooks(&path, &status, false), Ok(Change::Created));
        assert_eq!(install_hooks(&path, &status, false), Ok(Change::Unchanged));
        assert_eq!(hooks_state(&path, &status), Ok(Installed::Current));
        let installed: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(installed["mine"], own["mine"]);
        let command = installed["btmux-context"]["SessionStart"][0]["command"]
            .as_str()
            .unwrap();
        assert!(command.contains("--agent agy") && command.ends_with(CONTEXT_MARKER));
        assert!(installed["btmux"]["Stop"].is_array());

        // A changed hook is outdated, and updated in place.
        let mut changed = installed.clone();
        changed["btmux"]["Stop"] = json!([]);
        fs::write(&path, changed.to_string()).unwrap();
        assert_eq!(hooks_state(&path, &status), Ok(Installed::Outdated));
        assert_eq!(install_hooks(&path, &status, false), Ok(Change::Updated));

        assert_eq!(remove_hooks(&path, &context, false), Ok(Change::Removed));
        assert_eq!(remove_hooks(&path, &status, false), Ok(Change::Removed));
        assert_eq!(remove_hooks(&path, &status, false), Ok(Change::Absent));
        let left: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(left, own);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn antigravity_alone_is_not_gemini_cli() {
        let home = temp();
        let gemini = home.join(".gemini");
        fs::create_dir_all(gemini.join("antigravity-cli")).unwrap();
        fs::create_dir_all(gemini.join("config")).unwrap();
        // What setup writes for Gemini CLI doesn't count either.
        fs::write(gemini.join("settings.json"), "{}").unwrap();
        assert!(Agent::Agy.configured(&home));
        assert!(!Agent::Gemini.configured(&home));
        fs::write(gemini.join("installation_id"), "id").unwrap();
        assert!(Agent::Gemini.configured(&home));
        fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn antigravity_context_is_an_injected_step() {
        let output = context_output(Some(Agent::Agy), "here".into());
        assert_eq!(
            output["injectSteps"][0]["systemMessage"]["systemMessage"],
            "here"
        );
        let output = context_output(Some(Agent::Claude), "here".into());
        assert_eq!(output["hookSpecificOutput"]["additionalContext"], "here");
    }

    #[test]
    fn skills_are_told_apart() {
        let dir = temp();
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
        // An empty directory isn't a skill.
        fs::create_dir_all(&dir).unwrap();
        assert_eq!(skill_state(&dir), Installed::Missing);
        fs::write(dir.join("notes.txt"), "mine").unwrap();
        assert_eq!(skill_state(&dir), Installed::Foreign);
    }

    /// A skill directory symlinked to another, as after one removal through
    /// it, is set up again through the link.
    #[cfg(unix)]
    #[test]
    fn a_dangling_skill_link_is_set_up_again() {
        let root = temp();
        fs::create_dir_all(&root).unwrap();
        let shared = root.join("shared");
        let link = root.join("link");
        std::os::unix::fs::symlink(&shared, &link).unwrap();
        assert_eq!(skill_state(&link), Installed::Missing);
        write_skill(&link, false).unwrap();
        assert_eq!(skill_state(&link), Installed::Current);
        assert_eq!(skill_state(&shared), Installed::Current);
        assert!(fs::symlink_metadata(&link)
            .unwrap()
            .file_type()
            .is_symlink());
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn removing_the_last_hook_keeps_a_symlinked_config() {
        let dir = temp();
        fs::create_dir_all(&dir).unwrap();
        let (target, link) = (dir.join("managed.json"), dir.join("settings.json"));
        fs::write(&target, "{}").unwrap();
        std::os::unix::fs::symlink(&target, &link).unwrap();
        let hook = Agent::Claude.context_hook().unwrap();
        assert_eq!(install_hooks(&link, &hook, false), Ok(Change::Created));
        assert_eq!(remove_hooks(&link, &hook, false), Ok(Change::Removed));
        assert!(fs::symlink_metadata(&link)
            .unwrap()
            .file_type()
            .is_symlink());
        let left: Value = serde_json::from_str(&fs::read_to_string(&target).unwrap()).unwrap();
        assert_eq!(left, json!({ "hooks": {} }));
        fs::remove_dir_all(dir).unwrap();
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
