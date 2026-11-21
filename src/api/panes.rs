use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::session::layout::LayoutPreset;
use crate::ws;
use crate::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/api/sessions/{session_id}/panes/{pane_id}/split",
            axum::routing::post(api_split_pane),
        )
        .route(
            "/api/sessions/{session_id}/panes/{pane_id}",
            axum::routing::delete(api_kill_pane),
        )
        .route(
            "/api/sessions/{session_id}/panes/{pane_id}/zoom",
            axum::routing::post(api_zoom_pane),
        )
        .route(
            "/api/sessions/{session_id}/panes/{pane_id}/select",
            axum::routing::post(api_select_pane),
        )
        .route(
            "/api/sessions/{session_id}/panes/navigate",
            axum::routing::post(api_navigate),
        )
        .route(
            "/api/sessions/{session_id}/panes/cycle",
            axum::routing::post(api_cycle_pane),
        )
        .route(
            "/api/sessions/{session_id}/panes/swap",
            axum::routing::post(api_swap_pane),
        )
        .route(
            "/api/sessions/{session_id}/layout",
            axum::routing::post(api_select_layout),
        )
        .route(
            "/api/sessions/{session_id}/layout/next",
            axum::routing::post(api_next_layout),
        )
        .route("/api/panes", axum::routing::get(api_list_panes))
        .route(
            "/api/panes/{pane_id}",
            axum::routing::delete(api_kill_pane_by_id),
        )
        .route(
            "/api/panes/{pane_id}/split",
            axum::routing::post(api_split_pane_by_id),
        )
        .route(
            "/api/panes/{pane_id}/select",
            axum::routing::post(api_select_pane_by_id),
        )
        .route(
            "/api/panes/{pane_id}/ports",
            axum::routing::get(api_pane_ports),
        )
        .route(
            "/api/panes/{pane_id}/exit",
            axum::routing::get(crate::pane_exit::api_wait_exit),
        )
}

/// The TCP ports that the pane's programs listen on, as `btmux forward` can
/// forward them.
#[cfg_attr(test, utoipa::path(
    get,
    path = "/api/panes/{pane_id}/ports",
    tag = "Panes",
    operation_id = "list-pane-ports",
    summary = "List a pane's listening ports",
    description = "The TCP ports that the pane's programs listen on, including its popups', as `btmux forward` can forward them.",
    params(
        ("pane_id", description = "Pane ID"),
    ),
    responses(
        (status = 200, description = "The listeners, by port", body = Vec<crate::pane_ports::Listener>),
        (status = 404, description = "No such pane"),
        (status = 409, description = "The pane is in a sandboxed session", body = String, content_type = "text/plain"),
    ),
))]
pub(crate) async fn api_pane_ports(
    State(state): State<AppState>,
    Path(pane_id): Path<Uuid>,
) -> Response {
    match crate::pane_ports::pane_listeners(&state, pane_id).await {
        Ok(listeners) => Json(listeners).into_response(),
        Err(unavailable) => unavailable.into_response(),
    }
}

#[cfg_attr(test, utoipa::path(
    get,
    path = "/api/panes",
    tag = "Panes",
    operation_id = "list-panes",
    summary = "List panes",
    responses(
        (status = 200, description = "Every pane, with its window and session", body = Vec<crate::session::PaneInfo>),
    ),
))]
pub(crate) async fn api_list_panes(State(state): State<AppState>) -> Response {
    Json(state.read().await.pane_infos()).into_response()
}

#[derive(Deserialize)]
#[cfg_attr(test, derive(utoipa::ToSchema))]
pub(crate) struct SplitPaneByIdRequest {
    /// Where the new pane goes: left, right, up, or down.
    direction: String,
    cwd: Option<String>,
    /// Make the new pane active.
    #[cfg_attr(test, schema(default = true))]
    select: Option<bool>,
    /// Run this instead of the shell, starting now rather than once a client
    /// shows the pane. The pane closes when it exits, unless `keep_open`.
    #[serde(default)]
    command: Option<crate::popup::PopupCommand>,
    /// With `command`: replaces the server's environment.
    #[serde(default)]
    #[cfg_attr(test, schema(value_type = crate::popup::Environment, inline))]
    env: Option<Vec<(String, String)>>,
    /// Keep the pane, with its output, after its program exits.
    #[serde(default)]
    keep_open: bool,
}

/// 400 unless `direction` is one the split routes accept.
fn check_split_direction(direction: &str) -> Result<(), (StatusCode, &'static str)> {
    if ["left", "right", "up", "down"].contains(&direction) {
        return Ok(());
    }
    Err((
        StatusCode::BAD_REQUEST,
        "direction must be left, right, up, or down",
    ))
}

#[derive(Serialize)]
#[cfg_attr(test, derive(utoipa::ToSchema))]
pub(crate) struct SplitPaneResponse {
    pane_id: Uuid,
}

/// Splits a pane in any window, returning the new pane's ID.
#[cfg_attr(test, utoipa::path(
    post,
    path = "/api/panes/{pane_id}/split",
    tag = "Panes",
    operation_id = "split-a-pane",
    summary = "Split a pane",
    params(
        ("pane_id", description = "Pane ID"),
    ),
    responses(
        (status = 200, description = "The new pane", body = SplitPaneResponse),
        (status = 400, description = "An invalid direction, an empty command, `env` without one, or a `cwd` that isn't a directory", body = String, content_type = "text/plain"),
        (status = 404, description = "No such pane"),
        (status = 422, description = "The command can't start", body = String, content_type = "text/plain"),
    ),
))]
pub(crate) async fn api_split_pane_by_id(
    State(state): State<AppState>,
    Path(pane_id): Path<Uuid>,
    Json(body): Json<SplitPaneByIdRequest>,
) -> Response {
    if let Err(rejection) = check_split_direction(&body.direction) {
        return rejection.into_response();
    }
    let spawn = match crate::pane_exit::spawn_of(body.command, body.env, body.cwd.as_deref()) {
        Ok(spawn) => spawn,
        Err(error) => return (StatusCode::BAD_REQUEST, error).into_response(),
    };
    let mut mgr = state.write().await;
    let cwd = body.cwd.map(std::path::PathBuf::from);
    let Some(new_pane_id) = mgr
        .split_pane(pane_id, &body.direction, cwd, body.select.unwrap_or(true))
        .await
    else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let launched = mgr.launch(new_pane_id, spawn, body.keep_open).await;
    ws::control::broadcast_state(&mgr);
    if let Err(error) = launched {
        return (StatusCode::UNPROCESSABLE_ENTITY, error).into_response();
    }

    Json(SplitPaneResponse {
        pane_id: new_pane_id,
    })
    .into_response()
}

/// Kills a pane in any window; its last pane closes the window, and the last
/// window the session.
#[cfg_attr(test, utoipa::path(
    delete,
    path = "/api/panes/{pane_id}",
    tag = "Panes",
    operation_id = "kill-a-pane",
    summary = "Kill a pane",
    description = "Kills a pane in any window; its last pane closes the window, and the last window the session.",
    params(
        ("pane_id", description = "Pane ID"),
    ),
    responses(
        (status = 204, description = "Done"),
        (status = 404, description = "No such pane"),
        (status = 409, description = "The pane can't be closed", body = String, content_type = "text/plain"),
    ),
))]
pub(crate) async fn api_kill_pane_by_id(
    State(state): State<AppState>,
    Path(pane_id): Path<Uuid>,
) -> Response {
    let mut mgr = state.write().await;
    if mgr.find_pane(pane_id).is_none() {
        return StatusCode::NOT_FOUND.into_response();
    }
    if let Err(error) = mgr.kill_pane_by_id(pane_id) {
        return (StatusCode::CONFLICT, error).into_response();
    }

    ws::control::broadcast_state(&mgr);

    StatusCode::NO_CONTENT.into_response()
}

/// Makes the pane active and its window the session's active window.
#[cfg_attr(test, utoipa::path(
    post,
    path = "/api/panes/{pane_id}/select",
    tag = "Panes",
    operation_id = "select-a-pane",
    summary = "Select a pane",
    description = "Makes the pane active and its window the session's active window.",
    params(
        ("pane_id", description = "Pane ID"),
    ),
    responses(
        (status = 204, description = "Done"),
        (status = 404, description = "No such pane"),
    ),
))]
pub(crate) async fn api_select_pane_by_id(
    State(state): State<AppState>,
    Path(pane_id): Path<Uuid>,
) -> Response {
    let mut mgr = state.write().await;
    if !mgr.select_pane_by_id(pane_id) {
        return StatusCode::NOT_FOUND.into_response();
    }

    ws::control::broadcast_state(&mgr);

    StatusCode::NO_CONTENT.into_response()
}

/// Resolves session-not-found (404) vs. pane-not-in-active-window (409) for
/// the pane-addressed endpoints below. `Ok(())` means it's safe to mutate.
fn check_pane(
    mgr: &crate::session::manager::SessionManager,
    session_id: Uuid,
    pane_id: Uuid,
) -> Result<(), StatusCode> {
    match super::pane_in_active_window(mgr, session_id, pane_id) {
        None => Err(StatusCode::NOT_FOUND),
        Some(false) => Err(StatusCode::CONFLICT),
        Some(true) => Ok(()),
    }
}

#[derive(Deserialize)]
#[cfg_attr(test, derive(utoipa::ToSchema))]
pub(crate) struct SplitPaneRequest {
    direction: String,
}

#[cfg_attr(test, utoipa::path(
    post,
    path = "/api/sessions/{session_id}/panes/{pane_id}/split",
    tag = "Session panes",
    operation_id = "split-an-active-window-pane",
    summary = "Split an active-window pane",
    params(
        ("session_id", description = "Session ID"),
        ("pane_id", description = "Pane ID"),
    ),
    responses(
        (status = 204, description = "Done"),
        (status = 400, description = "Invalid direction", body = String, content_type = "text/plain"),
        (status = 404, description = "No such session"),
        (status = 409, description = "The pane isn't in the session's active window"),
    ),
))]
pub(crate) async fn api_split_pane(
    State(state): State<AppState>,
    Path((session_id, pane_id)): Path<(Uuid, Uuid)>,
    Json(body): Json<SplitPaneRequest>,
) -> Response {
    if let Err(rejection) = check_split_direction(&body.direction) {
        return rejection.into_response();
    }
    let mut mgr = state.write().await;
    if let Err(status) = check_pane(&mgr, session_id, pane_id) {
        return status.into_response();
    }
    mgr.split_pane(pane_id, &body.direction, None, true).await;

    ws::control::broadcast_state(&mgr);

    StatusCode::NO_CONTENT.into_response()
}

#[cfg_attr(test, utoipa::path(
    delete,
    path = "/api/sessions/{session_id}/panes/{pane_id}",
    tag = "Session panes",
    operation_id = "kill-an-active-window-pane",
    summary = "Kill an active-window pane",
    params(
        ("session_id", description = "Session ID"),
        ("pane_id", description = "Pane ID"),
    ),
    responses(
        (status = 204, description = "Done"),
        (status = 404, description = "No such session"),
        (status = 409, description = "The pane isn't in the session's active window"),
    ),
))]
pub(crate) async fn api_kill_pane(
    State(state): State<AppState>,
    Path((session_id, pane_id)): Path<(Uuid, Uuid)>,
) -> Response {
    let mut mgr = state.write().await;
    if let Err(status) = check_pane(&mgr, session_id, pane_id) {
        return status.into_response();
    }
    mgr.kill_pane(session_id, pane_id);

    ws::control::broadcast_state(&mgr);

    StatusCode::NO_CONTENT.into_response()
}

#[cfg_attr(test, utoipa::path(
    post,
    path = "/api/sessions/{session_id}/panes/{pane_id}/zoom",
    tag = "Session panes",
    operation_id = "zoom-a-pane",
    summary = "Zoom a pane",
    params(
        ("session_id", description = "Session ID"),
        ("pane_id", description = "Pane ID"),
    ),
    responses(
        (status = 204, description = "Done"),
        (status = 404, description = "No such session"),
        (status = 409, description = "The pane isn't in the session's active window"),
    ),
))]
pub(crate) async fn api_zoom_pane(
    State(state): State<AppState>,
    Path((session_id, pane_id)): Path<(Uuid, Uuid)>,
) -> Response {
    let mut mgr = state.write().await;
    if let Err(status) = check_pane(&mgr, session_id, pane_id) {
        return status.into_response();
    }
    mgr.zoom_pane(session_id, pane_id);

    ws::control::broadcast_state(&mgr);

    StatusCode::NO_CONTENT.into_response()
}

#[cfg_attr(test, utoipa::path(
    post,
    path = "/api/sessions/{session_id}/panes/{pane_id}/select",
    tag = "Session panes",
    operation_id = "select-an-active-window-pane",
    summary = "Select an active-window pane",
    params(
        ("session_id", description = "Session ID"),
        ("pane_id", description = "Pane ID"),
    ),
    responses(
        (status = 204, description = "Done"),
        (status = 404, description = "No such session"),
        (status = 409, description = "The pane isn't in the session's active window"),
    ),
))]
pub(crate) async fn api_select_pane(
    State(state): State<AppState>,
    Path((session_id, pane_id)): Path<(Uuid, Uuid)>,
) -> Response {
    let mut mgr = state.write().await;
    if let Err(status) = check_pane(&mgr, session_id, pane_id) {
        return status.into_response();
    }
    mgr.select_pane(session_id, pane_id);

    ws::control::broadcast_state(&mgr);

    StatusCode::NO_CONTENT.into_response()
}

#[derive(Deserialize)]
#[cfg_attr(test, derive(utoipa::ToSchema))]
pub(crate) struct NavigateRequest {
    direction: String,
}

#[cfg_attr(test, utoipa::path(
    post,
    path = "/api/sessions/{session_id}/panes/navigate",
    tag = "Session panes",
    operation_id = "move-to-a-neighboring-pane",
    summary = "Move to a neighboring pane",
    description = "Selects the pane in `direction` (`left`, `right`, `up`, or `down`) from the active one.",
    params(
        ("session_id", description = "Session ID"),
    ),
    responses(
        (status = 204, description = "Done"),
        (status = 404, description = "No such session"),
    ),
))]
pub(crate) async fn api_navigate(
    State(state): State<AppState>,
    Path(session_id): Path<Uuid>,
    Json(body): Json<NavigateRequest>,
) -> Response {
    let mut mgr = state.write().await;
    if !super::session_exists(&mgr, session_id) {
        return StatusCode::NOT_FOUND.into_response();
    }
    mgr.navigate(session_id, body.direction);

    ws::control::broadcast_state(&mgr);

    StatusCode::NO_CONTENT.into_response()
}

#[derive(Deserialize)]
#[cfg_attr(test, derive(utoipa::ToSchema))]
pub(crate) struct DeltaRequest {
    delta: i32,
}

#[cfg_attr(test, utoipa::path(
    post,
    path = "/api/sessions/{session_id}/panes/cycle",
    tag = "Session panes",
    operation_id = "cycle-panes",
    summary = "Cycle panes",
    description = "Selects the pane `delta` places on in layout order.",
    params(
        ("session_id", description = "Session ID"),
    ),
    responses(
        (status = 204, description = "Done"),
        (status = 404, description = "No such session"),
    ),
))]
pub(crate) async fn api_cycle_pane(
    State(state): State<AppState>,
    Path(session_id): Path<Uuid>,
    Json(body): Json<DeltaRequest>,
) -> Response {
    let mut mgr = state.write().await;
    if !super::session_exists(&mgr, session_id) {
        return StatusCode::NOT_FOUND.into_response();
    }
    mgr.cycle_pane(session_id, body.delta);

    ws::control::broadcast_state(&mgr);

    StatusCode::NO_CONTENT.into_response()
}

#[cfg_attr(test, utoipa::path(
    post,
    path = "/api/sessions/{session_id}/panes/swap",
    tag = "Session panes",
    operation_id = "swap-panes",
    summary = "Swap panes",
    description = "Swaps the active pane with the one `delta` places on in layout order.",
    params(
        ("session_id", description = "Session ID"),
    ),
    responses(
        (status = 204, description = "Done"),
        (status = 404, description = "No such session"),
    ),
))]
pub(crate) async fn api_swap_pane(
    State(state): State<AppState>,
    Path(session_id): Path<Uuid>,
    Json(body): Json<DeltaRequest>,
) -> Response {
    let mut mgr = state.write().await;
    if !super::session_exists(&mgr, session_id) {
        return StatusCode::NOT_FOUND.into_response();
    }
    mgr.swap_pane(session_id, body.delta);

    ws::control::broadcast_state(&mgr);

    StatusCode::NO_CONTENT.into_response()
}

#[derive(Deserialize)]
#[cfg_attr(test, derive(utoipa::ToSchema))]
pub(crate) struct SelectLayoutRequest {
    #[cfg_attr(test, schema(value_type = LayoutPreset, inline))]
    preset: String,
}

#[cfg_attr(test, utoipa::path(
    post,
    path = "/api/sessions/{session_id}/layout",
    tag = "Session panes",
    operation_id = "select-a-layout",
    summary = "Select a layout",
    description = "Rearranges the active window's panes in a named preset.",
    params(
        ("session_id", description = "Session ID"),
    ),
    responses(
        (status = 204, description = "Done"),
        (status = 400, description = "Unknown preset"),
        (status = 404, description = "No such session"),
    ),
))]
pub(crate) async fn api_select_layout(
    State(state): State<AppState>,
    Path(session_id): Path<Uuid>,
    Json(body): Json<SelectLayoutRequest>,
) -> Response {
    let mut mgr = state.write().await;
    if !super::session_exists(&mgr, session_id) {
        return StatusCode::NOT_FOUND.into_response();
    }
    let Some(preset) = LayoutPreset::from_name(&body.preset) else {
        return StatusCode::BAD_REQUEST.into_response();
    };
    mgr.select_layout(session_id, preset);

    ws::control::broadcast_state(&mgr);

    StatusCode::NO_CONTENT.into_response()
}

#[cfg_attr(test, utoipa::path(
    post,
    path = "/api/sessions/{session_id}/layout/next",
    tag = "Session panes",
    operation_id = "next-layout",
    summary = "Next layout",
    params(
        ("session_id", description = "Session ID"),
    ),
    responses(
        (status = 204, description = "Done"),
        (status = 404, description = "No such session"),
    ),
))]
pub(crate) async fn api_next_layout(
    State(state): State<AppState>,
    Path(session_id): Path<Uuid>,
) -> Response {
    let mut mgr = state.write().await;
    if !super::session_exists(&mgr, session_id) {
        return StatusCode::NOT_FOUND.into_response();
    }
    mgr.next_layout(session_id);

    ws::control::broadcast_state(&mgr);

    StatusCode::NO_CONTENT.into_response()
}
