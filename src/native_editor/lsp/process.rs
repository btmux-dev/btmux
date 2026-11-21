//! A language server process: `Content-Length` framed JSON-RPC over stdio.

use std::collections::VecDeque;
use std::ffi::OsString;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::Value;
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::sync::{mpsc, oneshot};

use crate::config::LanguageServerConfig;

/// Messages larger than this end the connection.
const MAX_MESSAGE: usize = 64 * 1024 * 1024;
/// The tracing target written to `lsp.log`: server stderr, starts, and exits.
pub const LOG_TARGET: &str = "lsp";
/// Stderr lines kept for the error shown when a server exits.
const STDERR_LINES: usize = 20;
/// Longer stderr lines are split, so output without newlines stays bounded.
const STDERR_LINE_BYTES: u64 = 64 * 1024;

/// Read one message; `None` at the end of the stream.
pub async fn read_message<R: AsyncBufRead + Unpin>(
    reader: &mut R,
) -> std::io::Result<Option<Value>> {
    let invalid = |e: String| std::io::Error::new(std::io::ErrorKind::InvalidData, e);
    let mut length = None;
    let mut line = String::new();
    loop {
        line.clear();
        if reader.read_line(&mut line).await? == 0 {
            return Ok(None);
        }
        let header = line.trim_end();
        if header.is_empty() {
            break;
        }
        if let Some((name, value)) = header.split_once(':') {
            if name.eq_ignore_ascii_case("content-length") {
                length = Some(
                    value
                        .trim()
                        .parse::<usize>()
                        .map_err(|e| invalid(e.to_string()))?,
                );
            }
        }
    }
    let length = length.ok_or_else(|| invalid("no Content-Length".into()))?;
    if length > MAX_MESSAGE {
        return Err(invalid(format!("message of {length} bytes")));
    }
    let mut body = vec![0; length];
    reader.read_exact(&mut body).await?;
    serde_json::from_slice(&body)
        .map(Some)
        .map_err(|e| invalid(e.to_string()))
}

pub fn frame(message: &Value) -> Vec<u8> {
    let body = serde_json::to_vec(message).expect("JSON serializes");
    let mut out = format!("Content-Length: {}\r\n\r\n", body.len()).into_bytes();
    out.extend_from_slice(&body);
    out
}

/// How a server process ended.
pub struct Exit {
    pub message: String,
    /// Worth starting again (it ran, rather than failing to start).
    pub started: bool,
}

/// The running process's handles: messages to it, and dropping `stop` ends it.
pub struct Handle {
    pub tx: mpsc::UnboundedSender<Value>,
    pub stop: oneshot::Sender<()>,
}

/// Start `config` in `root`. Its messages go to `on_message`; `on_exit` runs once at the end.
pub fn spawn(
    name: String,
    root: PathBuf,
    config: LanguageServerConfig,
    shell: String,
    sandbox: Option<String>,
    on_message: impl Fn(Value) + Send + Sync + 'static,
    on_exit: impl FnOnce(Exit) + Send + 'static,
) -> Handle {
    let (tx, rx) = mpsc::unbounded_channel();
    let (stop, stopped) = oneshot::channel();
    tokio::spawn(async move {
        let sandboxed = sandbox.is_some();
        let command = match sandbox {
            Some(instance) => in_sandbox(&instance, &root, &config)
                .ok_or_else(|| "Multipass isn't installed".to_string()),
            None => on_host(&config, &shell).await,
        };
        let exit = match command {
            Ok(mut command) => {
                command.current_dir(&root);
                run(&name, &config, sandboxed, command, rx, stopped, on_message).await
            }
            Err(message) => {
                tracing::info!(target: LOG_TARGET, "{name}: {message}");
                Exit {
                    message,
                    started: false,
                }
            }
        };
        on_exit(exit);
    });
    Handle { tx, stop }
}

/// The server on this machine, found on `PATH`, else the shell's.
async fn on_host(
    config: &LanguageServerConfig,
    shell: &str,
) -> Result<tokio::process::Command, String> {
    if config.command.is_empty() {
        return Err("no command".into());
    }
    let program = OsString::from(&config.command);
    let [shell_path] = crate::agent_limits::programs_paths(
        shell,
        [program.clone()],
        crate::agent_limits::AskShell::Retry,
    )
    .await;
    let search = shell_path
        .clone()
        .or_else(|| std::env::var_os("PATH"))
        .ok_or("PATH is unset")?;
    let path = crate::agent_limits::find_in(&program, &search)
        .ok_or_else(|| format!("{} not found", config.command))?;
    let mut command = tokio::process::Command::new(path);
    command.args(&config.args).envs(&config.environment);
    if let Some(path) = shell_path {
        command.env("PATH", path);
    }
    Ok(command)
}

/// The server in a Multipass guest, where the mount has the same path, run
/// by a login shell so the guest's `PATH` (toolchains) applies.
fn in_sandbox(
    instance: &str,
    root: &std::path::Path,
    config: &LanguageServerConfig,
) -> Option<tokio::process::Command> {
    let mut command = tokio::process::Command::new(crate::multipass::program()?);
    command.args(sandbox_args(instance, root, config));
    Some(command)
}

fn sandbox_args(
    instance: &str,
    root: &std::path::Path,
    config: &LanguageServerConfig,
) -> Vec<OsString> {
    let mut args: Vec<OsString> =
        vec!["exec".into(), instance.into(), "--working-directory".into()];
    args.push(root.into());
    args.extend(["--".into(), "env".into()]);
    args.extend(
        config
            .environment
            .iter()
            .map(|(k, v)| format!("{k}={v}").into()),
    );
    args.extend(["bash".into(), "-lc".into(), r#"exec "$0" "$@""#.into()]);
    args.push((&config.command).into());
    args.extend(config.args.iter().map(OsString::from));
    args
}

/// Log each stderr line and keep the last few, decoding invalid UTF-8 lossily
/// so one bad byte doesn't end the stream.
async fn drain_stderr<R: AsyncBufRead + Unpin>(
    mut reader: R,
    label: &str,
    tail: &Mutex<VecDeque<String>>,
) {
    let mut buf = Vec::new();
    loop {
        buf.clear();
        match (&mut reader)
            .take(STDERR_LINE_BYTES)
            .read_until(b'\n', &mut buf)
            .await
        {
            Ok(0) | Err(_) => break,
            Ok(_) => {}
        }
        let line = String::from_utf8_lossy(&buf);
        let line = line.trim_end_matches(['\n', '\r']);
        tracing::info!(target: LOG_TARGET, "{label}: {line}");
        let mut tail = tail.lock().unwrap();
        if tail.len() == STDERR_LINES {
            tail.pop_front();
        }
        tail.push_back(line.to_owned());
    }
}

/// The shell's status for a program it couldn't find.
const NOT_FOUND: i32 = 127;

async fn run(
    name: &str,
    config: &LanguageServerConfig,
    sandboxed: bool,
    mut command: tokio::process::Command,
    mut rx: mpsc::UnboundedReceiver<Value>,
    mut stopped: oneshot::Receiver<()>,
    on_message: impl Fn(Value) + Sync,
) -> Exit {
    let failed = |message: String| Exit {
        message,
        started: false,
    };
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    // Its own group, so stopping it reaches helpers it starts (tsserver).
    #[cfg(unix)]
    command.process_group(0);
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(e) => {
            let message = format!("Cannot start {}: {e}", config.command);
            tracing::info!(target: LOG_TARGET, "{name}: {message}");
            return failed(message);
        }
    };
    let pid = child.id();
    let label = match pid {
        Some(pid) => format!("{name}[{pid}]"),
        None => name.to_owned(),
    };
    let root = command.as_std().get_current_dir().unwrap_or(".".as_ref());
    tracing::info!(
        target: LOG_TARGET,
        "{label}: started {} in {}",
        std::iter::once(&config.command)
            .chain(&config.args)
            .map(String::as_str)
            .collect::<Vec<_>>()
            .join(" "),
        root.display()
    );
    let mut stdin = child.stdin.take().expect("piped");
    let stdout = child.stdout.take().expect("piped");
    let stderr = child.stderr.take().expect("piped");

    let tail = Arc::new(Mutex::new(VecDeque::new()));
    let stderr_tail = tail.clone();
    let server = label.clone();
    let drain =
        tokio::spawn(
            async move { drain_stderr(BufReader::new(stderr), &server, &stderr_tail).await },
        );
    let writer = tokio::spawn(async move {
        while let Some(message) = rx.recv().await {
            if stdin.write_all(&frame(&message)).await.is_err() || stdin.flush().await.is_err() {
                break;
            }
        }
        // Closing stdin asks the server to exit.
    });
    let reader = async {
        let mut stdout = BufReader::new(stdout);
        loop {
            match read_message(&mut stdout).await {
                Ok(Some(message)) => on_message(message),
                Ok(None) => break,
                Err(e) => {
                    tracing::warn!(target: LOG_TARGET, "{label}: invalid message: {e}");
                    break;
                }
            }
        }
    };
    tokio::select! {
        () = reader => {}
        _ = &mut stopped => {
            // Give `shutdown` and `exit` a moment, then end the group.
            let _ = tokio::time::timeout(Duration::from_secs(2), child.wait()).await;
        }
    }
    #[cfg(unix)]
    if let Some(pid) = pid {
        // SAFETY: signals the server's own process group.
        unsafe { libc::killpg(pid as i32, libc::SIGKILL) };
    }
    #[cfg(not(unix))]
    let _ = pid;
    let _ = child.kill().await;
    let status = child.wait().await;
    writer.abort();
    // The group is gone, so stderr ends: take its last lines.
    let _ = tokio::time::timeout(Duration::from_secs(1), drain).await;
    let tail: Vec<String> = tail.lock().unwrap().iter().cloned().collect();
    let code = status.as_ref().ok().and_then(|status| status.code());
    let status = match status {
        Ok(status) => status.to_string(),
        Err(e) => e.to_string(),
    };
    tracing::info!(target: LOG_TARGET, "{label}: exited ({status})");
    // The guest shell's status for a missing program; a host server's own 127 is a crash.
    if sandboxed && code == Some(NOT_FOUND) {
        return failed(format!("{} not found in the sandbox", config.command));
    }
    Exit {
        message: match tail.last() {
            Some(last) => format!("{name} exited ({status}): {last}"),
            None => format!("{name} exited ({status})"),
        },
        started: true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn sandboxed_servers_run_in_a_guest_login_shell() {
        let config = LanguageServerConfig {
            command: "rust-analyzer".into(),
            args: vec!["--x".into()],
            environment: [("A".to_string(), "1".to_string())].into(),
            ..Default::default()
        };
        let args = sandbox_args("btmux-sandbox", std::path::Path::new("/w"), &config);
        let args: Vec<_> = args.iter().map(|a| a.to_str().unwrap()).collect();
        assert_eq!(
            args,
            [
                "exec",
                "btmux-sandbox",
                "--working-directory",
                "/w",
                "--",
                "env",
                "A=1",
                "bash",
                "-lc",
                r#"exec "$0" "$@""#,
                "rust-analyzer",
                "--x",
            ]
        );
    }

    #[derive(Clone, Default)]
    struct Captured(Arc<Mutex<Vec<u8>>>);

    impl std::io::Write for Captured {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    /// Never enabled, but never rules a callsite out: tracing caches each
    /// callsite's interest globally, so one first hit on another test's
    /// thread, with no subscriber, would stay disabled for this thread's.
    struct Undecided;

    impl tracing::Subscriber for Undecided {
        fn register_callsite(
            &self,
            _: &'static tracing::Metadata<'static>,
        ) -> tracing::subscriber::Interest {
            tracing::subscriber::Interest::sometimes()
        }
        fn max_level_hint(&self) -> Option<tracing::level_filters::LevelFilter> {
            Some(tracing::level_filters::LevelFilter::INFO)
        }
        fn enabled(&self, _: &tracing::Metadata<'_>) -> bool {
            false
        }
        fn new_span(&self, _: &tracing::span::Attributes<'_>) -> tracing::span::Id {
            tracing::span::Id::from_u64(1)
        }
        fn record(&self, _: &tracing::span::Id, _: &tracing::span::Record<'_>) {}
        fn record_follows_from(&self, _: &tracing::span::Id, _: &tracing::span::Id) {}
        fn event(&self, _: &tracing::Event<'_>) {}
        fn enter(&self, _: &tracing::span::Id) {}
        fn exit(&self, _: &tracing::span::Id) {}
    }

    /// Captures this thread's logs.
    fn capture() -> (Captured, tracing::subscriber::DefaultGuard) {
        static UNDECIDED: std::sync::Once = std::sync::Once::new();
        UNDECIDED.call_once(|| {
            let _ = tracing::subscriber::set_global_default(Undecided);
        });
        let captured = Captured::default();
        let writer = captured.clone();
        let subscriber = tracing_subscriber::fmt()
            .with_writer(move || writer.clone())
            .with_ansi(false)
            .finish();
        let guard = tracing::subscriber::set_default(subscriber);
        // Callsites registered as no one's before `Undecided` was installed.
        tracing::callsite::rebuild_interest_cache();
        (captured, guard)
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn logs_all_of_stderr_and_the_exit() {
        let (captured, _guard) = capture();
        let mut command = tokio::process::Command::new("sh");
        command.args(["-c", r"printf 'a\n\377\nb\n' >&2; exit 127"]);
        let config = LanguageServerConfig {
            command: "fake".into(),
            ..Default::default()
        };
        let (_tx, rx) = mpsc::unbounded_channel();
        let (_stop, stopped) = oneshot::channel();
        let exit = run("fake", &config, true, command, rx, stopped, |_| {}).await;
        assert!(!exit.started);
        let text = String::from_utf8_lossy(&captured.0.lock().unwrap()).into_owned();
        assert!(text.contains(": a\n"), "{text}");
        assert!(text.contains(": \u{fffd}\n"), "{text}");
        // All of stderr comes before the exit.
        let b = text.find(": b\n").expect(&text);
        assert!(text[b..].contains("exited (exit status: 127)"), "{text}");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_host_servers_127_is_a_crash() {
        let mut command = tokio::process::Command::new("sh");
        command.args(["-c", "exit 127"]);
        let (_tx, rx) = mpsc::unbounded_channel();
        let (_stop, stopped) = oneshot::channel();
        let config = LanguageServerConfig::default();
        let exit = run("fake", &config, false, command, rx, stopped, |_| {}).await;
        assert!(exit.started, "{}", exit.message);
    }

    #[tokio::test]
    async fn frames_round_trip() {
        let a = json!({ "jsonrpc": "2.0", "id": 1, "method": "x", "params": { "ü": "\r\n" } });
        let b = json!({ "jsonrpc": "2.0", "method": "y" });
        let mut bytes = frame(&a);
        bytes.extend(frame(&b));
        let mut reader = BufReader::new(&bytes[..]);
        assert_eq!(read_message(&mut reader).await.unwrap(), Some(a));
        assert_eq!(read_message(&mut reader).await.unwrap(), Some(b));
        assert_eq!(read_message(&mut reader).await.unwrap(), None);
    }
}
