//! `/ws/editor`: a tab's connection to the native editor's buffers.

use axum::extract::ws::{Message, WebSocket};
use axum::extract::WebSocketUpgrade;
use axum::response::IntoResponse;
use futures_util::{SinkExt, StreamExt};

use crate::native_editor::{hub, EditorClientMessage};

pub async fn handle(ws: WebSocketUpgrade) -> impl IntoResponse {
    // Pushes carry whole pastes; files are capped at 16 MiB.
    ws.max_message_size(64 * 1024 * 1024)
        .on_upgrade(handle_socket)
}

async fn handle_socket(socket: WebSocket) {
    let (mut ws_tx, mut ws_rx) = socket.split();
    let (id, mut rx) = hub().connect();

    let writer = async {
        // Ends when the hub drops this socket (lagging) or the tab goes away.
        while let Some(msg) = rx.recv().await {
            if ws_tx.send(Message::Text(msg.into())).await.is_err() {
                break;
            }
        }
        let _ = ws_tx.close().await;
    };
    let reader = async {
        while let Some(Ok(msg)) = ws_rx.next().await {
            let Message::Text(text) = msg else { continue };
            match serde_json::from_str::<EditorClientMessage>(&text) {
                // In order, so a push lands before a later save.
                Ok(msg) => hub().handle(id, msg).await,
                Err(e) => tracing::debug!("invalid editor message: {e}"),
            }
        }
    };
    tokio::select! {
        () = writer => {}
        () = reader => {}
    }
    hub().disconnect(id);
}
