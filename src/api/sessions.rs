use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::ws;
use crate::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/api/sessions/{session_id}",
            get(api_get_session).delete(api_kill_session),
        )
        .route(
            "/api/sessions/{session_id}/rename",
            axum::routing::post(api_rename_session),
        )
}

#[cfg_attr(test, utoipa::path(
    get,
    path = "/api/sessions/{session_id}",
    tag = "Sessions",
    operation_id = "get-a-session",
    summary = "Get a session",
    params(
        ("session_id", description = "Session ID"),
    ),
    responses(
        (status = 200, description = "The session with its windows, panes, and layouts", body = crate::session::manager::SessionSnapshot),
        (status = 404, description = "No such session"),
    ),
))]
pub(crate) async fn api_get_session(
    State(state): State<AppState>,
    Path(session_id): Path<Uuid>,
) -> Response {
    let mgr = state.read().await;
    match mgr.snapshot_by_id(session_id) {
        Some(snapshot) => Json(snapshot).into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

#[derive(Deserialize)]
#[cfg_attr(test, derive(utoipa::ToSchema))]
pub(crate) struct RenameSessionRequest {
    name: String,
}

#[derive(Serialize)]
#[cfg_attr(test, derive(utoipa::ToSchema))]
pub(crate) struct RenameSessionResponse {
    id: Uuid,
    name: String,
}

#[cfg_attr(test, utoipa::path(
    post,
    path = "/api/sessions/{session_id}/rename",
    tag = "Sessions",
    operation_id = "rename-a-session",
    summary = "Rename a session",
    params(
        ("session_id", description = "Session ID"),
    ),
    responses(
        (status = 200, description = "The session's new name", body = RenameSessionResponse),
        (status = 404, description = "No such session"),
    ),
))]
pub(crate) async fn api_rename_session(
    State(state): State<AppState>,
    Path(session_id): Path<Uuid>,
    Json(body): Json<RenameSessionRequest>,
) -> Response {
    let mut mgr = state.write().await;
    if !super::session_exists(&mgr, session_id) {
        return StatusCode::NOT_FOUND.into_response();
    }
    mgr.rename_session(session_id, body.name);
    let name = mgr
        .sessions
        .iter()
        .find(|s| s.id == session_id)
        .map(|s| s.name.clone())
        .unwrap_or_default();

    ws::control::broadcast_state(&mgr);

    Json(RenameSessionResponse {
        id: session_id,
        name,
    })
    .into_response()
}

#[cfg_attr(test, utoipa::path(
    delete,
    path = "/api/sessions/{session_id}",
    tag = "Sessions",
    operation_id = "kill-a-session",
    summary = "Kill a session",
    params(
        ("session_id", description = "Session ID"),
    ),
    responses(
        (status = 204, description = "Done"),
        (status = 404, description = "No such session"),
        (status = 409, description = "It is the last session", body = String, content_type = "text/plain"),
    ),
))]
pub(crate) async fn api_kill_session(
    State(state): State<AppState>,
    Path(session_id): Path<Uuid>,
) -> Response {
    let mut mgr = state.write().await;
    if !super::session_exists(&mgr, session_id) {
        return StatusCode::NOT_FOUND.into_response();
    }
    if mgr.sessions.len() <= 1 {
        return (StatusCode::CONFLICT, "cannot kill the last session").into_response();
    }
    mgr.kill_session(session_id);

    ws::control::broadcast_state(&mgr);

    StatusCode::NO_CONTENT.into_response()
}
