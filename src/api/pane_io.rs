use axum::{
    body::Body,
    extract::{Path, Query, State},
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::pty::screen::MAX_CAPTURE_HISTORY;
use crate::{automation, config, AppState};

pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/api/panes/{pane_id}/input",
            axum::routing::post(api_pane_input),
        )
        .route(
            "/api/panes/{pane_id}/output",
            axum::routing::get(api_pane_output),
        )
        .route(
            "/api/panes/{pane_id}/capture",
            axum::routing::get(api_pane_capture),
        )
        .route(
            "/api/panes/{pane_id}/prompt",
            axum::routing::post(api_pane_prompt),
        )
        .route(
            "/api/panes/{pane_id}/wait",
            axum::routing::get(api_pane_wait),
        )
        .route("/api/agents", axum::routing::get(api_agents))
}

#[derive(Deserialize)]
#[cfg_attr(test, derive(utoipa::ToSchema))]
struct PaneInputRequest {
    /// Sent verbatim, before `keys`.
    #[serde(default)]
    text: String,
    /// tmux-style key names, e.g. `["Enter"]` or `["C-c"]`.
    #[serde(default)]
    keys: Vec<String>,
    /// Only used if the PTY hasn't been spawned yet (no browser has attached
    /// to this pane). Ignored once a shell is already running.
    cols: Option<u16>,
    rows: Option<u16>,
}

#[derive(Serialize)]
#[cfg_attr(test, derive(utoipa::ToSchema))]
struct PaneInputResponse {
    /// True if this call caused the shell to spawn. Callers polling
    /// `/output` afterward should expect a short delay before the shell
    /// prints its first prompt.
    newly_spawned: bool,
}

#[cfg_attr(test, utoipa::path(post, path = "/api/panes/{pane_id}/input", tag = "Pane I/O", operation_id = "send-input", summary = "Send input", params(("pane_id" = Uuid, Path, description = "Pane ID")), request_body = PaneInputRequest, responses((status = 200, description = "Sent", body = PaneInputResponse), (status = 404, description = "No such pane"), (status = 400, description = "Invalid request", body = String, content_type = "text/plain"), (status = 409, description = "Conflict", body = String, content_type = "text/plain"))))]
async fn api_pane_input(
    State(state): State<AppState>,
    Path(pane_id): Path<Uuid>,
    Json(body): Json<PaneInputRequest>,
) -> Response {
    let size = (
        body.cols.unwrap_or(config::DEFAULT_PTY_COLS),
        body.rows.unwrap_or(config::DEFAULT_PTY_ROWS),
    );
    match automation::send_keys(&state, pane_id, &body.text, &body.keys, Some(size)).await {
        Ok(newly_spawned) => Json(PaneInputResponse { newly_spawned }).into_response(),
        Err(error) => error_response(error),
    }
}

#[derive(Deserialize)]
#[cfg_attr(test, derive(utoipa::IntoParams))]
#[cfg_attr(test, into_params(parameter_in = Query))]
struct PaneOutputQuery {
    cols: Option<u16>,
    rows: Option<u16>,
}

#[cfg_attr(test, utoipa::path(get, path = "/api/panes/{pane_id}/output", tag = "Pane I/O", operation_id = "read-raw-output", summary = "Read raw output", params(("pane_id" = Uuid, Path, description = "Pane ID"), PaneOutputQuery), responses((status = 200, description = "Raw PTY bytes, escapes included", body = Vec<u8>, content_type = "application/octet-stream"), (status = 400, description = "The shell failed to start"), (status = 404, description = "No such pane"))))]
/// Returns the pane's current scrollback as raw PTY bytes — exactly what the
/// terminal emulator receives (ANSI escapes included). `/capture` renders it
/// as text.
async fn api_pane_output(
    State(state): State<AppState>,
    Path(pane_id): Path<Uuid>,
    Query(query): Query<PaneOutputQuery>,
) -> Response {
    let mut mgr = state.write().await;
    let Some(pane) = mgr.find_pane_mut(pane_id) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    if let Err(error) = pane.pty.ensure_spawned(
        query.cols.unwrap_or(config::DEFAULT_PTY_COLS),
        query.rows.unwrap_or(config::DEFAULT_PTY_ROWS),
    ) {
        return (StatusCode::BAD_REQUEST, error).into_response();
    }
    let (_rx, scrollback) = pane.pty.subscribe_and_get_scrollback();

    (
        [(header::CONTENT_TYPE, "application/octet-stream")],
        Body::from(scrollback),
    )
        .into_response()
}

#[derive(Deserialize)]
#[cfg_attr(test, derive(utoipa::IntoParams))]
#[cfg_attr(test, into_params(parameter_in = Query))]
struct PaneCaptureQuery {
    /// Include retained scrollback above the screen.
    #[serde(default)]
    scrollback: bool,
    /// Include at most this many rows of history (`tmux capture-pane -S -N`).
    history: Option<usize>,
    /// Only the last N lines.
    lines: Option<usize>,
}

#[cfg_attr(test, utoipa::path(get, path = "/api/panes/{pane_id}/capture", tag = "Pane I/O", operation_id = "capture-the-screen", summary = "Capture the screen", params(("pane_id" = Uuid, Path, description = "Pane ID"), PaneCaptureQuery), responses((status = 200, description = "The rendered text", body = String, content_type = "text/plain"), (status = 404, description = "No such pane"), (status = 400, description = "Invalid request", body = String, content_type = "text/plain"), (status = 409, description = "Conflict", body = String, content_type = "text/plain"))))]
/// The pane as rendered plain text, like `tmux capture-pane -pJ`.
async fn api_pane_capture(
    State(state): State<AppState>,
    Path(pane_id): Path<Uuid>,
    Query(query): Query<PaneCaptureQuery>,
) -> Response {
    let history = if query.scrollback {
        MAX_CAPTURE_HISTORY
    } else {
        query.history.unwrap_or(0)
    };
    match automation::capture_pane(&state, pane_id, history, query.lines).await {
        Ok(text) => ([(header::CONTENT_TYPE, "text/plain; charset=utf-8")], text).into_response(),
        Err(error) => error_response(error),
    }
}

#[derive(Deserialize)]
#[cfg_attr(test, derive(utoipa::ToSchema))]
struct PanePromptRequest {
    prompt: String,
    #[serde(default = "default_true")]
    wait: bool,
    timeout_ms: Option<u64>,
}

fn default_true() -> bool {
    true
}

#[cfg_attr(test, utoipa::path(post, path = "/api/panes/{pane_id}/prompt", tag = "Agents", operation_id = "prompt-an-agent", summary = "Prompt an agent", description = "Pastes `prompt` into the pane's agent and submits it. With `wait` (the default), replies once the agent has started and settled (idle, done, or blocked).", params(("pane_id" = Uuid, Path, description = "Pane ID")), request_body = PanePromptRequest, responses((status = 200, description = "With `wait`, the turn's outcome; without, `{\"submitted\": true}`", body = crate::automation::PromptOutcome), (status = 404, description = "No such pane"), (status = 400, description = "Invalid request", body = String, content_type = "text/plain"), (status = 409, description = "Conflict", body = String, content_type = "text/plain"))))]
async fn api_pane_prompt(
    State(state): State<AppState>,
    Path(pane_id): Path<Uuid>,
    Json(body): Json<PanePromptRequest>,
) -> Response {
    match automation::send_prompt(
        &state,
        pane_id,
        &body.prompt,
        body.wait,
        automation::wait_duration(body.timeout_ms),
    )
    .await
    {
        Ok(Some(outcome)) => Json(outcome).into_response(),
        Ok(None) => Json(serde_json::json!({"submitted": true})).into_response(),
        Err(error) => error_response(error),
    }
}

#[derive(Deserialize)]
#[cfg_attr(test, derive(utoipa::IntoParams))]
#[cfg_attr(test, into_params(parameter_in = Query))]
struct PaneWaitQuery {
    /// Comma-separated agent states; default `idle,done,blocked`.
    states: Option<String>,
    timeout_ms: Option<u64>,
}

#[cfg_attr(test, utoipa::path(get, path = "/api/panes/{pane_id}/wait", tag = "Agents", operation_id = "wait-for-an-agent", summary = "Wait for an agent", params(("pane_id" = Uuid, Path, description = "Pane ID"), PaneWaitQuery), responses((status = 200, description = "The agent's status once it settles, exits, or the wait times out", body = crate::automation::AgentWait), (status = 404, description = "No such pane"), (status = 400, description = "Invalid request", body = String, content_type = "text/plain"), (status = 409, description = "Conflict", body = String, content_type = "text/plain"))))]
/// Long-polls until the pane's agent reaches one of `states` or exits.
async fn api_pane_wait(
    State(state): State<AppState>,
    Path(pane_id): Path<Uuid>,
    Query(query): Query<PaneWaitQuery>,
) -> Response {
    let states = match query.states {
        Some(names) => match automation::parse_states(&names.split(',').collect::<Vec<_>>()) {
            Ok(states) => states,
            Err(error) => return (StatusCode::BAD_REQUEST, error).into_response(),
        },
        None => automation::SETTLED.to_vec(),
    };
    match automation::wait_for_agent(
        &state,
        pane_id,
        &states,
        automation::wait_duration(query.timeout_ms),
    )
    .await
    {
        Ok(outcome) => Json(outcome).into_response(),
        Err(error) => error_response(error),
    }
}

#[cfg_attr(test, utoipa::path(get, path = "/api/agents", tag = "Agents", operation_id = "list-agents", summary = "List agents", responses((status = 200, description = "Every pane running an agent", body = Vec<crate::session::AgentPane>))))]
async fn api_agents(State(state): State<AppState>) -> Response {
    Json(state.read().await.agent_panes()).into_response()
}

pub(super) fn error_response(error: automation::Error) -> Response {
    let status = match error {
        automation::Error::PaneNotFound(_) => StatusCode::NOT_FOUND,
        automation::Error::Invalid(_) => StatusCode::BAD_REQUEST,
        automation::Error::Conflict(_) => StatusCode::CONFLICT,
        automation::Error::Failed(_) => StatusCode::INTERNAL_SERVER_ERROR,
    };
    (status, error.to_string()).into_response()
}
