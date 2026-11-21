//! tmux-style subcommands (`btmux send-keys`, `split-window`, ...) for shell
//! scripts and agents. They resolve tmux targets against `GET /api/panes`.

use std::time::Duration;

use clap::{ArgAction, Subcommand};
use reqwest::Method;
use serde_json::{json, Value};
use uuid::Uuid;

use crate::api_client::request;
use crate::pty::screen::MAX_CAPTURE_HISTORY;
use crate::session::PaneInfo;

const TIMEOUT: Duration = Duration::from_secs(10);

const TARGET: &str = "A pane, window, or session ID, or SESSION[:WINDOW[.PANE]] by name or index; \
an empty part is the current one (:1, :.2). Default: this pane";

#[derive(Subcommand, Clone)]
pub enum TmuxCommand {
    /// List sessions as JSON, or one line each with a -F format.
    #[command(visible_alias = "ls")]
    ListSessions {
        /// Line format, e.g. '#{session_name}'.
        #[arg(short = 'F')]
        format: Option<String>,
    },
    /// List the target session's windows (-a: all) as JSON, or with -F.
    #[command(visible_alias = "lsw")]
    ListWindows {
        #[arg(short = 'a')]
        all: bool,
        #[arg(short = 't', help = TARGET)]
        target: Option<String>,
        #[arg(short = 'F')]
        format: Option<String>,
    },
    /// List the target window's panes (-s: session, -a: all) as JSON, or with -F.
    #[command(visible_alias = "lsp")]
    ListPanes {
        #[arg(short = 'a')]
        all: bool,
        #[arg(short = 's')]
        session: bool,
        #[arg(short = 't', help = TARGET)]
        target: Option<String>,
        #[arg(short = 'F')]
        format: Option<String>,
    },
    /// Create a session and print its pane's ID.
    #[command(visible_alias = "new")]
    NewSession {
        #[arg(short = 's')]
        name: Option<String>,
        /// Window name.
        #[arg(short = 'n')]
        window_name: Option<String>,
        /// Working directory.
        #[arg(short = 'c')]
        cwd: Option<String>,
        /// Print with this format instead of the pane ID.
        #[arg(short = 'F')]
        format: Option<String>,
        #[arg(short = 'd', hide = true)]
        _detached: bool,
        #[arg(short = 'P', hide = true)]
        _print: bool,
    },
    /// Create a window in the target session and print its pane's ID. With
    /// COMMAND, its pane runs that instead of the shell and closes when it
    /// exits (unless --keep-open).
    #[command(visible_alias = "neww")]
    NewWindow {
        #[arg(short = 't', help = TARGET)]
        target: Option<String>,
        #[arg(short = 'n')]
        name: Option<String>,
        /// Working directory; with COMMAND, the current one by default.
        #[arg(short = 'c')]
        cwd: Option<String>,
        /// Don't make it the active window.
        #[arg(short = 'd')]
        detached: bool,
        /// Print with this format instead of the pane ID.
        #[arg(short = 'F')]
        format: Option<String>,
        #[arg(short = 'P', hide = true)]
        _print: bool,
        #[command(flatten)]
        launch: Launch,
    },
    /// Split the target pane and print the new pane's ID. With COMMAND, the
    /// new pane runs that instead of the shell and closes when it exits
    /// (unless --keep-open).
    #[command(visible_alias = "splitw", disable_help_flag = true)]
    SplitWindow {
        #[arg(short = 't', help = TARGET)]
        target: Option<String>,
        /// Side by side (new pane on the right).
        #[arg(short = 'h', conflicts_with = "vertical")]
        horizontal: bool,
        /// Stacked (new pane below); the default.
        #[arg(short = 'v')]
        vertical: bool,
        /// Put the new pane left or above.
        #[arg(short = 'b')]
        before: bool,
        /// Don't make the new pane active.
        #[arg(short = 'd')]
        detached: bool,
        /// Working directory; default the target pane's, or with COMMAND the current one.
        #[arg(short = 'c')]
        cwd: Option<String>,
        /// Print with this format instead of the pane ID.
        #[arg(short = 'F')]
        format: Option<String>,
        #[arg(short = 'P', hide = true)]
        _print: bool,
        #[arg(long, action = ArgAction::Help)]
        help: Option<bool>,
        #[command(flatten)]
        launch: Launch,
    },
    /// Wait for the target pane's program (its command, or its shell) to
    /// exit, and exit with its status: 128+N if signal N ended it, 1 if the
    /// pane closed before it reported one, 124 on timeout. A pane that is kept
    /// open, or closed in the last 5 minutes, reports the status it exited with.
    WaitExit {
        /// A pane ID (also of a closed pane) or tmux target. Default: this pane
        #[arg(short = 't')]
        target: Option<String>,
        /// Seconds to wait, at most a year; without it, wait until the program exits.
        #[arg(
            long,
            value_name = "SECONDS",
            value_parser = clap::value_parser!(u64).range(..=crate::pane_exit::MAX_TIMEOUT_SECS)
        )]
        timeout: Option<u64>,
    },
    /// Kill the target pane. Its window closes with its last pane, and the
    /// session with its last window.
    #[command(visible_alias = "killp")]
    KillPane {
        #[arg(short = 't', help = TARGET)]
        target: Option<String>,
    },
    /// Kill the target window, and the session with its last window.
    #[command(visible_alias = "killw")]
    KillWindow {
        #[arg(short = 't', help = TARGET)]
        target: Option<String>,
    },
    /// Kill the target session.
    KillSession {
        #[arg(short = 't', help = TARGET)]
        target: Option<String>,
    },
    /// Make the target pane active, and its window.
    #[command(visible_alias = "selectp")]
    SelectPane {
        #[arg(short = 't', help = TARGET)]
        target: Option<String>,
    },
    /// Make the target window active.
    #[command(visible_alias = "selectw")]
    SelectWindow {
        #[arg(short = 't', help = TARGET)]
        target: Option<String>,
    },
    /// Rename the target window.
    #[command(visible_alias = "renamew")]
    RenameWindow {
        #[arg(short = 't', help = TARGET)]
        target: Option<String>,
        name: String,
    },
    /// Rename the target session.
    #[command(visible_alias = "rename")]
    RenameSession {
        #[arg(short = 't', help = TARGET)]
        target: Option<String>,
        name: String,
    },
    /// Type into the target pane. Each argument is a key name (Enter, C-c,
    /// M-x, Up, F5, ...) or else literal text; no spaces are added between
    /// arguments.
    #[command(visible_alias = "send")]
    SendKeys {
        #[arg(short = 't', help = TARGET)]
        target: Option<String>,
        /// Send every argument as literal text.
        #[arg(short = 'l')]
        literal: bool,
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        keys: Vec<String>,
    },
    /// Print the target pane as rendered text, with wrapped lines joined.
    #[command(visible_alias = "capturep")]
    CapturePane {
        #[arg(short = 't', help = TARGET)]
        target: Option<String>,
        /// History to include: '-' for all, -N for N lines above the screen.
        #[arg(short = 'S', allow_hyphen_values = true)]
        start: Option<String>,
        #[arg(short = 'E', allow_hyphen_values = true, hide = true)]
        end: Option<String>,
        #[arg(short = 'p', hide = true)]
        _print: bool,
        #[arg(short = 'J', hide = true)]
        _join: bool,
    },
    /// Create a git worktree for BRANCH beside the target pane's repo (the
    /// `worktree-path` setting), open a window there in its session, and
    /// print its pane's ID. An existing local or remote branch is checked
    /// out; a new one starts at the target pane's HEAD.
    NewWorktree {
        #[arg(short = 't', help = TARGET)]
        target: Option<String>,
        /// Don't make it the active window.
        #[arg(short = 'd')]
        detached: bool,
        /// Print with this format instead of the pane ID.
        #[arg(short = 'F')]
        format: Option<String>,
        branch: String,
    },
    /// Remove the linked git worktree the target pane is in, close every pane
    /// in it, and print its path. Refuses uncommitted changes and panes
    /// running a program; the branch is kept.
    RemoveWorktree {
        #[arg(short = 't', help = TARGET)]
        target: Option<String>,
    },
}

/// A program for a new pane, as `new-window` and `split-window` take it.
#[derive(clap::Args, Clone)]
pub struct Launch {
    /// Set an environment variable for COMMAND, as NAME=VALUE. Repeatable.
    #[arg(short = 'e', value_name = "NAME=VALUE", requires = "command")]
    env: Vec<String>,
    /// Keep the pane, with its output, after its program exits.
    #[arg(long)]
    keep_open: bool,
    /// After printing, wait for the program to exit and exit with its status,
    /// as `wait-exit` does.
    #[arg(long)]
    wait: bool,
    /// The program and its arguments, run now; a single argument is a command
    /// line for the shell. Default: the shell.
    #[arg(
        trailing_var_arg = true,
        allow_hyphen_values = true,
        value_name = "COMMAND"
    )]
    command: Vec<String>,
}

impl Launch {
    /// Request fields for the pane's program, run with this process's
    /// environment, as `btmux popup` runs one.
    fn fields(&self, cwd: Option<String>) -> Result<Value, String> {
        if self.command.is_empty() {
            return Ok(json!({ "cwd": absolute(cwd)?, "keep_open": self.keep_open }));
        }
        let extra = crate::popup::parse_env(&self.env)?;
        let cwd = match absolute(cwd)? {
            Some(cwd) => cwd,
            None => std::env::current_dir()
                .map_err(|e| e.to_string())?
                .to_string_lossy()
                .into_owned(),
        };
        Ok(json!({
            "cwd": cwd,
            "command": crate::popup::command_of(&self.command),
            "env": crate::popup::caller_env(&extra),
            "keep_open": self.keep_open,
        }))
    }
}

/// What a command printed, and the pane to wait for after printing it.
struct Reply {
    text: String,
    wait: Option<(Uuid, Option<Duration>)>,
}

impl From<String> for Reply {
    fn from(text: String) -> Self {
        Self { text, wait: None }
    }
}

/// Runs the command. Returns the exit code.
pub async fn run(profile: Option<&str>, command: TmuxCommand) -> i32 {
    // Outside its own server, this pane's ID means nothing.
    let current = profile
        .is_none()
        .then(|| std::env::var("BTMUX_PANE_ID").ok())
        .flatten()
        .and_then(|id| Uuid::parse_str(&id).ok());
    let client = Client { profile, current };
    match client.execute(command).await {
        Ok(Reply { text, wait }) => {
            print!("{text}");
            let Some((pane, timeout)) = wait else {
                return 0;
            };
            let _ = std::io::Write::flush(&mut std::io::stdout());
            crate::pane_exit::wait_cli(profile, pane, timeout).await
        }
        Err(error) => {
            eprintln!("btmux: {error}");
            1
        }
    }
}

/// The pane `target` names, or without one this pane.
pub async fn resolve_pane(profile: Option<&str>, target: Option<&str>) -> Result<Uuid, String> {
    // This pane's ID means nothing to a server named with `--server`.
    let current = (profile.is_none() && crate::send_notification::SERVER_URL.get().is_none())
        .then(|| std::env::var("BTMUX_PANE_ID").ok())
        .flatten()
        .and_then(|id| Uuid::parse_str(&id).ok());
    match (target, current) {
        // Guest panes can't list panes.
        (None, Some(current)) => Ok(current),
        (None, None) => Err("this must run in a btmux pane, or name one with -t".into()),
        (Some(target), _) => {
            let client = Client { profile, current };
            Ok(client.target(Some(target)).await?.pane_id)
        }
    }
}

struct Client<'a> {
    profile: Option<&'a str>,
    current: Option<Uuid>,
}

impl Client<'_> {
    async fn execute(&self, command: TmuxCommand) -> Result<Reply, String> {
        use TmuxCommand::*;
        match command {
            NewWindow {
                target,
                name,
                cwd,
                detached,
                format,
                launch,
                ..
            } => {
                let session = self.target(target.as_deref()).await?.session_id;
                let mut body = launch.fields(cwd)?;
                body["name"] = json!(name);
                body["select"] = json!(!detached);
                let path = format!("/api/sessions/{session}/windows");
                let created = self.call(Method::POST, &path, Some(body)).await?;
                self.print_created(&created, format.as_deref(), launch.wait)
                    .await
            }
            SplitWindow {
                target,
                horizontal,
                before,
                detached,
                cwd,
                format,
                launch,
                ..
            } => {
                let pane = self.target(target.as_deref()).await?.pane_id;
                let direction = match (horizontal, before) {
                    (true, false) => "right",
                    (true, true) => "left",
                    (false, false) => "down",
                    (false, true) => "up",
                };
                let mut body = launch.fields(cwd)?;
                body["direction"] = json!(direction);
                body["select"] = json!(!detached);
                let path = format!("/api/panes/{pane}/split");
                let created = self.call(Method::POST, &path, Some(body)).await?;
                self.print_created(&created, format.as_deref(), launch.wait)
                    .await
            }
            WaitExit { target, timeout } => {
                // A closed pane is no longer listed, but its status is kept.
                let pane = match target.as_deref().map(Uuid::parse_str) {
                    Some(Ok(pane)) => pane,
                    _ => self.target(target.as_deref()).await?.pane_id,
                };
                Ok(Reply {
                    text: String::new(),
                    wait: Some((pane, timeout.map(Duration::from_secs))),
                })
            }
            command => self.execute_text(command).await.map(Reply::from),
        }
    }

    async fn execute_text(&self, command: TmuxCommand) -> Result<String, String> {
        use TmuxCommand::*;
        match command {
            NewWindow { .. } | SplitWindow { .. } | WaitExit { .. } => unreachable!(),
            ListSessions { format } => {
                let panes = self.panes().await?;
                Ok(print_rows(
                    sessions(&panes),
                    format.as_deref(),
                    &["session_"],
                ))
            }
            ListWindows {
                all,
                target,
                format,
            } => {
                let panes = self.panes().await?;
                let scope = self.scope(&panes, all, target.as_deref())?;
                let panes: Vec<_> = panes
                    .iter()
                    .filter(|p| scope.is_none_or(|s| s.session_id == p.session_id))
                    .cloned()
                    .collect();
                Ok(print_rows(
                    windows(&panes),
                    format.as_deref(),
                    &["session_", "window_"],
                ))
            }
            ListPanes {
                all,
                session,
                target,
                format,
            } => {
                let panes = self.panes().await?;
                let scope = self.scope(&panes, all, target.as_deref())?;
                let rows = panes
                    .iter()
                    .filter(|p| match scope {
                        None => true,
                        Some(s) if session => s.session_id == p.session_id,
                        Some(s) => s.window_id == p.window_id,
                    })
                    .map(|p| serde_json::to_value(p).unwrap())
                    .collect();
                Ok(print_rows(rows, format.as_deref(), &[]))
            }
            NewSession {
                name,
                window_name,
                cwd,
                format,
                ..
            } => {
                let body =
                    json!({ "name": name, "window_name": window_name, "cwd": absolute(cwd)? });
                let created = self.call(Method::POST, "/api/sessions", Some(body)).await?;
                Ok(self
                    .print_created(&created, format.as_deref(), false)
                    .await?
                    .text)
            }
            KillPane { target } => {
                let pane = self.target(target.as_deref()).await?.pane_id;
                self.done(Method::DELETE, &format!("/api/panes/{pane}"), None)
                    .await
            }
            KillWindow { target } => {
                let panes = self.panes().await?;
                let window = resolve(&panes, self.current, target.as_deref())?;
                let last = !panes
                    .iter()
                    .any(|p| p.session_id == window.session_id && p.window_id != window.window_id);
                let path = if last {
                    format!("/api/sessions/{}", window.session_id)
                } else {
                    format!("/api/windows/{}", window.window_id)
                };
                self.done(Method::DELETE, &path, None).await
            }
            NewWorktree {
                target,
                detached,
                format,
                branch,
            } => {
                let pane = self.target(target.as_deref()).await?.pane_id;
                let body = json!({ "branch": branch, "select": !detached });
                let path = format!("/api/panes/{pane}/worktree");
                let created = self.call(Method::POST, &path, Some(body)).await?;
                Ok(self
                    .print_created(&created, format.as_deref(), false)
                    .await?
                    .text)
            }
            RemoveWorktree { target } => {
                let pane = self.target(target.as_deref()).await?.pane_id;
                let path = format!("/api/panes/{pane}/worktree");
                let removed: Value =
                    serde_json::from_str(&self.call(Method::DELETE, &path, None).await?)
                        .map_err(|e| e.to_string())?;
                Ok(format!(
                    "{}\n",
                    removed["path"].as_str().unwrap_or_default()
                ))
            }
            KillSession { target } => {
                let session = self.target(target.as_deref()).await?.session_id;
                self.done(Method::DELETE, &format!("/api/sessions/{session}"), None)
                    .await
            }
            SelectPane { target } => {
                let pane = self.target(target.as_deref()).await?.pane_id;
                self.done(Method::POST, &format!("/api/panes/{pane}/select"), None)
                    .await
            }
            SelectWindow { target } => {
                let window = self.target(target.as_deref()).await?.window_id;
                self.done(Method::POST, &format!("/api/windows/{window}/select"), None)
                    .await
            }
            RenameWindow { target, name } => {
                let window = self.target(target.as_deref()).await?.window_id;
                let path = format!("/api/windows/{window}/rename");
                self.done(Method::POST, &path, Some(json!({ "name": name })))
                    .await
            }
            RenameSession { target, name } => {
                let session = self.target(target.as_deref()).await?.session_id;
                let path = format!("/api/sessions/{session}/rename");
                self.done(Method::POST, &path, Some(json!({ "name": name })))
                    .await
            }
            SendKeys {
                target,
                literal,
                keys,
            } => {
                let pane = self.target(target.as_deref()).await?.pane_id;
                let path = format!("/api/panes/{pane}/input");
                for (text, keys) in input_runs(&keys, literal) {
                    let body = json!({ "text": text, "keys": keys });
                    self.call(Method::POST, &path, Some(body)).await?;
                }
                Ok(String::new())
            }
            CapturePane {
                target, start, end, ..
            } => {
                if end.as_deref().is_some_and(|end| end != "-") {
                    return Err("only -E - is supported".into());
                }
                let history = history_rows(start.as_deref())?;
                let pane = self.target(target.as_deref()).await?.pane_id;
                let path = format!("/api/panes/{pane}/capture?history={history}");
                self.call(Method::GET, &path, None).await
            }
        }
    }

    async fn call(
        &self,
        method: Method,
        path: &str,
        body: Option<Value>,
    ) -> Result<String, String> {
        request(self.profile, method, path, body, TIMEOUT).await
    }

    async fn done(
        &self,
        method: Method,
        path: &str,
        body: Option<Value>,
    ) -> Result<String, String> {
        self.call(method, path, body).await?;
        Ok(String::new())
    }

    async fn panes(&self) -> Result<Vec<PaneInfo>, String> {
        let body = self.call(Method::GET, "/api/panes", None).await?;
        serde_json::from_str(&body).map_err(|e| e.to_string())
    }

    async fn target(&self, target: Option<&str>) -> Result<PaneInfo, String> {
        let panes = self.panes().await?;
        resolve(&panes, self.current, target).cloned()
    }

    /// The pane a list is scoped to; `None` lists everything, as `-a` or
    /// outside a pane without `-t`.
    fn scope<'p>(
        &self,
        panes: &'p [PaneInfo],
        all: bool,
        target: Option<&str>,
    ) -> Result<Option<&'p PaneInfo>, String> {
        if all || (target.is_none() && self.current.is_none()) {
            return Ok(None);
        }
        resolve(panes, self.current, target).map(Some)
    }

    /// The new pane's ID or `format` line, and with `wait` the pane to wait for.
    async fn print_created(
        &self,
        created: &str,
        format: Option<&str>,
        wait: bool,
    ) -> Result<Reply, String> {
        let created: Value = serde_json::from_str(created).map_err(|e| e.to_string())?;
        let pane = created["pane_id"]
            .as_str()
            .and_then(|pane| Uuid::parse_str(pane).ok())
            .ok_or("no pane in the reply")?;
        let wait = wait.then_some((pane, None));
        let Some(format) = format else {
            return Ok(Reply {
                text: format!("{pane}\n"),
                wait,
            });
        };
        // As it was created: its command may have closed it already.
        let info = match created.get("pane").filter(|pane| pane.is_object()) {
            Some(info) => info.clone(),
            None => {
                let panes = self.panes().await?;
                let info = panes
                    .iter()
                    .find(|p| p.pane_id == pane)
                    .ok_or("the new pane is gone")?;
                serde_json::to_value(info).unwrap()
            }
        };
        Ok(Reply {
            text: format_line(format, &info) + "\n",
            wait,
        })
    }
}

/// Resolves a tmux target to a pane: the named one, else the active pane of
/// the named window or session.
fn resolve<'p>(
    panes: &'p [PaneInfo],
    current: Option<Uuid>,
    target: Option<&str>,
) -> Result<&'p PaneInfo, String> {
    let current_pane = || {
        let id = current.ok_or("not in a btmux pane; pass -t")?;
        panes
            .iter()
            .find(|p| p.pane_id == id)
            .ok_or_else(|| format!("current pane {id} not found"))
    };
    let Some(target) = target else {
        return current_pane();
    };
    if let Ok(id) = Uuid::parse_str(target) {
        return panes
            .iter()
            .find(|p| p.pane_id == id)
            .or_else(|| active(panes, |p| p.window_id == id))
            .or_else(|| active(panes, |p| p.session_id == id && p.window_active))
            .ok_or_else(|| format!("no pane, window, or session {id}"));
    }

    let (session, rest) = target.split_once(':').unwrap_or((target, ""));
    let (window, pane) = match rest.rsplit_once('.') {
        Some((window, pane)) if !pane.is_empty() && pane.bytes().all(|b| b.is_ascii_digit()) => {
            (window, Some(pane))
        }
        _ => (rest, None),
    };
    if session.is_empty() && window.is_empty() && pane.is_none() {
        return current_pane();
    }
    let session_id = if session.is_empty() {
        current_pane()?.session_id
    } else {
        panes
            .iter()
            .find(|p| p.session_name == session)
            .ok_or_else(|| format!("no session {session}"))?
            .session_id
    };
    let window_id = if window.is_empty() {
        if session.is_empty() {
            current_pane()?.window_id
        } else {
            active(panes, |p| p.session_id == session_id && p.window_active)
                .ok_or_else(|| format!("no active window in {session}"))?
                .window_id
        }
    } else {
        // An index wins over a window named like one, as in tmux.
        let in_session = || panes.iter().filter(|p| p.session_id == session_id);
        window
            .parse::<usize>()
            .ok()
            .and_then(|index| in_session().find(|p| p.window_index == index))
            .or_else(|| in_session().find(|p| p.window_name == window))
            .ok_or_else(|| format!("no window {window}"))?
            .window_id
    };
    match pane {
        Some(index) => panes
            .iter()
            .find(|p| p.window_id == window_id && p.pane_index.to_string() == index)
            .ok_or_else(|| format!("no pane {index}")),
        None => active(panes, |p| p.window_id == window_id).ok_or_else(|| "no pane".into()),
    }
}

/// The active pane among those matching, else the first.
fn active(panes: &[PaneInfo], matches: impl Fn(&PaneInfo) -> bool) -> Option<&PaneInfo> {
    let mut matching = panes.iter().filter(|p| matches(p));
    let first = matching.next()?;
    Some(
        std::iter::once(first)
            .chain(matching)
            .find(|p| p.pane_active)
            .unwrap_or(first),
    )
}

/// One row per session: its active pane, plus `session_windows`.
fn sessions(panes: &[PaneInfo]) -> Vec<Value> {
    let mut ids: Vec<Uuid> = Vec::new();
    for pane in panes {
        if !ids.contains(&pane.session_id) {
            ids.push(pane.session_id);
        }
    }
    ids.into_iter()
        .filter_map(|id| {
            let pane = active(panes, |p| p.session_id == id && p.window_active)
                .or_else(|| active(panes, |p| p.session_id == id))?;
            let mut row = serde_json::to_value(pane).unwrap();
            let mut windows: Vec<Uuid> = Vec::new();
            for p in panes.iter().filter(|p| p.session_id == id) {
                if !windows.contains(&p.window_id) {
                    windows.push(p.window_id);
                }
            }
            row["session_windows"] = windows.len().into();
            Some(row)
        })
        .collect()
}

/// One row per window: its active pane, plus `window_panes`.
fn windows(panes: &[PaneInfo]) -> Vec<Value> {
    let mut ids: Vec<Uuid> = Vec::new();
    for pane in panes {
        if !ids.contains(&pane.window_id) {
            ids.push(pane.window_id);
        }
    }
    ids.into_iter()
        .filter_map(|id| {
            let pane = active(panes, |p| p.window_id == id)?;
            let mut row = serde_json::to_value(pane).unwrap();
            row["window_panes"] = panes.iter().filter(|p| p.window_id == id).count().into();
            Some(row)
        })
        .collect()
}

/// Rows as a JSON array of the fields with `prefixes` (all if empty), or one
/// `format` line each.
fn print_rows(rows: Vec<Value>, format: Option<&str>, prefixes: &[&str]) -> String {
    if let Some(format) = format {
        return rows
            .iter()
            .map(|row| format_line(format, row) + "\n")
            .collect();
    }
    let rows: Vec<Value> = rows
        .into_iter()
        .map(|mut row| {
            if let (Some(fields), false) = (row.as_object_mut(), prefixes.is_empty()) {
                fields.retain(|key, _| prefixes.iter().any(|prefix| key.starts_with(prefix)));
            }
            row
        })
        .collect();
    serde_json::to_string_pretty(&rows).unwrap() + "\n"
}

/// Replaces `#{name}` with the row's field; flags print as 1 or 0, unknown
/// names as nothing.
fn format_line(format: &str, row: &Value) -> String {
    let mut line = String::new();
    let mut rest = format;
    while let Some(start) = rest.find("#{") {
        let Some(end) = rest[start..].find('}') else {
            break;
        };
        line.push_str(&rest[..start]);
        let name = &rest[start + 2..start + end];
        match row.get(name) {
            Some(Value::String(text)) => line.push_str(text),
            Some(Value::Bool(flag)) => line.push(if *flag { '1' } else { '0' }),
            Some(Value::Null) | None => {}
            Some(value) => line.push_str(&value.to_string()),
        }
        rest = &rest[start + end + 1..];
    }
    line.push_str(rest);
    line
}

/// Groups send-keys arguments into input requests of text, then key names,
/// keeping their order.
fn input_runs(args: &[String], literal: bool) -> Vec<(String, Vec<String>)> {
    if literal {
        return vec![(args.concat(), Vec::new())];
    }
    let mut runs: Vec<(String, Vec<String>)> = Vec::new();
    for arg in args {
        let is_key = crate::automation::is_key(arg);
        match runs.last_mut() {
            Some((_, keys)) if is_key => keys.push(arg.clone()),
            Some((text, keys)) if keys.is_empty() => text.push_str(arg),
            _ if is_key => runs.push((String::new(), vec![arg.clone()])),
            _ => runs.push((arg.clone(), Vec::new())),
        }
    }
    runs
}

/// History rows for `capture-pane -S`: `-` is all, `-N` is N.
fn history_rows(start: Option<&str>) -> Result<usize, String> {
    match start {
        None => Ok(0),
        Some("-") => Ok(MAX_CAPTURE_HISTORY),
        Some(start) => start
            .parse::<i64>()
            .map(|line| line.min(0).unsigned_abs() as usize)
            .map_err(|_| format!("bad -S value: {start}")),
    }
}

/// `-c` relative to the caller's directory, not the server's.
fn absolute(dir: Option<String>) -> Result<Option<String>, String> {
    dir.map(|dir| {
        std::path::absolute(&dir)
            .map(|path| path.to_string_lossy().into_owned())
            .map_err(|e| format!("{dir}: {e}"))
    })
    .transpose()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pane(session: (&str, u128), window: (usize, u128), index: usize, active: bool) -> PaneInfo {
        PaneInfo {
            session_id: Uuid::from_u128(session.1),
            session_name: session.0.into(),
            window_id: Uuid::from_u128(window.1),
            window_index: window.0,
            window_name: format!("w{}", window.0),
            window_active: window.1 != 10,
            window_zoomed_flag: false,
            pane_id: Uuid::from_u128(window.1 * 10 + index as u128),
            pane_index: index,
            pane_active: active,
            pane_current_path: None,
            pane_title: None,
            pane_dead: false,
            pane_dead_status: None,
        }
    }

    fn panes() -> Vec<PaneInfo> {
        vec![
            pane(("main", 1), (0, 10), 0, true),
            pane(("main", 1), (1, 11), 0, false),
            pane(("main", 1), (1, 11), 1, true),
            pane(("build", 2), (0, 20), 0, true),
        ]
    }

    fn id(target: &str, current: Option<u128>) -> Result<u128, String> {
        let panes = panes();
        resolve(&panes, current.map(Uuid::from_u128), Some(target)).map(|p| p.pane_id.as_u128())
    }

    #[test]
    fn resolves_targets() {
        assert_eq!(id("main", None), Ok(111));
        assert_eq!(id("main:0", None), Ok(100));
        assert_eq!(id("main:w1.0", None), Ok(110));
        assert_eq!(id("build:", None), Ok(200));
        assert_eq!(id(":.1", Some(110)), Ok(111));
        assert_eq!(id(":0", Some(110)), Ok(100));
        assert_eq!(id(":", Some(110)), Ok(110));
        assert_eq!(id(&Uuid::from_u128(11).to_string(), None), Ok(111));
        assert_eq!(id(&Uuid::from_u128(2).to_string(), None), Ok(200));
        assert!(id("nope", None).is_err());
        assert!(id(":0", None).is_err());
        assert!(resolve(&panes(), None, None).is_err());
    }

    #[test]
    fn prefers_window_index_to_numeric_name() {
        let mut panes = panes();
        panes[0].window_name = "1".into();
        let resolved = |target| resolve(&panes, None, Some(target)).map(|p| p.pane_id.as_u128());
        assert_eq!(resolved("main:1"), Ok(111));
        assert_eq!(resolved("main:w1"), Ok(111));
    }

    #[test]
    fn groups_keys_and_text() {
        let args = |args: &[&str]| args.iter().map(|a| a.to_string()).collect::<Vec<_>>();
        assert_eq!(
            input_runs(
                &args(&["echo hi", "Enter", "C-c", "ls", " -l", "Enter"]),
                false
            ),
            vec![
                ("echo hi".into(), args(&["Enter", "C-c"])),
                ("ls -l".into(), args(&["Enter"])),
            ]
        );
        assert_eq!(
            input_runs(&args(&["C-c"]), false),
            vec![(String::new(), args(&["C-c"]))]
        );
        assert_eq!(
            input_runs(&args(&["Enter", "x"]), true),
            vec![("Enterx".into(), Vec::new())]
        );
    }

    #[test]
    fn formats_lines() {
        let row =
            json!({ "pane_id": "p", "pane_active": true, "pane_index": 2, "pane_title": null });
        assert_eq!(
            format_line(
                "#{pane_id} #{pane_active} #{pane_index}#{pane_title}#{nope}",
                &row
            ),
            "p 1 2"
        );
        assert_eq!(format_line("x #{open", &row), "x #{open");
    }

    #[test]
    fn wait_timeouts_are_bounded() {
        use clap::Parser;
        #[derive(Parser)]
        struct Cli {
            #[command(subcommand)]
            command: TmuxCommand,
        }
        let parse = |timeout: &str| {
            Cli::try_parse_from(["btmux", "wait-exit", "-t", "p", "--timeout", timeout])
        };
        assert!(parse("600").is_ok());
        assert!(parse(&crate::pane_exit::MAX_TIMEOUT_SECS.to_string()).is_ok());
        assert!(parse("18446744073709551615").is_err());
        assert!(parse("-1").is_err());
    }

    #[test]
    fn parses_history() {
        assert_eq!(history_rows(None), Ok(0));
        assert_eq!(history_rows(Some("-")), Ok(MAX_CAPTURE_HISTORY));
        assert_eq!(history_rows(Some("-50")), Ok(50));
        assert_eq!(history_rows(Some("0")), Ok(0));
        assert!(history_rows(Some("x")).is_err());
    }
}
