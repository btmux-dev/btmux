use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::time::{Duration, Instant};
use tokio::sync::{broadcast, mpsc};
use uuid::Uuid;

use super::{
    agent::{AgentEvent, AgentLifecycle, AgentProcess, AgentReport, Authority, ToolCall},
    layout::{Layout, LayoutPreset},
    AgentPane, AgentState, AgentStatus, Pane, PaneInfo, PaneSnapshot, Runtime, Session,
    SessionSummary, Window,
};
use crate::config::{ClientConfig, ColorMode, ConfigUpdate, FileConfig};
use crate::git::RepoLayout;
use crate::pane_git::GitTracker;
use crate::pty::colors::{Palette, SharedPalette};
use crate::pty::{PtyEnv, PtyHandle};
use std::sync::Arc;

/// The detector scans every 300 ms; much longer means it stopped.
const DETECTION_STALE: Duration = Duration::from_secs(3);

/// Every pane's agent status, sent with each state broadcast.
pub type PaneStatuses = std::sync::Arc<HashMap<Uuid, Option<AgentStatus>>>;

/// A live shell handed to an upgraded server: pane, PTY master, and state.
#[cfg(unix)]
pub type HandoffPane = (Uuid, std::os::fd::RawFd, crate::pty::Handoff);

/// Sessions, popups, and live shells, for an upgraded server.
#[cfg(unix)]
pub type HandoffExport = (
    Vec<SessionSnapshot>,
    crate::popup::PopupsHandoff,
    Vec<HandoffPane>,
);

/// How long after an upgrade a pane's next agent can take its handed-off age.
#[cfg_attr(not(unix), allow(dead_code))]
const HANDED_SINCE_WAIT: Duration = Duration::from_secs(60);

/// Gives a pane's new agent the age an upgrade handed off for it.
fn hand_off(
    handed: &mut HashMap<Uuid, (AgentState, u64)>,
    until: Option<Instant>,
    pane_id: Uuid,
    agent: &mut AgentLifecycle,
) {
    if until.is_some_and(|until| Instant::now() <= until) {
        if let Some((state, since)) = handed.remove(&pane_id) {
            agent.hand_off(state, since);
        }
    }
}

pub struct SessionManager {
    pub sessions: Vec<Session>,
    pub telemetry: crate::telemetry::TelemetryStore,
    pub widgets: crate::widget::Widgets,
    pub popups: crate::popup::Popups,
    shell: String,
    cli_shell: Option<String>,
    /// The config as last read from disk. Kept so a session-only override can be
    /// re-layered onto it without re-reading the file.
    file_config: FileConfig,
    /// Session-only overrides from the settings UI and command palette, dropped
    /// on every config-file reload. Written to disk only by `save_config`.
    overrides: ConfigUpdate,
    has_overrides: bool,
    /// The text `save_config` last wrote, so the watcher skips its echo.
    saved_config: Option<String>,
    /// The system color mode a browser last reported, for `appearance = "auto"`.
    system_color_mode: ColorMode,
    /// Why config.toml was rejected; cleared when it loads.
    config_error: Option<String>,
    /// `file_config` + `overrides`, resolved — this is what the browser sees.
    config: ClientConfig,
    events: broadcast::Sender<String>,
    /// Lets automation see each agent transition, not just the latest.
    statuses: broadcast::Sender<PaneStatuses>,
    /// Live agent state is runtime-only and is never included in persistence.
    agents: HashMap<Uuid, AgentLifecycle>,
    /// Agent states and ages from before an upgrade, for each pane's next
    /// agent, until the deadline.
    handed_since: HashMap<Uuid, (AgentState, u64)>,
    handed_until: Option<Instant>,
    /// The detector's current rules, for automation's own screen checks.
    detector: Option<super::detection::Detector>,
    /// When the detector last scanned screens.
    detection_tick: Option<Instant>,
    pty_env: PtyEnv,
    /// Repo, worktree, and branch of pane working directories.
    git: GitTracker,
    next_client: u32,
    clients: usize,
    /// The control client the user focused most recently; it alone shows OS notifications.
    alert_client: Option<u32>,
}

#[derive(Serialize, Deserialize, Clone)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct SessionSnapshot {
    pub id: Uuid,
    pub name: String,
    #[serde(default)]
    pub runtime: Runtime,
    pub windows: Vec<WindowSnapshot>,
    pub active_window: usize,
}

#[derive(Serialize, Deserialize, Clone)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct WindowSnapshot {
    pub id: Uuid,
    pub name: String,
    pub panes: Vec<PaneSnapshot>,
    pub active_pane: usize,
    pub layout: Layout,
    pub zoomed_pane: Option<Uuid>,
}

impl SessionManager {
    pub fn new(
        shell: String,
        file_config: FileConfig,
        exit_tx: mpsc::UnboundedSender<Uuid>,
        meta_tx: mpsc::UnboundedSender<()>,
        port: u16,
        cli_shell: Option<String>,
    ) -> Self {
        let (events, _) = broadcast::channel::<String>(64);
        let (statuses, _) = broadcast::channel(64);
        let mut config = crate::config::resolve_binds(&file_config, ColorMode::default());
        config.cli_shell = cli_shell.clone();
        let pty_env = PtyEnv {
            exit_tx,
            meta_tx,
            port,
            palette: SharedPalette::default(),
        };
        pty_env.palette.set(Palette::from_theme(&config.theme));
        crate::vscode::configure(&config);
        crate::native_editor::lsp::config::configure(&file_config, &shell);
        Self {
            sessions: Vec::new(),
            telemetry: crate::telemetry::TelemetryStore::default(),
            shell,
            cli_shell,
            file_config,
            overrides: ConfigUpdate::default(),
            has_overrides: false,
            saved_config: None,
            system_color_mode: ColorMode::default(),
            config_error: None,
            config,
            events,
            statuses,
            agents: HashMap::new(),
            handed_since: HashMap::new(),
            handed_until: None,
            detector: None,
            detection_tick: None,
            next_client: 0,
            clients: 0,
            alert_client: None,
            widgets: Default::default(),
            popups: Default::default(),
            git: GitTracker::new(pty_env.meta_tx.clone()),
            pty_env,
        }
    }

    pub fn events(&self) -> &broadcast::Sender<String> {
        &self.events
    }

    pub fn statuses(&self) -> &broadcast::Sender<PaneStatuses> {
        &self.statuses
    }

    pub fn pane_statuses(&self) -> PaneStatuses {
        let panes = self
            .sessions
            .iter()
            .flat_map(|session| &session.windows)
            .flat_map(|window| &window.panes);
        std::sync::Arc::new(
            panes
                .map(|pane| (pane.id, self.agent_status(pane.id).cloned()))
                .collect(),
        )
    }

    /// The grid and status badges read the same lifecycle record.
    pub fn running_agent_panes(&self) -> Vec<Uuid> {
        self.sessions
            .iter()
            .flat_map(|session| &session.windows)
            .flat_map(|window| &window.panes)
            .filter_map(|pane| {
                self.agents
                    .get(&pane.id)
                    .is_some_and(AgentLifecycle::is_active)
                    .then_some(pane.id)
            })
            .collect()
    }

    /// Panes running an agent, for automation.
    pub fn agent_panes(&self) -> Vec<AgentPane> {
        let mut panes = Vec::new();
        for session in &self.sessions {
            for (window_index, window) in session.windows.iter().enumerate() {
                for pane in &window.panes {
                    if let Some(status) = self.agent_status(pane.id) {
                        panes.push(AgentPane {
                            pane_id: pane.id,
                            session_id: session.id,
                            session_name: session.name.clone(),
                            window_index,
                            window_name: window.name.clone(),
                            cwd: pane.pty.effective_cwd(),
                            status: status.clone(),
                        });
                    }
                }
            }
        }
        panes
    }

    pub fn agent_status(&self, pane_id: Uuid) -> Option<&AgentStatus> {
        self.agents
            .get(&pane_id)
            .filter(|agent| agent.is_active())
            .map(|agent| &agent.status)
    }

    /// Identifies the pane's active agent instance, including explicitly reported ones.
    pub fn agent_id(&self, pane_id: Uuid) -> Option<u64> {
        self.agents
            .get(&pane_id)
            .filter(|agent| agent.is_active())
            .map(AgentLifecycle::incarnation)
    }

    /// Identifies the pane's running agent instance; explicit reports name none.
    pub fn agent_incarnation(&self, pane_id: Uuid) -> Option<u64> {
        self.agents
            .get(&pane_id)
            .filter(|agent| agent.is_active() && agent.process().is_some())
            .map(AgentLifecycle::incarnation)
    }

    /// Why typed text could reach something other than the agent's prompt,
    /// judged from the current screen rather than the settled status, whose
    /// transitions can lag a dialog.
    pub fn prompt_conflict(&self, pane_id: Uuid, now: Instant) -> Option<&'static str> {
        const BLOCKED: &str =
            "the agent is waiting for a decision; read the pane and answer it by sending keys";
        let pane = self.find_pane(pane_id)?;
        let agent = self
            .agents
            .get(&pane_id)
            .filter(|agent| agent.is_active())?;
        if agent.status.state == AgentState::Blocked {
            return Some(BLOCKED);
        }
        let name = agent.screen_manifest()?;
        let (Some(detector), Some(tick)) = (&self.detector, self.detection_tick) else {
            return Some("agent detection isn't running; send keys instead");
        };
        if now.saturating_duration_since(tick) > DETECTION_STALE {
            return Some("agent detection has stalled; send keys instead");
        }
        let detection = pane
            .pty
            .screen_snapshot()
            .and_then(|screen| agent.detect(detector, &screen));
        if detection.is_some_and(|detection| detection.state == AgentState::Blocked) {
            return Some(BLOCKED);
        }
        // Agents without idle rules stay unknown at their prompt.
        (agent.status.state == AgentState::Unknown && detector.reports_idle(name)).then_some(
            "the agent's state is unknown, so typed text could answer a prompt; read the pane and send keys instead",
        )
    }

    /// The agent state and blocked tool, while the last hook report still owns them.
    pub fn hook_agent_state(&self, pane_id: Uuid) -> Option<(AgentState, Option<ToolCall>)> {
        self.agents
            .get(&pane_id)
            .filter(|agent| agent.is_active() && agent.authority() == Authority::Hook)
            .map(|agent| (agent.status.state.clone(), agent.blocked_tool().cloned()))
    }

    pub fn pane_shells(&self) -> Vec<(Uuid, u32, Option<u32>)> {
        self.sessions
            .iter()
            .flat_map(|session| &session.windows)
            .flat_map(|window| &window.panes)
            .filter_map(|pane| {
                pane.pty.shell_pid().map(|pid| {
                    (
                        pane.id,
                        pid,
                        pane.pty
                            .foreground_pgrp()
                            .and_then(|group| group.try_into().ok()),
                    )
                })
            })
            .collect()
    }

    /// Each started pane's shell, and whether its programs are that shell's
    /// OS descendants. Unlike `pane_shells`, this probes nothing.
    pub fn shell_pids(&self) -> Vec<(Uuid, u32, bool)> {
        self.sessions
            .iter()
            .flat_map(|session| &session.windows)
            .flat_map(|window| &window.panes)
            .filter_map(|pane| {
                let pid = pane.pty.shell_pid()?;
                Some((pane.id, pid, pane.pty.programs_are_descendants()))
            })
            // A popup's programs count for its pane.
            .chain(self.popups.iter().filter_map(|popup| {
                let pid = popup.pty.shell_pid()?;
                Some((popup.pane_id, pid, popup.pty.programs_are_descendants()))
            }))
            .collect()
    }

    pub fn update_detected_agents(
        &mut self,
        detected: &[(Uuid, AgentProcess, String)],
        alive: impl Fn(AgentProcess) -> bool,
    ) -> bool {
        let mut changed = false;
        for (pane_id, process, name) in detected {
            let Some(pane) = self.find_pane(*pane_id) else {
                continue;
            };
            let revision = pane.pty.screen_revision();
            if self
                .agents
                .get(pane_id)
                .is_some_and(|agent| !agent.is_active() && agent.ended_process() == Some(*process))
            {
                continue;
            }
            if let Some(agent) = self.agents.get_mut(pane_id) {
                changed |= agent.observe_process(*process, name, revision, Instant::now(), &alive);
                continue;
            }
            let mut agent = AgentLifecycle::detected(*process, name.clone(), Instant::now());
            agent.suppress_old_signals(revision);
            self.insert_agent(*pane_id, agent);
            changed = true;
        }
        let active: std::collections::HashSet<_> = detected.iter().map(|(id, _, _)| *id).collect();
        for (pane_id, agent) in &mut self.agents {
            if agent.has_observed_process() && !active.contains(pane_id) {
                agent.end(Instant::now());
                changed = true;
            }
        }
        self.sync_screen_tracking();
        changed
    }

    pub fn set_detector(&mut self, detector: super::detection::Detector) {
        self.detector = Some(detector);
    }

    pub fn scan_agent_screens(
        &mut self,
        detector: &super::detection::Detector,
        now: Instant,
        reload: bool,
    ) -> bool {
        self.detection_tick = Some(now);
        let mut changed = false;
        let panes = self
            .sessions
            .iter()
            .flat_map(|session| &session.windows)
            .flat_map(|window| &window.panes);
        for pane in panes {
            let Some(agent) = self.agents.get_mut(&pane.id) else {
                continue;
            };
            let Some(process) = agent.observed_process().filter(|_| agent.is_active()) else {
                continue;
            };
            let group = crate::process::group(process.pid as crate::process::Pid);
            let revision = pane.pty.screen_revision();
            if group.is_none() || pane.pty.foreground_pgrp() != group {
                changed |= agent.observe_background(revision);
                continue;
            }
            if agent.program_suspended() {
                changed |= agent.resume_program(pane.pty.program_summary().as_ref(), now);
            }
            if reload {
                agent.invalidate_screen();
            }
            // Text is built only for screens that can change the status.
            if !agent.wants_screen(revision) {
                continue;
            }
            if let Some(screen) = pane.pty.screen_snapshot() {
                changed |= agent.scan_screen(detector, &screen, now);
            }
        }
        changed
    }

    /// Applies changed OSC 7501 records, first dropping those of programs
    /// that exited. A report gives a pane without an agent one; records that
    /// only ended don't, and results no agent shows are forgotten, so a later
    /// report doesn't bring them back.
    pub fn sync_program_status(&mut self, now: Instant) -> bool {
        let mut changed = false;
        let panes = self
            .sessions
            .iter()
            .flat_map(|session| &session.windows)
            .flat_map(|window| &window.panes);
        for pane in panes {
            pane.pty.drop_exited_programs();
            let agent = self
                .agents
                .get_mut(&pane.id)
                .filter(|agent| agent.is_active());
            let change = pane.pty.take_program_status();
            match (agent, change) {
                (Some(agent), Some(change)) => {
                    changed |= agent.observe_program(change.summary.as_ref(), now);
                }
                (Some(_), None) => {}
                (None, Some(change)) if change.reported => {
                    if let Some(summary) = change.summary {
                        let mut agent = AgentLifecycle::program(&summary, now);
                        hand_off(
                            &mut self.handed_since,
                            self.handed_until,
                            pane.id,
                            &mut agent,
                        );
                        self.agents.insert(pane.id, agent);
                        changed = true;
                    }
                }
                (None, _) => pane.pty.forget_program_results(),
            }
        }
        changed
    }

    /// Records await `sync_program_status`.
    pub fn program_status_pending(&self) -> bool {
        self.sessions
            .iter()
            .flat_map(|session| &session.windows)
            .flat_map(|window| &window.panes)
            .any(|pane| pane.pty.program_status_pending())
    }

    /// Ends hook-only agents once their pane is back at the shell prompt:
    /// nothing else runs in the foreground, and the agent's reported PID may
    /// belong to a server that never exits (Codex).
    pub fn end_agents_at_prompt(&mut self, now: Instant, grace: Duration) -> bool {
        let mut changed = false;
        let panes = self
            .sessions
            .iter()
            .flat_map(|session| &session.windows)
            .flat_map(|window| &window.panes);
        for pane in panes {
            let Some(agent) = self.agents.get_mut(&pane.id) else {
                continue;
            };
            if !agent.is_unobserved_for(now, grace) {
                continue;
            }
            let Some(shell) = pane.pty.shell_pid() else {
                continue;
            };
            let group = crate::process::group(shell as crate::process::Pid);
            if group.is_some() && pane.pty.foreground_pgrp() == group {
                agent.end(now);
                changed = true;
            }
        }
        changed
    }

    /// Forgets all agent state for the pane; a still-running agent reappears
    /// on the next process scan or hook.
    pub fn reset_agent(&mut self, pane_id: Uuid) -> bool {
        let removed = self.agents.remove(&pane_id).is_some();
        self.sync_screen_tracking();
        removed
    }

    /// Live screens are parsed only for panes with a detected agent.
    fn sync_screen_tracking(&self) {
        for pane in self
            .sessions
            .iter()
            .flat_map(|session| &session.windows)
            .flat_map(|window| &window.panes)
        {
            let tracked = self
                .agents
                .get(&pane.id)
                .is_some_and(AgentLifecycle::has_observed_process);
            pane.pty.track_screen(tracked);
        }
    }

    pub fn agent_diagnostics(&self, pane_id: Uuid) -> serde_json::Value {
        let agent = self.agents.get(&pane_id).filter(|agent| agent.is_active());
        serde_json::json!({
            "status": agent.map(|agent| &agent.status),
            "authority": agent.map(AgentLifecycle::authority),
            "process": agent.and_then(AgentLifecycle::process),
            "detection": agent.and_then(|agent| agent.detection.as_ref()),
        })
    }

    pub fn reap_stale_agents(
        &mut self,
        now: Instant,
        process_alive: impl Fn(AgentProcess) -> bool,
    ) -> bool {
        let mut changed = false;
        self.agents.retain(|_, agent| {
            if !agent.is_stale(now, &process_alive) {
                return true;
            }
            if agent.is_active() {
                agent.end(now);
                changed = true;
                true
            } else {
                false
            }
        });
        changed
    }

    /// Returns None for stale events and missing panes. A false result means
    /// the event was accepted but did not change the visible state.
    pub fn apply_agent_report(
        &mut self,
        pane_id: Uuid,
        report: AgentReport,
        now: Instant,
    ) -> Option<bool> {
        let revision = self.find_pane(pane_id)?.pty.screen_revision();
        if report.event == AgentEvent::SessionEnd {
            let current = self.agents.get_mut(&pane_id)?;
            if !current.matches_session(report.session_id.as_deref()) {
                return None;
            }
            current.end(now);
            return Some(true);
        }
        let was_active = self.agent_status(pane_id).is_some();
        let new = !self.agents.contains_key(&pane_id);
        let agent = self
            .agents
            .entry(pane_id)
            .or_insert_with(|| AgentLifecycle::reported(now));
        let changed = agent.apply(report, revision, now);
        // After the report, which sets the state the age is for.
        if new {
            hand_off(&mut self.handed_since, self.handed_until, pane_id, agent);
        }
        let changed = changed?;
        // A new agent starts idle, so a report can announce it without changing its status.
        Some(changed || agent.is_active() != was_active)
    }

    pub fn register_client(&mut self) -> u32 {
        self.clients += 1;
        self.next_client = self.next_client.wrapping_add(1);
        self.next_client
    }

    pub fn focus_client(&mut self, client: u32) {
        self.alert_client = Some(client);
    }

    pub fn release_client(&mut self, client: u32) {
        self.clients = self.clients.saturating_sub(1);
        if self.alert_client == Some(client) {
            self.alert_client = None;
        }
    }

    pub fn alert_client(&self) -> Option<u32> {
        self.alert_client
    }

    pub fn config(&self) -> &ClientConfig {
        &self.config
    }

    /// The shell new panes run.
    pub fn shell(&self) -> &str {
        &self.shell
    }

    pub fn github_config(&self) -> &crate::config::GithubConfig {
        &self.file_config.github
    }

    /// Connected control sockets.
    pub fn client_count(&self) -> usize {
        self.clients
    }

    /// The repo and branch of every pane on a local branch.
    pub fn pane_branches(&self) -> std::collections::HashSet<(String, String)> {
        self.sessions
            .iter()
            .flat_map(|session| &session.windows)
            .flat_map(|window| &window.panes)
            .filter_map(|pane| {
                let git = self.git.lookup(&pane.pty.local_cwd()?)?;
                Some((git.repo_id, git.branch?))
            })
            .collect()
    }

    pub fn check_for_updates(&self) -> bool {
        self.file_config.check_for_updates
    }

    /// Install a freshly-read config file. Session-only overrides are **dropped**:
    /// the file is the source of truth, so a reload is the point where whatever
    /// was tried out from the command palette goes away.
    pub fn set_file_config(&mut self, file_config: FileConfig) -> &ClientConfig {
        self.file_config = file_config;
        self.overrides = ConfigUpdate::default();
        self.has_overrides = false;
        self.config_error = None;
        self.config = self.resolve();
        if self.cli_shell.is_none() {
            self.shell = crate::config::resolve_shell(self.config.shell.as_deref());
        }
        crate::native_editor::lsp::config::configure(&self.file_config, &self.shell);
        self.sync_palette();
        &self.config
    }

    /// Apply a config change from the settings UI or command palette. It is layered
    /// over the on-disk config in memory only and lasts until the next restart
    /// or config reload, unless the settings dialog saves it.
    pub fn apply_config_override(&mut self, update: &ConfigUpdate) -> &ClientConfig {
        self.overrides.merge(update);
        self.has_overrides = true;
        self.config = self.resolve();
        if update.shell.is_some() && self.cli_shell.is_none() {
            self.shell = crate::config::resolve_shell(self.config.shell.as_deref());
        }
        self.sync_palette();
        &self.config
    }

    /// Drop all session-only config overrides and restore the config resolved
    /// from the last-loaded config file (including built-in defaults).
    pub fn reset_config_overrides(&mut self) -> &ClientConfig {
        self.overrides = ConfigUpdate::default();
        self.has_overrides = false;
        self.config = self.resolve();
        if let Some(shell) = &self.cli_shell {
            self.shell = shell.clone();
        } else {
            self.shell = crate::config::resolve_shell(self.config.shell.as_deref());
        }
        self.sync_palette();
        &self.config
    }

    /// Record a browser's system color mode. Returns the new config if the
    /// shown color scheme changed.
    pub fn set_system_color_mode(&mut self, mode: ColorMode) -> Option<&ClientConfig> {
        if self.system_color_mode == mode {
            return None;
        }
        self.system_color_mode = mode;
        if self.config.color_mode == self.config.appearance.color_mode(mode) {
            return None;
        }
        self.config = self.resolve();
        self.sync_palette();
        Some(&self.config)
    }

    /// Resolve the config again, as when something it detects was found.
    pub fn refresh_config(&mut self) -> &ClientConfig {
        self.config = self.resolve();
        self.sync_palette();
        &self.config
    }

    /// Record that config.toml was rejected; the current config stays in use.
    pub fn set_config_error(&mut self, error: String) -> &ClientConfig {
        self.config_error = Some(error.clone());
        self.config.config_error = Some(error);
        &self.config
    }

    fn resolve(&self) -> ClientConfig {
        let mut config = crate::config::resolve_with_overrides(
            &self.file_config,
            &self.overrides,
            self.system_color_mode,
        );
        config.config_error = self.config_error.clone();
        config.has_overrides = self.has_overrides;
        config.cli_shell = self.cli_shell.clone();
        config
    }

    /// The session-only overrides with `update` on top: what saving writes.
    pub fn overrides_with(&self, update: &ConfigUpdate) -> ConfigUpdate {
        let mut overrides = self.overrides.clone();
        overrides.merge(update);
        overrides
    }

    /// Record (or with `None`, forget) the text `save_config` writes.
    pub fn set_saved_config(&mut self, text: Option<String>) {
        self.saved_config = text;
    }

    /// Whether `text` is what `save_config` last wrote, which it already loaded.
    pub fn is_saved_config(&self, text: &str) -> bool {
        self.saved_config.as_deref() == Some(text)
    }

    /// Publish the resolved theme to PTY color queries and notify programs
    /// that enabled mode 2031.
    fn sync_palette(&mut self) {
        // VS Code also follows the terminal font and picks its server.
        crate::vscode::configure(&self.config);
        let palette = Palette::from_theme(&self.config.theme);
        if !self.pty_env.palette.set(palette.clone()) {
            return;
        }
        let Some(palette) = palette else { return };
        for pane in self
            .sessions
            .iter()
            .flat_map(|session| &session.windows)
            .flat_map(|window| &window.panes)
        {
            pane.pty.report_color_scheme(&palette);
        }
    }

    fn scrollback_lines(&self) -> u32 {
        self.config
            .terminal
            .scrollback
            .unwrap_or(crate::config::DEFAULT_SCROLLBACK_LINES)
    }

    fn guest(&self, runtime: &Runtime) -> Option<Arc<crate::multipass::Guest>> {
        let Runtime::Multipass { mounts, shell } = runtime else {
            return None;
        };
        let config = &self.config.multipass_config;
        Some(Arc::new(crate::multipass::Guest {
            instance: crate::multipass::instance_name(config),
            mounts: std::sync::RwLock::new(mounts.clone()),
            shell: *shell,
            config: config.clone(),
        }))
    }

    fn new_pty(
        &self,
        pane_id: Uuid,
        cwd: Option<std::path::PathBuf>,
        runtime: &Runtime,
    ) -> PtyHandle {
        PtyHandle::new_with_cwd(
            &self.shell,
            pane_id,
            &self.pty_env,
            cwd,
            self.scrollback_lines(),
        )
        .with_guest(self.guest(runtime))
    }

    /// Where `new_popup_pty` runs a popup over `pane_id`.
    pub fn popup_target(&self, pane_id: Uuid) -> crate::popup::Target {
        let guest = self.guest(&self.pane_runtime(pane_id));
        crate::popup::Target {
            // As `PtyHandle` decides.
            dialect: match guest {
                Some(_) => crate::shell::Dialect::Posix,
                None => crate::shell::Dialect::of(&self.shell),
            },
            guest: guest.map(|guest| guest.instance.clone()),
        }
    }

    /// A popup's PTY over `pane_id`, in the pane's runtime.
    pub fn new_popup_pty(
        &self,
        id: Uuid,
        pane_id: Uuid,
        cwd: Option<std::path::PathBuf>,
        spawn: Option<crate::popup::Spawn>,
    ) -> PtyHandle {
        let pty = self.new_pty(id, cwd, &self.pane_runtime(pane_id));
        match spawn {
            Some(spawn) => pty.with_popup(pane_id, spawn),
            None => pty,
        }
    }

    /// A pane's or a popup's PTY.
    pub fn find_pty(&self, id: Uuid) -> Option<&PtyHandle> {
        match self.find_pane(id) {
            Some(pane) => Some(&pane.pty),
            None => self
                .popups
                .iter()
                .find(|popup| popup.id == id)
                .map(|popup| &popup.pty),
        }
    }

    pub fn find_pty_mut(&mut self, id: Uuid) -> Option<&mut PtyHandle> {
        if self.find_pane(id).is_some() {
            return self.find_pane_mut(id).map(|pane| &mut pane.pty);
        }
        self.popups.get_mut(id).map(|popup| &mut popup.pty)
    }

    fn runtime_shell_name(&self, runtime: &Runtime) -> String {
        match runtime {
            Runtime::Local => shell_name(&self.shell),
            Runtime::Multipass { shell, .. } => shell.name().to_string(),
        }
    }

    pub fn session_runtime(&self, session_id: Uuid) -> Option<&Runtime> {
        self.sessions
            .iter()
            .find(|s| s.id == session_id)
            .map(|s| &s.runtime)
    }

    /// Add checked mounts to a sandboxed session and its panes.
    pub fn add_sandbox_mounts(&mut self, session_id: Uuid, added: Vec<String>) {
        if let Some(Session {
            runtime: Runtime::Multipass { mounts, .. },
            windows,
            ..
        }) = self.session_mut(session_id)
        {
            for mount in &added {
                if !mounts.contains(mount) {
                    mounts.push(mount.clone());
                }
            }
            for pane in windows.iter().flat_map(|w| &w.panes) {
                pane.pty.add_guest_mounts(&added);
            }
        }
    }

    fn pane_runtime(&self, pane_id: Uuid) -> Runtime {
        self.session_of_pane(pane_id)
            .and_then(|id| self.sessions.iter().find(|s| s.id == id))
            .map(|s| s.runtime.clone())
            .unwrap_or_default()
    }

    /// The shared instance, while a session uses it.
    pub fn guest_instance(&self) -> Option<String> {
        self.sessions
            .iter()
            .any(|s| matches!(s.runtime, Runtime::Multipass { .. }))
            .then(|| crate::multipass::instance_name(&self.config.multipass_config))
    }

    /// Panes whose shell runs, or is starting, in the guest.
    pub fn guest_shells(&self) -> usize {
        self.sessions
            .iter()
            .flat_map(|s| &s.windows)
            .flat_map(|w| &w.panes)
            .filter(|p| p.pty.is_guest() && p.pty.is_spawned())
            .count()
    }

    /// Whether the pane's shell, or the popup's program, runs in the guest.
    pub fn is_guest_pane(&self, pane_id: Uuid) -> bool {
        self.find_pty(pane_id).is_some_and(PtyHandle::is_guest)
    }

    pub async fn create_session(&mut self, name: Option<String>) -> Uuid {
        self.create_session_with_cwd(name, None).await
    }

    pub async fn create_session_with_cwd(
        &mut self,
        name: Option<String>,
        cwd: Option<std::path::PathBuf>,
    ) -> Uuid {
        self.create_session_with_cwd_and_window(name, cwd, None)
            .await
    }

    pub async fn create_session_with_cwd_and_window(
        &mut self,
        name: Option<String>,
        cwd: Option<std::path::PathBuf>,
        window_name: Option<String>,
    ) -> Uuid {
        self.create_session_in(name, cwd, window_name, Runtime::Local)
            .await
    }

    /// A session whose shells run in `runtime`, which `Runtime::validate` checked.
    pub async fn create_session_in(
        &mut self,
        name: Option<String>,
        cwd: Option<std::path::PathBuf>,
        window_name: Option<String>,
        runtime: Runtime,
    ) -> Uuid {
        let session_id = Uuid::new_v4();
        let pane_id = Uuid::new_v4();
        let cwd = cwd.or_else(|| match &runtime {
            Runtime::Multipass { mounts, .. } => mounts.first().map(std::path::PathBuf::from),
            Runtime::Local => None,
        });
        let pty = self.new_pty(pane_id, cwd, &runtime);

        let pane = Pane::new(pane_id, pty);
        let wname = window_name
            .unwrap_or_else(|| self.unique_window_name_in(&[], &self.runtime_shell_name(&runtime)));
        let window = Window {
            id: Uuid::new_v4(),
            name: wname,
            panes: vec![pane],
            active_pane: 0,
            prev_pane: None,
            layout: Layout::Leaf { pane_id },
            zoomed_pane: None,
            last_preset: None,
        };

        let base = name.unwrap_or_else(|| self.sessions.len().to_string());
        let sname = self.unique_session_name(&base, None);
        let session = Session {
            id: session_id,
            name: sname,
            runtime,
            windows: vec![window],
            active_window: 0,
            prev_window: None,
        };

        self.sessions.push(session);
        session_id
    }

    pub fn find_pane(&self, pane_id: Uuid) -> Option<&Pane> {
        for session in &self.sessions {
            for window in &session.windows {
                for pane in &window.panes {
                    if pane.id == pane_id {
                        return Some(pane);
                    }
                }
            }
        }
        None
    }

    pub fn session_of_pane(&self, pane_id: Uuid) -> Option<Uuid> {
        self.sessions
            .iter()
            .find(|s| {
                s.windows
                    .iter()
                    .any(|w| w.panes.iter().any(|p| p.id == pane_id))
            })
            .map(|s| s.id)
    }

    /// Panes whose local directory is in one of `dirs`, as named or through
    /// symlinks. `dirs` should include the canonical path.
    pub fn panes_under(&self, dirs: &[&std::path::Path]) -> Vec<Uuid> {
        let under = |path: &std::path::Path| dirs.iter().any(|dir| path.starts_with(dir));
        self.sessions
            .iter()
            .flat_map(|s| &s.windows)
            .flat_map(|w| &w.panes)
            .filter(|p| {
                p.pty.local_cwd().is_some_and(|cwd| {
                    let cwd = std::path::Path::new(&cwd);
                    under(cwd) || under(&crate::pane_git::real_path(cwd))
                })
            })
            .map(|p| p.id)
            .collect()
    }

    /// `worktree-path`, where new worktrees go.
    pub fn worktree_path_template(&self) -> String {
        self.file_config
            .worktree_path
            .clone()
            .unwrap_or_else(|| crate::worktree::DEFAULT_PATH.to_string())
    }

    /// The folder an editor opens for `pane`: its worktree's root, else its
    /// cwd, unless that's on another host. Mirrors `workspaceFolder` in the frontend.
    pub fn editor_folder(&self, pane: &Pane) -> Option<String> {
        let cwd = pane.pty.local_cwd()?;
        Some(self.git.lookup(&cwd).map_or(cwd, |git| git.worktree_root))
    }

    pub fn find_pane_mut(&mut self, pane_id: Uuid) -> Option<&mut Pane> {
        for session in &mut self.sessions {
            for window in &mut session.windows {
                for pane in &mut window.panes {
                    if pane.id == pane_id {
                        return Some(pane);
                    }
                }
            }
        }
        None
    }

    pub fn set_agent_status(&mut self, pane_id: Uuid, status: AgentStatus) -> bool {
        if self.find_pane(pane_id).is_none() {
            return false;
        }
        if status.state == AgentState::Unknown {
            self.agents.remove(&pane_id);
        } else {
            let agent = AgentLifecycle::explicit(status, self.agents.get(&pane_id), Instant::now());
            self.insert_agent(pane_id, agent);
        }
        true
    }

    /// Mark a completed agent as seen by the shared btmux clients. Other
    /// lifecycle states remain untouched because focusing a pane does not mean
    /// that work has stopped or that a blocked prompt was answered.
    pub fn acknowledge_pane(&mut self, pane_id: Uuid) -> bool {
        let unread = self
            .find_pane_mut(pane_id)
            .and_then(|pane| pane.unread.take())
            .is_some();
        let Some(agent) = self.agents.get_mut(&pane_id) else {
            return unread;
        };
        if agent.status.state == AgentState::Done {
            agent.set_state(AgentState::Idle);
            agent.status.message = None;
            // Seen, so later records can't bring the result back.
            if let Some(pane) = self.find_pane(pane_id) {
                pane.pty.forget_program_results();
            }
            return true;
        }
        unread
    }

    pub fn has_session(&self, session_id: Uuid) -> bool {
        self.sessions.iter().any(|s| s.id == session_id)
    }

    fn session_mut(&mut self, session_id: Uuid) -> Option<&mut Session> {
        self.sessions.iter_mut().find(|s| s.id == session_id)
    }

    /// Splits `pane_id` in whichever window holds it, returning the new pane.
    /// `select` makes the new pane its window's active pane.
    pub async fn split_pane(
        &mut self,
        pane_id: Uuid,
        direction: &str,
        cwd: Option<std::path::PathBuf>,
        select: bool,
    ) -> Option<Uuid> {
        self.find_pane(pane_id)?;
        let new_pane_id = Uuid::new_v4();
        // Inherit the source pane's working directory (its spawn cwd until the
        // shell reports one), but only if that directory still exists.
        let cwd = cwd.or_else(|| {
            self.find_pane(pane_id)
                .and_then(|p| p.pty.effective_cwd())
                .map(std::path::PathBuf::from)
                .filter(|path| path.is_dir())
        });
        let pty = self.new_pty(new_pane_id, cwd, &self.pane_runtime(pane_id));
        let new_pane = Pane::new(new_pane_id, pty);

        let window = self
            .sessions
            .iter_mut()
            .flat_map(|s| s.windows.iter_mut())
            .find(|w| w.panes.iter().any(|p| p.id == pane_id))?;
        window
            .layout
            .find_and_split(pane_id, new_pane_id, direction);
        window.panes.push(new_pane);
        if select {
            window.prev_pane = Some(window.active_pane);
            window.active_pane = window.panes.len() - 1;
        }
        // As in tmux, a split unzooms so the new pane is visible.
        window.zoomed_pane = None;
        // A manual split breaks the preset arrangement; next-layout cycles fresh.
        window.last_preset = None;
        Some(new_pane_id)
    }

    pub fn kill_pane(&mut self, session_id: Uuid, pane_id: Uuid) {
        let removed = {
            let Some(session) = self.session_mut(session_id) else {
                return;
            };
            let window = &mut session.windows[session.active_window];
            if window.panes.len() <= 1 || !window.panes.iter().any(|pane| pane.id == pane_id) {
                return;
            }
            window.remove_pane(pane_id)
        };
        if let Some(pane) = removed {
            self.forget_pane(&pane);
        }
    }

    /// Runtime state of a pane that just left the tree.
    fn forget_pane(&mut self, pane: &Pane) {
        self.agents.remove(&pane.id);
        self.telemetry.pane_closed(pane.id, pane.pty.is_guest());
        if self.popups.close_pane(pane.id) {
            crate::popup::broadcast(self);
        }
    }

    pub async fn handle_pane_exit(&mut self, pane_id: Uuid) {
        if let Some(ended) = self.popups.exited(pane_id) {
            if ended {
                crate::popup::broadcast(self);
            }
            return;
        }
        let Some((si, wi)) = self.sessions.iter().enumerate().find_map(|(si, s)| {
            s.windows
                .iter()
                .position(|w| w.panes.iter().any(|p| p.id == pane_id))
                .map(|wi| (si, wi))
        }) else {
            return;
        };

        if let Some(pane) = self.sessions[si].windows[wi].remove_pane(pane_id) {
            self.forget_pane(&pane);
        }
        if !self.sessions[si].windows[wi].panes.is_empty() {
            return;
        }

        let session = &mut self.sessions[si];
        session.windows.remove(wi);
        if !session.windows.is_empty() {
            if wi < session.active_window {
                session.active_window -= 1;
            } else if session.active_window >= session.windows.len() {
                session.active_window = session.windows.len() - 1;
            }
            return;
        }

        self.sessions.remove(si);
        if self.sessions.is_empty() {
            self.create_session(Some("0".to_string())).await;
        }
    }

    pub fn navigate(&mut self, session_id: Uuid, direction: String) {
        let Some(session) = self.session_mut(session_id) else {
            return;
        };
        let window = &mut session.windows[session.active_window];
        let current_pane_id = window
            .zoomed_pane
            .unwrap_or(window.panes[window.active_pane].id);

        if let Some(next_id) = window.layout.navigate_from(current_pane_id, &direction) {
            if let Some(idx) = window.panes.iter().position(|p| p.id == next_id) {
                let prev = window.active_pane;
                if idx != prev {
                    window.prev_pane = Some(prev);
                    window.active_pane = idx;
                }
                window.zoomed_pane = None;
            }
        }
    }

    pub fn select_pane(&mut self, session_id: Uuid, pane_id: Uuid) {
        let Some(session) = self.session_mut(session_id) else {
            return;
        };
        let window = &mut session.windows[session.active_window];
        if let Some(idx) = window.panes.iter().position(|p| p.id == pane_id) {
            let prev = window.active_pane;
            if idx != prev {
                window.prev_pane = Some(prev);
                window.active_pane = idx;
            }
            if window.zoomed_pane != Some(pane_id) {
                window.zoomed_pane = None;
            }
        }
    }

    pub fn last_pane(&mut self, session_id: Uuid) {
        let Some(session) = self.session_mut(session_id) else {
            return;
        };
        let window = &mut session.windows[session.active_window];
        if let Some(prev) = window.prev_pane {
            if prev < window.panes.len() {
                window.prev_pane = Some(window.active_pane);
                window.active_pane = prev;
            }
        }
    }

    /// Cycle active pane forward (+1) or backward (-1) in layout order.
    pub fn cycle_pane(&mut self, session_id: Uuid, delta: i32) {
        let Some(session) = self.session_mut(session_id) else {
            return;
        };
        let window = &mut session.windows[session.active_window];
        let ids = window.layout.pane_ids();
        if ids.len() <= 1 {
            return;
        }
        let current_id = window.panes[window.active_pane].id;
        let Some(pos) = ids.iter().position(|id| *id == current_id) else {
            return;
        };
        let next_pos = ((pos as i32 + delta).rem_euclid(ids.len() as i32)) as usize;
        let next_id = ids[next_pos];
        if let Some(idx) = window.panes.iter().position(|p| p.id == next_id) {
            let prev = window.active_pane;
            if idx != prev {
                window.prev_pane = Some(prev);
                window.active_pane = idx;
            }
        }
    }

    /// Swap the active pane with the next (+1) or previous (-1) pane in layout order.
    pub fn swap_pane(&mut self, session_id: Uuid, delta: i32) {
        let Some(session) = self.session_mut(session_id) else {
            return;
        };
        let window = &mut session.windows[session.active_window];
        let ids = window.layout.pane_ids();
        if ids.len() <= 1 {
            return;
        }
        let current_id = window.panes[window.active_pane].id;
        let Some(pos) = ids.iter().position(|id| *id == current_id) else {
            return;
        };
        let other_pos = ((pos as i32 + delta).rem_euclid(ids.len() as i32)) as usize;
        let other_id = ids[other_pos];
        window.layout.swap_panes(current_id, other_id);
    }

    /// Re-arrange the active window's panes into a named preset layout
    /// (`even-horizontal`, `even-vertical`, `main-vertical`, `main-horizontal`,
    /// `tiled`). Panes keep their identity and order — only the layout tree is
    /// rebuilt — so PTYs and scrollback are untouched. A zoom is cleared (the
    /// new arrangement supersedes it). Records the preset so `next-layout` can
    /// continue the cycle from here.
    pub fn select_layout(&mut self, session_id: Uuid, preset: LayoutPreset) {
        let Some(session) = self.session_mut(session_id) else {
            return;
        };
        let window = &mut session.windows[session.active_window];
        // Preserve the on-screen left-to-right / top-to-bottom order of the
        // current tree so re-laying-out feels stable rather than scrambling panes.
        let ids = window.layout.pane_ids();
        if let Some(layout) = preset.build(&ids) {
            window.layout = layout;
            window.zoomed_pane = None;
            window.last_preset = Some(preset);
        }
    }

    /// Cycle the active window to the next preset layout (tmux `next-layout` /
    /// the Space binding). Starts at `even-horizontal` when no preset has been
    /// applied yet, otherwise advances from the last one.
    pub fn next_layout(&mut self, session_id: Uuid) {
        let next = self
            .session_mut(session_id)
            .map(|s| &s.windows[s.active_window])
            .and_then(|w| w.last_preset)
            .map_or(LayoutPreset::ALL[0], |p| p.next());
        self.select_layout(session_id, next);
    }

    pub async fn create_window(&mut self, session_id: Uuid) {
        self.create_window_named(session_id, None, None, true).await;
    }

    /// Returns the new window's pane. `select` makes it the active window.
    pub async fn create_window_named(
        &mut self,
        session_id: Uuid,
        name: Option<String>,
        cwd: Option<std::path::PathBuf>,
        select: bool,
    ) -> Option<Uuid> {
        let pane_id = Uuid::new_v4();
        let runtime = self
            .sessions
            .iter()
            .find(|s| s.id == session_id)?
            .runtime
            .clone();
        let pty = self.new_pty(pane_id, cwd, &runtime);
        let pane = Pane::new(pane_id, pty);
        let base = name.unwrap_or_else(|| self.runtime_shell_name(&runtime));

        let session = self.session_mut(session_id)?;
        let existing: Vec<_> = session.windows.iter().map(|w| w.name.as_str()).collect();
        let wname = unique_name_in(&existing, &base);
        let window = Window {
            id: Uuid::new_v4(),
            name: wname,
            panes: vec![pane],
            active_pane: 0,
            prev_pane: None,
            layout: Layout::Leaf { pane_id },
            zoomed_pane: None,
            last_preset: None,
        };
        session.windows.push(window);
        if select {
            session.active_window = session.windows.len() - 1;
        }
        Some(pane_id)
    }

    pub fn switch_window(&mut self, session_id: Uuid, index: i32) {
        let Some(session) = self.session_mut(session_id) else {
            return;
        };
        let len = session.windows.len();
        let prev = session.active_window;
        let next = resolve_index(prev, len, index);
        if next != prev {
            session.prev_window = Some(prev);
            session.active_window = next;
        }
    }

    pub fn last_window(&mut self, session_id: Uuid) {
        let Some(session) = self.session_mut(session_id) else {
            return;
        };
        if let Some(prev) = session.prev_window {
            if prev < session.windows.len() {
                session.prev_window = Some(session.active_window);
                session.active_window = prev;
            }
        }
    }

    pub fn rename_window(&mut self, session_id: Uuid, name: String) {
        let Some(session) = self.session_mut(session_id) else {
            return;
        };
        let window_id = session.windows[session.active_window].id;
        self.rename_window_by_id(window_id, name);
    }

    pub fn rename_window_by_id(&mut self, window_id: Uuid, name: String) {
        let Some(session) = self
            .sessions
            .iter_mut()
            .find(|s| s.windows.iter().any(|w| w.id == window_id))
        else {
            return;
        };
        let existing: Vec<_> = session
            .windows
            .iter()
            .filter(|w| w.id != window_id)
            .map(|w| w.name.as_str())
            .collect();
        let unique = unique_name_in(&existing, &name);
        if let Some(window) = session.windows.iter_mut().find(|w| w.id == window_id) {
            window.name = unique;
        }
    }

    /// Makes `window_id` its session's active window.
    pub fn select_window_by_id(&mut self, window_id: Uuid) -> bool {
        let Some((session_id, index)) = self.sessions.iter().find_map(|s| {
            s.windows
                .iter()
                .position(|w| w.id == window_id)
                .map(|i| (s.id, i))
        }) else {
            return false;
        };
        self.switch_window(session_id, index as i32);
        true
    }

    /// Makes `pane_id` the active pane and its window the active window.
    pub fn select_pane_by_id(&mut self, pane_id: Uuid) -> bool {
        let Some((session_id, window_id)) = self.sessions.iter().find_map(|s| {
            s.windows
                .iter()
                .find(|w| w.panes.iter().any(|p| p.id == pane_id))
                .map(|w| (s.id, w.id))
        }) else {
            return false;
        };
        self.select_window_by_id(window_id);
        self.select_pane(session_id, pane_id);
        true
    }

    /// tmux's kill-pane, for any pane: the last pane closes its window and the
    /// last window kills its session. Refuses the server's last pane.
    pub fn kill_pane_by_id(&mut self, pane_id: Uuid) -> Result<(), String> {
        let Some(session) = self.sessions.iter().find(|s| {
            s.windows
                .iter()
                .any(|w| w.panes.iter().any(|p| p.id == pane_id))
        }) else {
            return Err(format!("pane {pane_id} not found"));
        };
        let window = session
            .windows
            .iter()
            .find(|w| w.panes.iter().any(|p| p.id == pane_id))
            .unwrap();
        if window.panes.len() > 1 {
            let window_id = window.id;
            if let Some(pane) = self
                .sessions
                .iter_mut()
                .flat_map(|s| s.windows.iter_mut())
                .find(|w| w.id == window_id)
                .and_then(|window| window.remove_pane(pane_id))
            {
                self.forget_pane(&pane);
            }
        } else if session.windows.len() > 1 {
            self.kill_window(window.id);
        } else if self.sessions.len() > 1 {
            self.kill_session(session.id);
        } else {
            return Err("cannot kill the last pane".into());
        }
        Ok(())
    }

    pub fn close_window(&mut self, session_id: Uuid) {
        let removed_panes = {
            let Some(session) = self.session_mut(session_id) else {
                return;
            };
            if session.windows.len() <= 1 {
                return;
            }
            let removed = session.windows.remove(session.active_window);
            if session.active_window >= session.windows.len() {
                session.active_window = session.windows.len() - 1;
            }
            removed.panes
        };
        for pane in &removed_panes {
            self.forget_pane(pane);
        }
    }

    /// tmux-style kill for the control socket: the last pane closes its window
    /// and the last window kills its session (never the last session).
    pub fn kill_pane_cascading(&mut self, session_id: Uuid, pane_id: Uuid) {
        let Some(session) = self.sessions.iter().find(|s| s.id == session_id) else {
            return;
        };
        if session.windows[session.active_window].panes.len() > 1 {
            self.kill_pane(session_id, pane_id);
        } else {
            self.close_window_cascading(session_id);
        }
    }

    pub fn close_window_cascading(&mut self, session_id: Uuid) {
        let Some(session) = self.sessions.iter().find(|s| s.id == session_id) else {
            return;
        };
        if session.windows.len() > 1 {
            self.close_window(session_id);
        } else {
            self.kill_session(session_id);
        }
    }

    pub fn zoom_pane(&mut self, session_id: Uuid, pane_id: Uuid) {
        let Some(session) = self.session_mut(session_id) else {
            return;
        };
        let window = &mut session.windows[session.active_window];
        if window.zoomed_pane == Some(pane_id) {
            window.zoomed_pane = None;
        } else {
            window.zoomed_pane = Some(pane_id);
        }
    }

    pub fn rename_session(&mut self, session_id: Uuid, name: String) {
        let unique = self.unique_session_name(&name, Some(session_id));
        if let Some(session) = self.session_mut(session_id) {
            session.name = unique;
        }
    }

    pub fn resize_split(&mut self, session_id: Uuid, split_id: Uuid, ratio: f32) {
        let Some(session) = self.session_mut(session_id) else {
            return;
        };
        let window = &mut session.windows[session.active_window];
        window.layout.update_split_ratio(split_id, ratio);
    }

    pub fn kill_session(&mut self, id: Uuid) {
        if self.sessions.len() <= 1 {
            return;
        }
        if let Some(idx) = self.sessions.iter().position(|s| s.id == id) {
            let removed = self.sessions.remove(idx);
            for pane in removed.windows.iter().flat_map(|window| &window.panes) {
                self.forget_pane(pane);
            }
        }
    }

    /// Kill every session and start over with a single fresh default session.
    /// Dropping the `Session` values drops their `PtyHandle`s, which tears down
    /// the shells. The "always ≥1 session" invariant is preserved (mirrors
    /// `kill_session` refusing to remove the last one and `handle_pane_exit`
    /// recreating "0" when the tree empties), so this resets rather than empties.
    pub async fn clear_sessions(&mut self) {
        for session in std::mem::take(&mut self.sessions) {
            for pane in session.windows.into_iter().flat_map(|window| window.panes) {
                self.telemetry.pane_closed(pane.id, pane.pty.is_guest());
            }
        }
        self.agents.clear();
        if self.popups.close_all() {
            crate::popup::broadcast(self);
        }
        self.create_session(Some("0".to_string())).await;
    }

    /// Best-known working directory of a session's active pane (OSC-7 cwd, with
    /// spawn-cwd fallback). Used as the base directory for the
    /// `create-sessions-from-git-repos` command palette entry.
    pub fn active_pane_cwd(&self, session_id: Uuid) -> Option<std::path::PathBuf> {
        let session = self.sessions.iter().find(|s| s.id == session_id)?;
        let window = session.windows.get(session.active_window)?;
        let pane = window.panes.get(window.active_pane)?;
        pane.pty.effective_cwd().map(std::path::PathBuf::from)
    }

    /// Create one session per git repo in `layouts` (produced by
    /// `git::discover_repo_layouts`, which does the blocking filesystem/`git`
    /// work off the lock). Each repo becomes a session named after its directory,
    /// with one window per worktree, named after its branch (the
    /// first worktree is the initial window, the rest are added windows). Repos
    /// whose sanitized name already matches an existing session are skipped, so
    /// re-running is idempotent.
    pub async fn create_sessions_from_git_repos(&mut self, layouts: Vec<RepoLayout>) {
        for layout in layouts {
            if self.sessions.iter().any(|s| s.name == layout.name) {
                tracing::info!("session '{}' already exists, skipping", layout.name);
                continue;
            }

            // First worktree (if any) is the session's initial window; the rest
            // become extra windows. With no named worktrees, fall back to a
            // single window at the repo root.
            let (first_window_name, first_window_cwd, extra) = match layout.windows.split_first() {
                Some(((name, path), rest)) => (Some(name.clone()), path.clone(), rest),
                None => (None, layout.root.clone(), &[][..]),
            };

            let session_id = self
                .create_session_with_cwd_and_window(
                    Some(layout.name.clone()),
                    Some(first_window_cwd),
                    first_window_name,
                )
                .await;
            tracing::info!("created session '{}' ({})", layout.name, session_id);

            for (window_name, path) in extra {
                self.create_window_named(
                    session_id,
                    Some(window_name.clone()),
                    Some(path.clone()),
                    true,
                )
                .await;
            }
        }
    }

    /// Add a window for each worktree in `layout` to the session `session_id`,
    /// skipping any worktree whose sanitized branch name already exists as a
    /// window name in that session.
    pub async fn add_worktree_windows(&mut self, session_id: Uuid, layout: RepoLayout) {
        for (window_name, path) in layout.windows {
            let already_exists = self
                .sessions
                .iter()
                .find(|s| s.id == session_id)
                .map(|s| s.windows.iter().any(|w| w.name == window_name))
                .unwrap_or(false);
            if already_exists {
                tracing::info!("window '{}' already exists, skipping", window_name);
                continue;
            }
            self.create_window_named(session_id, Some(window_name), Some(path), true)
                .await;
        }
    }

    pub fn kill_window(&mut self, window_id: Uuid) {
        let mut removed = None;
        for session in &mut self.sessions {
            if let Some(wi) = session.windows.iter().position(|w| w.id == window_id) {
                if session.windows.len() <= 1 {
                    return;
                }
                removed = Some(session.windows.remove(wi));
                if wi < session.active_window {
                    session.active_window -= 1;
                } else if session.active_window >= session.windows.len() {
                    session.active_window = session.windows.len() - 1;
                }
                break;
            }
        }
        for pane in removed.map(|window| window.panes).unwrap_or_default() {
            self.forget_pane(&pane);
        }
    }

    /// Sandboxed sessions' mounts, with the instance they are mounted in.
    pub fn sandbox_mounts(&self) -> Vec<(std::path::PathBuf, String)> {
        let instance = crate::multipass::instance_name(&self.config.multipass_config);
        let mut mounts: Vec<_> = self
            .sessions
            .iter()
            .filter_map(|s| match &s.runtime {
                Runtime::Multipass { mounts, .. } => Some(mounts),
                Runtime::Local => None,
            })
            .flatten()
            .map(|m| (std::path::PathBuf::from(m), instance.clone()))
            .collect();
        mounts.sort();
        mounts.dedup();
        mounts
    }

    pub fn session_summaries(&self) -> Vec<SessionSummary> {
        self.sessions
            .iter()
            .map(|s| SessionSummary {
                id: s.id,
                name: s.name.clone(),
                runtime: s.runtime.clone(),
            })
            .collect()
    }

    pub fn pane_infos(&self) -> Vec<PaneInfo> {
        let mut infos = Vec::new();
        for session in &self.sessions {
            for (window_index, window) in session.windows.iter().enumerate() {
                let active_pane = window.panes.get(window.active_pane).map(|p| p.id);
                for (pane_index, pane_id) in window.layout.pane_ids().into_iter().enumerate() {
                    let Some(pane) = window.panes.iter().find(|p| p.id == pane_id) else {
                        continue;
                    };
                    infos.push(PaneInfo {
                        session_id: session.id,
                        session_name: session.name.clone(),
                        window_id: window.id,
                        window_index,
                        window_name: window.name.clone(),
                        window_active: window_index == session.active_window,
                        window_zoomed_flag: window.zoomed_pane.is_some(),
                        pane_id,
                        pane_index,
                        pane_active: active_pane == Some(pane_id),
                        pane_current_path: pane.pty.effective_cwd(),
                        pane_title: pane.pty.title.lock().unwrap().clone(),
                    });
                }
            }
        }
        infos
    }

    pub fn all_snapshots(&self) -> Vec<SessionSnapshot> {
        self.sessions
            .iter()
            .map(|s| self.session_snapshot(s))
            .collect()
    }

    /// Adds a pane's agent, which takes an age handed off for the pane.
    fn insert_agent(&mut self, pane_id: Uuid, mut agent: AgentLifecycle) {
        hand_off(
            &mut self.handed_since,
            self.handed_until,
            pane_id,
            &mut agent,
        );
        self.agents.insert(pane_id, agent);
    }

    /// When the pane's agent entered its current state, in Unix seconds.
    fn agent_since(&self, pane_id: Uuid) -> Option<u64> {
        self.agents
            .get(&pane_id)
            .filter(|agent| agent.is_active())
            .map(|agent| agent.since)
    }

    /// Rebuild the session tree from saved snapshots. Process state is *not*
    /// restored — each pane gets a fresh shell, spawned lazily on first
    /// `/ws/pane` connection in its saved cwd (if that directory still exists).
    /// Pane and window/session ids are preserved so the saved layout tree still
    /// resolves and any open tab's URL still points at a live session.
    ///
    /// Snapshots that are structurally empty (a window with no panes, or a
    /// session with no windows) are dropped. Returns the number of sessions
    /// actually restored, so the caller can fall back to a default session when
    /// nothing usable survived.
    pub fn restore_from_snapshots(&mut self, snapshots: Vec<SessionSnapshot>) -> usize {
        let shell = self.shell.clone();
        self.sessions = snapshots
            .into_iter()
            .filter_map(|s| self.build_session(&shell, s))
            .collect();
        self.sessions.len()
    }

    fn build_session(&self, shell: &str, snap: SessionSnapshot) -> Option<Session> {
        let windows: Vec<Window> = snap
            .windows
            .into_iter()
            .filter_map(|w| self.build_window(shell, w, &snap.runtime))
            .collect();
        if windows.is_empty() {
            return None;
        }
        let active_window = snap.active_window.min(windows.len() - 1);
        Some(Session {
            id: snap.id,
            name: snap.name,
            runtime: snap.runtime,
            windows,
            active_window,
            prev_window: None,
        })
    }

    fn build_window(&self, shell: &str, snap: WindowSnapshot, runtime: &Runtime) -> Option<Window> {
        let panes: Vec<Pane> = snap
            .panes
            .into_iter()
            .map(|p| {
                // Re-spawn into the saved cwd only if it still exists.
                let cwd = p
                    .cwd
                    .as_deref()
                    .map(std::path::PathBuf::from)
                    .filter(|path| path.is_dir());
                let pty = PtyHandle::new_with_cwd(
                    shell,
                    p.id,
                    &self.pty_env,
                    cwd,
                    self.scrollback_lines(),
                )
                .with_guest(self.guest(runtime));
                let mut pane = Pane::new(p.id, pty);
                pane.editor = p.editor;
                pane
            })
            .collect();
        if panes.is_empty() {
            return None;
        }
        let active_pane = snap.active_pane.min(panes.len() - 1);
        // Drop a stale zoom that points at a pane that's no longer present.
        let zoomed_pane = snap
            .zoomed_pane
            .filter(|z| panes.iter().any(|p| p.id == *z));
        Some(Window {
            id: snap.id,
            name: snap.name,
            panes,
            active_pane,
            prev_pane: None,
            layout: snap.layout,
            zoomed_pane,
            // Transient: the cycle starts fresh after a restore.
            last_preset: None,
        })
    }

    #[cfg(unix)]
    /// Stop every PTY reader and writer and export the session tree and live shells for
    /// an in-place upgrade. If a pane doesn't stop in time, every shell resumes.
    pub fn export_handoff(&mut self, timeout: Duration) -> Result<HandoffExport, String> {
        for pty in self.ptys_mut() {
            pty.stop_io();
        }
        let deadline = Instant::now() + timeout;
        let mut exported = Vec::new();
        let mut stuck = 0;
        for (id, pty) in self.ptys_with_ids_mut() {
            match pty.export_handoff(deadline) {
                Ok(Some((fd, handoff))) => exported.push((id, fd, handoff)),
                Ok(None) => {}
                Err(error) => {
                    tracing::warn!(%id, "cannot hand off shell: {error}");
                    stuck += 1;
                }
            }
        }
        if stuck > 0 {
            self.resume_handoff(exported);
            return Err(format!("{stuck} pane(s) did not stop in time"));
        }
        let handed_off = |id: Uuid| exported.iter().any(|(pane, _, _)| *pane == id);
        // A program reaped before its exit was handled keeps its status; one
        // still running without a reader can't be handed off and is hung up.
        let lost: Vec<_> = self
            .popups
            .iter()
            .filter(|popup| popup.pty.is_spawned() && !handed_off(popup.id))
            .map(|popup| popup.id)
            .collect();
        let ended = !lost.is_empty();
        for id in lost {
            if self.popups.exited(id) == Some(false) {
                self.popups
                    .failed(id, "the popup did not survive the upgrade".into());
            }
        }
        if ended {
            crate::popup::broadcast(self);
        }
        let popups = crate::popup::PopupsHandoff {
            open: self
                .popups
                .summaries()
                .into_iter()
                .filter(|popup| handed_off(popup.id))
                .collect(),
            starting: self.popups.export_starting(),
            ended: self.popups.export_ended(),
        };
        Ok((self.all_snapshots(), popups, exported))
    }

    #[cfg(unix)]
    fn ptys_with_ids_mut(&mut self) -> impl Iterator<Item = (Uuid, &mut PtyHandle)> {
        self.sessions
            .iter_mut()
            .flat_map(|s| &mut s.windows)
            .flat_map(|w| &mut w.panes)
            .map(|pane| (pane.id, &mut pane.pty))
            .chain(
                self.popups
                    .iter_mut()
                    .map(|popup| (popup.id, &mut popup.pty)),
            )
    }

    #[cfg(unix)]
    fn ptys_mut(&mut self) -> impl Iterator<Item = &mut PtyHandle> {
        self.ptys_with_ids_mut().map(|(_, pty)| pty)
    }

    #[cfg(unix)]
    /// Restart the shells `export_handoff` stopped, after a failed upgrade.
    pub fn resume_handoff(&mut self, panes: Vec<HandoffPane>) {
        let deadline = Instant::now() + Duration::from_secs(5);
        for (id, pty) in self.ptys_with_ids_mut() {
            if let Err(error) = pty.recover(deadline) {
                tracing::warn!(%id, "cannot resume shell: {error}");
            }
        }
        for (id, _, handoff) in panes {
            let pid = handoff.pid;
            let resumed = match self.find_pty_mut(id) {
                Some(pty) => pty.resume(handoff),
                None => Err("pane not found".into()),
            };
            if let Err(error) = resumed {
                tracing::warn!(%id, "cannot resume shell: {error}");
                // Its waiter reaps it and removes the pane.
                unsafe { libc::kill(pid, libc::SIGHUP) };
            }
        }
    }

    #[cfg(unix)]
    /// Rebuild the session tree from a handoff and adopt its running shells.
    pub fn restore_handoff(
        &mut self,
        sessions: Vec<SessionSnapshot>,
        popups: crate::popup::PopupsHandoff,
        panes: Vec<(Uuid, std::os::fd::OwnedFd, crate::pty::Handoff)>,
    ) -> usize {
        let unread: Vec<_> = sessions
            .iter()
            .flat_map(|s| &s.windows)
            .flat_map(|w| &w.panes)
            .filter_map(|p| Some((p.id, p.unread.clone()?)))
            .collect();
        self.handed_since = sessions
            .iter()
            .flat_map(|s| &s.windows)
            .flat_map(|w| &w.panes)
            .filter_map(|p| Some((p.id, (p.agent_status.state.clone(), p.agent_since?))))
            .collect();
        self.handed_until = Some(Instant::now() + HANDED_SINCE_WAIT);
        let restored = self.restore_from_snapshots(sessions);
        self.popups.restore_ended(popups.ended);
        let handed_off: Vec<_> = popups.open.iter().map(|popup| popup.id).collect();
        let starting = popups
            .starting
            .into_iter()
            .map(|starting| (starting.popup, Some((starting.spawn, starting.cwd))));
        let open = popups.open.into_iter().map(|popup| (popup, None));
        for (popup, spawn) in open.chain(starting) {
            if self.find_pane(popup.pane_id).is_none() {
                continue;
            }
            let pty = match spawn {
                Some((spawn, cwd)) => self.new_popup_pty(popup.id, popup.pane_id, cwd, Some(spawn)),
                None => self.new_popup_pty(popup.id, popup.pane_id, None, None),
            };
            self.popups.insert(crate::popup::Popup {
                id: popup.id,
                pane_id: popup.pane_id,
                title: popup.title,
                pty,
            });
        }
        let mut adopted = Vec::new();
        for (id, master, handoff) in panes {
            let pid = handoff.pid;
            let result = match self.find_pty_mut(id) {
                Some(pty) => pty.adopt(master, handoff),
                None => Err("pane not found".into()),
            };
            match result {
                Ok(()) => adopted.push(id),
                Err(error) => {
                    tracing::warn!(%id, "cannot adopt shell: {error}");
                    crate::pty::hang_up(pid);
                }
            }
        }
        let orphans: Vec<_> = self
            .popups
            .iter()
            .filter(|popup| handed_off.contains(&popup.id) && !adopted.contains(&popup.id))
            .map(|popup| popup.id)
            .collect();
        for id in orphans {
            self.popups
                .failed(id, "the popup did not survive the upgrade".into());
        }
        for (id, alert) in unread {
            if let Some(pane) = self.find_pane_mut(id) {
                pane.unread = Some(alert);
            }
        }
        restored
    }

    pub fn snapshot_by_id(&self, session_id: Uuid) -> Option<SessionSnapshot> {
        self.sessions
            .iter()
            .find(|s| s.id == session_id)
            .map(|s| self.session_snapshot(s))
    }

    fn session_snapshot(&self, session: &super::Session) -> SessionSnapshot {
        SessionSnapshot {
            id: session.id,
            name: session.name.clone(),
            runtime: session.runtime.clone(),
            windows: session
                .windows
                .iter()
                .map(|w| WindowSnapshot {
                    id: w.id,
                    name: w.name.clone(),
                    panes: w
                        .panes
                        .iter()
                        .map(|p| PaneSnapshot {
                            id: p.id,
                            title: p.pty.title.lock().unwrap().clone(),
                            cwd: p.pty.effective_cwd(),
                            cwd_remote: p.pty.effective_cwd().is_some()
                                && p.pty.local_cwd().is_none(),
                            agent_status: self.agent_status(p.id).cloned().unwrap_or_default(),
                            agent_id: self.agent_id(p.id),
                            agent_since: self.agent_since(p.id),
                            unread: p.unread.clone(),
                            git: p.pty.local_cwd().and_then(|cwd| self.git.lookup(&cwd)),
                            editor: p.editor.clone(),
                        })
                        .collect(),
                    active_pane: w.active_pane,
                    layout: w.layout.clone(),
                    zoomed_pane: w.zoomed_pane,
                })
                .collect(),
            active_window: session.active_window,
        }
    }
}

impl SessionManager {
    fn unique_session_name(&self, base: &str, exclude_id: Option<Uuid>) -> String {
        let existing: Vec<_> = self
            .sessions
            .iter()
            .filter(|s| Some(s.id) != exclude_id)
            .map(|s| s.name.as_str())
            .collect();
        unique_name_in(&existing, base)
    }

    fn unique_window_name_in(&self, existing: &[&str], base: &str) -> String {
        unique_name_in(existing, base)
    }
}

fn unique_name_in(existing: &[&str], base: &str) -> String {
    if !existing.contains(&base) {
        return base.to_string();
    }
    let mut n = 2u32;
    loop {
        let candidate = format!("{}-{}", base, n);
        if !existing.contains(&candidate.as_str()) {
            return candidate;
        }
        n += 1;
    }
}

fn shell_name(shell: &str) -> String {
    let program = crate::shell::argv(shell).into_iter().next();
    let program = program.as_deref().unwrap_or(shell);
    std::path::Path::new(program)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(program)
        .to_string()
}

fn resolve_index(current: usize, len: usize, index: i32) -> usize {
    if len == 0 {
        return current;
    }
    match index {
        -1 => (current + 1) % len,
        -2 => {
            if current == 0 {
                len - 1
            } else {
                current - 1
            }
        }
        i if i >= 0 && (i as usize) < len => i as usize,
        _ => current,
    }
}

#[cfg(test)]
mod pane_tests {
    use super::*;

    #[tokio::test]
    async fn splitting_an_unspawned_pane_inherits_its_spawn_cwd() {
        let (exit_tx, _) = mpsc::unbounded_channel();
        let (meta_tx, _) = mpsc::unbounded_channel();
        let mut mgr = SessionManager::new(
            "/bin/sh".to_string(),
            FileConfig::default(),
            exit_tx,
            meta_tx,
            8044,
            None,
        );
        let dir = std::env::temp_dir();
        mgr.create_session_with_cwd(None, Some(dir.clone())).await;
        let first = mgr.sessions[0].windows[0].panes[0].id;
        let second = mgr.split_pane(first, "v", None, false).await.unwrap();
        let pane = mgr.find_pane(second).unwrap();
        assert_eq!(
            pane.pty.effective_cwd(),
            Some(dir.to_string_lossy().into_owned())
        );
    }

    #[tokio::test]
    async fn selecting_a_pane_reveals_it_without_toggling_its_own_zoom() {
        let (exit_tx, _) = mpsc::unbounded_channel();
        let (meta_tx, _) = mpsc::unbounded_channel();
        let mut mgr = SessionManager::new(
            "/bin/sh".to_string(),
            FileConfig::default(),
            exit_tx,
            meta_tx,
            8044,
            None,
        );
        let session_id = mgr.create_session(None).await;
        let first = mgr.sessions[0].windows[0].panes[0].id;
        mgr.split_pane(first, "v", None, true).await;
        let second = mgr.sessions[0].windows[0].panes[1].id;
        mgr.select_pane(session_id, first);
        mgr.zoom_pane(session_id, first);

        mgr.select_pane(session_id, first);
        mgr.select_pane(session_id, Uuid::new_v4());
        let window = &mgr.sessions[0].windows[0];
        assert_eq!(window.active_pane, 0);
        assert_eq!(window.zoomed_pane, Some(first));

        mgr.select_pane(session_id, second);
        let window = &mgr.sessions[0].windows[0];
        assert_eq!(window.active_pane, 1);
        assert_eq!(window.prev_pane, Some(0));
        assert_eq!(window.zoomed_pane, None);

        mgr.select_pane(session_id, second);
        assert_eq!(mgr.sessions[0].windows[0].prev_pane, Some(0));

        // The active pane can also be hidden behind a different zoomed pane.
        mgr.zoom_pane(session_id, first);
        mgr.select_pane(session_id, second);
        let window = &mgr.sessions[0].windows[0];
        assert_eq!(window.active_pane, 1);
        assert_eq!(window.prev_pane, Some(0));
        assert_eq!(window.zoomed_pane, None);

        mgr.zoom_pane(session_id, second);
        mgr.select_pane(session_id, second);
        assert_eq!(mgr.sessions[0].windows[0].zoomed_pane, Some(second));
    }

    #[tokio::test]
    async fn splitting_a_zoomed_pane_unzooms_and_selects_the_new_pane() {
        let (exit_tx, _exit_rx) = tokio::sync::mpsc::unbounded_channel();
        let (meta_tx, _meta_rx) = tokio::sync::mpsc::unbounded_channel();
        let mut mgr = SessionManager::new(
            "/bin/sh".to_string(),
            FileConfig::default(),
            exit_tx,
            meta_tx,
            8044,
            None,
        );
        let session_id = mgr.create_session(None).await;
        let first = mgr.sessions[0].windows[0].panes[0].id;
        mgr.zoom_pane(session_id, first);

        let second = mgr.split_pane(first, "v", None, true).await.unwrap();
        let window = &mgr.sessions[0].windows[0];
        assert_eq!(window.zoomed_pane, None);
        assert_eq!(window.panes[window.active_pane].id, second);
        assert_eq!(window.prev_pane, Some(0));
    }

    #[tokio::test]
    async fn zoomed_navigation_follows_the_full_layout_and_unzooms() {
        let (exit_tx, _) = mpsc::unbounded_channel();
        let (meta_tx, _) = mpsc::unbounded_channel();
        let mut mgr = SessionManager::new(
            "/bin/sh".to_string(),
            FileConfig::default(),
            exit_tx,
            meta_tx,
            8044,
            None,
        );
        let session_id = mgr.create_session(None).await;
        let top_left = mgr.sessions[0].windows[0].panes[0].id;
        mgr.split_pane(top_left, "v", None, true).await;
        let top_right = mgr.sessions[0].windows[0].panes[1].id;
        mgr.split_pane(top_left, "h", None, true).await;
        let bottom_left = mgr.sessions[0].windows[0].panes[2].id;
        mgr.split_pane(top_right, "h", None, true).await;
        let bottom_right = mgr.sessions[0].windows[0].panes[3].id;
        mgr.select_pane(session_id, top_left);

        for (direction, expected) in [
            ("right", top_right),
            ("down", bottom_right),
            ("left", bottom_left),
            ("up", top_left),
            ("left", bottom_right),
        ] {
            let current =
                mgr.sessions[0].windows[0].panes[mgr.sessions[0].windows[0].active_pane].id;
            mgr.zoom_pane(session_id, current);
            let previous = mgr.sessions[0].windows[0].active_pane;
            mgr.navigate(session_id, direction.to_string());
            let window = &mgr.snapshot_by_id(session_id).unwrap().windows[0];
            assert_eq!(window.panes[window.active_pane].id, expected);
            assert_eq!(window.zoomed_pane, None);
            assert_eq!(mgr.sessions[0].windows[0].prev_pane, Some(previous));
        }

        let window = &mgr.snapshot_by_id(session_id).unwrap().windows[0];
        assert_eq!(window.layout.pane_ids().len(), 4);
    }
}

#[cfg(test)]
mod palette_tests {
    use super::*;

    #[test]
    fn theme_changes_update_the_pty_palette() {
        let (exit_tx, _) = mpsc::unbounded_channel();
        let (meta_tx, _) = mpsc::unbounded_channel();
        let mut mgr = SessionManager::new(
            "/bin/sh".to_string(),
            FileConfig::default(),
            exit_tx,
            meta_tx,
            8044,
            None,
        );
        assert!(mgr.pty_env.palette.get().unwrap().is_dark());
        mgr.apply_config_override(&ConfigUpdate {
            colors_dark: Some("btmux-default-light".into()),
            ..Default::default()
        });
        assert!(!mgr.pty_env.palette.get().unwrap().is_dark());
        mgr.reset_config_overrides();
        assert!(mgr.pty_env.palette.get().unwrap().is_dark());
    }

    #[test]
    fn config_error_persists_until_the_file_loads() {
        let (exit_tx, _) = mpsc::unbounded_channel();
        let (meta_tx, _) = mpsc::unbounded_channel();
        let mut mgr = SessionManager::new(
            "/bin/sh".to_string(),
            FileConfig::default(),
            exit_tx,
            meta_tx,
            8044,
            None,
        );
        assert_eq!(mgr.config().config_error, None);
        mgr.set_config_error("bad key".into());
        mgr.apply_config_override(&ConfigUpdate {
            animations: Some(false),
            ..Default::default()
        });
        mgr.reset_config_overrides();
        assert_eq!(mgr.config().config_error.as_deref(), Some("bad key"));
        mgr.set_file_config(FileConfig::default());
        assert_eq!(mgr.config().config_error, None);
    }

    #[test]
    fn auto_appearance_follows_the_system_color_mode() {
        let (exit_tx, _) = mpsc::unbounded_channel();
        let (meta_tx, _) = mpsc::unbounded_channel();
        let mut mgr = SessionManager::new(
            "/bin/sh".to_string(),
            FileConfig::default(),
            exit_tx,
            meta_tx,
            8044,
            None,
        );
        assert!(mgr.set_system_color_mode(ColorMode::Light).is_some());
        assert_eq!(mgr.config().color_mode, ColorMode::Light);
        assert!(!mgr.pty_env.palette.get().unwrap().is_dark());
        assert!(mgr.set_system_color_mode(ColorMode::Light).is_none());
        mgr.apply_config_override(&ConfigUpdate {
            appearance: Some(crate::config::Appearance::Dark),
            ..Default::default()
        });
        assert!(mgr.pty_env.palette.get().unwrap().is_dark());
        // A pinned appearance ignores the system.
        assert!(mgr.set_system_color_mode(ColorMode::Dark).is_none());
        assert!(mgr.set_system_color_mode(ColorMode::Light).is_none());
        mgr.reset_config_overrides();
        assert_eq!(mgr.config().color_mode, ColorMode::Light);
    }
}

#[cfg(test)]
mod kill_cascade_tests {
    use super::*;

    async fn manager() -> SessionManager {
        let (exit_tx, _) = mpsc::unbounded_channel();
        let (meta_tx, _) = mpsc::unbounded_channel();
        SessionManager::new(
            "/bin/sh".to_string(),
            FileConfig::default(),
            exit_tx,
            meta_tx,
            8044,
            None,
        )
    }

    #[tokio::test]
    async fn added_sandbox_mounts_reach_existing_panes() {
        let mut mgr = manager().await;
        let runtime = Runtime::Multipass {
            mounts: vec!["/src/app".into()],
            shell: crate::multipass::GuestShell::Bash,
        };
        let session = mgr.create_session_in(None, None, None, runtime).await;
        let pane = &mgr.sessions.last().unwrap().windows[0].panes[0];
        assert!(!pane.pty.guest().unwrap().mounted("/src/app-feat/lib"));

        mgr.add_sandbox_mounts(session, vec!["/src/app".into(), "/src/app-feat".into()]);
        let pane = &mgr.sessions.last().unwrap().windows[0].panes[0];
        assert!(pane.pty.guest().unwrap().mounted("/src/app-feat/lib"));
        assert_eq!(
            mgr.session_runtime(session),
            Some(&Runtime::Multipass {
                mounts: vec!["/src/app".into(), "/src/app-feat".into()],
                shell: crate::multipass::GuestShell::Bash,
            })
        );
    }

    #[tokio::test]
    async fn last_pane_closes_window_then_session_but_never_the_last_session() {
        let mut mgr = manager().await;
        let first = mgr.create_session(Some("a".into())).await;
        let second = mgr.create_session(Some("b".into())).await;
        mgr.create_window(first).await;
        assert_eq!(mgr.sessions[0].windows.len(), 2);

        let pane = mgr.sessions[0].windows[1].panes[0].id;
        mgr.kill_pane_cascading(first, pane);
        assert_eq!(mgr.sessions[0].windows.len(), 1);

        let pane = mgr.sessions[0].windows[0].panes[0].id;
        mgr.kill_pane_cascading(first, pane);
        assert_eq!(mgr.sessions.len(), 1);
        assert_eq!(mgr.sessions[0].id, second);

        mgr.close_window_cascading(second);
        assert_eq!(mgr.sessions.len(), 1);
    }

    #[tokio::test]
    async fn pane_operations_reach_inactive_windows() {
        let mut mgr = manager().await;
        let first = mgr.create_session(Some("a".into())).await;
        let pane = mgr.sessions[0].windows[0].panes[0].id;
        let second = mgr
            .create_window_named(first, None, None, false)
            .await
            .unwrap();
        assert_eq!(mgr.sessions[0].active_window, 0);

        let left = mgr.split_pane(second, "left", None, false).await.unwrap();
        let window = &mgr.sessions[0].windows[1];
        assert_eq!(window.layout.pane_ids(), vec![left, second]);
        assert_eq!(window.panes[window.active_pane].id, second);
        let infos = mgr.pane_infos();
        let info = infos.iter().find(|p| p.pane_id == left).unwrap();
        assert_eq!((info.window_index, info.pane_index), (1, 0));
        assert!(!info.window_active && !info.pane_active);

        assert!(mgr.select_pane_by_id(left));
        assert_eq!(mgr.sessions[0].active_window, 1);
        let window = &mgr.sessions[0].windows[1];
        assert_eq!(window.panes[window.active_pane].id, left);

        mgr.kill_pane_by_id(pane).unwrap();
        assert_eq!(mgr.sessions[0].windows.len(), 1);
        assert_eq!(mgr.sessions[0].active_window, 0);
        mgr.kill_pane_by_id(left).unwrap();
        assert!(mgr.kill_pane_by_id(second).is_err());
    }
}

#[cfg(test)]
mod agent_tests {
    use super::*;

    fn report(event: AgentEvent, session: &str) -> AgentReport {
        AgentReport {
            event,
            session_id: Some(session.to_string()),
            turn_id: None,
            tool: None,
            process: None,
            ancestors: Vec::new(),
            agent: Some("codex".to_string()),
            source: Some("hook".to_string()),
            message: None,
        }
    }

    #[tokio::test]
    async fn a_nested_agent_ending_keeps_its_parent_listed() {
        let (exit_tx, _) = tokio::sync::mpsc::unbounded_channel();
        let (meta_tx, _) = tokio::sync::mpsc::unbounded_channel();
        let mut mgr = SessionManager::new(
            "/bin/sh".into(),
            crate::config::FileConfig::default(),
            exit_tx,
            meta_tx,
            8004,
            None,
        );
        mgr.create_session(None).await;
        let pane_id = mgr.sessions[0].windows[0].panes[0].id;
        let parent = AgentProcess {
            pid: 10,
            start_time: 5,
        };
        let now = Instant::now();
        mgr.update_detected_agents(&[(pane_id, parent, "claude".to_string())], |_| true);
        let mut start = report(AgentEvent::SessionStart, "parent");
        start.process = Some(parent);
        mgr.apply_agent_report(pane_id, start, now);

        let mut nested = report(AgentEvent::SessionStart, "child");
        nested.process = Some(AgentProcess {
            pid: 12,
            start_time: 7,
        });
        nested.ancestors = vec![parent];
        assert_eq!(mgr.apply_agent_report(pane_id, nested, now), None);
        assert_eq!(
            mgr.apply_agent_report(pane_id, report(AgentEvent::SessionEnd, "child"), now),
            None
        );
        mgr.update_detected_agents(&[(pane_id, parent, "claude".to_string())], |_| true);
        assert_eq!(mgr.running_agent_panes(), vec![pane_id]);
    }

    #[tokio::test]
    async fn process_detection_fills_multiple_panes_and_clears_departed_agents() {
        let (exit_tx, _) = mpsc::unbounded_channel();
        let (meta_tx, _) = mpsc::unbounded_channel();
        let mut mgr = SessionManager::new(
            "/bin/sh".to_string(),
            FileConfig::default(),
            exit_tx,
            meta_tx,
            8044,
            None,
        );
        let first_session = mgr.create_session(None).await;
        let second_session = mgr.create_session(None).await;
        let first = mgr.snapshot_by_id(first_session).unwrap().windows[0].panes[0].id;
        let second = mgr.snapshot_by_id(second_session).unwrap().windows[0].panes[0].id;
        let one = AgentProcess {
            pid: 11,
            start_time: 1,
        };
        let two = AgentProcess {
            pid: 21,
            start_time: 2,
        };
        assert!(mgr.update_detected_agents(
            &[
                (first, one, "codex".to_string()),
                (second, two, "codex".to_string()),
            ],
            |_| true,
        ));
        assert_eq!(mgr.running_agent_panes().len(), 2);
        mgr.apply_agent_report(
            first,
            report(AgentEvent::TurnStart, "session"),
            Instant::now(),
        );
        assert!(mgr.update_detected_agents(&[(second, two, "codex".to_string())], |_| true));
        assert_eq!(mgr.running_agent_panes(), vec![second]);
    }

    #[test]
    fn the_most_recently_focused_client_gets_os_notifications() {
        let (exit_tx, _) = mpsc::unbounded_channel();
        let (meta_tx, _) = mpsc::unbounded_channel();
        let mut mgr = SessionManager::new(
            "/bin/sh".to_string(),
            FileConfig::default(),
            exit_tx,
            meta_tx,
            8044,
            None,
        );
        let browser = mgr.register_client();
        let desktop = mgr.register_client();
        assert_ne!(browser, desktop);
        assert_eq!(mgr.alert_client(), None);
        mgr.focus_client(browser);
        mgr.focus_client(desktop);
        assert_eq!(mgr.alert_client(), Some(desktop));
        mgr.release_client(browser);
        assert_eq!(mgr.alert_client(), Some(desktop));
        mgr.release_client(desktop);
        assert_eq!(mgr.alert_client(), None);
    }

    #[tokio::test]
    async fn grid_and_snapshot_follow_the_same_lifecycle() {
        let (exit_tx, _) = mpsc::unbounded_channel();
        let (meta_tx, _) = mpsc::unbounded_channel();
        let mut mgr = SessionManager::new(
            "/bin/sh".to_string(),
            FileConfig::default(),
            exit_tx,
            meta_tx,
            8044,
            None,
        );
        let session_id = mgr.create_session(None).await;
        let pane_id = mgr.sessions[0].windows[0].panes[0].id;
        assert!(mgr.running_agent_panes().is_empty());
        mgr.apply_agent_report(
            pane_id,
            report(AgentEvent::SessionStart, "first"),
            Instant::now(),
        );
        assert_eq!(mgr.running_agent_panes(), vec![pane_id]);
        assert_eq!(
            mgr.snapshot_by_id(session_id).unwrap().windows[0].panes[0]
                .agent_status
                .state,
            AgentState::Idle
        );
        mgr.apply_agent_report(
            pane_id,
            report(AgentEvent::SessionStart, "second"),
            Instant::now(),
        );
        assert_eq!(
            mgr.apply_agent_report(
                pane_id,
                report(AgentEvent::SessionEnd, "first"),
                Instant::now()
            ),
            None
        );
        assert_eq!(mgr.running_agent_panes(), vec![pane_id]);
        mgr.apply_agent_report(
            pane_id,
            report(AgentEvent::SessionEnd, "second"),
            Instant::now(),
        );
        assert!(mgr.running_agent_panes().is_empty());
        assert_eq!(mgr.agent_status(pane_id), None);
        assert_eq!(
            mgr.apply_agent_report(
                pane_id,
                report(AgentEvent::TurnFinished, "second"),
                Instant::now()
            ),
            None
        );
        assert!(mgr.running_agent_panes().is_empty());
        mgr.set_agent_status(
            pane_id,
            AgentStatus {
                state: AgentState::Working,
                agent: Some("custom".to_string()),
                source: Some("api".to_string()),
                message: None,
                background: None,
            },
        );
        assert_eq!(mgr.running_agent_panes(), vec![pane_id]);
        mgr.set_agent_status(pane_id, AgentStatus::default());
        assert!(mgr.running_agent_panes().is_empty());
    }

    #[tokio::test]
    async fn agent_since_changes_only_with_the_state() {
        let (exit_tx, _) = mpsc::unbounded_channel();
        let (meta_tx, _) = mpsc::unbounded_channel();
        let mut mgr = SessionManager::new(
            "/bin/sh".into(),
            FileConfig::default(),
            exit_tx,
            meta_tx,
            8044,
            None,
        );
        let session = mgr.create_session(None).await;
        let pane_id = mgr.snapshot_by_id(session).unwrap().windows[0].panes[0].id;
        let since = |mgr: &SessionManager| mgr.all_snapshots()[0].windows[0].panes[0].agent_since;
        let status = |state, message: &str| AgentStatus {
            state,
            agent: Some("custom".into()),
            source: Some("api".into()),
            message: Some(message.into()),
            background: None,
        };
        assert_eq!(since(&mgr), None);
        mgr.set_agent_status(pane_id, status(AgentState::Working, "a"));
        assert!(since(&mgr).is_some());
        // Backdate it, as if it had worked a while.
        mgr.agents.get_mut(&pane_id).unwrap().since = 1;
        mgr.set_agent_status(pane_id, status(AgentState::Working, "b"));
        assert_eq!(since(&mgr), Some(1));
        mgr.set_agent_status(pane_id, status(AgentState::Blocked, "b"));
        assert!(since(&mgr).unwrap() > 1);
        mgr.set_agent_status(pane_id, AgentStatus::default());
        assert_eq!(since(&mgr), None);
    }

    #[tokio::test]
    async fn a_replacement_agent_starts_a_new_age() {
        let (exit_tx, _) = mpsc::unbounded_channel();
        let (meta_tx, _) = mpsc::unbounded_channel();
        let mut mgr = SessionManager::new(
            "/bin/sh".into(),
            FileConfig::default(),
            exit_tx,
            meta_tx,
            8044,
            None,
        );
        let session = mgr.create_session(None).await;
        let pane_id = mgr.snapshot_by_id(session).unwrap().windows[0].panes[0].id;
        let since = |mgr: &SessionManager| mgr.all_snapshots()[0].windows[0].panes[0].agent_since;
        let now = Instant::now();
        mgr.apply_agent_report(pane_id, report(AgentEvent::TurnStart, "first"), now);
        let first = mgr.agent_id(pane_id);
        mgr.agents.get_mut(&pane_id).unwrap().since = 1;
        // The same agent keeps its age.
        mgr.apply_agent_report(pane_id, report(AgentEvent::TurnStart, "first"), now);
        assert_eq!(since(&mgr), Some(1));
        // Another one in the same state is announced and starts over.
        assert_eq!(
            mgr.apply_agent_report(pane_id, report(AgentEvent::TurnStart, "second"), now),
            Some(true)
        );
        assert_ne!(mgr.agent_id(pane_id), first);
        assert!(since(&mgr).unwrap() > 1);
    }

    #[tokio::test]
    async fn an_upgrade_keeps_the_age_of_an_unchanged_state() {
        let (exit_tx, _) = mpsc::unbounded_channel();
        let (meta_tx, _) = mpsc::unbounded_channel();
        let mut mgr = SessionManager::new(
            "/bin/sh".into(),
            FileConfig::default(),
            exit_tx,
            meta_tx,
            8044,
            None,
        );
        let session = mgr.create_session(None).await;
        let pane_id = mgr.snapshot_by_id(session).unwrap().windows[0].panes[0].id;
        let since = |mgr: &SessionManager| mgr.all_snapshots()[0].windows[0].panes[0].agent_since;
        let status = |state| AgentStatus {
            state,
            agent: Some("custom".into()),
            source: Some("api".into()),
            message: None,
            background: None,
        };
        let hand_off = |mgr: &mut SessionManager, state| {
            mgr.handed_since.insert(pane_id, (state, 5));
            mgr.handed_until = Some(Instant::now() + HANDED_SINCE_WAIT);
        };
        hand_off(&mut mgr, AgentState::Blocked);
        mgr.set_agent_status(pane_id, status(AgentState::Blocked));
        assert_eq!(since(&mgr), Some(5));
        // A repeated report is a new instance in the same state.
        mgr.set_agent_status(pane_id, status(AgentState::Blocked));
        assert_eq!(since(&mgr), Some(5));
        mgr.set_agent_status(pane_id, status(AgentState::Working));
        assert!(since(&mgr).unwrap() > 5);
        mgr.set_agent_status(pane_id, status(AgentState::Blocked));
        assert!(since(&mgr).unwrap() > 5);
        assert!(mgr.handed_since.is_empty());

        // Another state drops it; so does the deadline.
        mgr.set_agent_status(pane_id, AgentStatus::default());
        hand_off(&mut mgr, AgentState::Blocked);
        mgr.set_agent_status(pane_id, status(AgentState::Working));
        assert!(since(&mgr).unwrap() > 5);
        mgr.set_agent_status(pane_id, AgentStatus::default());
        hand_off(&mut mgr, AgentState::Blocked);
        mgr.handed_until = Some(Instant::now() - Duration::from_secs(1));
        mgr.set_agent_status(pane_id, status(AgentState::Blocked));
        assert!(since(&mgr).unwrap() > 5);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn program_status_names_an_agent_until_cleared() {
        let (exit_tx, _) = mpsc::unbounded_channel();
        let (meta_tx, _) = mpsc::unbounded_channel();
        let mut mgr = SessionManager::new(
            "/bin/sh".into(),
            FileConfig::default(),
            exit_tx,
            meta_tx,
            8044,
            None,
        );
        let session = mgr.create_session(None).await;
        let pane_id = mgr.snapshot_by_id(session).unwrap().windows[0].panes[0].id;
        let pane = mgr.find_pane_mut(pane_id).unwrap();
        pane.pty.ensure_spawned(80, 24).unwrap();
        // A program in the foreground reports "deploy?" in base64, then clears.
        let report =
            b"sh -c 'printf \"\\033]7501;state=blocked:app=deploy:msg=ZGVwbG95Pw\\033\\\\\\\\\"; \
            read x; printf \"\\033]7501;state=clear\\007\"; read x'\n";
        pane.pty.input_tx.send(report.to_vec()).unwrap();
        let state = |mgr: &mut SessionManager| {
            mgr.sync_program_status(Instant::now());
            mgr.agent_status(pane_id).cloned()
        };
        let deadline = Instant::now() + Duration::from_secs(5);
        while state(&mut mgr).is_none() && Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        let status = state(&mut mgr).unwrap();
        assert_eq!(status.state, AgentState::Blocked);
        assert_eq!(status.agent.as_deref(), Some("deploy"));
        assert_eq!(status.message.as_deref(), Some("deploy?"));
        assert!(!mgr.sync_program_status(Instant::now()));

        let input = |mgr: &SessionManager, bytes: &[u8]| {
            mgr.find_pane(pane_id)
                .unwrap()
                .pty
                .input_tx
                .send(bytes.to_vec())
                .unwrap()
        };
        input(&mgr, b"\n");
        while state(&mut mgr).is_some() && Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert_eq!(state(&mut mgr), None);
        input(&mgr, b"\n");

        // A result outlives the program at the prompt until acknowledged.
        input(&mgr, b"printf '\\033]7501;state=done:app=build\\007'\n");
        while state(&mut mgr).is_none() && Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        let grace = Duration::from_secs(3);
        let later = Instant::now() + grace;
        assert!(!mgr.end_agents_at_prompt(later, grace));
        assert_eq!(state(&mut mgr).unwrap().state, AgentState::Done);
        assert!(mgr.acknowledge_pane(pane_id));
        // Acknowledged, it is gone from the records too.
        assert_eq!(mgr.find_pane(pane_id).unwrap().pty.program_summary(), None);
        assert!(mgr.end_agents_at_prompt(later, grace));
        assert_eq!(state(&mut mgr), None);
    }

    #[tokio::test]
    async fn hook_only_agent_ends_at_shell_prompt_and_reset_forgets_it() {
        let (exit_tx, _) = mpsc::unbounded_channel();
        let (meta_tx, _) = mpsc::unbounded_channel();
        let mut mgr = SessionManager::new(
            if cfg!(windows) { "cmd.exe" } else { "/bin/sh" }.into(),
            FileConfig::default(),
            exit_tx,
            meta_tx,
            8044,
            None,
        );
        let session = mgr.create_session(None).await;
        let pane_id = mgr.snapshot_by_id(session).unwrap().windows[0].panes[0].id;
        mgr.find_pane_mut(pane_id)
            .unwrap()
            .pty
            .ensure_spawned(80, 24)
            .unwrap();
        let now = Instant::now();
        // A long-lived process (here, this one) stands in for Codex's server.
        let mut start = report(AgentEvent::SessionStart, "first");
        start.process = Some(AgentProcess {
            pid: std::process::id(),
            start_time: 1,
        });
        mgr.apply_agent_report(pane_id, start, now);
        assert_eq!(mgr.running_agent_panes(), vec![pane_id]);
        let grace = Duration::from_secs(3);
        assert!(!mgr.end_agents_at_prompt(now, grace));
        // The new shell may not be in the foreground yet.
        let mut ended = false;
        for _ in 0..200 {
            ended = mgr.end_agents_at_prompt(now + grace, grace);
            if ended {
                break;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        assert!(ended);
        assert!(mgr.running_agent_panes().is_empty());

        mgr.apply_agent_report(pane_id, report(AgentEvent::SessionStart, "second"), now);
        assert_eq!(mgr.running_agent_panes(), vec![pane_id]);
        assert!(mgr.reset_agent(pane_id));
        assert!(mgr.running_agent_panes().is_empty());
        assert!(!mgr.reset_agent(pane_id));
    }
}
