//! The REST API's OpenAPI document, `docs/public/openapi.json`, which the
//! docs render as the API reference and `GET /api/openapi.json` serves.
//! Test-only, like the generated protocol: operations are `utoipa::path`
//! attributes on their handlers, which read parameters and request bodies
//! from the handlers' extractors.
//!
//! `routes_match` checks each path and method against the router,
//! `api_routes_are_documented` that each `/api` route is documented or
//! listed in `UNDOCUMENTED`, and `responses_match` real replies against the
//! documented statuses and schemas.

use utoipa::openapi::content::ContentBuilder;
use utoipa::openapi::response::ResponseBuilder;
use utoipa::openapi::schema::{ObjectBuilder, Type};
use utoipa::openapi::security::{HttpAuthScheme, HttpBuilder, SecurityScheme};
use utoipa::openapi::{RefOr, Response};
use utoipa::{Modify, OpenApi};

use super::{pane_io, panes, sessions, windows, worktrees};
use crate::{agent_limits, health, popup, server, system_info, telemetry};

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
    modifiers(&Errors),
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
        server::api_open_editor,
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
        telemetry::snapshot,
        agent_limits::limits,
        worktrees::api_create_worktree,
        worktrees::api_remove_worktree,
        popup::api_popup,
        popup::api_wait,
        server::api_notify,
        server::api_pane_notify,
        server::api_pane_notify_clear,
        server::api_info,
        system_info::handle,
        health::handle,
        server::api_upgrade,
        server::api_shutdown,
        server::api_openapi,
    )
)]
struct ApiDoc;

/// Raw bytes, for clients to read as a buffer or stream.
pub(super) struct Binary;

impl utoipa::PartialSchema for Binary {
    fn schema() -> RefOr<utoipa::openapi::schema::Schema> {
        use utoipa::openapi::schema::{KnownFormat, SchemaFormat};
        ObjectBuilder::new()
            .schema_type(Type::String)
            .format(Some(SchemaFormat::KnownFormat(KnownFormat::Binary)))
            .into()
    }
}

impl utoipa::ToSchema for Binary {}

/// Bearer authentication, and the errors every handler shares: `auth.rs`'s
/// 401 and 403, and axum's rejections of malformed IDs, query parameters,
/// and JSON bodies, and of bodies over the size limit.
struct Errors;

impl Modify for Errors {
    fn modify(&self, openapi: &mut utoipa::openapi::OpenApi) {
        for item in openapi.paths.paths.values_mut() {
            for operation in [&mut item.get, &mut item.post, &mut item.delete]
                .into_iter()
                .flatten()
            {
                let parameters = operation.parameters.as_deref().unwrap_or_default();
                let path = parameters.iter().any(|p| is_in(p, "path"));
                let query = parameters.iter().any(|p| is_in(p, "query"));
                let body = operation.request_body.is_some();
                let malformed = [
                    (path, "ID"),
                    (query, "query parameter"),
                    (body, "JSON body"),
                ]
                .into_iter()
                .filter_map(|(applies, cause)| applies.then_some(cause))
                .collect::<Vec<_>>();
                let responses = &mut operation.responses.responses;
                if !malformed.is_empty() {
                    let cause = format!("a malformed {}", join_or(&malformed));
                    add_cause(responses, "400", &cause, true);
                }
                if body {
                    add_cause(responses, "413", "a body over the size limit", true);
                    add_cause(
                        responses,
                        "415",
                        "no `Content-Type: application/json`",
                        true,
                    );
                    add_cause(
                        responses,
                        "422",
                        "a body that doesn't match its schema",
                        true,
                    );
                }
                add_cause(responses, "401", "missing or wrong access token", true);
                let forbidden = "the request's `Host` or `Origin` isn't allowed, or the caller \
                                 can't use this route (a guest token, or btmux connect)";
                add_cause(responses, "403", forbidden, false);
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

fn is_in(parameter: &RefOr<utoipa::openapi::path::Parameter>, location: &str) -> bool {
    matches!(parameter, RefOr::T(parameter)
        if serde_json::to_value(&parameter.parameter_in).unwrap() == location)
}

fn join_or(causes: &[&str]) -> String {
    match causes {
        [one] => one.to_string(),
        [rest @ .., last] => format!("{} or {last}", rest.join(", ")),
        [] => String::new(),
    }
}

/// Documents a rejection, with a plain-text body if `text`, adding its cause
/// to the response's description if the handler sends that status too.
fn add_cause(
    responses: &mut std::collections::BTreeMap<String, RefOr<Response>>,
    status: &str,
    cause: &str,
    text_body: bool,
) {
    let text = || {
        ContentBuilder::new()
            .schema(Some(ObjectBuilder::new().schema_type(Type::String)))
            .build()
    };
    match responses.get_mut(status) {
        Some(RefOr::T(response)) => {
            response.description = format!("{}; or {cause}", response.description);
            if text_body {
                response
                    .content
                    .entry("text/plain".into())
                    .or_insert_with(|| text().into());
            }
        }
        Some(RefOr::Ref(_)) => unreachable!("responses are inline"),
        None => {
            let mut description = cause.to_string();
            description[..1].make_ascii_uppercase();
            let mut response = ResponseBuilder::new().description(description);
            if text_body {
                response = response.content("text/plain", text());
            }
            let response = response.build();
            responses.insert(status.into(), response.into());
        }
    }
}

fn document() -> String {
    ApiDoc::openapi().to_pretty_json().unwrap() + "\n"
}

/// Routes the reference leaves out, used by the app, CLI commands, or
/// agents' exporters rather than by automation.
const UNDOCUMENTED: &[&str] = &[
    // The app's own surfaces.
    "/api/config/preview",
    "/api/editors",
    "/api/file",
    "/api/multipass",
    "/api/multipass/{action}",
    "/api/nvim/file",
    "/api/panes/{pane_id}/editor-file",
    "/api/panes/{pane_id}/paste-image",
    "/api/preview/{*path}",
    "/api/update",
    "/api/vscode",
    "/api/vscode/license",
    "/api/wallpaper/check",
    "/api/widgets/{id}",
    // `btmux pick`, `confirm`, and `input`, and sandboxed shells.
    "/api/panes/{pane_id}/widget",
    "/api/multipass/bridge",
    // OTLP receivers.
    "/api/panes/{pane_id}/telemetry/v1/logs",
    "/api/panes/{pane_id}/telemetry/v1/metrics",
    // Signing in: pairing and btmux connect.
    "/api/pair",
    "/api/connect/enrollments",
    "/api/connect/passkeys",
    "/api/connect/passkeys/{id}",
    "/api/connect/register",
    "/api/connect/sign-out-everywhere",
];

#[test]
fn openapi_is_current() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("docs/public/openapi.json");
    let generated = document();
    if std::env::var_os("BTMUX_UPDATE_PROTOCOL").is_some() {
        std::fs::write(&path, generated).unwrap();
        return;
    }
    let current = std::fs::read_to_string(&path).unwrap_or_default();
    assert!(
        current == generated,
        "docs/public/openapi.json is stale; run `just protocol`"
    );
}

/// `text` without its inline `#[cfg(test)] mod name { … }` blocks.
fn without_test_modules(text: &str) -> String {
    const CFG: &str = "#[cfg(test)]";
    let mut kept = String::new();
    let mut rest = text;
    while let Some(start) = rest.find(CFG) {
        kept.push_str(&rest[..start + CFG.len()]);
        rest = &rest[start + CFG.len()..];
        let item = rest.trim_start();
        let Some(module) = item.strip_prefix("mod ") else {
            continue;
        };
        let name = module
            .find(|c: char| !c.is_alphanumeric() && c != '_')
            .unwrap_or(module.len());
        if module[name..].trim_start().starts_with('{') {
            let open = rest.find('{').unwrap();
            rest = &rest[open + block_len(&rest[open..])..];
        }
    }
    kept + rest
}

/// The length of the block `text` starts with, through its closing brace,
/// skipping braces in comments, strings, and chars.
fn block_len(text: &str) -> usize {
    let bytes = text.as_bytes();
    let ident = |i: usize| i > 0 && (bytes[i - 1].is_ascii_alphanumeric() || bytes[i - 1] == b'_');
    let mut depth = 0;
    let mut i = 0;
    while i < bytes.len() {
        let next =
            |from: usize, end: &str| text[from..].find(end).map_or(bytes.len(), |n| from + n);
        match bytes[i] {
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return i + 1;
                }
            }
            b'/' if bytes.get(i + 1) == Some(&b'/') => i = next(i, "\n"),
            b'/' if bytes.get(i + 1) == Some(&b'*') => i = next(i + 2, "*/") + 1,
            // Raw strings, after `r` or `br`.
            b'r' if (!ident(i) || (bytes[i - 1] == b'b' && !ident(i - 1)))
                && matches!(bytes.get(i + 1), Some(b'"' | b'#')) =>
            {
                let hashes = text[i + 1..].bytes().take_while(|&b| b == b'#').count();
                if bytes.get(i + 1 + hashes) == Some(&b'"') {
                    let end = format!("\"{}", "#".repeat(hashes));
                    i = next(i + 2 + hashes, &end) + end.len() - 1;
                }
            }
            b'"' => {
                i += 1;
                while i < bytes.len() && bytes[i] != b'"' {
                    i += if bytes[i] == b'\\' { 2 } else { 1 };
                }
            }
            // A char, not a lifetime.
            b'\'' => {
                if bytes.get(i + 1) == Some(&b'\\') {
                    i = next(i + 2, "'");
                } else if let Some(c) = text[i + 1..].chars().next() {
                    if text[i + 1 + c.len_utf8()..].starts_with('\'') {
                        i += c.len_utf8() + 1;
                    }
                }
            }
            _ => {}
        }
        i += 1;
    }
    bytes.len()
}

#[test]
fn test_modules_are_skipped() {
    let text = r##"
a
#[cfg(test)]
mod helpers;
b
#[cfg(test)]
mod tests {
    // }
    /* } */
    fn f<'a>(x: &'a str) { let _ = ('}', '\'', "}\"", r#"}"#, br"}"); }
}
c
#[cfg(test)]
fn g() {}
"##;
    let kept = without_test_modules(text);
    assert!(["a", "b", "c", "mod helpers;", "fn g() {}"]
        .iter()
        .all(|line| kept.contains(line)));
    assert!(!kept.contains("fn f"), "{kept}");
}

/// Each `/api` route in the sources (outside test modules) is documented or
/// in `UNDOCUMENTED`, so a new one needs a decision. It sees `.route` calls
/// with a literal path, as every `/api` route is declared.
#[test]
fn api_routes_are_documented() {
    fn sources(dir: &std::path::Path, files: &mut Vec<String>) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                sources(&path, files);
            } else if path.extension().is_some_and(|ext| ext == "rs") {
                let text = std::fs::read_to_string(&path).unwrap();
                files.push(without_test_modules(&text));
            }
        }
    }
    let mut files = Vec::new();
    sources(
        &std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src"),
        &mut files,
    );
    let route = regex::Regex::new(r#"\.route\(\s*"(/api/[^"]*)""#).unwrap();
    let routed: std::collections::BTreeSet<String> = files
        .iter()
        .flat_map(|text| route.captures_iter(text))
        .map(|captures| captures[1].to_owned())
        .collect();

    let doc = tests::doc();
    let documented = doc["paths"].as_object().unwrap();
    for path in &routed {
        let listed = UNDOCUMENTED.contains(&path.as_str());
        assert!(
            documented.contains_key(path) != listed,
            "{path}: document it, or list it in UNDOCUMENTED, not both"
        );
    }
    for path in UNDOCUMENTED {
        assert!(
            routed.contains(*path),
            "{path} in UNDOCUMENTED isn't routed"
        );
    }
}

mod tests {
    use super::*;

    use axum::body::{to_bytes, Body};
    use axum::extract::{MatchedPath, Request};
    use axum::http::{header, Method, StatusCode};
    use axum::middleware::{self, Next};
    use axum::Router;
    use serde_json::{json, Value};
    use tower::ServiceExt;

    const NIL: &str = "00000000-0000-0000-0000-000000000000";
    const TOKEN: &str = "test-token";

    /// The routes `main` serves, minus sign-in.
    fn app() -> Router {
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
        server::create_app(std::sync::Arc::new(tokio::sync::RwLock::new(manager))).merge(
            server::info_routes(server::ServerInfo {
                version: crate::config::VERSION.into(),
                profile: None,
                config_file: None,
                state_file: None,
                token_file: None,
                token_source: "test".into(),
                listen_address: "127.0.0.1:8004".into(),
                executable: None,
                log_files: None,
                instance: "test".into(),
                pid: 0,
            }),
        )
    }

    /// Behind `auth.rs`, as `main` serves it.
    fn protected(app: Router) -> Router {
        let auth = crate::auth::Auth::new(TOKEN.into(), "127.0.0.1", 8004, &[]).unwrap();
        app.layer(middleware::from_fn_with_state(
            std::sync::Arc::new(auth),
            crate::auth::protect,
        ))
    }

    pub(super) fn doc() -> Value {
        serde_json::to_value(ApiDoc::openapi()).unwrap()
    }

    /// Every documented path is routed with that template, and the route
    /// takes exactly the documented methods. Handlers don't run.
    #[tokio::test]
    async fn routes_match() {
        let app = app();
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

        for (path, item) in doc()["paths"].as_object().unwrap() {
            let uri = path
                .split('/')
                .map(|part| match part.starts_with('{') {
                    true => NIL,
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

    /// Sends requests and checks each reply against the document: its
    /// status is documented for the operation, a body has a documented
    /// media type, and JSON matches its schema.
    struct Checker {
        app: Router,
        doc: Value,
    }

    /// A request body: JSON, or JSON sent without a content type.
    enum Sent {
        Json(String),
        Untyped(String),
    }

    /// Who sends a request: with the token, without it, or with the token
    /// from a page on another origin.
    #[derive(Clone, Copy)]
    enum Caller {
        Owner,
        Anonymous,
        OtherOrigin,
    }

    impl Checker {
        async fn call(
            &self,
            method: &Method,
            template: &str,
            uri: &str,
            body: Option<Sent>,
        ) -> (StatusCode, Value) {
            self.call_as(Caller::Owner, method, template, uri, body)
                .await
        }

        async fn call_as(
            &self,
            caller: Caller,
            method: &Method,
            template: &str,
            uri: &str,
            body: Option<Sent>,
        ) -> (StatusCode, Value) {
            let mut request = axum::http::Request::builder()
                .method(method.clone())
                .uri(uri)
                .header(header::HOST, "127.0.0.1:8004");
            if !matches!(caller, Caller::Anonymous) {
                request = request.header(header::AUTHORIZATION, format!("Bearer {TOKEN}"));
            }
            if matches!(caller, Caller::OtherOrigin) {
                request = request.header(header::ORIGIN, "https://example.com");
            }
            let body = match body {
                Some(Sent::Json(body)) => {
                    request = request.header(header::CONTENT_TYPE, "application/json");
                    Body::from(body)
                }
                Some(Sent::Untyped(body)) => Body::from(body),
                None => Body::empty(),
            };
            let response = self
                .app
                .clone()
                .oneshot(request.body(body).unwrap())
                .await
                .unwrap();
            let status = response.status();
            let media = response
                .headers()
                .get(header::CONTENT_TYPE)
                .map(|value| {
                    value
                        .to_str()
                        .unwrap()
                        .split(';')
                        .next()
                        .unwrap()
                        .to_owned()
                })
                .unwrap_or_default();
            let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();

            let operation = &self.doc["paths"][template][method.as_str().to_lowercase()];
            assert!(
                operation.is_object(),
                "{method} {template} isn't documented"
            );
            let documented = &operation["responses"][status.as_str()];
            assert!(
                documented.is_object(),
                "{method} {uri}: {status} isn't documented ({})",
                String::from_utf8_lossy(&bytes)
            );
            if bytes.is_empty() {
                return (status, Value::Null);
            }
            let Some(content) = documented["content"].get(&media) else {
                panic!("{method} {uri}: a {status} {media} body isn't documented");
            };
            if media != "application/json" {
                return (status, Value::Null);
            }
            let value: Value = serde_json::from_slice(&bytes).unwrap();
            let mut schema = content["schema"].clone();
            schema["components"] = self.doc["components"].clone();
            let validator = jsonschema::validator_for(&schema).unwrap();
            if let Err(error) = validator.validate(&value) {
                panic!("{method} {uri}: {error}\n{value:#}");
            }
            (status, value)
        }

        /// Whether `body` matches the operation's request schema.
        fn accepts(&self, method: &Method, template: &str, body: &Value) -> bool {
            let operation = &self.doc["paths"][template][method.as_str().to_lowercase()];
            let mut schema =
                operation["requestBody"]["content"]["application/json"]["schema"].clone();
            schema["components"] = self.doc["components"].clone();
            jsonschema::validator_for(&schema).unwrap().is_valid(body)
        }

        /// Like `call`, expecting `expected`.
        async fn expect(
            &self,
            expected: StatusCode,
            method: Method,
            template: &str,
            uri: &str,
            body: Option<Value>,
        ) -> Value {
            let body = body.map(|body| Sent::Json(body.to_string()));
            let (status, value) = self.call(&method, template, uri, body).await;
            assert_eq!(status, expected, "{method} {uri}");
            value
        }
    }

    /// Replies on a fresh server, without shells, match the document.
    #[tokio::test]
    async fn responses_match() {
        use Method as M;
        use StatusCode as S;
        let check = Checker {
            app: protected(app()),
            doc: doc(),
        };

        let created = check
            .expect(
                S::OK,
                M::POST,
                "/api/sessions",
                "/api/sessions",
                Some(json!({"name": "first"})),
            )
            .await;
        let session = created["id"].as_str().unwrap().to_owned();
        let sessions = check
            .expect(S::OK, M::GET, "/api/sessions", "/api/sessions", None)
            .await;
        assert_eq!(sessions[0]["id"], created["id"]);
        let snapshot = check
            .expect(
                S::OK,
                M::GET,
                "/api/sessions/{session_id}",
                &format!("/api/sessions/{session}"),
                None,
            )
            .await;
        let window = snapshot["windows"][0]["id"].as_str().unwrap().to_owned();
        let pane = snapshot["windows"][0]["panes"][0]["id"]
            .as_str()
            .unwrap()
            .to_owned();
        for (uri, status) in [
            (format!("/api/sessions/{NIL}"), S::NOT_FOUND),
            ("/api/sessions/nope".to_owned(), S::BAD_REQUEST),
        ] {
            check
                .expect(status, M::GET, "/api/sessions/{session_id}", &uri, None)
                .await;
        }

        for path in [
            "/api/panes",
            "/api/agents",
            "/api/telemetry",
            "/api/info",
            "/api/system",
        ] {
            check.expect(S::OK, M::GET, path, path, None).await;
        }

        let split = format!("/api/panes/{pane}/split");
        let template = "/api/panes/{pane_id}/split";
        for (body, status) in [
            (Sent::Json(r#"{"direction":"right"}"#.into()), S::OK),
            (
                Sent::Json(r#"{"direction":"sideways"}"#.into()),
                S::BAD_REQUEST,
            ),
            (Sent::Json("{".into()), S::BAD_REQUEST),
            (Sent::Json("{}".into()), S::UNPROCESSABLE_ENTITY),
            (
                Sent::Untyped(r#"{"direction":"right"}"#.into()),
                S::UNSUPPORTED_MEDIA_TYPE,
            ),
        ] {
            let (got, _) = check.call(&M::POST, template, &split, Some(body)).await;
            assert_eq!(got, status, "POST {split}");
        }
        check
            .expect(
                S::NOT_FOUND,
                M::POST,
                template,
                &format!("/api/panes/{NIL}/split"),
                Some(json!({"direction": "right"})),
            )
            .await;

        let created = check
            .expect(
                S::OK,
                M::POST,
                "/api/sessions",
                "/api/sessions",
                Some(json!({"name": "second"})),
            )
            .await;
        let second = created["id"].as_str().unwrap().to_owned();
        let calls = [
            (
                S::OK,
                M::POST,
                "/api/sessions/{session_id}/windows",
                format!("/api/sessions/{session}/windows"),
                Some(json!({})),
            ),
            (
                S::OK,
                M::POST,
                "/api/sessions/{session_id}/rename",
                format!("/api/sessions/{session}/rename"),
                Some(json!({"name": "main"})),
            ),
            (
                S::OK,
                M::POST,
                "/api/sessions/{session_id}/windows/rename",
                format!("/api/sessions/{session}/windows/rename"),
                Some(json!({"name": "second"})),
            ),
            (
                S::OK,
                M::POST,
                "/api/windows/{window_id}/rename",
                format!("/api/windows/{window}/rename"),
                Some(json!({"name": "first"})),
            ),
            (
                S::BAD_REQUEST,
                M::POST,
                "/api/windows/{window_id}/rename",
                format!("/api/windows/{window}/rename"),
                Some(json!({"name": ""})),
            ),
            (
                S::UNPROCESSABLE_ENTITY,
                M::POST,
                "/api/notify",
                "/api/notify".into(),
                Some(json!({"title": ""})),
            ),
            (
                S::NO_CONTENT,
                M::POST,
                "/api/notify",
                "/api/notify".into(),
                Some(json!({"title": "Hi"})),
            ),
            (
                S::OK,
                M::GET,
                "/api/panes/{pane_id}/agent-status",
                format!("/api/panes/{pane}/agent-status"),
                None,
            ),
            (
                S::NO_CONTENT,
                M::POST,
                "/api/panes/{pane_id}/open-file-browser",
                format!("/api/panes/{pane}/open-file-browser"),
                Some(json!({"path": "/"})),
            ),
            (
                S::OK,
                M::POST,
                "/api/panes/{pane_id}/open-editor",
                format!("/api/panes/{pane}/open-editor"),
                Some(json!({"editor": "pane"})),
            ),
            (
                S::BAD_REQUEST,
                M::POST,
                "/api/sessions/{session_id}/layout",
                format!("/api/sessions/{session}/layout"),
                Some(json!({"preset": "nope"})),
            ),
            (
                S::CONFLICT,
                M::POST,
                "/api/sessions/{session_id}/panes/{pane_id}/zoom",
                format!("/api/sessions/{second}/panes/{pane}/zoom"),
                None,
            ),
            (
                S::NO_CONTENT,
                M::DELETE,
                "/api/sessions/{session_id}",
                format!("/api/sessions/{second}"),
                None,
            ),
            (
                S::CONFLICT,
                M::DELETE,
                "/api/sessions/{session_id}",
                format!("/api/sessions/{session}"),
                None,
            ),
            (
                S::NO_CONTENT,
                M::POST,
                "/api/sessions/{session_id}/windows/close",
                format!("/api/sessions/{session}/windows/close"),
                None,
            ),
            (
                S::CONFLICT,
                M::POST,
                "/api/sessions/{session_id}/windows/close",
                format!("/api/sessions/{session}/windows/close"),
                None,
            ),
        ];
        for (status, method, template, uri, body) in calls {
            check.expect(status, method, template, &uri, body).await;
        }

        for (caller, status) in [
            (Caller::Anonymous, S::UNAUTHORIZED),
            (Caller::OtherOrigin, S::FORBIDDEN),
        ] {
            let (got, _) = check
                .call_as(caller, &M::GET, "/api/sessions", "/api/sessions", None)
                .await;
            assert_eq!(got, status);
        }

        // Over the default limit, and the popup route's own.
        let big = "x".repeat(2 << 20);
        check
            .expect(
                S::PAYLOAD_TOO_LARGE,
                M::POST,
                "/api/notify",
                "/api/notify",
                Some(json!({"title": big})),
            )
            .await;
        let popup = format!("/api/panes/{pane}/popup");
        let template = "/api/panes/{pane_id}/popup";
        check
            .expect(
                S::PAYLOAD_TOO_LARGE,
                M::POST,
                template,
                &popup,
                Some(json!({"command": {"shell": "true"}, "title": big})),
            )
            .await;

        // Each environment entry is a name and a value.
        for (env, valid) in [
            (json!([["NAME", "value"]]), true),
            (json!([["NAME"]]), false),
        ] {
            let body = json!({"command": {"shell": "true"}, "env": env});
            assert_eq!(check.accepts(&M::POST, template, &body), valid, "{env}");
        }
        let body = json!({"command": {"shell": "true"}, "env": [["NAME"]]});
        check
            .expect(
                S::UNPROCESSABLE_ENTITY,
                M::POST,
                template,
                &popup,
                Some(body),
            )
            .await;

        // Every documented preset is one the handler takes.
        let preset = &check.doc["components"]["schemas"]["SelectLayoutRequest"]["properties"]
            ["preset"]["enum"];
        let presets = preset.as_array().unwrap();
        assert_eq!(
            presets.len(),
            crate::session::layout::LayoutPreset::ALL.len()
        );
        for preset in presets {
            check
                .expect(
                    S::NO_CONTENT,
                    M::POST,
                    "/api/sessions/{session_id}/layout",
                    &format!("/api/sessions/{session}/layout"),
                    Some(json!({ "preset": preset })),
                )
                .await;
        }
        check
            .expect(S::OK, M::DELETE, "/api/sessions", "/api/sessions", None)
            .await;

        let served = check
            .expect(
                S::OK,
                M::GET,
                "/api/openapi.json",
                "/api/openapi.json",
                None,
            )
            .await;
        let mut expected = doc();
        expected.as_object_mut().unwrap().remove("servers");
        assert!(
            served == expected,
            "/api/openapi.json serves a stale document; run `just protocol`"
        );
    }
}
