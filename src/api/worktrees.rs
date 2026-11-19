use axum::{
    extract::{Path, State},
    response::{IntoResponse, Response},
    Json, Router,
};
use serde::Deserialize;
use uuid::Uuid;

use super::pane_io::error_response;
use crate::worktree;
use crate::AppState;

pub fn router() -> Router<AppState> {
    Router::new().route(
        "/api/panes/{pane_id}/worktree",
        axum::routing::post(api_create_worktree).delete(api_remove_worktree),
    )
}

#[derive(Deserialize)]
#[cfg_attr(test, derive(utoipa::ToSchema))]
pub(crate) struct CreateWorktreeRequest {
    branch: String,
    /// Make its window the active one.
    #[cfg_attr(test, schema(default = true))]
    select: Option<bool>,
}

#[cfg_attr(test, utoipa::path(
    post,
    path = "/api/panes/{pane_id}/worktree",
    tag = "Worktrees",
    operation_id = "create-a-worktree",
    summary = "Create a worktree",
    description = "Creates a Git worktree for `branch` beside the pane's repository (the `worktree-path` setting) and opens a window in it.",
    params(
        ("pane_id", description = "Pane ID"),
    ),
    responses(
        (status = 200, description = "The new worktree and its window's pane", body = crate::worktree::Created),
        (status = 400, description = "An invalid branch name, or no such remote branch", body = String, content_type = "text/plain"),
        (status = 404, description = "No such pane", body = String, content_type = "text/plain"),
        (status = 409, description = "The pane isn't in a local Git repository, the worktree's path exists, or the branch tracks another remote branch", body = String, content_type = "text/plain"),
        (status = 500, description = "Git failed", body = String, content_type = "text/plain"),
    ),
))]
pub(crate) async fn api_create_worktree(
    State(state): State<AppState>,
    Path(pane_id): Path<Uuid>,
    Json(body): Json<CreateWorktreeRequest>,
) -> Response {
    match worktree::create(&state, pane_id, &body.branch, body.select.unwrap_or(true)).await {
        Ok(created) => Json(created).into_response(),
        Err(error) => error_response(error),
    }
}

#[cfg_attr(test, utoipa::path(
    delete,
    path = "/api/panes/{pane_id}/worktree",
    tag = "Worktrees",
    operation_id = "remove-a-worktree",
    summary = "Remove a worktree",
    description = "Removes the Git worktree the pane is in, closing every pane in it.",
    params(
        ("pane_id", description = "Pane ID"),
    ),
    responses(
        (status = 200, description = "The removed worktree and the panes it closed", body = crate::worktree::Removed),
        (status = 404, description = "No such pane", body = String, content_type = "text/plain"),
        (status = 409, description = "The pane isn't in a linked worktree, the worktree is locked, or a pane in it is running a program", body = String, content_type = "text/plain"),
        (status = 500, description = "Git failed", body = String, content_type = "text/plain"),
    ),
))]
pub(crate) async fn api_remove_worktree(
    State(state): State<AppState>,
    Path(pane_id): Path<Uuid>,
) -> Response {
    match worktree::remove(&state, pane_id).await {
        Ok(removed) => Json(removed).into_response(),
        Err(error) => error_response(error),
    }
}
