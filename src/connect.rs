//! Connections that reach the server through a btmux connect relay. Until the
//! tunnel client lands, the hidden `--connect-listen` listener stands in for
//! it, behind a proxy such as the relay spike's agent.
//!
//! Relayed tabs get no browser or VS Code surface: the browser streams a JPEG
//! per paint, which the relay can't afford, and `code tunnel` already covers
//! VS Code. The routes refuse them, and each relayed control socket gets a
//! config without them.

use axum::{
    extract::Request,
    http::StatusCode,
    middleware::Next,
    response::{IntoResponse, Response},
    Extension, Router,
};
use serde_json::Value;

/// Marks requests that arrived through the relay. It comes from the listener
/// that accepted the connection, never from anything a client sends.
#[derive(Clone, Copy)]
pub struct Tunneled;

/// The app as relayed connections see it.
pub fn app(app: Router) -> Router {
    app.layer(axum::middleware::from_fn(refuse_surfaces))
        .layer(Extension(Tunneled))
}

/// Binds `--connect-listen` and serves relayed connections in the background.
pub async fn listen(addr: &str, app: Router) {
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .unwrap_or_else(|e| {
            eprintln!("error: cannot bind --connect-listen {addr}: {e}");
            std::process::exit(1);
        });
    tracing::info!("serving relayed connections on {addr}");
    let app = self::app(app);
    tokio::spawn(async move {
        if let Err(error) = axum::serve(listener, app).await {
            tracing::warn!("relayed listener stopped: {error}");
        }
    });
}

fn refused(path: &str) -> bool {
    path == "/ws/browser"
        || path == "/api/vscode"
        || path.starts_with("/api/vscode/")
        || path == "/vscode"
        || path.starts_with("/vscode/")
}

async fn refuse_surfaces(request: Request, next: Next) -> Response {
    if refused(request.uri().path()) {
        return (StatusCode::FORBIDDEN, "Not available over btmux connect").into_response();
    }
    next.run(request).await
}

/// Announces compressed output on a relayed pane socket; it precedes every
/// other frame.
pub const PANE_COMPRESSION_FRAME: &str = r#"{"type":"compression","format":"deflate-raw"}"#;

/// Compresses a relayed pane socket's output frames. Terminal output is mostly
/// escape sequences repeated across small messages, so one deflate stream per
/// socket, sync-flushed after each frame, shrinks it about 12x, where
/// compressing frames separately gets about 2x. Each frame starts with its
/// uncompressed length as a big-endian u32, so the browser knows when a
/// frame's output is complete and can keep text frames in order with it.
pub struct PaneDeflater(flate2::Compress);

impl PaneDeflater {
    pub fn new() -> Self {
        Self(flate2::Compress::new(flate2::Compression::new(6), false))
    }

    pub fn frame(&mut self, data: &[u8]) -> Vec<u8> {
        let mut out = Vec::with_capacity(4 + data.len() / 2 + 64);
        out.extend_from_slice(&(data.len() as u32).to_be_bytes());
        let mut consumed = 0;
        loop {
            if out.capacity() - out.len() < 64 {
                out.reserve(out.capacity());
            }
            let before = self.0.total_in();
            self.0
                .compress_vec(&data[consumed..], &mut out, flate2::FlushCompress::Sync)
                .expect("deflate on an in-memory buffer");
            consumed += (self.0.total_in() - before) as usize;
            // Spare room left means the sync flush was written in full.
            if consumed == data.len() && out.len() < out.capacity() {
                return out;
            }
        }
    }
}

/// serde writes an internally tagged enum's tag first.
const CONFIG_PREFIX: &str = r#"{"type":"config","#;

/// Rewrites a pre-serialized control event for a relayed tab; others pass
/// through untouched, so only config events are parsed.
pub fn relay_event(json: String) -> String {
    if !json.starts_with(CONFIG_PREFIX) {
        return json;
    }
    let Ok(mut message) = serde_json::from_str::<Value>(&json) else {
        return json;
    };
    let Some(config) = message.get_mut("config").and_then(Value::as_object_mut) else {
        return json;
    };
    config.insert("relayed".into(), true.into());
    config.insert("browser".into(), false.into());
    let mut neovim = false;
    if let Some(editors) = config.get_mut("editors").and_then(Value::as_array_mut) {
        editors.retain(|editor| editor != "vscode");
        neovim = editors.iter().any(|editor| editor == "neovim");
    }
    if config
        .get("editor")
        .is_some_and(|editor| editor == "vscode")
    {
        let editor = if neovim { "neovim" } else { "pane" };
        config.insert("editor".into(), editor.into());
    }
    message.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{ColorMode, Editor, FileConfig};
    use crate::ws::control::ServerMessage;

    fn config_event(editor: Editor, editors: Vec<Editor>) -> String {
        let mut config = crate::config::resolve_binds(&FileConfig::default(), ColorMode::Dark);
        config.editor = editor;
        config.editors = editors;
        config.browser = true;
        serde_json::to_string(&ServerMessage::Config {
            config: Box::new(config),
        })
        .unwrap()
    }

    #[test]
    fn relayed_config_drops_the_heavy_surfaces() {
        let json = config_event(Editor::Vscode, vec![Editor::Neovim, Editor::Vscode]);
        assert!(json.starts_with(CONFIG_PREFIX));
        let relayed: Value = serde_json::from_str(&relay_event(json)).unwrap();
        let config = &relayed["config"];
        assert_eq!(config["relayed"], true);
        assert_eq!(config["browser"], false);
        assert_eq!(config["editors"], serde_json::json!(["neovim"]));
        assert_eq!(config["editor"], "neovim");

        let json = config_event(Editor::Vscode, vec![Editor::Vscode]);
        let relayed: Value = serde_json::from_str(&relay_event(json)).unwrap();
        assert_eq!(relayed["config"]["editor"], "pane");

        let state = r#"{"type":"state","sessions":[]}"#.to_string();
        assert_eq!(relay_event(state.clone()), state);
    }

    #[test]
    fn pane_frames_inflate_in_order_from_one_stream() {
        let mut deflater = PaneDeflater::new();
        let mut inflater = flate2::Decompress::new(false);
        let prompt = b"\x1b[32m~/repos/btmux\x1b[0m main \xe2\x9d\xaf ".repeat(4);
        let big: Vec<u8> = (0..200_000u32).map(|i| (i * 7919 % 251) as u8).collect();
        let mut sizes = Vec::new();
        for message in [&b"x"[..], &prompt, &prompt, &big, &prompt] {
            let frame = deflater.frame(message);
            sizes.push(frame.len());
            let size = u32::from_be_bytes(frame[..4].try_into().unwrap()) as usize;
            assert_eq!(size, message.len());
            // Everything a frame encodes is available without the next one.
            let mut out = Vec::with_capacity(size + 1);
            inflater
                .decompress_vec(&frame[4..], &mut out, flate2::FlushDecompress::Sync)
                .unwrap();
            assert_eq!(out, message);
        }
        // A repeat compresses against the shared window.
        assert!(sizes[2] < prompt.len() / 4, "{sizes:?}");
    }

    #[tokio::test]
    async fn relayed_requests_cannot_reach_the_surfaces() {
        use axum::{body::Body, http::Request, routing::get};
        use tower::ServiceExt;
        let app = app(Router::new().fallback(get(|| async { "ok" })));
        for (path, status) in [
            ("/ws/browser", StatusCode::FORBIDDEN),
            ("/api/vscode", StatusCode::FORBIDDEN),
            ("/api/vscode/license", StatusCode::FORBIDDEN),
            ("/vscode/", StatusCode::FORBIDDEN),
            ("/vscode/static/x.js", StatusCode::FORBIDDEN),
            ("/api/sessions", StatusCode::OK),
            ("/vscodex", StatusCode::OK),
        ] {
            let request = Request::get(path).body(Body::empty()).unwrap();
            let response = app.clone().oneshot(request).await.unwrap();
            assert_eq!(response.status(), status, "{path}");
        }
    }
}
