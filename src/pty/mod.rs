pub mod colors;
pub mod kitty_keyboard;
pub mod program_status;
pub mod replay;
pub mod screen;
#[cfg(unix)]
mod unix;
#[allow(dead_code)]
pub mod vt_query;
#[cfg(windows)]
mod windows;
use axum::body::Bytes;
use replay::{Output, Replay};

pub use crate::process::Pid;
use portable_pty::{CommandBuilder, NativePtySystem, PtySize, PtySystem};
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};
use std::sync::{Arc, LazyLock, Mutex};
use tokio::sync::{broadcast, mpsc, oneshot, watch};
#[cfg(unix)]
use unix::{attached, foreground, resize_master, Master, Probe, Stopping};
#[cfg(unix)]
pub use unix::{hang_up, set_cloexec, Handoff};
use uuid::Uuid;
#[cfg(windows)]
use windows::{attached, foreground, resize_master, Master, Probe};

/// Largest single write to a PTY's input queue.
pub const MAX_INPUT: usize = 64 * 1024;

/// What the server's own terminal or multiplexer sets; programs detect
/// features (and tmux/screen passthrough) from them.
pub(crate) const OUTER_TERMINAL_ENV: [&str; 29] = [
    "TERM_SESSION_ID",
    "TERMINFO",
    "LC_TERMINAL",
    "LC_TERMINAL_VERSION",
    "COLORFGBG",
    "GHOSTTY_BIN_DIR",
    "GHOSTTY_RESOURCES_DIR",
    "GHOSTTY_SHELL_FEATURES",
    "KITTY_INSTALLATION_DIR",
    "KITTY_LISTEN_ON",
    "KITTY_PID",
    "KITTY_PUBLIC_KEY",
    "KITTY_WINDOW_ID",
    "ITERM_PROFILE",
    "ITERM_SESSION_ID",
    "WEZTERM_EXECUTABLE",
    "WEZTERM_EXECUTABLE_DIR",
    "WEZTERM_PANE",
    "WEZTERM_UNIX_SOCKET",
    "ALACRITTY_SOCKET",
    "ALACRITTY_WINDOW_ID",
    "VTE_VERSION",
    "KONSOLE_VERSION",
    "WT_SESSION",
    "WT_PROFILE_ID",
    "TMUX",
    "TMUX_PANE",
    "STY",
    "ZELLIJ",
];

/// What btmux sets for a pane's shell, which processes outside panes mustn't inherit.
#[cfg(unix)]
pub(crate) const PANE_ENV: [&str; 7] = [
    "TERM_PROGRAM",
    "TERM_PROGRAM_VERSION",
    "BTMUX_PANE_ID",
    "BTMUX_BIN",
    "BTMUX_AUTH_TOKEN",
    "BTMUX_API_URL",
    "BTMUX_OTEL_ENDPOINT",
];

/// This server's binary, read at startup: once it is replaced on disk, Linux
/// reports `<path> (deleted)`.
pub fn server_exe() -> Option<&'static Path> {
    static EXE: LazyLock<Option<PathBuf>> = LazyLock::new(|| std::env::current_exe().ok());
    EXE.as_deref()
}

/// Server-wide context shared by every PTY.
#[derive(Clone)]
pub struct PtyEnv {
    /// Notifies the session manager that a shell exited so its pane can be removed.
    pub exit_tx: mpsc::UnboundedSender<Uuid>,
    /// Notifies the session manager that OSC title/cwd changed.
    pub meta_tx: mpsc::UnboundedSender<()>,
    /// Server port, injected as BTMUX_API_URL into the shell environment.
    pub port: u16,
    /// Theme colors reported to programs in the PTY.
    pub palette: colors::SharedPalette,
    /// Held by a handoff, so programs aren't reaped while it runs.
    pub reap_gate: Arc<ReapGate>,
}

/// While held, exited programs stay unreaped zombies for the next server, so
/// a handoff sees each pane as running or as exited, never reaped in between.
#[derive(Default)]
#[cfg_attr(not(unix), allow(dead_code))]
pub struct ReapGate {
    held: Mutex<bool>,
    released: std::sync::Condvar,
}

#[cfg_attr(not(unix), allow(dead_code))]
impl ReapGate {
    /// Hold or release reaping. Holding waits for reaps in progress.
    pub fn hold(&self, held: bool) {
        *self.held.lock().unwrap() = held;
        if !held {
            self.released.notify_all();
        }
    }

    /// Blocks while held; a reap happens under the guard.
    fn reaping(&self) -> std::sync::MutexGuard<'_, bool> {
        let mut held = self.held.lock().unwrap();
        while *held {
            held = self.released.wait(held).unwrap();
        }
        held
    }
}

/// The PTY's input queue; `true` once its program exited and the queue closed.
#[derive(Clone)]
pub struct InputSender(mpsc::Sender<PtyInput>, bool);

enum PtyInput {
    Raw(Vec<u8>),
    ShellCommand {
        data: Vec<u8>,
        shell_pid: Pid,
        reply: oneshot::Sender<Result<(), String>>,
    },
    /// Wakes an idle writer for a handoff.
    #[cfg(unix)]
    Stop,
}

impl InputSender {
    /// A closed queue, for a pane whose program exited.
    fn exited() -> Self {
        Self(mpsc::channel(1).0, true)
    }

    fn unavailable(&self, error: impl std::fmt::Display) -> String {
        if self.1 {
            return "the pane's program has exited".into();
        }
        format!("PTY input unavailable: {error}")
    }

    pub async fn send_wait(&self, data: Vec<u8>) -> Result<(), String> {
        if data.len() > MAX_INPUT {
            return Err("PTY input exceeds 64 KiB".into());
        }
        self.0
            .send(PtyInput::Raw(data))
            .await
            .map_err(|e| self.unavailable(e))
    }
    pub fn send(&self, data: Vec<u8>) -> Result<(), String> {
        if data.len() > MAX_INPUT {
            return Err("PTY input exceeds 64 KiB; split it into smaller requests".into());
        }
        self.0
            .try_send(PtyInput::Raw(data))
            .map_err(|e| self.unavailable(e))
    }

    async fn send_shell_command(&self, data: Vec<u8>, shell_pid: Pid) -> Result<(), String> {
        if data.len() > MAX_INPUT {
            return Err("PTY input exceeds 64 KiB".into());
        }
        let (reply, result) = oneshot::channel();
        self.0
            .send(PtyInput::ShellCommand {
                data,
                shell_pid,
                reply,
            })
            .await
            .map_err(|e| self.unavailable(e))?;
        result
            .await
            .map_err(|_| "PTY input unavailable".to_string())?
    }
}

pub struct PtyHandle {
    pub input_tx: InputSender,
    output_tx: broadcast::Sender<Output>,
    scrollback: Arc<Mutex<Replay>>,
    /// The shell's pid until it is reaped.
    child: Arc<Mutex<Option<Pid>>>,
    master: Option<Master>,
    /// Write end of the pipe that stops the reader.
    #[cfg(unix)]
    stop_reader: Option<std::os::fd::OwnedFd>,
    #[cfg(unix)]
    stopping: Mutex<Option<Stopping>>,
    /// The `BTMUX_BIN` the shell was started with.
    bin: Option<std::path::PathBuf>,
    viewers: Vec<(Uuid, u16, u16)>,
    /// The interactive viewer whose size the PTY follows.
    owner_tx: watch::Sender<Option<Uuid>>,
    size: Arc<Mutex<(u16, u16)>>,
    spawned: Arc<Mutex<bool>>,
    shell_pid: Option<Pid>,
    /// The shell's executable when it started.
    #[cfg(unix)]
    shell_name: Option<String>,
    shell: String,
    dialect: crate::shell::Dialect,
    /// OSC 133 marks: 0 before the first, odd at a prompt, `2n` while
    /// command `n` runs.
    prompt: Arc<AtomicU32>,
    /// OSC 7501 records.
    program_status: Arc<Mutex<program_status::ProgramStatus>>,
    /// Initial working directory for the shell process.
    spawn_cwd: Option<std::path::PathBuf>,
    /// Id of the pane owning this PTY, reported on `exit_tx` when the shell dies.
    pane_id: Uuid,
    /// Notifies the session manager that the shell exited (EOF on the master) so
    /// the pane can be removed.
    exit_tx: mpsc::UnboundedSender<Uuid>,
    /// Notifies the session manager that OSC title/cwd changed so it can re-broadcast state.
    meta_tx: mpsc::UnboundedSender<()>,
    #[cfg_attr(not(unix), allow(dead_code))]
    reap_gate: Arc<ReapGate>,
    /// Foreground pgroup at the time each DSR 6 query was forwarded. When a
    /// CPR response arrives, pop the front: if tcgetpgrp(master) differs, the
    /// requester died and the response is stale.
    pending_cpr_pgrps: Arc<Mutex<VecDeque<Pid>>>,
    /// Finds the foreground process group; invalidated under this mutex on drop.
    probe: Arc<Mutex<Option<Probe>>>,
    /// Last OSC 0/2 title emitted by the shell.
    pub title: Arc<Mutex<Option<String>>>,
    /// The shell's working directory: its last OSC 7, or on Unix, from the OS.
    pub cwd: Arc<Mutex<Option<String>>>,
    /// The last OSC 7 named another host (an ssh session).
    pub cwd_remote: Arc<AtomicBool>,
    /// Whether the terminal hides what's typed, as at a password prompt.
    hidden_input: HiddenInput,
    /// Server port, injected as BTMUX_API_URL into the shell environment.
    port: u16,
    /// Maximum backend replay buffer size in bytes, derived from the config's
    /// scrollback line count. Controls how much PTY output is kept for replay
    /// on reconnect; the browser emulator's scrollback uses the line count directly.
    scrollback_bytes: usize,
    palette: colors::SharedPalette,
    /// Mode 2031, set by the program in the PTY.
    color_reports: Arc<AtomicBool>,
    /// The Multipass guest the shell runs in, through `btmux guest-shell`.
    guest: Option<Arc<crate::multipass::Guest>>,
    /// A popup's or a pane's program, run instead of the shell.
    command: Option<crate::popup::Spawn>,
    /// `BTMUX_PANE_ID`: a popup's names the pane it opened over.
    env_pane: Uuid,
    /// The shell's exit status once reaped; 128 + the signal if killed.
    exit_status: Arc<Mutex<Option<i32>>>,
    /// The pane stays, with its output, once its program exits.
    keep_open: bool,
    /// It runs a command, not a shell: its program is never at a prompt.
    runs_command: bool,
    /// The reader reached the end of the output.
    reader_done: Arc<AtomicBool>,
    /// A kept pane finished: its exit was written to its output.
    exit_noted: AtomicBool,
    /// When its exit was handled with its program or output still running.
    exited_at: Mutex<Option<std::time::Instant>>,
    /// A finished pane drops output its reader still reads.
    output_muted: Arc<AtomicBool>,
}

/// How long a kept pane waits for the rest of its output after its program
/// exited, while something else, such as a background process, holds the
/// terminal open.
#[cfg(not(test))]
pub const OUTPUT_GRACE: std::time::Duration = std::time::Duration::from_secs(2);
#[cfg(test)]
pub const OUTPUT_GRACE: std::time::Duration = std::time::Duration::from_millis(300);

/// A finished kept pane, for an upgraded server.
#[cfg(unix)]
pub struct Exited {
    pub status: Option<i32>,
    pub cols: u16,
    pub rows: u16,
    pub journal: Vec<Output>,
    pub title: Option<String>,
    pub cwd: Option<String>,
    pub cwd_remote: bool,
}

impl PtyHandle {
    pub fn new_with_cwd(
        shell: &str,
        pane_id: Uuid,
        env: &PtyEnv,
        spawn_cwd: Option<std::path::PathBuf>,
        scrollback_lines: u32,
    ) -> Self {
        let (input_tx, _) = mpsc::channel::<PtyInput>(32);
        let (output_tx, _) = broadcast::channel::<Output>(256);
        let probe = Arc::new(Mutex::new(None));

        Self {
            input_tx: InputSender(input_tx, false),
            output_tx,
            scrollback: Arc::new(Mutex::new(Replay::new(
                80,
                24,
                scrollback_lines as usize * 1000,
            ))),
            child: Arc::new(Mutex::new(None)),
            master: None,
            #[cfg(unix)]
            stop_reader: None,
            #[cfg(unix)]
            stopping: Mutex::new(None),
            bin: None,
            viewers: Vec::new(),
            owner_tx: watch::channel(None).0,
            size: Arc::new(Mutex::new((0, 0))),
            spawned: Arc::new(Mutex::new(false)),
            shell_pid: None,
            #[cfg(unix)]
            shell_name: None,
            shell: shell.to_string(),
            dialect: crate::shell::Dialect::of(shell),
            prompt: Arc::default(),
            program_status: Arc::default(),
            spawn_cwd,
            pane_id,
            exit_tx: env.exit_tx.clone(),
            meta_tx: env.meta_tx.clone(),
            reap_gate: env.reap_gate.clone(),
            pending_cpr_pgrps: Arc::new(Mutex::new(VecDeque::new())),
            hidden_input: HiddenInput::new(&probe),
            probe,
            title: Arc::new(Mutex::new(None)),
            cwd: Arc::new(Mutex::new(None)),
            cwd_remote: Arc::default(),
            port: env.port,
            scrollback_bytes: scrollback_lines as usize * 1000,
            palette: env.palette.clone(),
            color_reports: Arc::default(),
            guest: None,
            command: None,
            env_pane: pane_id,
            exit_status: Arc::default(),
            keep_open: false,
            runs_command: false,
            reader_done: Arc::default(),
            exit_noted: AtomicBool::new(false),
            exited_at: Mutex::new(None),
            output_muted: Arc::default(),
        }
    }

    /// Run a popup's program over `pane` instead of the shell.
    pub fn with_popup(mut self, pane: Uuid, spawn: crate::popup::Spawn) -> Self {
        self.env_pane = pane;
        self.command = Some(spawn);
        self
    }

    fn is_popup(&self) -> bool {
        self.env_pane != self.pane_id
    }

    /// Run a program instead of the shell, once spawned.
    pub fn set_command(&mut self, spawn: crate::popup::Spawn) {
        self.command = Some(spawn);
        self.runs_command = true;
    }

    /// It runs a command rather than a shell.
    pub fn runs_command(&self) -> bool {
        self.runs_command
    }

    pub fn keep_open(&self) -> bool {
        self.keep_open
    }

    pub fn set_keep_open(&mut self, keep_open: bool) {
        self.keep_open = keep_open;
    }

    /// A popup's program and directory, until it is spawned.
    pub fn popup_spawn(&self) -> Option<(&crate::popup::Spawn, Option<&Path>)> {
        Some((self.command.as_ref()?, self.spawn_cwd.as_deref()))
    }

    /// The exit status, once the shell was reaped.
    pub fn exit_status(&self) -> Option<i32> {
        *self.exit_status.lock().unwrap()
    }

    /// Its program ran and is gone.
    pub fn exited(&self) -> bool {
        self.is_spawned() && !self.child_alive()
    }

    /// Whether the pane's program is still running, for viewers' input.
    pub fn alive_probe(&self) -> impl Fn() -> bool + Send + Sync + 'static {
        let child = self.child.clone();
        move || child.lock().unwrap().is_some()
    }

    /// Its program exited and, for a kept pane, the pane finished: what
    /// waiters report, so a capture after the wait has all the output.
    pub fn finished(&self) -> bool {
        self.exited() && (!self.keep_open || self.exit_noted.load(Ordering::Acquire))
    }

    /// As when the reader reaches the end of the output.
    #[cfg(test)]
    pub fn end_output(&self) {
        self.reader_done.store(true, Ordering::Release);
    }

    /// Whether it still holds its terminal.
    #[cfg(test)]
    pub fn has_terminal(&self) -> bool {
        self.master.is_some()
    }

    /// ConPTY's output ends only once its console closes, after the program
    /// exited: closing it lets the reader take what it still holds and end,
    /// so a kept pane finishes with all its output rather than at the grace.
    #[cfg(windows)]
    pub fn close_console(&mut self) {
        if self.exited() && !self.output_done() {
            self.master = None;
        }
    }

    /// The reader reached the end of the output.
    pub fn output_done(&self) -> bool {
        self.reader_done.load(Ordering::Acquire)
    }

    /// Start the wait for a kept pane's last output after its program
    /// exited, or for a program to exit after its output ended; `false` if
    /// it already started.
    pub fn begin_output_grace(&self) -> bool {
        let mut since = self.exited_at.lock().unwrap();
        if since.is_some() {
            return false;
        }
        *since = Some(std::time::Instant::now());
        true
    }

    /// The wait from `begin_output_grace` is over.
    pub fn output_grace_over(&self) -> bool {
        self.exited_at
            .lock()
            .unwrap()
            .is_some_and(|since| since.elapsed() >= OUTPUT_GRACE)
    }

    /// Finish a kept pane whose program exited once its output is complete,
    /// or now if `force`d: write its exit after the last output and release
    /// the terminal, keeping what it shows. `true` once finished.
    pub fn finish(&mut self, force: bool) -> bool {
        if !self.keep_open || !self.exited() {
            return false;
        }
        if self.exit_noted.load(Ordering::Acquire) {
            return true;
        }
        if !force && !self.output_done() {
            return false;
        }
        // A program that died full-screen leaves the alternate screen and its
        // input modes on: back to the main screen and its history, with the
        // cursor shown and mouse, focus, and paste reports off, as tmux does.
        // Leaving restores the cursor, so only from the alternate screen.
        const RESET: &str =
            "\x1b[?1000l\x1b[?1002l\x1b[?1003l\x1b[?1004l\x1b[?1006l\x1b[?2004l\x1b[?25h\x1b[0m";
        let mut replay = self.scrollback.lock().unwrap();
        let leave = if replay.alternate_screen() {
            "\x1b[?1049l"
        } else {
            ""
        };
        let text = match self.exit_status() {
            Some(status) => format!("{leave}{RESET}\r\n[exited with status {status}]\r\n"),
            None => format!("{leave}{RESET}\r\n[exited]\r\n"),
        };
        let bytes = Bytes::from(text.into_bytes());
        {
            // Under the replay's lock: what a reader still running reads
            // now is dropped, so nothing follows the exit.
            self.output_muted.store(true, Ordering::Release);
            replay.push(Output::Data(bytes.clone()));
            let _ = self.output_tx.send(Output::Data(bytes));
        }
        drop(replay);
        // Its reader and writer stop on their own, off the manager's lock.
        self.release();
        // Its pid may be reused; nothing descends from it now.
        self.shell_pid = None;
        self.exit_noted.store(true, Ordering::Release);
        true
    }

    /// A finished kept pane, for an upgraded server.
    #[cfg(unix)]
    pub fn export_exited(&self) -> Option<Exited> {
        if !self.keep_open || !self.finished() {
            return None;
        }
        let (cols, rows) = self.size();
        Some(Exited {
            status: self.exit_status(),
            cols,
            rows,
            journal: self.journal(),
            title: self.title.lock().unwrap().clone(),
            cwd: self.cwd.lock().unwrap().clone(),
            cwd_remote: self.cwd_remote.load(Ordering::Relaxed),
        })
    }

    /// Show what an old server's finished kept pane showed.
    #[cfg(unix)]
    pub fn restore_exited(&mut self, exited: Exited) {
        *self.scrollback.lock().unwrap() = Replay::restore(
            exited.cols,
            exited.rows,
            self.scrollback_bytes,
            exited.journal,
        );
        *self.size.lock().unwrap() = (exited.cols, exited.rows);
        *self.exit_status.lock().unwrap() = exited.status;
        *self.title.lock().unwrap() = exited.title;
        if exited.cwd.is_some() {
            *self.cwd.lock().unwrap() = exited.cwd;
        }
        self.cwd_remote.store(exited.cwd_remote, Ordering::Relaxed);
        self.keep_open = true;
        self.reader_done.store(true, Ordering::Release);
        self.exit_noted.store(true, Ordering::Release);
        self.input_tx = InputSender::exited();
        *self.spawned.lock().unwrap() = true;
    }

    /// Whether the terminal hides what's typed, as at a password prompt.
    pub fn hidden_input(&self) -> HiddenInput {
        self.hidden_input.clone()
    }

    /// Whether the shell runs and hasn't been reaped.
    pub fn child_alive(&self) -> bool {
        self.child.lock().unwrap().is_some()
    }

    /// Run the shell in a Multipass guest instead.
    pub fn with_guest(mut self, guest: Option<Arc<crate::multipass::Guest>>) -> Self {
        if guest.is_some() {
            self.dialect = crate::shell::Dialect::Posix;
        }
        self.guest = guest;
        self
    }

    pub fn is_guest(&self) -> bool {
        self.guest.is_some()
    }

    /// Shared with the OSC parser, so its cwds under them become local.
    pub fn add_guest_mounts(&self, added: &[String]) {
        if let Some(guest) = &self.guest {
            guest.add_mounts(added);
        }
    }

    #[cfg(test)]
    pub fn guest(&self) -> Option<&crate::multipass::Guest> {
        self.guest.as_deref()
    }

    /// The Multipass instance the shell runs in.
    pub fn guest_instance(&self) -> Option<&str> {
        self.guest.as_deref().map(|guest| guest.instance.as_str())
    }

    /// Best-known working directory for snapshots: the live OSC 7 cwd if the
    /// shell has reported one, otherwise the directory the shell was spawned in.
    /// Without the fallback, a pane whose shell was never attached (lazily
    /// spawned) — e.g. sessions created by `create-session-from-git-repos` and
    /// never viewed — would persist a null cwd and lose it across restarts.
    pub fn effective_cwd(&self) -> Option<String> {
        if let Some(cwd) = self.cwd.lock().unwrap().clone() {
            return Some(cwd);
        }
        self.spawn_cwd
            .as_ref()
            .map(|p| p.to_string_lossy().into_owned())
    }

    /// The name of a program in the foreground instead of the shell.
    pub fn foreground_program(&self) -> Option<String> {
        let foreground = self.foreground_pgrp()?;
        // Guest programs are no host processes; OSC 133 marks tell them apart.
        if self.guest.is_some() {
            return (self.runs_command || Some(foreground) != self.shell_pid)
                .then(|| "a program".into());
        }
        let name = crate::process::name(foreground);
        // A command, even one a shell runs (`sh -c`), is never at a prompt.
        let shell = !self.runs_command && Some(foreground) == self.shell_pid;
        // The shell can exec another program in its group (`exec nvim`).
        #[cfg(unix)]
        let shell = shell
            && name.as_deref().is_none_or(|name| {
                unix::is_shell(name) || self.shell_name.as_deref() == Some(name)
            });
        (!shell).then(|| name.unwrap_or_else(|| "a program".into()))
    }

    /// `effective_cwd`, unless it's on another host.
    pub fn local_cwd(&self) -> Option<String> {
        if self.cwd_remote.load(Ordering::Relaxed) {
            return None;
        }
        self.effective_cwd()
    }

    pub fn screen_revision(&self) -> u64 {
        self.scrollback.lock().unwrap().screen_revision()
    }

    pub fn screen_snapshot(&self) -> Option<screen::ScreenSnapshot> {
        self.scrollback.lock().unwrap().screen_snapshot()
    }

    /// The checkpoint and journal, as replayed to a new viewer.
    pub fn journal(&self) -> Vec<Output> {
        self.scrollback.lock().unwrap().snapshot()
    }

    /// What the OSC 7501 records say, if they changed since the last call.
    pub fn take_program_status(&self) -> Option<program_status::Change> {
        self.program_status.lock().unwrap().take_change()
    }

    /// OSC 7501 records changed since `take_program_status`.
    pub fn program_status_pending(&self) -> bool {
        self.program_status.lock().unwrap().pending()
    }

    pub fn program_summary(&self) -> Option<program_status::Summary> {
        self.program_status.lock().unwrap().summary()
    }

    /// Drops OSC 7501 done and error records.
    pub fn forget_program_results(&self) {
        self.program_status.lock().unwrap().forget_results();
    }

    /// Drops OSC 7501 records of programs that exited.
    pub fn drop_exited_programs(&self) -> bool {
        self.program_status
            .lock()
            .unwrap()
            .drop_exited(crate::process::group_alive)
    }

    pub fn track_screen(&self, enabled: bool) {
        self.scrollback.lock().unwrap().track_screen(enabled);
    }

    pub fn shell_pid(&self) -> Option<u32> {
        self.shell_pid.and_then(|pid| pid.try_into().ok())
    }

    pub fn foreground_pgrp(&self) -> Option<Pid> {
        // A master whose program exited may still name its last group.
        if !self.child_alive() {
            return None;
        }
        self.probe.lock().unwrap().as_ref().and_then(foreground)
    }

    pub fn ensure_spawned(&mut self, cols: u16, rows: u16) -> Result<(), String> {
        validate_size(cols, rows)?;
        {
            let spawned = self.spawned.lock().unwrap();
            if *spawned {
                return Ok(());
            }
        }

        // Not kept: a popup's environment can be large. A popup that fails to
        // start is closed.
        let popup = self.command.take();
        let is_popup = self.is_popup();
        // Prepare all fallible resources before starting worker threads. The PTY
        // kernel buffer retains initial shell output until the reader starts.
        let argv: Vec<String> = match (&self.guest, &popup) {
            (Some(guest), popup) => {
                let exe = server_exe().ok_or("cannot find the btmux executable")?;
                let cwd = self.spawn_cwd.as_deref().and_then(Path::to_str);
                std::iter::once(exe.to_string_lossy().into_owned())
                    // A popup's or command's directory is the caller's.
                    .chain(guest.wrapper_args(cwd, popup.is_some()))
                    .chain(popup.iter().flat_map(|popup| popup.command.guest_args()))
                    .collect()
            }
            (None, Some(popup)) => popup.command.argv(&self.shell, popup.env.is_none()),
            (None, None) => crate::shell::argv(&self.shell),
        };
        let (program, args) = argv.split_first().ok_or("no shell configured")?;
        let mut cmd = CommandBuilder::new(program);
        cmd.args(args);
        // A popup gets its caller's environment; a guest's comes from the guest.
        if let Some(env) = popup.as_ref().and_then(|popup| popup.env.as_ref()) {
            cmd.env_clear();
            for (key, value) in env {
                cmd.env(key, value);
            }
        }
        cmd.env("TERM", "xterm-256color");
        cmd.env("COLORTERM", "truecolor");
        // Not the server's own terminal, which panes would otherwise inherit.
        for var in OUTER_TERMINAL_ENV {
            cmd.env_remove(var);
        }
        cmd.env("TERM_PROGRAM", "btmux");
        cmd.env("TERM_PROGRAM_VERSION", env!("CARGO_PKG_VERSION"));
        cmd.env("BTMUX_PANE_ID", self.env_pane.to_string());
        // Widgets its programs open show in the popup.
        if is_popup {
            cmd.env("BTMUX_POPUP_ID", self.pane_id.to_string());
        } else {
            cmd.env_remove("BTMUX_POPUP_ID");
        }
        self.bin = server_exe().map(Path::to_path_buf);
        if let Some(exe) = &self.bin {
            cmd.env("BTMUX_BIN", exe);
        }
        if let Some(token) = crate::auth::shell_token() {
            cmd.env("BTMUX_AUTH_TOKEN", token);
        }
        cmd.env(
            "BTMUX_API_URL",
            format!("http://{}:{}", crate::config::DEFAULT_HOST, self.port),
        );
        cmd.env(
            "BTMUX_OTEL_ENDPOINT",
            format!(
                "http://{}:{}/api/panes/{}/telemetry",
                crate::config::DEFAULT_HOST,
                self.port,
                self.env_pane
            ),
        );
        if std::env::var_os("LANG").is_none() {
            cmd.env("LANG", "en_US.UTF-8");
        }
        if let Some(dir) = self.spawn_cwd.as_ref().filter(|dir| dir.is_dir()) {
            cmd.cwd(dir);
        }
        #[cfg(windows)]
        if popup.is_none() {
            windows::integrate_shell(&mut cmd, self.dialect, self.spawn_cwd.is_none());
        }

        self.spawn(cmd, cols, rows)
    }

    /// The program that runs `btmux` subcommands in this pane's shell: its
    /// `$BTMUX_BIN`, unless an upgrade replaced that binary.
    pub fn shell_bin(&self) -> String {
        let exe =
            server_exe().filter(|exe| self.guest.is_none() && self.bin.as_deref() != Some(*exe));
        self.dialect
            .btmux(exe.map(|exe| exe.to_string_lossy()).as_deref())
    }

    pub fn dialect(&self) -> crate::shell::Dialect {
        self.dialect
    }

    /// Git Bash starts programs from short-lived forks, and WSL's are Linux
    /// processes, so neither descends from the Windows shell process.
    pub fn programs_are_descendants(&self) -> bool {
        self.guest.is_none() && !cfg!(windows)
            || !matches!(
                self.dialect,
                crate::shell::Dialect::Posix | crate::shell::Dialect::Wsl
            )
    }

    /// Tell a program that enabled mode 2031 that the theme changed.
    pub fn report_color_scheme(&self, palette: &colors::Palette) {
        if self.is_spawned() && self.color_reports.load(Ordering::Relaxed) {
            let _ = self.input_tx.send(palette.scheme_report());
        }
    }

    pub async fn send_shell_command(&self, data: Vec<u8>) -> Result<(), String> {
        if self.runs_command {
            return Err("The pane runs a command, not a shell".into());
        }
        let shell_pid = self
            .shell_pid
            .ok_or_else(|| "The pane's shell is unavailable".to_string())?;
        self.input_tx.send_shell_command(data, shell_pid).await
    }

    /// Record the current foreground process group for a forwarded DSR 6 query.
    fn record_cpr_pgrp(probe: &Mutex<Option<Probe>>, pending_cpr_pgrps: &Mutex<VecDeque<Pid>>) {
        let pgrp = probe
            .lock()
            .unwrap()
            .as_ref()
            .and_then(foreground)
            .unwrap_or(-1);
        let mut pending = pending_cpr_pgrps.lock().unwrap();
        if pending.len() == 256 {
            pending.pop_front();
        }
        pending.push_back(pgrp);
    }

    /// Filter CPR responses (`ESC[row;colR`) from emulator input. A response
    /// is valid only if (a) a pending DSR 6 was recorded AND (b) the foreground
    /// pgroup hasn't changed since the query was sent. If the pgroup differs,
    /// the requester exited (e.g. Ctrl-C killed the app) and writing the
    /// response would echo garbage to the new foreground (the shell).
    fn filter_stale_cpr(
        data: &[u8],
        pending_cpr_pgrps: &Mutex<VecDeque<Pid>>,
        probe: &Mutex<Option<Probe>>,
    ) -> Vec<u8> {
        let mut out = Vec::with_capacity(data.len());
        let mut i = 0;
        while i < data.len() {
            if data[i] == 0x1b && i + 2 < data.len() && data[i + 1] == b'[' {
                let start = i + 2;
                let mut end = start;
                while end < data.len() && (data[end].is_ascii_digit() || data[end] == b';') {
                    end += 1;
                }
                // CPR: ESC[row;colR (final byte 'R')
                if end < data.len() && data[end] == b'R' {
                    let params = &data[start..end];
                    if params.contains(&b';') {
                        let recorded_pgrp = pending_cpr_pgrps.lock().unwrap().pop_front();
                        if let Some(pgrp) = recorded_pgrp {
                            let current_pgrp = probe
                                .lock()
                                .unwrap()
                                .as_ref()
                                .and_then(foreground)
                                .unwrap_or(-1);
                            if pgrp == current_pgrp {
                                out.extend_from_slice(&data[i..end + 1]);
                            }
                            // else: pgrp changed → requester dead → drop
                        }
                        // else: no pending query → unsolicited → drop
                        i = end + 1;
                        continue;
                    }
                }
            }
            out.push(data[i]);
            i += 1;
        }
        out
    }

    pub fn resize(&self, cols: u16, rows: u16) {
        if validate_size(cols, rows).is_err() {
            return;
        }
        let mut size = self.size.lock().unwrap();
        if *size != (cols, rows) {
            *size = (cols, rows);
            drop(size);
            let mut replay = self.scrollback.lock().unwrap();
            replay.push(Output::Size(cols, rows));
            let _ = self.output_tx.send(Output::Size(cols, rows));
            if let Some(master) = &self.master {
                resize_master(master, cols, rows);
            }
        }
    }

    pub fn size(&self) -> (u16, u16) {
        *self.size.lock().unwrap()
    }

    /// Clears the scrollback of the replay and every viewer and, on the normal
    /// screen, the lines above the cursor, as VS Code's terminal clear does.
    pub fn clear(&self) {
        let mut replay = self.scrollback.lock().unwrap();
        let bytes = replay.clear();
        let _ = self.output_tx.send(Output::Data(bytes));
    }

    pub fn is_spawned(&self) -> bool {
        *self.spawned.lock().unwrap()
    }

    /// Subscribe to live output and snapshot the scrollback buffer atomically.
    /// Holding the scrollback lock while subscribing ensures the reader thread
    /// cannot append-then-broadcast a chunk between the two operations, which
    /// would cause that chunk to appear in both the replay and the live stream.
    pub fn subscribe_and_get_scrollback(&self) -> (broadcast::Receiver<Output>, Vec<u8>) {
        let replay = self.scrollback.lock().unwrap();
        let snapshot = replay
            .snapshot()
            .into_iter()
            .filter_map(|event| match event {
                Output::Data(bytes) => Some(bytes),
                _ => None,
            })
            .flatten()
            .collect();
        (self.output_tx.subscribe(), snapshot)
    }

    pub fn subscribe_output(&self) -> broadcast::Receiver<Output> {
        self.output_tx.subscribe()
    }

    pub fn subscribe_replay(&self) -> (broadcast::Receiver<Output>, Vec<Output>) {
        let replay = self.scrollback.lock().unwrap();
        (self.output_tx.subscribe(), replay.viewer_snapshot())
    }

    // The first interactive viewer owns the shared PTY dimensions until it
    // disconnects or another viewer claims them. Followers receive ordered size
    // events; mirrors never acquire ownership.
    pub fn attach_viewer(&mut self, id: Uuid, cols: u16, rows: u16) {
        self.viewers.push((id, cols, rows));
        if self.viewers.len() == 1 {
            self.resize(cols, rows);
            self.publish_owner();
        }
    }
    /// Make a viewer the size owner, e.g. after user input in its tab.
    pub fn claim_viewer(&mut self, id: Uuid) {
        let Some(index) = self.viewers.iter().position(|v| v.0 == id) else {
            return;
        };
        if index == 0 {
            return;
        }
        let viewer = self.viewers.remove(index);
        self.viewers.insert(0, viewer);
        self.resize(viewer.1, viewer.2);
        self.publish_owner();
    }
    pub fn subscribe_owner(&self) -> watch::Receiver<Option<Uuid>> {
        self.owner_tx.subscribe()
    }
    fn publish_owner(&self) {
        self.owner_tx
            .send_replace(self.viewers.first().map(|v| v.0));
    }
    pub fn resize_viewer(&mut self, id: Uuid, cols: u16, rows: u16) {
        if validate_size(cols, rows).is_err() {
            return;
        }
        if let Some(viewer) = self.viewers.iter_mut().find(|v| v.0 == id) {
            *viewer = (id, cols, rows);
        }
        if self.viewers.first().is_some_and(|v| v.0 == id) {
            self.resize(cols, rows);
        }
    }
    pub fn detach_viewer(&mut self, id: Uuid) {
        self.viewers.retain(|v| v.0 != id);
        if let Some(&(_, cols, rows)) = self.viewers.first() {
            self.resize(cols, rows);
        }
        self.publish_owner();
    }
}

/// Whether the program in a PTY reads a line without echoing it, as `sudo`,
/// `ssh`, and `read -s` do for passwords; raw-mode programs (editors, `ssh`
/// sessions) don't count. `None` where btmux can't tell (Windows).
#[derive(Clone)]
pub struct HiddenInput {
    #[cfg(unix)]
    probe: Arc<Mutex<Option<Probe>>>,
    tx: Arc<watch::Sender<Option<bool>>>,
    /// Sockets reporting it; the reader checks only while there are any.
    watchers: Arc<AtomicUsize>,
}

/// A socket reporting hidden input, until dropped.
pub struct HiddenInputWatch(Arc<AtomicUsize>);

impl Drop for HiddenInputWatch {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::Relaxed);
    }
}

impl HiddenInput {
    fn new(#[allow(unused_variables)] probe: &Arc<Mutex<Option<Probe>>>) -> Self {
        Self {
            #[cfg(unix)]
            probe: probe.clone(),
            tx: Arc::new(watch::channel(cfg!(unix).then_some(false)).0),
            watchers: Arc::default(),
        }
    }

    /// Have the reader check after each output chunk while the guard lives.
    pub fn watch(&self) -> HiddenInputWatch {
        self.watchers.fetch_add(1, Ordering::Relaxed);
        HiddenInputWatch(self.watchers.clone())
    }

    fn watched(&self) -> bool {
        self.watchers.load(Ordering::Relaxed) > 0
    }

    /// Check the terminal now, publishing a change to subscribers.
    pub fn check(&self) -> Option<bool> {
        #[cfg(unix)]
        if let Some(now) = unix::hides_input(&self.probe) {
            self.tx.send_if_modified(|hidden| {
                let changed = *hidden != Some(now);
                *hidden = Some(now);
                changed
            });
        }
        *self.tx.borrow()
    }

    pub fn subscribe(&self) -> watch::Receiver<Option<bool>> {
        self.tx.subscribe()
    }
}

pub fn validate_size(cols: u16, rows: u16) -> Result<(), String> {
    if cols == 0 || rows == 0 || cols > 500 || rows > 200 {
        return Err("PTY size must be 1..500 columns and 1..200 rows".into());
    }
    Ok(())
}

/// Turns PTY output into replies, replay, and broadcast.
struct OutputPipeline {
    interceptor: vt_query::VtQueryInterceptor,
    osc_parser: OscParser,
    response_tx: mpsc::Sender<PtyInput>,
    pending_cpr_pgrps: Arc<Mutex<VecDeque<Pid>>>,
    probe: Arc<Mutex<Option<Probe>>>,
    title: Arc<Mutex<Option<String>>>,
    cwd: Arc<Mutex<Option<String>>>,
    hidden_input: HiddenInput,
    program_status: Arc<Mutex<program_status::ProgramStatus>>,
    scrollback: Arc<Mutex<Replay>>,
    output_tx: broadcast::Sender<Output>,
    muted: Arc<AtomicBool>,
    pane_id: Uuid,
    /// The pane's shell and its name when the pipeline started.
    #[cfg(unix)]
    shell: Option<(Pid, String)>,
    /// The last foreground group polled for its cwd, and whether it's a shell.
    #[cfg(unix)]
    leader: Option<(Pid, bool)>,
}

impl PtyHandle {
    fn output_pipeline(
        &self,
        interceptor: vt_query::VtQueryInterceptor,
        response_tx: mpsc::Sender<PtyInput>,
    ) -> OutputPipeline {
        OutputPipeline {
            interceptor,
            osc_parser: OscParser::new(
                self.meta_tx.clone(),
                self.prompt.clone(),
                self.cwd_remote.clone(),
            )
            .with_guest(self.guest.clone()),
            response_tx,
            pending_cpr_pgrps: self.pending_cpr_pgrps.clone(),
            probe: self.probe.clone(),
            title: self.title.clone(),
            cwd: self.cwd.clone(),
            hidden_input: self.hidden_input.clone(),
            program_status: self.program_status.clone(),
            scrollback: self.scrollback.clone(),
            output_tx: self.output_tx.clone(),
            muted: self.output_muted.clone(),
            pane_id: self.pane_id,
            #[cfg(unix)]
            shell: self.shell_pid.zip(self.shell_name.clone()),
            #[cfg(unix)]
            leader: None,
        }
    }
}

impl OutputPipeline {
    /// `Err` holds the replies once the PTY input queue is closed.
    fn feed(&mut self, chunk: &[u8]) -> Result<(), Vec<u8>> {
        if chunk.is_empty() {
            return Ok(());
        }
        let pane_id = self.pane_id;
        let result = self.interceptor.feed(chunk);

        // One queue entry per chunk. A full queue drops the replies: blocking
        // here could deadlock against a program that is not reading its input.
        let responses = result.responses.concat();
        let mut sent = Ok(());
        if !responses.is_empty() {
            match self.response_tx.try_send(PtyInput::Raw(responses)) {
                Ok(()) => {}
                Err(mpsc::error::TrySendError::Full(_)) => {
                    tracing::warn!(%pane_id, "PTY input queue full; dropped replies");
                }
                Err(mpsc::error::TrySendError::Closed(input)) => {
                    let PtyInput::Raw(responses) = input else {
                        unreachable!()
                    };
                    sent = Err(responses);
                }
            }
        }
        for _ in 0..result.forwarded_cpr_queries {
            PtyHandle::record_cpr_pgrp(&self.probe, &self.pending_cpr_pgrps);
        }

        self.osc_parser
            .feed(&result.scrollback, &self.title, &self.cwd);
        self.apply_program_status();
        #[cfg(unix)]
        self.poll_cwd();
        // Before the output goes out: password prompts turn echo off before
        // writing themselves, so viewers learn it no later than they see them.
        if self.hidden_input.watched() {
            self.hidden_input.check();
        }
        let mut sb = self.scrollback.lock().unwrap();
        if self.muted.load(Ordering::Acquire) {
            return sent;
        }
        if !result.scrollback.is_empty() {
            sb.push(Output::Data(exact_bytes(result.scrollback)));
        }
        sb.set_kitty(self.interceptor.kitty());
        if !result.broadcast.is_empty() {
            let _ = self
                .output_tx
                .send(Output::Data(exact_bytes(result.broadcast)));
        }
        sent
    }
}

impl OutputPipeline {
    fn apply_program_status(&mut self) {
        if self.osc_parser.program.is_empty() {
            return;
        }
        let mut group = None;
        let mut status = self.program_status.lock().unwrap();
        let mut changed = false;
        for mut event in self.osc_parser.program.drain(..) {
            if let program_status::Event::Report(program_status::Report::Set(record)) = &mut event {
                let group = *group
                    .get_or_insert_with(|| self.probe.lock().unwrap().as_ref().and_then(attached));
                // With the shell in the foreground, the reporter exited or
                // runs in the background, and nothing would end the record.
                if group.is_none() && program_status::transient(record.state) {
                    continue;
                }
                record.group = group;
            }
            changed |= status.apply(event);
        }
        if changed {
            let _ = self.osc_parser.meta_tx.send(());
        }
    }
}

/// `Bytes` keeps a `Vec`'s spare capacity, which the journal budget can't see:
/// filtered reads would hold whole read buffers at a fraction of their cost.
fn exact_bytes(mut data: Vec<u8>) -> Bytes {
    data.shrink_to_fit();
    Bytes::from(data)
}

/// Record an OSC 133 prompt or command mark.
fn mark_prompt(state: &AtomicU32, command: bool) {
    let _ = state.try_update(Ordering::Relaxed, Ordering::Relaxed, |state| {
        match (command, state % 2 == 1) {
            (true, false) if state == 0 => Some(2),
            (false, false) | (true, true) => Some(state + 1),
            _ => None,
        }
    });
}

/// Decode `%XX` escapes; malformed escapes and non-UTF-8 results stay raw.
fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let hex = bytes
            .get(i + 1..i + 3)
            .filter(|h| h.iter().all(u8::is_ascii_hexdigit))
            .and_then(|h| u8::from_str_radix(std::str::from_utf8(h).ok()?, 16).ok());
        match (bytes[i], hex) {
            (b'%', Some(b)) => {
                out.push(b);
                i += 3;
            }
            (b, _) => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8(out).unwrap_or_else(|_| s.to_string())
}

/// Whether an OSC 7 host names this machine. Shells disagree on a `.local`
/// suffix, so only the first label is compared.
fn is_local_host(host: &str) -> bool {
    let label = |name: &str| {
        name.split('.')
            .next()
            .unwrap_or_default()
            .to_ascii_lowercase()
    };
    host.is_empty()
        || host.eq_ignore_ascii_case("localhost")
        || local_hostname().is_none_or(|local| label(host) == label(&local))
}

#[cfg(unix)]
fn local_hostname() -> Option<String> {
    let mut buf = [0u8; 256];
    if unsafe { libc::gethostname(buf.as_mut_ptr().cast(), buf.len()) } != 0 {
        return None;
    }
    let len = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
    String::from_utf8(buf[..len].to_vec()).ok()
}

#[cfg(windows)]
fn local_hostname() -> Option<String> {
    std::env::var("COMPUTERNAME").ok()
}

/// Incremental OSC sequence parser. Handles sequences split across read() chunks.
/// Parses OSC 0/2 (window title), OSC 7 (working directory URI), OSC 133
/// prompt (`A`) and command (`C`) marks, and OSC 7501 program status.
struct OscParser {
    buf: Vec<u8>,
    /// OSC 7501 reports and what ends records (prompts, RIS), in order.
    program: Vec<program_status::Event>,
    in_osc: bool,
    /// An OSC 7 arrived; it then owns the cwd.
    reported_cwd: bool,
    meta_tx: mpsc::UnboundedSender<()>,
    prompt: Arc<AtomicU32>,
    cwd_remote: Arc<AtomicBool>,
    /// Its OSC 7s name guest paths, local under its mounts.
    guest: Option<Arc<crate::multipass::Guest>>,
}

impl OscParser {
    fn with_guest(mut self, guest: Option<Arc<crate::multipass::Guest>>) -> Self {
        self.guest = guest;
        self
    }

    fn new(
        meta_tx: mpsc::UnboundedSender<()>,
        prompt: Arc<AtomicU32>,
        cwd_remote: Arc<AtomicBool>,
    ) -> Self {
        Self {
            buf: Vec::new(),
            program: Vec::new(),
            in_osc: false,
            reported_cwd: false,
            meta_tx,
            prompt,
            cwd_remote,
            guest: None,
        }
    }

    fn feed(
        &mut self,
        data: &[u8],
        title: &Arc<Mutex<Option<String>>>,
        cwd: &Arc<Mutex<Option<String>>>,
    ) {
        for &b in data {
            if self.in_osc {
                // BEL (0x07) or ST (ESC \) terminates the sequence.
                if b == 0x07 {
                    self.dispatch(title, cwd);
                    self.buf.clear();
                    self.in_osc = false;
                } else if b == 0x1b {
                    // Start of ESC \ — next byte should be '\', handled on next iteration.
                    // We treat ESC itself as a terminator and check on the next byte.
                    self.dispatch(title, cwd);
                    self.buf.clear();
                    self.in_osc = false;
                } else if self.buf.len() < 8192 {
                    self.buf.push(b);
                }
            } else if b == 0x1b {
                // Could be ESC ] — we peek on next byte via a small state trick:
                // store ESC in buf and treat the next byte as the start.
                self.buf.push(b);
            } else if b == b']' && self.buf.last() == Some(&0x1b) {
                self.buf.clear();
                self.in_osc = true;
            } else if b == b'c' && self.buf.last() == Some(&0x1b) {
                self.buf.clear();
                self.program.push(program_status::Event::Reset);
            } else {
                self.buf.clear();
            }
        }
    }

    fn dispatch(&mut self, title: &Arc<Mutex<Option<String>>>, cwd: &Arc<Mutex<Option<String>>>) {
        // buf contains everything after "ESC ]" and before the terminator.
        // Format: "<code>;<payload>"
        let Ok(s) = std::str::from_utf8(&self.buf) else {
            return;
        };
        let Some((code_str, payload)) = s.split_once(';') else {
            return;
        };
        let Ok(code) = code_str.parse::<u32>() else {
            return;
        };
        let changed = match code {
            0 | 2 if !payload.is_empty() => {
                *title.lock().unwrap() = Some(payload.to_string());
                true
            }
            7 => {
                // OSC 7 payload is normally a URI "scheme://hostname/path" —
                // extract the path. The scheme is usually `file://`, but some
                // shells emit other schemes with the same shape (e.g. zsh on
                // macOS emits `kitty-shell-cwd://host/path`), so match any
                // `scheme://` rather than `file://` specifically. Only `file`
                // URIs are percent-encoded (`kitty-shell-cwd` is raw). A bare
                // path (no scheme) is taken as-is.
                let mut host = "";
                let path = if let Some(scheme_end) = payload.find("://") {
                    let rest = &payload[scheme_end + 3..];
                    // Strip optional hostname (everything up to next '/').
                    let slash = rest.find('/').unwrap_or(rest.len());
                    host = &rest[..slash];
                    let path = &rest[slash..];
                    if payload[..scheme_end].eq_ignore_ascii_case("file") {
                        percent_decode(path)
                    } else {
                        path.to_string()
                    }
                } else {
                    payload.to_string()
                };
                // `file:///C:/x` names `C:/x`, `file:////server/share` a UNC path.
                #[cfg(windows)]
                let path = match path.as_bytes() {
                    [b'/', drive, b':', ..] if drive.is_ascii_alphabetic() => path[1..].to_string(),
                    [b'/', b'/', b'/', ..] => path[1..].replace('/', "\\"),
                    _ => path,
                };
                if !path.is_empty() {
                    let local = match &self.guest {
                        Some(guest) => guest.is_host(host) && guest.mounted(&path),
                        None => is_local_host(host),
                    };
                    *cwd.lock().unwrap() = Some(path);
                    self.cwd_remote.store(!local, Ordering::Relaxed);
                    self.reported_cwd = true;
                    true
                } else {
                    false
                }
            }
            133 => {
                match payload.as_bytes().first() {
                    Some(b'A') => {
                        mark_prompt(&self.prompt, false);
                        self.program.push(program_status::Event::Prompt);
                    }
                    Some(b'C') => mark_prompt(&self.prompt, true),
                    _ => {}
                }
                false
            }
            7501 => {
                if let Some(report) = program_status::parse(payload.as_bytes()) {
                    self.program.push(program_status::Event::Report(report));
                }
                false
            }
            _ => false,
        };
        if changed {
            let _ = self.meta_tx.send(());
        }
    }
}

#[cfg(all(test, unix))]
mod lifecycle_tests {
    use super::*;
    use std::os::fd::BorrowedFd;
    use std::time::Instant;
    fn pane(shell: &str) -> PtyHandle {
        let env = PtyEnv {
            exit_tx: mpsc::unbounded_channel().0,
            meta_tx: mpsc::unbounded_channel().0,
            port: 8044,
            palette: colors::SharedPalette::default(),
            reap_gate: Default::default(),
        };
        PtyHandle::new_with_cwd(shell, Uuid::new_v4(), &env, None, 100)
    }
    #[tokio::test]
    async fn spawn_failures_are_recoverable() {
        let mut pane = pane("/nonexistent/btmux-test-shell");
        assert!(pane.ensure_spawned(80, 24).is_err());
        assert!(!pane.is_spawned());
        assert!(pane.ensure_spawned(0, 24).is_err());
    }
    #[tokio::test]
    async fn input_is_bounded() {
        let (tx, _rx) = mpsc::channel(1);
        let input = InputSender(tx, false);
        assert!(input.send(vec![0; 65537]).is_err());
        assert!(input.send(vec![0; 65536]).is_ok());
        assert!(input.send(vec![0]).is_err());
    }
    #[tokio::test]
    async fn shell_commands_do_not_reach_foreground_programs() {
        let mut pane = pane("/bin/bash");
        pane.ensure_spawned(80, 24).unwrap();
        let shell_pid = pane.shell_pid.unwrap();
        let pane_ref = &pane;
        let wait_for_foreground = |shell_owns_terminal| async move {
            tokio::time::timeout(std::time::Duration::from_secs(5), async {
                loop {
                    let foreground = pane_ref.foreground_pgrp();
                    if (foreground == Some(shell_pid)) == shell_owns_terminal {
                        break;
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                }
            })
            .await
            .unwrap();
        };

        wait_for_foreground(true).await;
        pane.input_tx.send(b"cat\r".to_vec()).unwrap();
        wait_for_foreground(false).await;
        let result = pane
            .send_shell_command(b"printf 'SHOULD_NOT_RUN'\r".to_vec())
            .await;
        assert!(result.unwrap_err().contains("not at a shell prompt"));

        pane.input_tx.send(vec![3]).unwrap();
        wait_for_foreground(true).await;
        pane.send_shell_command(b"printf 'SHELL_READY'\r".to_vec())
            .await
            .unwrap();
    }
    async fn wait_until(pane: &PtyHandle, condition: impl Fn(&PtyHandle) -> bool) {
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            while !condition(pane) {
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
    }
    fn cwd(pane: &PtyHandle) -> Option<String> {
        pane.cwd.lock().unwrap().clone()
    }
    #[tokio::test]
    async fn program_status_ends_with_its_program() {
        let mut pane = pane("/bin/bash");
        pane.ensure_spawned(80, 24).unwrap();
        // From the shell itself, only results are kept: nothing would end
        // the rest.
        pane.input_tx
            .send(
                b"printf '\\033]7501;state=working:id=a\\007\\033]7501;state=done:id=b\\007'\r"
                    .to_vec(),
            )
            .unwrap();
        wait_until(&pane, |pane| {
            !pane.program_status.lock().unwrap().records().is_empty()
        })
        .await;
        let ids: Vec<_> = pane
            .program_status
            .lock()
            .unwrap()
            .records()
            .iter()
            .map(|r| r.id.clone())
            .collect();
        assert_eq!(ids, ["b"]);
        pane.input_tx
            .send(b"printf '\\033]7501;state=clear\\007'; sh -c 'printf \"\\033]7501;state=working\\007\"; exec sleep 30'\r".to_vec())
            .unwrap();
        let state = |pane: &PtyHandle| {
            pane.drop_exited_programs();
            pane.program_status
                .lock()
                .unwrap()
                .summary()
                .map(|s| s.state)
        };
        wait_until(&pane, |pane| {
            state(pane) == Some(program_status::State::Working)
        })
        .await;
        pane.input_tx.send(vec![3]).unwrap();
        wait_until(&pane, |pane| state(pane).is_none()).await;
    }
    #[tokio::test]
    async fn cwd_follows_foreground_shells_without_osc7() {
        let mut pane = pane("/bin/sh");
        pane.ensure_spawned(80, 24).unwrap();
        pane.input_tx.send(b"cd /\n".to_vec()).unwrap();
        wait_until(&pane, |pane| cwd(pane).as_deref() == Some("/")).await;
        pane.input_tx.send(b"sh\ncd /usr\n".to_vec()).unwrap();
        wait_until(&pane, |pane| cwd(pane).as_deref() == Some("/usr")).await;
        let nested = pane.foreground_pgrp();

        // A shell that execs a program: the program's directory isn't the pane's.
        let (mut rx, _) = pane.subscribe_replay();
        pane.input_tx
            .send(
                b"sh -c \"echo BTMUX_''READY; read x; exec perl -e 'chdir q(/bin); exec q(cat)'\"\n"
                    .to_vec(),
            )
            .unwrap();
        read_until(&mut rx, "BTMUX_READY").await;
        assert_ne!(pane.foreground_pgrp(), nested);
        pane.input_tx.send(b"\nx\n".to_vec()).unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        assert_eq!(cwd(&pane).as_deref(), Some("/usr"));
    }
    #[tokio::test]
    async fn reply_bursts_keep_the_pane_alive() {
        let (exit_tx, mut exit_rx) = mpsc::unbounded_channel();
        let env = PtyEnv {
            exit_tx,
            meta_tx: mpsc::unbounded_channel().0,
            port: 8044,
            palette: colors::SharedPalette::default(),
            reap_gate: Default::default(),
        };
        env.palette
            .set(colors::Palette::from_theme(&crate::config::default_theme()));
        let mut pane = PtyHandle::new_with_cwd("/bin/sh", Uuid::new_v4(), &env, None, 100);
        pane.ensure_spawned(80, 24).unwrap();
        let (mut rx, _) = pane.subscribe_replay();
        pane.input_tx
            .send(
                b"s=; i=0; while [ $i -lt 256 ]; do s=\"$s\\033]4;$i;?\\007\"; \
                  i=$((i+1)); done; printf \"$s\"; printf 'BTMUX_''ALIVE\\n'\r"
                    .to_vec(),
            )
            .unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            let mut text = String::new();
            while !text.contains("BTMUX_ALIVE") {
                if let Output::Data(bytes) = rx.recv().await.unwrap() {
                    text.push_str(&String::from_utf8_lossy(&bytes));
                }
            }
        })
        .await
        .unwrap();
        assert!(exit_rx.try_recv().is_err());
    }
    fn deadline() -> Instant {
        Instant::now() + std::time::Duration::from_secs(2)
    }
    async fn read_until(rx: &mut broadcast::Receiver<Output>, needle: &str) -> String {
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            let mut text = String::new();
            while !text.contains(needle) {
                match rx.recv().await {
                    Ok(Output::Data(bytes)) => text.push_str(&String::from_utf8_lossy(&bytes)),
                    Ok(_) | Err(broadcast::error::RecvError::Lagged(_)) => {}
                    Err(error) => panic!("{error}"),
                }
            }
            text
        })
        .await
        .unwrap()
    }
    #[tokio::test]
    async fn filtered_reads_add_no_events() {
        let pane = pane("/bin/sh");
        let (response_tx, _responses) = mpsc::channel(8);
        let mut output = pane.output_pipeline(vt_query::VtQueryInterceptor::new(), response_tx);
        let (mut rx, _) = pane.subscribe_replay();
        let revision = pane.screen_revision();

        // Swallowed queries, then an unterminated title.
        output.feed(&b"\x1b[?99n".repeat(600)).unwrap();
        output.feed(b"\x1b]2;").unwrap();
        output.feed(&[b'Z'; 4096]).unwrap();
        assert_eq!(pane.screen_revision(), revision);
        assert!(rx.try_recv().is_err());

        output.feed(b"\x07text").unwrap();
        assert_eq!(pane.screen_revision(), revision + 1);
        assert!(matches!(rx.try_recv(), Ok(Output::Data(_))));
    }

    #[tokio::test]
    async fn handoff_keeps_shell_replay_and_colors() {
        let mut old = pane("/bin/sh");
        old.ensure_spawned(80, 24).unwrap();
        let pid = old.shell_pid.unwrap();
        let (mut rx, _) = old.subscribe_replay();
        // A program-set color and keyboard flags, then a sequence split
        // across the handoff.
        old.input_tx
            .send(
                b"printf '\\033]10;#123456\\007\\033[>5uBEFORE_''HANDOFF\\n\\033]2;ti'\r".to_vec(),
            )
            .unwrap();
        read_until(&mut rx, "BEFORE_HANDOFF").await;
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        old.stop_io();
        let (fd, handoff) = old.export_handoff(deadline()).unwrap().unwrap();
        assert_eq!(handoff.pid, pid);
        // The prompt joins the unterminated title.
        assert!(handoff.pending.starts_with(b"\x1b]2;ti"));
        let master = unsafe { BorrowedFd::borrow_raw(fd) }
            .try_clone_to_owned()
            .unwrap();
        // The old process image is gone after exec; never drop it here.
        std::mem::forget(old);

        let mut new = pane("/bin/sh");
        new.adopt(master, handoff).unwrap();
        let (mut rx, replay) = new.subscribe_replay();
        let replay: Vec<u8> = replay
            .into_iter()
            .filter_map(|event| match event {
                Output::Data(bytes) => Some(bytes.to_vec()),
                _ => None,
            })
            .flatten()
            .collect();
        assert!(String::from_utf8_lossy(&replay).contains("BEFORE_HANDOFF"));
        assert!(replay.ends_with(b"\x1b[<8u\x1b[>5u"));
        new.input_tx
            .send(b"printf 'tle\\007AFTER_''HANDOFF:%s\\n' $$\r".to_vec())
            .unwrap();
        read_until(&mut rx, &format!("AFTER_HANDOFF:{pid}")).await;
        let title = new.title.lock().unwrap().clone().unwrap();
        assert!(
            title.starts_with("ti") && title.ends_with("tle"),
            "{title:?}"
        );
        new.stop_io();
        let (_, handoff) = new.export_handoff(deadline()).unwrap().unwrap();
        assert_eq!(handoff.colors[256], Some([0x12, 0x34, 0x56]));
        let mut kitty = kitty_keyboard::KittyKeyboard::default();
        kitty.csi(b'>', &[5]);
        assert_eq!(handoff.kitty, kitty);
    }
    #[tokio::test]
    async fn handoff_keeps_unwritten_input() {
        let mut old = pane("/bin/sh");
        old.ensure_spawned(80, 24).unwrap();
        let (mut rx, _) = old.subscribe_replay();
        old.input_tx
            .send(
                b"stty raw -echo; printf 'RAW_''READY'; sleep 2; head -c 262144 | wc -c\r".to_vec(),
            )
            .unwrap();
        read_until(&mut rx, "RAW_READY").await;
        for _ in 0..4 {
            old.input_tx.send(vec![b'x'; 64 * 1024]).unwrap();
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        old.stop_io();
        let (fd, handoff) = old.export_handoff(deadline()).unwrap().unwrap();
        assert!(!handoff.input.is_empty());
        let master = unsafe { BorrowedFd::borrow_raw(fd) }
            .try_clone_to_owned()
            .unwrap();
        std::mem::forget(old);

        let mut new = pane("/bin/sh");
        new.adopt(master, handoff).unwrap();
        let (mut rx, _) = new.subscribe_replay();
        read_until(&mut rx, "262144").await;
    }

    #[tokio::test]
    async fn handoff_keeps_programs_that_replaced_the_shell() {
        let mut old = pane("/bin/sh");
        old.ensure_spawned(80, 24).unwrap();
        old.input_tx.send(b"exec sleep 30\r".to_vec()).unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            while old.foreground_program().is_none() {
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        old.stop_io();
        let (fd, handoff) = old.export_handoff(deadline()).unwrap().unwrap();
        let master = unsafe { BorrowedFd::borrow_raw(fd) }
            .try_clone_to_owned()
            .unwrap();
        std::mem::forget(old);

        let mut new = pane("/bin/sh");
        new.adopt(master, handoff).unwrap();
        assert!(new.foreground_program().is_some());
    }

    /// A command that closed its terminal but still runs is handed off,
    /// not dropped, and stays a command. Only Linux ends a PTY's output once
    /// the last descriptor closes; macOS waits for the session to let go.
    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn handoff_keeps_commands_whose_output_ended() {
        let mut old = pane("/bin/sh");
        old.set_command(crate::popup::Spawn {
            command: crate::popup::PopupCommand::Exec(vec![
                "sh".into(),
                "-c".into(),
                "exec </dev/null >/dev/null 2>&1; sleep 30".into(),
            ]),
            env: None,
        });
        old.ensure_spawned(80, 24).unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            while !old.output_done() {
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        assert!(old.child_alive());
        old.stop_io();
        let (fd, handoff) = old.export_handoff(deadline()).unwrap().unwrap();
        assert!(handoff.runs_command);
        let master = unsafe { BorrowedFd::borrow_raw(fd) }
            .try_clone_to_owned()
            .unwrap();
        let pid = old.shell_pid.unwrap();
        std::mem::forget(old);

        let mut new = pane("/bin/sh");
        new.adopt(master, handoff).unwrap();
        assert!(new.runs_command());
        assert!(new.foreground_program().is_some() || !new.child_alive());
        unsafe { libc::kill(pid as libc::pid_t, libc::SIGKILL) };
    }

    #[tokio::test]
    async fn replies_racing_a_handoff_keep_the_shell() {
        let palette = colors::SharedPalette::default();
        palette.set(colors::Palette::from_theme(&crate::config::default_theme()));
        let env = PtyEnv {
            exit_tx: mpsc::unbounded_channel().0,
            meta_tx: mpsc::unbounded_channel().0,
            port: 8044,
            palette: palette.clone(),
            reap_gate: Default::default(),
        };
        let mut pane = PtyHandle::new_with_cwd("/bin/sh", Uuid::new_v4(), &env, None, 100);
        pane.ensure_spawned(80, 24).unwrap();
        let pid = pane.shell_pid.unwrap();
        let (mut rx, _) = pane.subscribe_replay();
        pane.input_tx.send(b"printf 'READY'\r".to_vec()).unwrap();
        read_until(&mut rx, "READY").await;
        // The reader blocks answering the query while the writer stops.
        let lock = palette.lock();
        pane.input_tx
            .send(b"printf '\\033]11;?\\007'\r".to_vec())
            .unwrap();
        std::thread::sleep(std::time::Duration::from_millis(300));
        pane.stop_io();
        std::thread::sleep(std::time::Duration::from_millis(100));
        drop(lock);
        let (_, handoff) = pane
            .export_handoff(deadline())
            .unwrap()
            .expect("the stopped shell is handed off");
        assert_eq!(handoff.pid, pid);
        // The next server writes the reply.
        assert!(
            handoff.input.starts_with(b"\x1b]11;"),
            "{:?}",
            handoff.input
        );
    }

    #[tokio::test]
    async fn failed_handoff_resumes_shell() {
        let mut pane = pane("/bin/sh");
        pane.ensure_spawned(80, 24).unwrap();
        let pid = pane.shell_pid.unwrap();
        let (mut rx, _) = pane.subscribe_replay();
        pane.stop_io();
        let (_, handoff) = pane.export_handoff(deadline()).unwrap().unwrap();
        pane.resume(handoff).unwrap();
        pane.input_tx
            .send(b"printf 'RESUMED_''%s\\n' $$\r".to_vec())
            .unwrap();
        read_until(&mut rx, &format!("RESUMED_{pid}")).await;
    }

    /// A pane whose reader is blocked answering a color query.
    async fn stuck_pane() -> (
        PtyHandle,
        colors::SharedPalette,
        broadcast::Receiver<Output>,
    ) {
        let palette = colors::SharedPalette::default();
        palette.set(colors::Palette::from_theme(&crate::config::default_theme()));
        let env = PtyEnv {
            exit_tx: mpsc::unbounded_channel().0,
            meta_tx: mpsc::unbounded_channel().0,
            port: 8044,
            palette: palette.clone(),
            reap_gate: Default::default(),
        };
        let mut pane = PtyHandle::new_with_cwd("/bin/sh", Uuid::new_v4(), &env, None, 100);
        pane.ensure_spawned(80, 24).unwrap();
        let (mut rx, _) = pane.subscribe_replay();
        pane.input_tx.send(b"printf 'READY'\r".to_vec()).unwrap();
        read_until(&mut rx, "READY").await;
        (pane, palette, rx)
    }

    #[tokio::test]
    async fn stuck_handoff_recovers_shell() {
        let (mut pane, palette, mut rx) = stuck_pane().await;
        let pid = pane.shell_pid.unwrap();
        let lock = palette.lock();
        // `ESC ] 11 ; rgb:RRRR/GGGG/BBBB BEL` is 24 bytes.
        pane.input_tx
            .send(
                b"stty -echo -icanon; printf '\\033]11;?\\007'; head -c 24 >/dev/null; \
                  stty sane; echo GOT_''REPLY\r"
                    .to_vec(),
            )
            .unwrap();
        std::thread::sleep(std::time::Duration::from_millis(300));
        pane.stop_io();
        assert!(pane.export_handoff(Instant::now()).is_err());
        drop(lock);
        pane.recover(deadline()).unwrap();
        // The query's program gets the reply.
        read_until(&mut rx, "GOT_REPLY").await;
        pane.input_tx
            .send(b"printf 'RECOVERED_''%s\\n' $$\r".to_vec())
            .unwrap();
        read_until(&mut rx, &format!("RECOVERED_{pid}")).await;
    }

    #[tokio::test]
    async fn late_reader_is_restarted_without_its_state() {
        let (mut pane, palette, mut rx) = stuck_pane().await;
        let pid = pane.shell_pid.unwrap();
        let lock = palette.lock();
        pane.input_tx
            .send(b"printf '\\033]11;?\\007'\r".to_vec())
            .unwrap();
        std::thread::sleep(std::time::Duration::from_millis(300));
        pane.stop_io();
        assert!(pane.export_handoff(Instant::now()).is_err());
        pane.recover(Instant::now()).unwrap();
        drop(lock);
        pane.input_tx
            .send(b"printf 'RESTARTED_''%s\\n' $$\r".to_vec())
            .unwrap();
        read_until(&mut rx, &format!("RESTARTED_{pid}")).await;
    }

    #[tokio::test]
    async fn recovering_a_running_pane_does_nothing() {
        let (mut pane, _palette, mut rx) = stuck_pane().await;
        pane.recover(Instant::now()).unwrap();
        pane.input_tx
            .send(b"printf 'STILL_''RUNNING'\r".to_vec())
            .unwrap();
        read_until(&mut rx, "STILL_RUNNING").await;
    }

    #[tokio::test]
    async fn adopted_shells_run_the_current_binary() {
        let mut pane = pane("/bin/sh");
        pane.bin = server_exe().map(Path::to_path_buf);
        assert_eq!(pane.shell_bin(), "\"$BTMUX_BIN\"");
        pane.bin = Some("/old/btmux".into());
        let exe = server_exe().unwrap();
        assert_eq!(
            pane.shell_bin(),
            crate::shell::Dialect::Posix.quote(&exe.to_string_lossy())
        );
    }
    #[tokio::test]
    async fn oldest_viewer_owns_size_until_disconnect() {
        let mut pane = pane("/bin/sh");
        let a = Uuid::new_v4();
        let b = Uuid::new_v4();
        pane.attach_viewer(a, 100, 30);
        pane.attach_viewer(b, 80, 24);
        pane.resize_viewer(b, 90, 25);
        assert_eq!(*pane.size.lock().unwrap(), (100, 30));
        pane.detach_viewer(a);
        assert_eq!(*pane.size.lock().unwrap(), (90, 25));
    }
    #[tokio::test]
    async fn claiming_viewer_takes_size_ownership() {
        let mut pane = pane("/bin/sh");
        let a = Uuid::new_v4();
        let b = Uuid::new_v4();
        let owner = pane.subscribe_owner();
        pane.attach_viewer(a, 100, 30);
        pane.attach_viewer(b, 80, 24);
        assert_eq!(*owner.borrow(), Some(a));
        pane.resize_viewer(b, 90, 25);
        pane.claim_viewer(b);
        assert_eq!(*pane.size.lock().unwrap(), (90, 25));
        assert_eq!(*owner.borrow(), Some(b));
        pane.resize_viewer(a, 120, 40);
        assert_eq!(*pane.size.lock().unwrap(), (90, 25));
        pane.detach_viewer(b);
        assert_eq!(*pane.size.lock().unwrap(), (120, 40));
        assert_eq!(*owner.borrow(), Some(a));
    }
    #[tokio::test]
    async fn dropping_pane_terminates_and_reaps_shell() {
        let mut pane = pane("/bin/sh");
        pane.ensure_spawned(80, 24).unwrap();
        let (mut rx, _) = pane.subscribe_replay();
        pane.input_tx
            .send(b"printf '\\nBTMUX_PID:%s\\n' $$\r".to_vec())
            .unwrap();
        let pid: Pid = tokio::time::timeout(std::time::Duration::from_secs(5), async {
            let mut text = String::new();
            loop {
                if let Output::Data(bytes) = rx.recv().await.unwrap() {
                    text.push_str(&String::from_utf8_lossy(&bytes));
                }
                if let Some(pid) = text.lines().find_map(|line| {
                    line.trim()
                        .strip_prefix("BTMUX_PID:")
                        .and_then(|value| value.parse().ok())
                }) {
                    break pid;
                }
            }
        })
        .await
        .unwrap();
        drop(pane);
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            while unsafe { libc::kill(pid, 0) } == 0 {
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
    }
}

#[cfg(test)]
mod osc_tests {
    use super::*;

    #[test]
    fn prompt_marks_number_commands() {
        let state = AtomicU32::new(0);
        let mut seen = Vec::new();
        for command in [true, false, false, true, true, false] {
            mark_prompt(&state, command);
            seen.push(state.load(Ordering::Relaxed));
        }
        assert_eq!(seen, [2, 3, 3, 4, 4, 5]);
    }

    fn osc7(seq: &[u8]) -> (Option<String>, bool) {
        let remote = Arc::<AtomicBool>::default();
        let mut parser =
            OscParser::new(mpsc::unbounded_channel().0, Arc::default(), remote.clone());
        let (title, cwd) = Default::default();
        parser.feed(seq, &title, &cwd);
        let cwd = cwd.lock().unwrap().clone();
        (cwd, remote.load(Ordering::Relaxed))
    }

    fn cwd_of(seq: &[u8]) -> Option<String> {
        osc7(seq).0
    }

    #[test]
    fn aborted_osc_dispatches_before_a_stripped_replacement() {
        let result = vt_query::VtQueryInterceptor::new().feed(b"\x1b]7;/x\x1b[cvisible");
        assert_eq!(cwd_of(&result.scrollback).as_deref(), Some("/x"));
    }

    #[test]
    fn marks_cwds_on_other_hosts_remote() {
        let local = local_hostname().unwrap();
        let short = local.split('.').next().unwrap();
        for host in ["", "localhost", &local, &short.to_uppercase()] {
            let seq = format!("\x1b]7;file://{host}/x\x07");
            assert!(!osc7(seq.as_bytes()).1, "{host}");
        }
        assert!(!osc7(b"\x1b]7;/x\x07").1);
        assert!(osc7(b"\x1b]7;file://btmux-remote-host/x\x07").1);
    }

    #[test]
    fn decodes_file_uri_cwd() {
        assert_eq!(
            cwd_of(b"\x1b]7;file://host/a%20b/%C3%A9%2\x07").as_deref(),
            Some("/a b/\u{e9}%2")
        );
        assert_eq!(
            cwd_of(b"\x1b]7;file://host/x%+1\x07").as_deref(),
            Some("/x%+1")
        );
        assert_eq!(
            cwd_of(b"\x1b]7;kitty-shell-cwd://host/a%20b\x07").as_deref(),
            Some("/a%20b")
        );
        assert_eq!(cwd_of(b"\x1b]7;/a%20b\x07").as_deref(), Some("/a%20b"));
    }

    #[test]
    fn collects_program_status_events_in_order() {
        use program_status::{Event, Report, State};
        let mut parser =
            OscParser::new(mpsc::unbounded_channel().0, Arc::default(), Arc::default());
        let (title, cwd) = Default::default();
        parser.feed(
            b"\x1b]7501;state=working\x1b\\x\x1b]133;A\x07\x1bc\x1b]7501;state=bogus\x07",
            &title,
            &cwd,
        );
        assert!(matches!(
            parser.program.as_slice(),
            [Event::Report(Report::Set(record)), Event::Prompt, Event::Reset]
                if record.state == State::Working
        ));
    }

    #[test]
    fn utf8_continuation_bytes_are_not_c1() {
        // U+0F4C ends in 0x9d (C1 OSC); "Ü" contains 0x9c (C1 ST).
        assert_eq!(cwd_of("\u{f4c}7;/x\x07".as_bytes()), None);
        assert_eq!(
            cwd_of("\x1b]7;/Übersicht\x07".as_bytes()).as_deref(),
            Some("/Übersicht")
        );
    }
}
