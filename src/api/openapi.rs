//! The REST API's OpenAPI document, `docs/openapi.json`, which the docs
//! render as the API reference. Test-only, like the generated protocol.
//!
//! Each operation is a stub that casts its handler to the extractor types
//! it documents, so a handler taking other types doesn't compile, and
//! `routes_match` checks each path and method against the router.

use utoipa::openapi::security::{HttpAuthScheme, HttpBuilder, SecurityScheme};
use utoipa::{Modify, OpenApi};

use axum::extract::{Path, Query, State};
use axum::http::HeaderMap;
use axum::Json;
use uuid::Uuid;

use super::{pane_io, panes, sessions, windows, worktrees};
use crate::{agent_limits, health, popup, server, system_info, telemetry, AppState};

#[derive(OpenApi)]
#[openapi(
    info(
        title = "btmux REST API",
        version = "unstable",
        description = "Manage sessions, windows, and panes, drive pane input and output, and \
                       coordinate coding agents. Every request sends the access token as \
                       `Authorization: Bearer <token>`.",
        license(name = "MIT"),
    ),
    servers((url = "http://127.0.0.1:8004", description = "Default local server")),
    security(("token" = [])),
    modifiers(&Token),
    tags(
        (name = "Sessions"),
        (name = "Windows"),
        (name = "Panes", description = "Panes in any window, by ID."),
        (name = "Session panes", description = "Panes in a session's active window."),
        (name = "Pane I/O", description = "Type into panes and read their screens."),
        (name = "Agents", description = "Coding agents running in panes."),
        (name = "Worktrees", description = "Git worktrees, each with its own window."),
        (name = "Popups", description = "Programs in a popup over a pane."),
        (name = "Notifications"),
        (name = "Server"),
    ),
    paths(
        list_sessions,
        create_a_session,
        clear_sessions,
        get_a_session,
        rename_a_session,
        kill_a_session,
        create_a_window,
        switch_windows,
        rename_the_active_window,
        close_the_active_window,
        kill_a_window,
        select_a_window,
        rename_a_window,
        list_panes,
        split_a_pane,
        kill_a_pane,
        select_a_pane,
        focus_a_pane,
        open_the_file_browser,
        split_an_active_window_pane,
        kill_an_active_window_pane,
        zoom_a_pane,
        select_an_active_window_pane,
        move_to_a_neighboring_pane,
        cycle_panes,
        swap_panes,
        select_a_layout,
        next_layout,
        send_input,
        read_raw_output,
        capture_the_screen,
        list_agents,
        prompt_an_agent,
        wait_for_an_agent,
        explain_agent_status,
        set_agent_status,
        clear_agent_status,
        agent_usage,
        subscription_limits,
        create_a_worktree,
        remove_a_worktree,
        open_popup,
        wait_for_popup,
        notify,
        notify_pane,
        clear_pane_notification,
        system_info,
        health,
        shut_down,
    )
)]
struct ApiDoc;

/// Raw bytes, for clients to read as a buffer or stream.
struct Binary;

impl utoipa::PartialSchema for Binary {
    fn schema() -> utoipa::openapi::RefOr<utoipa::openapi::schema::Schema> {
        use utoipa::openapi::schema::{KnownFormat, ObjectBuilder, SchemaFormat, Type};
        ObjectBuilder::new()
            .schema_type(Type::String)
            .format(Some(SchemaFormat::KnownFormat(KnownFormat::Binary)))
            .into()
    }
}

impl utoipa::ToSchema for Binary {}

/// Bearer authentication, and its 401 on every operation.
struct Token;

impl Modify for Token {
    fn modify(&self, openapi: &mut utoipa::openapi::OpenApi) {
        for item in openapi.paths.paths.values_mut() {
            for operation in [&mut item.get, &mut item.post, &mut item.delete]
                .into_iter()
                .flatten()
            {
                operation.responses.responses.insert(
                    "401".into(),
                    utoipa::openapi::Response::new("Missing or wrong access token").into(),
                );
            }
        }
        let components = openapi.components.get_or_insert_with(Default::default);
        components.add_security_scheme(
            "token",
            SecurityScheme::Http(
                HttpBuilder::new()
                    .scheme(HttpAuthScheme::Bearer)
                    .description(Some("The profile's access token (`state.token`)."))
                    .build(),
            ),
        );
    }
}

#[utoipa::path(
    post,
    path = "/api/server/shutdown",
    tag = "Server",
    operation_id = "shut-down",
    summary = "Shut down the server",
    responses(
        (status = 202, description = "Stopping; every shell closes"),
    ),
)]
fn shut_down() {
    let _ = server::api_shutdown as fn() -> _;
}

#[utoipa::path(
    get,
    path = "/api/sessions",
    tag = "Sessions",
    operation_id = "list-sessions",
    summary = "List sessions",
    responses(
        (status = 200, description = "Every session", body = Vec<crate::session::SessionSummary>),
    ),
)]
fn list_sessions() {
    let _ = server::api_list_sessions as fn(State<AppState>) -> _;
}

#[utoipa::path(
    post,
    path = "/api/sessions",
    tag = "Sessions",
    operation_id = "create-a-session",
    summary = "Create a session",
    request_body = server::CreateSessionRequest,
    responses(
        (status = 200, description = "The new session and its first pane", body = server::CreateSessionResponse),
        (status = 400, description = "Invalid runtime", body = String, content_type = "text/plain"),
    ),
)]
fn create_a_session() {
    let _ =
        server::api_create_session as fn(State<AppState>, Json<server::CreateSessionRequest>) -> _;
}

#[utoipa::path(
    delete,
    path = "/api/sessions",
    tag = "Sessions",
    operation_id = "clear-sessions",
    summary = "Clear sessions",
    description = "Kills every session and starts one fresh default session, as `btmux clear-sessions` does.",
    responses(
        (status = 200, description = "The single fresh session left", body = Vec<crate::session::SessionSummary>),
    ),
)]
fn clear_sessions() {
    let _ = server::api_clear_sessions as fn(State<AppState>) -> _;
}

#[utoipa::path(
    post,
    path = "/api/sessions/{session_id}/windows",
    tag = "Windows",
    operation_id = "create-a-window",
    summary = "Create a window",
    params(
        ("session_id" = Uuid, Path, description = "Session ID"),
    ),
    request_body = server::CreateWindowRequest,
    responses(
        (status = 200, description = "The new window's first pane", body = server::CreateWindowResponse),
        (status = 404, description = "No such session"),
    ),
)]
fn create_a_window() {
    let _ = server::api_create_window
        as fn(State<AppState>, Path<Uuid>, Json<server::CreateWindowRequest>) -> _;
}

#[utoipa::path(
    post,
    path = "/api/panes/{pane_id}/notify",
    tag = "Notifications",
    operation_id = "notify-pane",
    summary = "Report an agent event",
    description = "Takes an agent hook's JSON payload as is (Claude Code, Codex, Gemini CLI), or `event`, `title`, and `body` from other callers. Updates the pane's agent status and, for events worth attention, marks the pane and notifies every client.",
    params(
        ("pane_id" = Uuid, Path, description = "Pane ID"),
    ),
    request_body = server::PaneNotifyRequest,
    responses(
        (status = 204, description = "Done"),
        (status = 404, description = "No such pane"),
    ),
)]
fn notify_pane() {
    let _ = server::api_pane_notify
        as fn(State<AppState>, Path<Uuid>, HeaderMap, Json<server::PaneNotifyRequest>) -> _;
}

#[utoipa::path(
    post,
    path = "/api/notify",
    tag = "Notifications",
    operation_id = "notify",
    summary = "Show a notification",
    description = "Shows a notification, not tied to a pane, in every client.",
    request_body = server::NotifyRequest,
    responses(
        (status = 204, description = "Done"),
        (status = 422, description = "Empty title"),
    ),
)]
fn notify() {
    let _ = server::api_notify as fn(State<AppState>, Json<server::NotifyRequest>) -> _;
}

#[utoipa::path(
    post,
    path = "/api/panes/{pane_id}/open-file-browser",
    tag = "Panes",
    operation_id = "open-the-file-browser",
    summary = "Open the file browser",
    description = "Switches every client to the pane, then opens the file browser, Git mode, or the process viewer there.",
    params(
        ("pane_id" = Uuid, Path, description = "Pane ID"),
    ),
    request_body = server::OpenFileBrowserRequest,
    responses(
        (status = 204, description = "Done"),
        (status = 404, description = "No such pane"),
    ),
)]
fn open_the_file_browser() {
    let _ = server::api_open_file_browser
        as fn(State<AppState>, Path<Uuid>, Json<server::OpenFileBrowserRequest>) -> _;
}

#[utoipa::path(
    post,
    path = "/api/panes/{pane_id}/focus",
    tag = "Panes",
    operation_id = "focus-a-pane",
    summary = "Focus a pane",
    params(
        ("pane_id" = Uuid, Path, description = "Pane ID"),
    ),
    responses(
        (status = 204, description = "Done"),
        (status = 404, description = "No such pane"),
    ),
)]
fn focus_a_pane() {
    let _ = server::api_focus_pane as fn(State<AppState>, Path<Uuid>) -> _;
}

#[utoipa::path(
    delete,
    path = "/api/panes/{pane_id}/notify",
    tag = "Notifications",
    operation_id = "clear-pane-notification",
    summary = "Clear a pane's notification",
    params(
        ("pane_id" = Uuid, Path, description = "Pane ID"),
    ),
    responses(
        (status = 204, description = "Done"),
        (status = 404, description = "No such pane"),
    ),
)]
fn clear_pane_notification() {
    let _ = server::api_pane_notify_clear as fn(State<AppState>, Path<Uuid>) -> _;
}

#[utoipa::path(
    get,
    path = "/api/panes/{pane_id}/agent-status",
    tag = "Agents",
    operation_id = "explain-agent-status",
    summary = "Explain an agent's status",
    params(
        ("pane_id" = Uuid, Path, description = "Pane ID"),
    ),
    responses(
        (status = 200, description = "The pane's agent status and what it was derived from", body = Object),
        (status = 404, description = "No such pane"),
    ),
)]
fn explain_agent_status() {
    let _ = server::api_pane_agent_diagnostics as fn(State<AppState>, Path<Uuid>) -> _;
}

#[utoipa::path(
    post,
    path = "/api/panes/{pane_id}/agent-status",
    tag = "Agents",
    operation_id = "set-agent-status",
    summary = "Set an agent's status",
    description = "Sets the pane's agent status explicitly, above what hooks and the screen report.",
    params(
        ("pane_id" = Uuid, Path, description = "Pane ID"),
    ),
    request_body = server::PaneAgentStatusRequest,
    responses(
        (status = 204, description = "Done"),
        (status = 404, description = "No such pane"),
    ),
)]
fn set_agent_status() {
    let _ = server::api_pane_agent_status
        as fn(State<AppState>, Path<Uuid>, Json<server::PaneAgentStatusRequest>) -> _;
}

#[utoipa::path(
    delete,
    path = "/api/panes/{pane_id}/agent-status",
    tag = "Agents",
    operation_id = "clear-agent-status",
    summary = "Clear an agent's status",
    params(
        ("pane_id" = Uuid, Path, description = "Pane ID"),
    ),
    responses(
        (status = 204, description = "Done"),
        (status = 404, description = "No such pane"),
    ),
)]
fn clear_agent_status() {
    let _ = server::api_pane_agent_status_clear as fn(State<AppState>, Path<Uuid>) -> _;
}

#[utoipa::path(
    post,
    path = "/api/panes/{pane_id}/popup",
    tag = "Popups",
    operation_id = "open-popup",
    summary = "Open a popup",
    description = "Runs a program in a popup over the pane. Replies once a client started it; then wait for its exit status.",
    params(
        ("pane_id" = Uuid, Path, description = "Pane ID"),
    ),
    request_body = popup::PopupRequest,
    responses(
        (status = 200, description = "Started in a client", body = popup::PopupOpened),
        (status = 400, description = "No command", body = String, content_type = "text/plain"),
        (status = 404, description = "No such pane"),
        (status = 409, description = "No client is showing the pane"),
        (status = 410, description = "The popup closed before it started"),
        (status = 422, description = "The program can't start", body = String, content_type = "text/plain"),
        (status = 423, description = "The pane already has a popup open"),
    ),
)]
fn open_popup() {
    let _ = popup::api_popup as fn(State<AppState>, Path<Uuid>, Json<popup::PopupRequest>) -> _;
}

#[utoipa::path(
    get,
    path = "/api/panes/{pane_id}/popup/{id}",
    tag = "Popups",
    operation_id = "wait-for-popup",
    summary = "Wait for a popup",
    params(
        ("pane_id" = Uuid, Path, description = "Pane ID"),
        ("id" = Uuid, Path, description = "Popup ID"),
    ),
    responses(
        (status = 200, description = "The program exited", body = popup::PopupExited),
        (status = 410, description = "The server stopped waiting"),
        (status = 404, description = "No such popup"),
        (status = 409, description = "No client showed the popup"),
        (status = 422, description = "The program failed to start"),
    ),
)]
fn wait_for_popup() {
    let _ = popup::api_wait as fn(State<AppState>, Path<(Uuid, Uuid)>) -> _;
}

#[utoipa::path(
    get,
    path = "/api/system",
    tag = "Server",
    operation_id = "system-info",
    summary = "Describe the server host",
    responses(
        (status = 200, description = "OS, CPU, memory, disks, and shell", body = system_info::SystemInfo),
    ),
)]
fn system_info() {
    let _ = system_info::handle as fn(State<AppState>) -> _;
}

#[utoipa::path(
    get,
    path = "/api/health",
    tag = "Server",
    operation_id = "health",
    summary = "Check optional programs",
    responses(
        (status = 200, description = "Which optional programs the server found and whether they run", body = health::HealthReport),
    ),
)]
fn health() {
    let _ = health::handle as fn(State<AppState>) -> _;
}

#[utoipa::path(
    post,
    path = "/api/panes/{pane_id}/input",
    tag = "Pane I/O",
    operation_id = "send-input",
    summary = "Send input",
    params(
        ("pane_id" = Uuid, Path, description = "Pane ID"),
    ),
    request_body = pane_io::PaneInputRequest,
    responses(
        (status = 200, description = "Sent", body = pane_io::PaneInputResponse),
        (status = 404, description = "No such pane"),
        (status = 400, description = "Invalid request", body = String, content_type = "text/plain"),
        (status = 409, description = "Conflict", body = String, content_type = "text/plain"),
        (status = 500, description = "The operation failed", body = String, content_type = "text/plain"),
    ),
)]
fn send_input() {
    let _ = pane_io::api_pane_input
        as fn(State<AppState>, Path<Uuid>, Json<pane_io::PaneInputRequest>) -> _;
}

#[utoipa::path(
    get,
    path = "/api/panes/{pane_id}/output",
    tag = "Pane I/O",
    operation_id = "read-raw-output",
    summary = "Read raw output",
    params(
        ("pane_id" = Uuid, Path, description = "Pane ID"),
        pane_io::PaneOutputQuery,
    ),
    responses(
        (status = 200, description = "Raw PTY bytes, escapes included", body = inline(Binary), content_type = "application/octet-stream"),
        (status = 400, description = "The shell failed to start"),
        (status = 404, description = "No such pane"),
    ),
)]
fn read_raw_output() {
    let _ = pane_io::api_pane_output
        as fn(State<AppState>, Path<Uuid>, Query<pane_io::PaneOutputQuery>) -> _;
}

#[utoipa::path(
    get,
    path = "/api/panes/{pane_id}/capture",
    tag = "Pane I/O",
    operation_id = "capture-the-screen",
    summary = "Capture the screen",
    params(
        ("pane_id" = Uuid, Path, description = "Pane ID"),
        pane_io::PaneCaptureQuery,
    ),
    responses(
        (status = 200, description = "The rendered text", body = String, content_type = "text/plain"),
        (status = 404, description = "No such pane"),
        (status = 400, description = "Invalid request", body = String, content_type = "text/plain"),
        (status = 409, description = "Conflict", body = String, content_type = "text/plain"),
        (status = 500, description = "The operation failed", body = String, content_type = "text/plain"),
    ),
)]
fn capture_the_screen() {
    let _ = pane_io::api_pane_capture
        as fn(State<AppState>, Path<Uuid>, Query<pane_io::PaneCaptureQuery>) -> _;
}

#[utoipa::path(
    post,
    path = "/api/panes/{pane_id}/prompt",
    tag = "Agents",
    operation_id = "prompt-an-agent",
    summary = "Prompt an agent",
    description = "Pastes `prompt` into the pane's agent and submits it. With `wait` (the default), replies once the agent has started and settled (idle, done, or blocked).",
    params(
        ("pane_id" = Uuid, Path, description = "Pane ID"),
    ),
    request_body = pane_io::PanePromptRequest,
    responses(
        (status = 200, description = "With `wait`, the turn's outcome; without, `{\"submitted\": true}`", body = pane_io::PromptReply),
        (status = 404, description = "No such pane"),
        (status = 400, description = "Invalid request", body = String, content_type = "text/plain"),
        (status = 409, description = "Conflict", body = String, content_type = "text/plain"),
        (status = 500, description = "The operation failed", body = String, content_type = "text/plain"),
    ),
)]
fn prompt_an_agent() {
    let _ = pane_io::api_pane_prompt
        as fn(State<AppState>, Path<Uuid>, Json<pane_io::PanePromptRequest>) -> _;
}

#[utoipa::path(
    get,
    path = "/api/panes/{pane_id}/wait",
    tag = "Agents",
    operation_id = "wait-for-an-agent",
    summary = "Wait for an agent",
    params(
        ("pane_id" = Uuid, Path, description = "Pane ID"),
        pane_io::PaneWaitQuery,
    ),
    responses(
        (status = 200, description = "The agent's status once it settles, exits, or the wait times out", body = crate::automation::AgentWait),
        (status = 404, description = "No such pane"),
        (status = 400, description = "Invalid request", body = String, content_type = "text/plain"),
        (status = 409, description = "Conflict", body = String, content_type = "text/plain"),
        (status = 500, description = "The operation failed", body = String, content_type = "text/plain"),
    ),
)]
fn wait_for_an_agent() {
    let _ = pane_io::api_pane_wait
        as fn(State<AppState>, Path<Uuid>, Query<pane_io::PaneWaitQuery>) -> _;
}

#[utoipa::path(
    get,
    path = "/api/agents",
    tag = "Agents",
    operation_id = "list-agents",
    summary = "List agents",
    responses(
        (status = 200, description = "Every pane running an agent", body = Vec<crate::session::AgentPane>),
    ),
)]
fn list_agents() {
    let _ = pane_io::api_agents as fn(State<AppState>) -> _;
}

#[utoipa::path(
    get,
    path = "/api/panes",
    tag = "Panes",
    operation_id = "list-panes",
    summary = "List panes",
    responses(
        (status = 200, description = "Every pane, with its window and session", body = Vec<crate::session::PaneInfo>),
    ),
)]
fn list_panes() {
    let _ = panes::api_list_panes as fn(State<AppState>) -> _;
}

#[utoipa::path(
    post,
    path = "/api/panes/{pane_id}/split",
    tag = "Panes",
    operation_id = "split-a-pane",
    summary = "Split a pane",
    params(
        ("pane_id" = Uuid, Path, description = "Pane ID"),
    ),
    request_body = panes::SplitPaneByIdRequest,
    responses(
        (status = 200, description = "The new pane", body = panes::SplitPaneResponse),
        (status = 400, description = "Invalid direction"),
        (status = 404, description = "No such pane"),
    ),
)]
fn split_a_pane() {
    let _ = panes::api_split_pane_by_id
        as fn(State<AppState>, Path<Uuid>, Json<panes::SplitPaneByIdRequest>) -> _;
}

#[utoipa::path(
    delete,
    path = "/api/panes/{pane_id}",
    tag = "Panes",
    operation_id = "kill-a-pane",
    summary = "Kill a pane",
    params(
        ("pane_id" = Uuid, Path, description = "Pane ID"),
    ),
    responses(
        (status = 204, description = "Done"),
        (status = 404, description = "No such pane"),
        (status = 409, description = "The pane can't be closed"),
    ),
)]
fn kill_a_pane() {
    let _ = panes::api_kill_pane_by_id as fn(State<AppState>, Path<Uuid>) -> _;
}

#[utoipa::path(
    post,
    path = "/api/panes/{pane_id}/select",
    tag = "Panes",
    operation_id = "select-a-pane",
    summary = "Select a pane",
    params(
        ("pane_id" = Uuid, Path, description = "Pane ID"),
    ),
    responses(
        (status = 204, description = "Done"),
        (status = 404, description = "No such pane"),
    ),
)]
fn select_a_pane() {
    let _ = panes::api_select_pane_by_id as fn(State<AppState>, Path<Uuid>) -> _;
}

#[utoipa::path(
    post,
    path = "/api/sessions/{session_id}/panes/{pane_id}/split",
    tag = "Session panes",
    operation_id = "split-an-active-window-pane",
    summary = "Split an active-window pane",
    params(
        ("session_id" = Uuid, Path, description = "Session ID"),
        ("pane_id" = Uuid, Path, description = "Pane ID"),
    ),
    request_body = panes::SplitPaneRequest,
    responses(
        (status = 204, description = "Done"),
        (status = 400, description = "Invalid direction"),
        (status = 404, description = "No such session"),
        (status = 409, description = "The pane isn't in the session's active window"),
    ),
)]
fn split_an_active_window_pane() {
    let _ = panes::api_split_pane
        as fn(State<AppState>, Path<(Uuid, Uuid)>, Json<panes::SplitPaneRequest>) -> _;
}

#[utoipa::path(
    delete,
    path = "/api/sessions/{session_id}/panes/{pane_id}",
    tag = "Session panes",
    operation_id = "kill-an-active-window-pane",
    summary = "Kill an active-window pane",
    params(
        ("session_id" = Uuid, Path, description = "Session ID"),
        ("pane_id" = Uuid, Path, description = "Pane ID"),
    ),
    responses(
        (status = 204, description = "Done"),
        (status = 404, description = "No such session"),
        (status = 409, description = "The pane isn't in the session's active window"),
    ),
)]
fn kill_an_active_window_pane() {
    let _ = panes::api_kill_pane as fn(State<AppState>, Path<(Uuid, Uuid)>) -> _;
}

#[utoipa::path(
    post,
    path = "/api/sessions/{session_id}/panes/{pane_id}/zoom",
    tag = "Session panes",
    operation_id = "zoom-a-pane",
    summary = "Zoom a pane",
    params(
        ("session_id" = Uuid, Path, description = "Session ID"),
        ("pane_id" = Uuid, Path, description = "Pane ID"),
    ),
    responses(
        (status = 204, description = "Done"),
        (status = 404, description = "No such session"),
        (status = 409, description = "The pane isn't in the session's active window"),
    ),
)]
fn zoom_a_pane() {
    let _ = panes::api_zoom_pane as fn(State<AppState>, Path<(Uuid, Uuid)>) -> _;
}

#[utoipa::path(
    post,
    path = "/api/sessions/{session_id}/panes/{pane_id}/select",
    tag = "Session panes",
    operation_id = "select-an-active-window-pane",
    summary = "Select an active-window pane",
    params(
        ("session_id" = Uuid, Path, description = "Session ID"),
        ("pane_id" = Uuid, Path, description = "Pane ID"),
    ),
    responses(
        (status = 204, description = "Done"),
        (status = 404, description = "No such session"),
        (status = 409, description = "The pane isn't in the session's active window"),
    ),
)]
fn select_an_active_window_pane() {
    let _ = panes::api_select_pane as fn(State<AppState>, Path<(Uuid, Uuid)>) -> _;
}

#[utoipa::path(
    post,
    path = "/api/sessions/{session_id}/panes/navigate",
    tag = "Session panes",
    operation_id = "move-to-a-neighboring-pane",
    summary = "Move to a neighboring pane",
    description = "Selects the pane in `direction` (`left`, `right`, `up`, or `down`) from the active one.",
    params(
        ("session_id" = Uuid, Path, description = "Session ID"),
    ),
    request_body = panes::NavigateRequest,
    responses(
        (status = 204, description = "Done"),
        (status = 404, description = "No such session"),
    ),
)]
fn move_to_a_neighboring_pane() {
    let _ =
        panes::api_navigate as fn(State<AppState>, Path<Uuid>, Json<panes::NavigateRequest>) -> _;
}

#[utoipa::path(
    post,
    path = "/api/sessions/{session_id}/panes/cycle",
    tag = "Session panes",
    operation_id = "cycle-panes",
    summary = "Cycle panes",
    description = "Selects the pane `delta` places on in layout order.",
    params(
        ("session_id" = Uuid, Path, description = "Session ID"),
    ),
    request_body = panes::DeltaRequest,
    responses(
        (status = 204, description = "Done"),
        (status = 404, description = "No such session"),
    ),
)]
fn cycle_panes() {
    let _ =
        panes::api_cycle_pane as fn(State<AppState>, Path<Uuid>, Json<panes::DeltaRequest>) -> _;
}

#[utoipa::path(
    post,
    path = "/api/sessions/{session_id}/panes/swap",
    tag = "Session panes",
    operation_id = "swap-panes",
    summary = "Swap panes",
    description = "Swaps the active pane with the one `delta` places on in layout order.",
    params(
        ("session_id" = Uuid, Path, description = "Session ID"),
    ),
    request_body = panes::DeltaRequest,
    responses(
        (status = 204, description = "Done"),
        (status = 404, description = "No such session"),
    ),
)]
fn swap_panes() {
    let _ = panes::api_swap_pane as fn(State<AppState>, Path<Uuid>, Json<panes::DeltaRequest>) -> _;
}

#[utoipa::path(
    post,
    path = "/api/sessions/{session_id}/layout",
    tag = "Session panes",
    operation_id = "select-a-layout",
    summary = "Select a layout",
    description = "Rearranges the active window's panes in a named preset.",
    params(
        ("session_id" = Uuid, Path, description = "Session ID"),
    ),
    request_body = panes::SelectLayoutRequest,
    responses(
        (status = 204, description = "Done"),
        (status = 400, description = "Unknown preset"),
        (status = 404, description = "No such session"),
    ),
)]
fn select_a_layout() {
    let _ = panes::api_select_layout
        as fn(State<AppState>, Path<Uuid>, Json<panes::SelectLayoutRequest>) -> _;
}

#[utoipa::path(
    post,
    path = "/api/sessions/{session_id}/layout/next",
    tag = "Session panes",
    operation_id = "next-layout",
    summary = "Next layout",
    params(
        ("session_id" = Uuid, Path, description = "Session ID"),
    ),
    responses(
        (status = 204, description = "Done"),
        (status = 404, description = "No such session"),
    ),
)]
fn next_layout() {
    let _ = panes::api_next_layout as fn(State<AppState>, Path<Uuid>) -> _;
}

#[utoipa::path(
    get,
    path = "/api/sessions/{session_id}",
    tag = "Sessions",
    operation_id = "get-a-session",
    summary = "Get a session",
    params(
        ("session_id" = Uuid, Path, description = "Session ID"),
    ),
    responses(
        (status = 200, description = "The session with its windows, panes, and layouts", body = crate::session::manager::SessionSnapshot),
        (status = 404, description = "No such session"),
    ),
)]
fn get_a_session() {
    let _ = sessions::api_get_session as fn(State<AppState>, Path<Uuid>) -> _;
}

#[utoipa::path(
    post,
    path = "/api/sessions/{session_id}/rename",
    tag = "Sessions",
    operation_id = "rename-a-session",
    summary = "Rename a session",
    params(
        ("session_id" = Uuid, Path, description = "Session ID"),
    ),
    request_body = sessions::RenameSessionRequest,
    responses(
        (status = 200, description = "The session's new name", body = sessions::RenameSessionResponse),
        (status = 404, description = "No such session"),
    ),
)]
fn rename_a_session() {
    let _ = sessions::api_rename_session
        as fn(State<AppState>, Path<Uuid>, Json<sessions::RenameSessionRequest>) -> _;
}

#[utoipa::path(
    delete,
    path = "/api/sessions/{session_id}",
    tag = "Sessions",
    operation_id = "kill-a-session",
    summary = "Kill a session",
    params(
        ("session_id" = Uuid, Path, description = "Session ID"),
    ),
    responses(
        (status = 204, description = "Done"),
        (status = 404, description = "No such session"),
        (status = 409, description = "It is the last session"),
    ),
)]
fn kill_a_session() {
    let _ = sessions::api_kill_session as fn(State<AppState>, Path<Uuid>) -> _;
}

#[utoipa::path(
    post,
    path = "/api/panes/{pane_id}/worktree",
    tag = "Worktrees",
    operation_id = "create-a-worktree",
    summary = "Create a worktree",
    params(
        ("pane_id" = Uuid, Path, description = "Pane ID"),
    ),
    request_body = worktrees::CreateWorktreeRequest,
    responses(
        (status = 200, description = "The new worktree and its window's pane", body = crate::worktree::Created),
        (status = 404, description = "No such pane"),
        (status = 400, description = "Invalid request", body = String, content_type = "text/plain"),
        (status = 409, description = "Conflict", body = String, content_type = "text/plain"),
        (status = 500, description = "The operation failed", body = String, content_type = "text/plain"),
    ),
)]
fn create_a_worktree() {
    let _ = worktrees::api_create_worktree
        as fn(State<AppState>, Path<Uuid>, Json<worktrees::CreateWorktreeRequest>) -> _;
}

#[utoipa::path(
    delete,
    path = "/api/panes/{pane_id}/worktree",
    tag = "Worktrees",
    operation_id = "remove-a-worktree",
    summary = "Remove a worktree",
    params(
        ("pane_id" = Uuid, Path, description = "Pane ID"),
    ),
    responses(
        (status = 200, description = "The removed worktree and the panes it closed", body = crate::worktree::Removed),
        (status = 404, description = "No such pane"),
        (status = 400, description = "Invalid request", body = String, content_type = "text/plain"),
        (status = 409, description = "Conflict", body = String, content_type = "text/plain"),
        (status = 500, description = "The operation failed", body = String, content_type = "text/plain"),
    ),
)]
fn remove_a_worktree() {
    let _ = worktrees::api_remove_worktree as fn(State<AppState>, Path<Uuid>) -> _;
}

#[utoipa::path(
    post,
    path = "/api/windows/{window_id}/select",
    tag = "Windows",
    operation_id = "select-a-window",
    summary = "Select a window",
    params(
        ("window_id" = Uuid, Path, description = "Window ID"),
    ),
    responses(
        (status = 204, description = "Done"),
        (status = 404, description = "No such window"),
    ),
)]
fn select_a_window() {
    let _ = windows::api_select_window_by_id as fn(State<AppState>, Path<Uuid>) -> _;
}

#[utoipa::path(
    post,
    path = "/api/windows/{window_id}/rename",
    tag = "Windows",
    operation_id = "rename-a-window",
    summary = "Rename a window",
    params(
        ("window_id" = Uuid, Path, description = "Window ID"),
    ),
    request_body = windows::RenameWindowRequest,
    responses(
        (status = 200, description = "The window's new name", body = windows::RenameWindowResponse),
        (status = 400, description = "Empty name"),
        (status = 404, description = "No such window"),
    ),
)]
fn rename_a_window() {
    let _ = windows::api_rename_window_by_id
        as fn(State<AppState>, Path<Uuid>, Json<windows::RenameWindowRequest>) -> _;
}

#[utoipa::path(
    post,
    path = "/api/sessions/{session_id}/windows/switch",
    tag = "Windows",
    operation_id = "switch-windows",
    summary = "Switch windows",
    description = "Makes another window active: `index` is absolute, or `-1` for the next window and `-2` for the previous.",
    params(
        ("session_id" = Uuid, Path, description = "Session ID"),
    ),
    request_body = windows::SwitchWindowRequest,
    responses(
        (status = 204, description = "Done"),
        (status = 404, description = "No such session"),
    ),
)]
fn switch_windows() {
    let _ = windows::api_switch_window
        as fn(State<AppState>, Path<Uuid>, Json<windows::SwitchWindowRequest>) -> _;
}

#[utoipa::path(
    post,
    path = "/api/sessions/{session_id}/windows/rename",
    tag = "Windows",
    operation_id = "rename-the-active-window",
    summary = "Rename the active window",
    description = "Renames the session's active window. To rename another, use `POST /api/windows/{window_id}/rename`.",
    params(
        ("session_id" = Uuid, Path, description = "Session ID"),
    ),
    request_body = windows::RenameWindowRequest,
    responses(
        (status = 200, description = "The window's new name", body = windows::RenameWindowResponse),
        (status = 404, description = "No such session"),
    ),
)]
fn rename_the_active_window() {
    let _ = windows::api_rename_window
        as fn(State<AppState>, Path<Uuid>, Json<windows::RenameWindowRequest>) -> _;
}

#[utoipa::path(
    post,
    path = "/api/sessions/{session_id}/windows/close",
    tag = "Windows",
    operation_id = "close-the-active-window",
    summary = "Close the active window",
    description = "Closes the session's active window. To close another, use `DELETE /api/windows/{window_id}`.",
    params(
        ("session_id" = Uuid, Path, description = "Session ID"),
    ),
    responses(
        (status = 204, description = "Done"),
        (status = 404, description = "No such session"),
        (status = 409, description = "It is the session's last window"),
    ),
)]
fn close_the_active_window() {
    let _ = windows::api_close_window as fn(State<AppState>, Path<Uuid>) -> _;
}

#[utoipa::path(
    delete,
    path = "/api/windows/{window_id}",
    tag = "Windows",
    operation_id = "kill-a-window",
    summary = "Kill a window",
    params(
        ("window_id" = Uuid, Path, description = "Window ID"),
    ),
    responses(
        (status = 204, description = "Done"),
        (status = 404, description = "No such window"),
        (status = 409, description = "It is the session's last window"),
    ),
)]
fn kill_a_window() {
    let _ = windows::api_kill_window as fn(State<AppState>, Path<Uuid>) -> _;
}

#[utoipa::path(
    get,
    path = "/api/telemetry",
    tag = "Agents",
    operation_id = "agent-usage",
    summary = "Agent usage",
    description = "Tokens, cost, requests, and tool calls per agent and model, per minute, for the last 24 hours, as agent view's Usage tab shows them.",
    responses(
        (status = 200, description = "Each agent's usage", body = Vec<crate::telemetry::AgentTelemetry>),
    ),
)]
fn agent_usage() {
    let _ = telemetry::snapshot as fn(State<AppState>) -> _;
}

#[utoipa::path(
    get,
    path = "/api/agent-limits",
    tag = "Agents",
    operation_id = "agent-limits",
    summary = "Subscription limits",
    description = "How much of each subscription limit Claude Code and Codex accounts on the server have used, and when each resets. Agents using an API key, or not logged in, aren't listed.",
    responses(
        (status = 200, description = "Each logged-in agent's limits", body = Vec<crate::agent_limits::AgentLimits>),
    ),
)]
fn subscription_limits() {
    let _ = agent_limits::limits as fn(State<AppState>) -> _;
}

fn document() -> String {
    ApiDoc::openapi().to_pretty_json().unwrap() + "\n"
}

#[test]
fn openapi_is_current() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("docs/openapi.json");
    let generated = document();
    if std::env::var_os("BTMUX_UPDATE_PROTOCOL").is_some() {
        std::fs::write(&path, generated).unwrap();
        return;
    }
    let current = std::fs::read_to_string(&path).unwrap_or_default();
    assert!(
        current == generated,
        "docs/openapi.json is stale; run `just protocol`"
    );
}

/// Every documented path is routed with that template, and the route takes
/// exactly the documented methods. Handlers don't run.
#[tokio::test]
async fn routes_match() {
    use axum::body::{to_bytes, Body};
    use axum::extract::{MatchedPath, Request};
    use axum::http::{header, Method, StatusCode};
    use axum::middleware::{self, Next};
    use tower::ServiceExt;

    let (exit_tx, _) = tokio::sync::mpsc::unbounded_channel();
    let (meta_tx, _) = tokio::sync::mpsc::unbounded_channel();
    let manager = crate::session::manager::SessionManager::new(
        "/bin/sh".into(),
        crate::config::FileConfig::default(),
        exit_tx,
        meta_tx,
        8004,
        None,
    );
    let app = server::create_app(std::sync::Arc::new(tokio::sync::RwLock::new(manager)));
    // Replies with the route's template, for any method.
    let matched = app.clone().route_layer(middleware::from_fn(
        |matched: Option<MatchedPath>, _: Request, _: Next| async move {
            matched
                .map(|path| path.as_str().to_owned())
                .unwrap_or_default()
        },
    ));
    let request = |method: Method, uri: &str| {
        axum::http::Request::builder()
            .method(method)
            .uri(uri)
            .body(Body::empty())
            .unwrap()
    };

    let doc = serde_json::to_value(ApiDoc::openapi()).unwrap();
    for (path, item) in doc["paths"].as_object().unwrap() {
        let uri = path
            .split('/')
            .map(|part| match part.starts_with('{') {
                true => "00000000-0000-0000-0000-000000000000",
                false => part,
            })
            .collect::<Vec<_>>()
            .join("/");

        let response = matched
            .clone()
            .oneshot(request(Method::GET, &uri))
            .await
            .unwrap();
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        assert_eq!(
            std::str::from_utf8(&body).unwrap(),
            path,
            "{path} isn't routed"
        );

        // No route takes TRACE, so axum lists the ones it does take.
        let response = app
            .clone()
            .oneshot(request(Method::TRACE, &uri))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::METHOD_NOT_ALLOWED, "{path}");
        let allowed = response.headers()[header::ALLOW].to_str().unwrap();
        let mut allowed: Vec<String> = allowed
            .split(',')
            .map(|method| method.trim().to_lowercase())
            .filter(|method| method != "head")
            .collect();
        let mut documented: Vec<String> = item.as_object().unwrap().keys().cloned().collect();
        allowed.sort();
        documented.sort();
        assert_eq!(allowed, documented, "{path} takes other methods");
    }
}
