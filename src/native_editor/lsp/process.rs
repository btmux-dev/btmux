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
/// Stderr lines kept for the error shown when a server exits.
const STDERR_LINES: usize = 20;

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
    on_message: impl Fn(Value) + Send + Sync + 'static,
    on_exit: impl FnOnce(Exit) + Send + 'static,
) -> Handle {
    let (tx, rx) = mpsc::unbounded_channel();
    let (stop, stopped) = oneshot::channel();
    tokio::spawn(async move {
        let exit = run(&name, &root, &config, &shell, rx, stopped, on_message).await;
        on_exit(exit);
    });
    Handle { tx, stop }
}

async fn run(
    name: &str,
    root: &PathBuf,
    config: &LanguageServerConfig,
    shell: &str,
    mut rx: mpsc::UnboundedReceiver<Value>,
    mut stopped: oneshot::Receiver<()>,
    on_message: impl Fn(Value) + Sync,
) -> Exit {
    let failed = |message: String| Exit {
        message,
        started: false,
    };
    if config.command.is_empty() {
        return failed(format!("{name} has no command"));
    }
    let program = OsString::from(&config.command);
    let [shell_path] = crate::agent_limits::programs_paths(
        shell,
        [program.clone()],
        crate::agent_limits::AskShell::Retry,
    )
    .await;
    let Some(search) = shell_path.clone().or_else(|| std::env::var_os("PATH")) else {
        return failed("PATH is unset".into());
    };
    let Some(path) = crate::agent_limits::find_in(&program, &search) else {
        return failed(format!("{} not found", config.command));
    };
    let mut command = tokio::process::Command::new(path);
    command
        .args(&config.args)
        .current_dir(root)
        .envs(&config.environment)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    if let Some(path) = shell_path {
        command.env("PATH", path);
    }
    // Its own group, so stopping it reaches helpers it starts (tsserver).
    #[cfg(unix)]
    command.process_group(0);
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(e) => return failed(format!("Cannot start {}: {e}", config.command)),
    };
    let pid = child.id();
    let mut stdin = child.stdin.take().expect("piped");
    let stdout = child.stdout.take().expect("piped");
    let stderr = child.stderr.take().expect("piped");

    let tail = Arc::new(Mutex::new(VecDeque::new()));
    let stderr_tail = tail.clone();
    let server = name.to_owned();
    tokio::spawn(async move {
        let mut lines = BufReader::new(stderr).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            tracing::debug!(server, "{line}");
            let mut tail = stderr_tail.lock().unwrap();
            if tail.len() == STDERR_LINES {
                tail.pop_front();
            }
            tail.push_back(line);
        }
    });
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
                    tracing::warn!(server = name, "invalid message: {e}");
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
    let tail: Vec<String> = tail.lock().unwrap().iter().cloned().collect();
    let status = match status {
        Ok(status) => status.to_string(),
        Err(e) => e.to_string(),
    };
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
