//! The REST API's OpenAPI document, `docs/openapi.json`, which the docs
//! render as the API reference. Test-only, like the generated protocol.

use std::path::Path;

use utoipa::openapi::security::{HttpAuthScheme, HttpBuilder, SecurityScheme};
use utoipa::{Modify, OpenApi};

use super::{pane_io, panes, sessions, windows, worktrees};
use crate::{health, popup, server, system_info};

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
        server::api_list_sessions,
        server::api_create_session,
        server::api_clear_sessions,
        sessions::api_get_session,
        sessions::api_rename_session,
        sessions::api_kill_session,
        server::api_create_window,
        windows::api_switch_window,
        windows::api_rename_window,
        windows::api_close_window,
        windows::api_kill_window,
        windows::api_select_window_by_id,
        windows::api_rename_window_by_id,
        panes::api_list_panes,
        panes::api_split_pane_by_id,
        panes::api_kill_pane_by_id,
        panes::api_select_pane_by_id,
        server::api_focus_pane,
        server::api_open_file_browser,
        panes::api_split_pane,
        panes::api_kill_pane,
        panes::api_zoom_pane,
        panes::api_select_pane,
        panes::api_navigate,
        panes::api_cycle_pane,
        panes::api_swap_pane,
        panes::api_select_layout,
        panes::api_next_layout,
        pane_io::api_pane_input,
        pane_io::api_pane_output,
        pane_io::api_pane_capture,
        pane_io::api_agents,
        pane_io::api_pane_prompt,
        pane_io::api_pane_wait,
        server::api_pane_agent_diagnostics,
        server::api_pane_agent_status,
        server::api_pane_agent_status_clear,
        worktrees::api_create_worktree,
        worktrees::api_remove_worktree,
        popup::api_popup,
        popup::api_wait,
        server::api_notify,
        server::api_pane_notify,
        server::api_pane_notify_clear,
        system_info::handle,
        health::handle,
        server::api_shutdown,
    )
)]
struct ApiDoc;

struct Token;

impl Modify for Token {
    fn modify(&self, openapi: &mut utoipa::openapi::OpenApi) {
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

fn document() -> String {
    ApiDoc::openapi().to_pretty_json().unwrap() + "\n"
}

#[test]
fn openapi_is_current() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("docs/openapi.json");
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
