use axum::{
    extract::{
        ws::{Message, WebSocket},
        Path, Query, State, WebSocketUpgrade,
    },
    response::IntoResponse,
    Extension,
};
use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;
use uuid::Uuid;

use crate::{config, AppState};

#[derive(Deserialize)]
pub struct PaneParams {
    cols: Option<u16>,
    rows: Option<u16>,
    /// Read-only mirror attach for window-grid thumbnails. A mirror subscribes to
    /// the pane's output and scrollback but never resizes the PTY and ignores all
    /// input — so it can attach alongside the real viewer without reflowing the
    /// live shell or clearing its scrollback. The PTY's size is pushed first as a
    /// `{"type":"current_size",...}` frame, then in stream order as `{"type":"size",...}`
    /// frames: replayed ones are history, those after `ready` are live.
    ///
    /// Parsed as a string (not `bool`) so `?mirror=1` works too — `serde_urlencoded`
    /// only accepts `true`/`false` for `bool`, and a parse failure would reject the
    /// whole query and fail the WebSocket upgrade. `is_truthy` treats `1`/`true` as set.
    mirror: Option<String>,
}

/// Whether a query flag string means "enabled" (`1` or `true`, case-insensitive).
fn is_truthy(v: &Option<String>) -> bool {
    matches!(
        v.as_deref(),
        Some("1") | Some("true") | Some("TRUE") | Some("True")
    )
}

pub async fn handle(
    Path(pane_id): Path<Uuid>,
    Query(params): Query<PaneParams>,
    State(state): State<AppState>,
    tunneled: Option<Extension<crate::connect::Tunneled>>,
    ws: WebSocketUpgrade,
) -> impl IntoResponse {
    let cols = params.cols.unwrap_or(config::DEFAULT_PTY_COLS);
    let rows = params.rows.unwrap_or(config::DEFAULT_PTY_ROWS);
    let mirror = is_truthy(&params.mirror);
    // Relay bandwidth is what compression saves; locally it would only cost CPU.
    let deflater = tunneled.map(|_| crate::connect::PaneDeflater::new());

    ws.max_message_size(64 * 1024).on_upgrade(move |socket| {
        handle_socket(socket, pane_id, cols, rows, mirror, deflater, state)
    })
}

/// Serialize a size update for a mirror socket.
fn size_frame(cols: u16, rows: u16) -> String {
    format!(r#"{{"type":"size","cols":{cols},"rows":{rows}}}"#)
}

/// The PTY's size now, ahead of a mirror's replay of older sizes.
fn current_size_frame((cols, rows): (u16, u16)) -> Message {
    Message::Text(format!(r#"{{"type":"current_size","cols":{cols},"rows":{rows}}}"#).into())
}

/// Tell a viewer whether its size drives the PTY.
fn owner_frame(owner: bool) -> Message {
    Message::Text(format!(r#"{{"type":"owner","owner":{owner}}}"#).into())
}

async fn handle_socket(
    socket: WebSocket,
    pane_id: Uuid,
    cols: u16,
    rows: u16,
    mirror: bool,
    mut deflater: Option<crate::connect::PaneDeflater>,
    state: AppState,
) {
    use crate::pty::replay::Output;
    let viewer_id = Uuid::new_v4();
    let (mut ws_tx, mut ws_rx) = socket.split();
    let attachment = {
        let mut mgr = state.write().await;
        let attachment = mgr.find_pty_mut(pane_id).map(|pty| {
            pty.ensure_spawned(cols, rows)?;
            if !mirror {
                pty.attach_viewer(viewer_id, cols, rows);
            }
            let (rx, replay) = pty.subscribe_replay();
            // After subscribing, so a later resize arrives as a live frame.
            let size = pty.size();
            Ok::<_, String>((
                pty.input_tx.clone(),
                rx,
                replay,
                pty.subscribe_owner(),
                size,
            ))
        });
        // A popup whose program can't start ends; one that started wakes its request.
        if mgr.popups.contains(pane_id) {
            match &attachment {
                Some(Err(error)) => {
                    if mgr.popups.failed(pane_id, error.clone()) {
                        crate::popup::broadcast(&mgr);
                    }
                }
                _ => mgr.popups.touch(),
            }
        }
        attachment
    };
    let (input_tx, mut output_rx, replay, mut owner_rx, size) = match attachment {
        Some(Ok(attachment)) => attachment,
        failure => {
            let error = failure
                .and_then(Result::err)
                .unwrap_or_else(|| "Pane no longer exists".into());
            tracing::warn!(%pane_id, %error, "pane attach failed");
            let frame = Message::Text(
                serde_json::json!({"type": "error", "message": error})
                    .to_string()
                    .into(),
            );
            let _ =
                tokio::time::timeout(std::time::Duration::from_secs(2), ws_tx.send(frame)).await;
            return;
        }
    };
    let is_owner = |owner: &Option<Uuid>| !mirror && *owner == Some(viewer_id);
    let send = async {
        let mut pending = replay;
        let mut replaying = true;
        let mut owner = is_owner(&owner_rx.borrow_and_update());
        let mut first = vec![if mirror {
            current_size_frame(size)
        } else {
            owner_frame(owner)
        }];
        if deflater.is_some() {
            first.insert(
                0,
                Message::Text(crate::connect::PANE_COMPRESSION_FRAME.into()),
            );
        }
        for frame in first {
            if !matches!(
                tokio::time::timeout(std::time::Duration::from_secs(10), ws_tx.send(frame)).await,
                Ok(Ok(()))
            ) {
                return;
            }
        }
        loop {
            for event in coalesce(std::mem::take(&mut pending)) {
                let frame = match event {
                    Output::Data(bytes) => match deflater.as_mut() {
                        Some(deflater) => Message::Binary(deflater.frame(&bytes).into()),
                        None => Message::Binary(bytes),
                    },
                    Output::Size(cols, rows) => Message::Text(size_frame(cols, rows).into()),
                };
                if !matches!(
                    tokio::time::timeout(std::time::Duration::from_secs(10), ws_tx.send(frame))
                        .await,
                    Ok(Ok(()))
                ) {
                    return;
                }
            }
            if replaying {
                let ready = Message::Text(
                    serde_json::json!({"type": "ready", "pane_id": pane_id})
                        .to_string()
                        .into(),
                );
                if !matches!(
                    tokio::time::timeout(std::time::Duration::from_secs(10), ws_tx.send(ready))
                        .await,
                    Ok(Ok(()))
                ) {
                    return;
                }
                replaying = false;
            }
            tokio::select! {
                event = output_rx.recv() => match event {
                    Ok(event) => pending.push(event),
                    // Output is stateful: disconnect instead of silently continuing
                    // past missing bytes. The client reconnects to a fresh snapshot.
                    Err(_) => return,
                },
                changed = owner_rx.changed(), if !mirror => {
                    if changed.is_err() {
                        return;
                    }
                    let next = is_owner(&owner_rx.borrow_and_update());
                    if next != owner {
                        owner = next;
                        if !matches!(
                            tokio::time::timeout(std::time::Duration::from_secs(10), ws_tx.send(owner_frame(owner))).await,
                            Ok(Ok(()))
                        ) {
                            return;
                        }
                    }
                }
            }
        }
    };
    let receive = async {
        while let Some(Ok(msg)) = ws_rx.next().await {
            if matches!(msg, Message::Close(_)) {
                break;
            }
            if mirror {
                continue;
            }
            match msg {
                Message::Binary(data) => {
                    if input_tx.send_wait(data.to_vec()).await.is_err() {
                        break;
                    }
                }
                Message::Text(text) => {
                    // Input is binary, so other text never reaches the shell.
                    let Ok(control) = serde_json::from_str::<ControlMsg>(&text) else {
                        tracing::debug!(%pane_id, "ignored invalid pane control frame");
                        continue;
                    };
                    let mut mgr = state.write().await;
                    if let Some(pty) = mgr.find_pty_mut(pane_id) {
                        match control {
                            ControlMsg::Resize { cols, rows } => {
                                pty.resize_viewer(viewer_id, cols, rows)
                            }
                            ControlMsg::Claim => pty.claim_viewer(viewer_id),
                        }
                    }
                }
                _ => {}
            }
        }
    };
    tokio::select! { _ = send => {}, _ = receive => {} }
    if !mirror {
        let mut mgr = state.write().await;
        if let Some(pty) = mgr.find_pty_mut(pane_id) {
            pty.detach_viewer(viewer_id);
        }
    }
}

/// Largest frame `coalesce` builds.
const MAX_FRAME: usize = 1 << 20;

/// Merges runs of output into frames of up to `MAX_FRAME`. A replay is
/// thousands of small reads, and each frame costs a message, plus a deflate
/// flush and an inflate step when relayed. Sizes stay between the bytes they
/// separate.
fn coalesce(events: Vec<crate::pty::replay::Output>) -> Vec<crate::pty::replay::Output> {
    use crate::pty::replay::Output;
    fn flush(out: &mut Vec<Output>, run: &mut Vec<axum::body::Bytes>, len: &mut usize) {
        match run.len() {
            0 => {}
            1 => out.extend(run.pop().map(Output::Data)),
            _ => {
                let mut merged = Vec::with_capacity(*len);
                for bytes in run.drain(..) {
                    merged.extend_from_slice(&bytes);
                }
                out.push(Output::Data(merged.into()));
            }
        }
        *len = 0;
    }
    let mut out = Vec::with_capacity(events.len());
    let (mut run, mut len) = (Vec::new(), 0);
    for event in events {
        match event {
            Output::Data(bytes) => {
                if len + bytes.len() > MAX_FRAME {
                    flush(&mut out, &mut run, &mut len);
                }
                len += bytes.len();
                run.push(bytes);
            }
            size => {
                flush(&mut out, &mut run, &mut len);
                out.push(size);
            }
        }
    }
    flush(&mut out, &mut run, &mut len);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pty::replay::Output;
    use axum::body::Bytes;

    #[test]
    fn coalesce_merges_output_runs_around_sizes() {
        let data = |s: &'static [u8]| Output::Data(Bytes::from_static(s));
        let big = Bytes::from(vec![b'x'; MAX_FRAME - 1]);
        let merged = coalesce(vec![
            data(b"a"),
            data(b"b"),
            Output::Size(80, 24),
            data(b"c"),
            Output::Data(big.clone()),
            data(b"de"),
        ]);
        let shape: Vec<String> = merged
            .iter()
            .map(|event| match event {
                Output::Data(bytes) => format!("data {}", bytes.len()),
                Output::Size(cols, rows) => format!("size {cols}x{rows}"),
            })
            .collect();
        assert_eq!(
            shape,
            [
                "data 2",
                "size 80x24",
                format!("data {MAX_FRAME}").as_str(),
                "data 2"
            ]
        );
        let Output::Data(first) = &merged[0] else {
            unreachable!()
        };
        assert_eq!(&first[..], b"ab");
    }
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ControlMsg {
    Resize {
        cols: u16,
        rows: u16,
    },
    /// User input in this viewer: make its size drive the PTY.
    Claim,
}
