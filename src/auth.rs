//! One authentication boundary for HTTP, WebSocket upgrades, and MCP.
use axum::{
    extract::{Request, State},
    http::{header, HeaderMap, HeaderName, HeaderValue, Method, StatusCode},
    middleware::Next,
    response::{Html, IntoResponse, Response},
    routing::post,
    Json, Router,
};
use base64::Engine;
use std::{
    path::Path,
    sync::{Arc, Mutex, OnceLock, RwLock},
    time::{Duration, Instant},
};

static TOKEN: OnceLock<String> = OnceLock::new();
pub fn shell_token() -> Option<&'static str> {
    TOKEN.get().map(String::as_str)
}

pub struct Auth {
    token: String,
    /// Allowed browser origins, e.g. `https://host:8004`.
    origins: RwLock<Vec<String>>,
    cookie_name: String,
    /// One-time codes that sign in another device through `/pair`.
    pairing_codes: Mutex<Vec<(String, Instant)>>,
    /// The only origin relayed requests may come from: the btmux connect
    /// registration's. Without one, relayed requests are refused.
    relayed_origin: RwLock<Option<String>>,
    /// Passkey sign-in for relayed requests.
    passkeys: OnceLock<Arc<crate::connect::passkeys::Passkeys>>,
}

// Browsers keep cookies at most 400 days.
const COOKIE_MAX_AGE: u32 = 400 * 24 * 60 * 60;
const PAIRING_CODE_TTL: Duration = Duration::from_secs(120);
const MAX_PAIRING_CODES: usize = 16;

pub(crate) fn parse_origin(value: &str) -> Result<String, String> {
    let url = reqwest::Url::parse(value).map_err(|e| e.to_string())?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.path() != "/"
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(format!(
            "invalid public URL {value:?}: supply an HTTP(S) origin without credentials or a path"
        ));
    }
    Ok(url.origin().ascii_serialization())
}

fn authority(origin: &str) -> &str {
    origin
        .split_once("://")
        .map_or(origin, |(_, authority)| authority)
}

impl Auth {
    pub fn new(
        token: String,
        host: &str,
        port: u16,
        public_urls: &[String],
    ) -> Result<Self, String> {
        if !crate::discovery::valid_token(&token) {
            return Err(
                "BTMUX_AUTH_TOKEN must contain at least 4 ASCII letters, digits, '-' or '_'".into(),
            );
        }
        let mut urls = public_urls.to_vec();
        let host = host.trim_matches(['[', ']']);
        urls.extend([
            format!("http://localhost:{port}"),
            format!("http://127.0.0.1:{port}"),
            format!("http://[::1]:{port}"),
        ]);
        if host != "localhost"
            && !host
                .parse::<std::net::IpAddr>()
                .is_ok_and(|ip| ip.is_loopback())
        {
            let host = if host.contains(':') {
                format!("[{host}]")
            } else {
                host.to_string()
            };
            urls.push(format!("http://{host}:{port}"));
        }
        let origins = urls
            .iter()
            .map(|value| parse_origin(value))
            .collect::<Result<_, _>>()?;
        Ok(Self {
            token,
            origins: RwLock::new(origins),
            cookie_name: format!("btmux_auth_{port}"),
            pairing_codes: Mutex::new(Vec::new()),
            relayed_origin: RwLock::new(None),
            passkeys: OnceLock::new(),
        })
    }

    pub fn set_passkeys(&self, passkeys: crate::connect::passkeys::Passkeys) {
        let _ = self.passkeys.set(Arc::new(passkeys));
    }

    pub fn passkeys(&self) -> Option<&Arc<crate::connect::passkeys::Passkeys>> {
        self.passkeys.get()
    }

    /// Accept relayed requests for the btmux connect registration's origin.
    pub fn set_relayed_origin(&self, url: &str) -> Result<(), String> {
        let origin = parse_origin(url)?;
        *self.relayed_origin.write().unwrap() = Some(origin);
        Ok(())
    }

    /// A random code that `/pair` exchanges once for the auth cookie.
    pub fn pairing_code(&self) -> String {
        let code = uuid::Uuid::new_v4().simple().to_string();
        let now = Instant::now();
        let mut codes = self.pairing_codes.lock().unwrap();
        codes.retain(|(_, expires)| *expires > now);
        if codes.len() >= MAX_PAIRING_CODES {
            codes.remove(0);
        }
        codes.push((code.clone(), now + PAIRING_CODE_TTL));
        code
    }

    /// The pending code equal to `code`, removed if `redeem`.
    fn pairing_code_matching(&self, code: &str, redeem: bool) -> Option<String> {
        let now = Instant::now();
        let mut codes = self.pairing_codes.lock().unwrap();
        codes.retain(|(_, expires)| *expires > now);
        let index = codes
            .iter()
            .position(|(valid, _)| constant_time_eq(valid.as_bytes(), code.as_bytes()))?;
        Some(if redeem {
            codes.swap_remove(index).0
        } else {
            codes[index].0.clone()
        })
    }

    fn redeem_pairing_code(&self, code: &str) -> bool {
        self.pairing_code_matching(code, true).is_some()
    }

    fn cookie(&self, headers: &HeaderMap) -> HeaderValue {
        let secure = headers
            .get(header::HOST)
            .and_then(|h| h.to_str().ok())
            .is_some_and(|host| {
                let origin = format!("https://{host}");
                self.origins.read().unwrap().iter().any(|o| o == &origin)
            });
        let cookie = format!(
            "{}={}; Path=/; Max-Age={COOKIE_MAX_AGE}; HttpOnly; SameSite=Strict{}",
            self.cookie_name,
            self.token,
            if secure { "; Secure" } else { "" }
        );
        HeaderValue::from_str(&cookie).unwrap()
    }

    /// Allow another origin after startup.
    pub fn allow(&self, url: &str) -> Result<(), String> {
        let origin = parse_origin(url)?;
        let mut origins = self.origins.write().unwrap();
        if !origins.contains(&origin) {
            origins.push(origin);
        }
        Ok(())
    }

    fn permitted_request(&self, headers: &HeaderMap) -> bool {
        let Some(host) = headers.get(header::HOST).and_then(|h| h.to_str().ok()) else {
            return false;
        };
        let origins = self.origins.read().unwrap();
        if !origins
            .iter()
            .any(|allowed| authority(allowed).eq_ignore_ascii_case(host))
        {
            return false;
        }
        // Browser origins are checked even when valid credentials are supplied.
        // Non-browser automation may omit Origin but still needs a token.
        match headers.get(header::ORIGIN) {
            None => true,
            Some(origin) => origin
                .to_str()
                .is_ok_and(|o| origins.iter().any(|allowed| allowed == o)),
        }
    }

    fn authenticated(&self, headers: &HeaderMap) -> bool {
        if let Some(value) = headers
            .get(header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
        {
            if let Some(token) = value.strip_prefix("Bearer ") {
                return constant_time_eq(token.as_bytes(), self.token.as_bytes());
            }
            // Browser-native password prompt: any username, token as password.
            if let Some(encoded) = value.strip_prefix("Basic ") {
                if let Ok(decoded) = base64::engine::general_purpose::STANDARD.decode(encoded) {
                    if let Some(index) = decoded.iter().position(|b| *b == b':') {
                        return constant_time_eq(&decoded[index + 1..], self.token.as_bytes());
                    }
                }
            }
            return false;
        }
        headers
            .get_all(header::COOKIE)
            .iter()
            .filter_map(|h| h.to_str().ok())
            .flat_map(|h| h.split(';'))
            .any(|cookie| {
                cookie.trim().split_once('=').is_some_and(|(name, value)| {
                    name == self.cookie_name
                        && constant_time_eq(value.as_bytes(), self.token.as_bytes())
                })
            })
    }
}

fn is_guest(headers: &HeaderMap) -> bool {
    let Some(token) = guest_token() else {
        return false;
    };
    headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .is_some_and(|bearer| constant_time_eq(bearer.as_bytes(), token.as_bytes()))
}

pub(crate) fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |diff, (a, b)| diff | (a ^ b)) == 0
}

pub async fn protect(State(auth): State<Arc<Auth>>, request: Request, next: Next) -> Response {
    if request
        .extensions()
        .get::<crate::connect::Tunneled>()
        .is_some()
    {
        return protect_relayed(&auth, request, next).await;
    }
    if !auth.permitted_request(request.headers()) {
        return StatusCode::FORBIDDEN.into_response();
    }
    if request.uri().path() == "/pair" {
        match *request.method() {
            Method::GET => return pair_page(&auth, &request),
            Method::POST => return pair(&auth, request).await,
            _ => {}
        }
    }
    if is_guest(request.headers()) {
        let Some(pane) = guest_route(request.method(), request.uri().path()) else {
            return StatusCode::FORBIDDEN.into_response();
        };
        let mut request = request;
        // Guest pids name no host process.
        request.headers_mut().remove("x-btmux-agent-pid");
        request.extensions_mut().insert(GuestRequest(pane));
        return next.run(request).await;
    }
    if !auth.authenticated(request.headers()) {
        if let Some(response) = return_same_site(&request) {
            return response;
        }
        let mut response = (StatusCode::UNAUTHORIZED, "Use your btmux access token as the password. The token file location is printed when the server starts.").into_response();
        if !request.uri().path().starts_with("/api/")
            && !request.uri().path().starts_with("/ws/")
            && !request.uri().path().starts_with("/mcp")
        {
            response.headers_mut().insert(
                header::WWW_AUTHENTICATE,
                "Basic realm=\"btmux\", charset=\"UTF-8\"".parse().unwrap(),
            );
        }
        return response;
    }
    let cookie = request
        .headers()
        .contains_key(header::AUTHORIZATION)
        .then(|| auth.cookie(request.headers()));
    let mut response = next.run(request).await;
    if let Some(cookie) = cookie {
        response.headers_mut().append(header::SET_COOKIE, cookie);
    }
    response
}

/// Requests through a btmux connect relay. The access token never works here,
/// in any form: whoever has the link reaches the internet-facing hostname, so
/// signing in takes a passkey enrolled on this machine (`connect::passkeys`),
/// which yields a session cookie.
async fn protect_relayed(auth: &Auth, mut request: Request, next: Next) -> Response {
    let Some(origin) = auth.relayed_origin.read().unwrap().clone() else {
        return StatusCode::FORBIDDEN.into_response();
    };
    if !relayed_request_permitted(&origin, request.method(), request.headers()) {
        return StatusCode::FORBIDDEN.into_response();
    }
    let Some(passkeys) = auth.passkeys() else {
        return relayed_unauthorized();
    };
    if crate::connect::passkeys::public_route(request.method(), request.uri().path()) {
        return next.run(request).await;
    }
    if let Some(session) = passkeys.session(request.headers()) {
        // So signing out cuts this connection, and the terminals it opens.
        if let Some(connection) = request
            .extensions()
            .get::<crate::connect::RelayedConnection>()
        {
            passkeys.track(session, connection);
        }
        request
            .extensions_mut()
            .insert(crate::connect::passkeys::RelayedSession(session));
        return next.run(request).await;
    }
    let mut response = if is_navigation(&request) {
        passkeys.login_page()
    } else {
        "Sign in with a passkey enrolled on this machine.".into_response()
    };
    *response.status_mut() = StatusCode::UNAUTHORIZED;
    mark_relayed_unauthorized(response.headers_mut());
    response
}

/// A page load, which gets the sign-in page in place of what it asked for.
fn is_navigation(request: &Request) -> bool {
    let path = request.uri().path();
    request.method() == Method::GET
        && !["/api/", "/ws/", "/mcp"]
            .iter()
            .any(|prefix| path.starts_with(prefix))
        && request
            .headers()
            .get(HeaderName::from_static("sec-fetch-dest"))
            .and_then(|v| v.to_str().ok())
            .map_or_else(
                || {
                    request
                        .headers()
                        .get(header::ACCEPT)
                        .and_then(|v| v.to_str().ok())
                        .is_some_and(|accept| accept.contains("text/html"))
                },
                |dest| dest == "document",
            )
}

/// A relayed request must name the registration's host, and come from its
/// origin when it says where it comes from; changes and WebSocket upgrades
/// must say. Machine hostnames share a parent domain, so another machine is
/// same-site and `SameSite=Strict` cookies would go along: refuse those.
fn relayed_request_permitted(origin: &str, method: &Method, headers: &HeaderMap) -> bool {
    let header = |name| {
        headers
            .get(name)
            .and_then(|v: &HeaderValue| v.to_str().ok())
    };
    if !header(header::HOST).is_some_and(|host| host.eq_ignore_ascii_case(authority(origin))) {
        return false;
    }
    if header(HeaderName::from_static("sec-fetch-site")) == Some("same-site") {
        return false;
    }
    match header(header::ORIGIN) {
        Some(sent) => sent == origin,
        None => {
            let changes = !matches!(*method, Method::GET | Method::HEAD);
            !changes && !headers.contains_key(header::UPGRADE)
        }
    }
}

/// Marked so the app knows to show passkey sign-in, not the token form.
fn relayed_unauthorized() -> Response {
    let mut response = (
        StatusCode::UNAUTHORIZED,
        "Sign in with a passkey enrolled on this machine.",
    )
        .into_response();
    mark_relayed_unauthorized(response.headers_mut());
    response
}

fn mark_relayed_unauthorized(headers: &mut HeaderMap) {
    headers.insert(
        HeaderName::from_static("x-btmux-login"),
        HeaderValue::from_static("passkey"),
    );
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    headers.insert(header::VARY, HeaderValue::from_static("Cookie"));
}

/// Where VS Code's sign-in flows (GitHub, Microsoft) return from their sites.
const VSCODE_CALLBACK: &str = "/vscode/callback";

/// Load a sign-in callback again from this site: arriving from the provider's,
/// the navigation is cross-site, which withholds the `SameSite=Strict` cookie,
/// as for `pair`. The refresh is the same URL, so it can lead nowhere else, and
/// a request still without credentials then gets the usual 401.
fn return_same_site(request: &Request) -> Option<Response> {
    let header = |name| request.headers().get(name).and_then(|v| v.to_str().ok());
    let cross_site = header(HeaderName::from_static("sec-fetch-site")) == Some("cross-site")
        && header(HeaderName::from_static("sec-fetch-dest")) == Some("document");
    if request.method() != Method::GET || request.uri().path() != VSCODE_CALLBACK || !cross_site {
        return None;
    }
    let target = request.uri().path_and_query()?.as_str();
    let refresh = HeaderValue::from_str(&format!("0; url={target}")).ok()?;
    Some(
        (
            [
                (header::REFRESH, refresh),
                (header::CACHE_CONTROL, HeaderValue::from_static("no-store")),
                (
                    header::CONTENT_SECURITY_POLICY,
                    HeaderValue::from_static("default-src 'none'"),
                ),
                (
                    header::REFERRER_POLICY,
                    HeaderValue::from_static("no-referrer"),
                ),
            ],
            Html("<!doctype html><title>Signing in…</title>"),
        )
            .into_response(),
    )
}

/// The value of `code` in a query string or form body; codes are hex.
fn pairing_code_param(params: &str) -> Option<&str> {
    params
        .split('&')
        .find_map(|pair| pair.strip_prefix("code="))
}

fn pairing_code_spent() -> Response {
    (
        [(header::CACHE_CONTROL, HeaderValue::from_static("no-store"))],
        (
            StatusCode::UNAUTHORIZED,
            "This pairing code was used or has expired. Get a new one.",
        ),
    )
        .into_response()
}

const PAIR_PAGE: &str = r#"<!doctype html>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<meta name="color-scheme" content="light dark">
<title>Sign in to btmux</title>
<style>
  body { font: 16px system-ui, sans-serif; display: grid; place-items: center; min-height: 90vh; margin: 0 1em; }
  main { text-align: center; }
  button { font: inherit; padding: 0.6em 2em; border-radius: 8px; cursor: pointer; }
</style>
<main>
  <h1>Sign in to btmux</h1>
  <p>This browser will be signed in to the btmux server that issued this link.</p>
  <form method="post" action="/pair">
    <input type="hidden" name="code" value="CODE">
    <button autofocus>Sign in</button>
  </form>
</main>
"#;

/// The page a pairing link (QR code or copied) opens. Fetching it leaves the
/// code unspent, so link previews and scanners can't use it up; its button
/// posts the code to `pair`.
fn pair_page(auth: &Auth, request: &Request) -> Response {
    let query = request.uri().query().unwrap_or_default();
    // Only a pending code goes into the page: hex, nothing to escape.
    let Some(code) =
        pairing_code_param(query).and_then(|code| auth.pairing_code_matching(code, false))
    else {
        return pairing_code_spent();
    };
    (
        [
            (header::CACHE_CONTROL, HeaderValue::from_static("no-store")),
            (
                header::CONTENT_SECURITY_POLICY,
                HeaderValue::from_static(
                    "default-src 'none'; style-src 'unsafe-inline'; form-action 'self'; frame-ancestors 'none'",
                ),
            ),
            // `no-referrer` would make the form's Origin `null`.
            (header::REFERRER_POLICY, HeaderValue::from_static("same-origin")),
        ],
        Html(PAIR_PAGE.replace("CODE", &code)),
    )
        .into_response()
}

/// Spend a pairing code from `pair_page`'s form for the auth cookie. The post
/// comes from this site, so the redirect stays same-site and the
/// `SameSite=Strict` cookie goes with it, even when a scanner's page opened
/// the link.
async fn pair(auth: &Auth, request: Request) -> Response {
    // Browsers send Origin with form posts; `protect` checked it's ours.
    if !request.headers().contains_key(header::ORIGIN) {
        return StatusCode::FORBIDDEN.into_response();
    }
    let (parts, body) = request.into_parts();
    let Ok(body) = axum::body::to_bytes(body, 1024).await else {
        return StatusCode::PAYLOAD_TOO_LARGE.into_response();
    };
    let body = String::from_utf8_lossy(&body);
    if !pairing_code_param(&body).is_some_and(|code| auth.redeem_pairing_code(code)) {
        return pairing_code_spent();
    }
    (
        StatusCode::SEE_OTHER,
        [
            (header::SET_COOKIE, auth.cookie(&parts.headers)),
            (header::LOCATION, HeaderValue::from_static("/")),
            (header::CACHE_CONTROL, HeaderValue::from_static("no-store")),
        ],
    )
        .into_response()
}

/// `POST /api/pair` issues a pairing code; it sits behind `protect`.
pub fn routes(auth: Arc<Auth>) -> Router {
    Router::new().route(
        "/api/pair",
        post(
            move |relayed: Option<axum::Extension<crate::connect::passkeys::RelayedSession>>| async move {
                // A pairing code signs in with the token, which a passkey
                // session over a relay must not be able to hand out.
                if relayed.is_some() {
                    return StatusCode::FORBIDDEN.into_response();
                }
                Json(serde_json::json!({
                    "code": auth.pairing_code(),
                    "expires_in": PAIRING_CODE_TTL.as_secs(),
                }))
                .into_response()
            },
        ),
    )
}

/// Persist a per-profile random credential with owner-only permissions. Never
/// print the credential or put it in URLs, browser history, or access logs.
pub fn load_token(state_path: Option<&Path>) -> Result<String, String> {
    let token = if let Ok(token) = std::env::var("BTMUX_AUTH_TOKEN") {
        token
    } else {
        let path = state_path
            .ok_or("set BTMUX_AUTH_TOKEN when no state directory is available")?
            .with_extension("token");
        let token = read_secret(&path)?;
        eprintln!(
            "btmux access token: {} (browser username: btmux; password: file contents)",
            path.display()
        );
        token
    };
    let _ = TOKEN.set(token.clone());
    Ok(token)
}

static GUEST_TOKEN: OnceLock<String> = OnceLock::new();

/// The token sandboxed guests get, which only reaches their panes' routes.
pub fn guest_token() -> Option<&'static str> {
    GUEST_TOKEN.get().map(String::as_str)
}

/// Kept beside the state file, as guests' shells outlive a restart.
pub fn load_guest_token(state_path: Option<&Path>) -> Result<(), String> {
    let token = match state_path {
        Some(path) => read_secret(&path.with_extension("guest-token"))?,
        None => random_token(),
    };
    let _ = GUEST_TOKEN.set(token);
    Ok(())
}

fn random_token() -> String {
    format!(
        "{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    )
}

/// An owner-only file's token, created if missing.
fn read_secret(path: &Path) -> Result<String, String> {
    use std::io::Write;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    match crate::discovery::create_private_file(path) {
        Ok(mut file) => {
            file.write_all(random_token().as_bytes())
                .map_err(|e| e.to_string())?;
            file.sync_all().map_err(|e| e.to_string())?;
        }
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(e) => return Err(e.to_string()),
    }
    let metadata = std::fs::symlink_metadata(path).map_err(|e| e.to_string())?;
    #[cfg(unix)]
    let private = std::os::unix::fs::PermissionsExt::mode(&metadata.permissions()) & 0o077 == 0;
    #[cfg(windows)]
    let private =
        std::fs::File::open(path).is_ok_and(|file| crate::discovery::acl::is_private(&file));
    if !metadata.is_file() || !private {
        return Err(format!(
            "{} must be a regular owner-only file ({})",
            path.display(),
            if cfg!(windows) {
                "readable only by you, SYSTEM, and Administrators"
            } else {
                "chmod 600"
            }
        ));
    }
    Ok(std::fs::read_to_string(path)
        .map_err(|e| e.to_string())?
        .trim()
        .to_string())
}

/// What a guest-token request may reach: `Some(pane)` for a route of that
/// pane (which `server::guest_scope` checks is a guest's), `Some(None)` for
/// one of no pane.
pub fn guest_route(method: &Method, path: &str) -> Option<Option<uuid::Uuid>> {
    if path == "/api/notify" && method == Method::POST {
        return Some(None);
    }
    let rest = path.strip_prefix("/api/panes/")?;
    let (pane, route) = rest.split_once('/')?;
    let pane = pane.parse().ok()?;
    let allowed = match route {
        "notify" => matches!(*method, Method::POST | Method::DELETE),
        "agent-status" => matches!(*method, Method::GET | Method::POST | Method::DELETE),
        "telemetry/v1/logs" | "telemetry/v1/metrics" | "widget" | "popup" => method == Method::POST,
        // Popups of guest panes run in the guest.
        route => {
            method == Method::GET
                && route
                    .strip_prefix("popup/")
                    .is_some_and(|id| id.parse::<uuid::Uuid>().is_ok())
        }
    };
    allowed.then_some(Some(pane))
}

/// Marks a request made with the guest token, and the pane it names.
#[derive(Clone, Copy)]
pub struct GuestRequest(pub Option<uuid::Uuid>);

#[cfg(test)]
mod tests {
    use super::*;
    fn auth() -> Auth {
        Auth::new("a".repeat(64), "127.0.0.1", 8004, &[]).unwrap()
    }
    /// Relayed requests go through `connect::app`, as the tunnel and
    /// `--connect-listen` serve them.
    #[tokio::test]
    async fn relayed_requests_never_take_the_token() {
        use axum::{body::Body, http::Request, routing::get};
        use tower::ServiceExt;
        let token = "a".repeat(64);
        let auth = Arc::new(Auth::new(token.clone(), "127.0.0.1", 8004, &[]).unwrap());
        let app = Router::new()
            .route("/", get(|| async { "app" }))
            .route("/api/x", post(|| async { "ok" }))
            .route("/ws/control", get(|| async { "ws" }))
            .layer(axum::middleware::from_fn_with_state(auth.clone(), protect));
        let relayed = crate::connect::app(app.clone());
        let request = |method: &str, path: &str, headers: &[(&str, &str)]| {
            let mut request = Request::builder().method(method).uri(path);
            for (name, value) in headers {
                request = request.header(*name, *value);
            }
            request.body(Body::empty()).unwrap()
        };
        let bearer = format!("Bearer {token}");
        let status = |app: &Router, request: Request<Body>| {
            let app = app.clone();
            async move { app.oneshot(request).await.unwrap().status() }
        };

        // Locally the token works; relayed, without a registration, nothing does.
        let local = [
            ("host", "localhost:8004"),
            ("authorization", bearer.as_str()),
        ];
        assert_eq!(status(&app, request("GET", "/", &local)).await, 200);
        let relay = [
            ("host", "laptop.btmux.test"),
            ("authorization", bearer.as_str()),
        ];
        assert_eq!(status(&relayed, request("GET", "/", &relay)).await, 403);

        auth.set_relayed_origin("https://laptop.btmux.test")
            .unwrap();
        let host = ("host", "laptop.btmux.test");
        let origin = ("origin", "https://laptop.btmux.test");
        let basic = format!(
            "Basic {}",
            base64::engine::general_purpose::STANDARD.encode(format!("btmux:{token}"))
        );
        let cookie = format!("btmux_auth_8004={token}");
        for credential in [
            ("authorization", bearer.as_str()),
            ("authorization", basic.as_str()),
            ("cookie", cookie.as_str()),
        ] {
            let response = relayed
                .clone()
                .oneshot(request("GET", "/", &[host, origin, credential]))
                .await
                .unwrap();
            assert_eq!(response.status(), 401, "{credential:?}");
            assert_eq!(response.headers()["x-btmux-login"], "passkey");
            assert!(response.headers().get(header::WWW_AUTHENTICATE).is_none());
            assert!(response.headers().get(header::SET_COOKIE).is_none());
        }
        let code = auth.pairing_code();
        let pair = request("GET", &format!("/pair?code={code}"), &[host]);
        assert_eq!(status(&relayed, pair).await, 401);
        let pair = request("POST", "/pair", &[host, origin]);
        assert_eq!(status(&relayed, pair).await, 401);
        assert!(auth.redeem_pairing_code(&code));

        for (method, path, headers, expected) in [
            // Another host, origin, or machine under the same parent domain.
            ("GET", "/", vec![("host", "localhost:8004")], 403),
            (
                "GET",
                "/",
                vec![host, ("origin", "https://other.btmux.test")],
                403,
            ),
            ("GET", "/", vec![host, ("sec-fetch-site", "same-site")], 403),
            // Changes and upgrades must say where they come from.
            ("POST", "/api/x", vec![host], 403),
            (
                "GET",
                "/ws/control",
                vec![host, ("upgrade", "websocket")],
                403,
            ),
            ("POST", "/api/x", vec![host, origin], 401),
            ("GET", "/", vec![host, ("sec-fetch-site", "none")], 401),
        ] {
            let response = status(&relayed, request(method, path, &headers)).await;
            assert_eq!(response, expected, "{method} {path} {headers:?}");
        }
    }

    #[test]
    fn rejects_cross_origin_and_rebound_hosts() {
        let auth = auth();
        let mut headers = HeaderMap::new();
        headers.insert(header::HOST, "localhost:8004".parse().unwrap());
        headers.insert(header::ORIGIN, "http://localhost:8004".parse().unwrap());
        assert!(auth.permitted_request(&headers));
        headers.insert(header::ORIGIN, "http://evil.example".parse().unwrap());
        assert!(!auth.permitted_request(&headers));
        headers.remove(header::ORIGIN);
        headers.insert(header::HOST, "evil.example:8004".parse().unwrap());
        assert!(!auth.permitted_request(&headers));
    }
    #[test]
    fn allows_origins_added_after_startup() {
        let auth = auth();
        let mut headers = HeaderMap::new();
        headers.insert(header::HOST, "box.tail1.ts.net:8004".parse().unwrap());
        headers.insert(
            header::ORIGIN,
            "https://box.tail1.ts.net:8004".parse().unwrap(),
        );
        assert!(!auth.permitted_request(&headers));
        auth.allow("https://box.tail1.ts.net:8004").unwrap();
        assert!(auth.permitted_request(&headers));
        headers.insert(
            header::ORIGIN,
            "http://box.tail1.ts.net:8004".parse().unwrap(),
        );
        assert!(!auth.permitted_request(&headers));
        assert!(auth.allow("https://box.tail1.ts.net/path").is_err());
    }
    #[test]
    fn requires_credentials_for_every_transport() {
        let auth = auth();
        let mut headers = HeaderMap::new();
        assert!(!auth.authenticated(&headers));
        headers.insert(
            header::AUTHORIZATION,
            format!("Bearer {}", "a".repeat(64)).parse().unwrap(),
        );
        assert!(auth.authenticated(&headers));
        headers.insert(
            header::AUTHORIZATION,
            format!(
                "Basic {}",
                base64::engine::general_purpose::STANDARD
                    .encode(format!("btmux:{}", "a".repeat(64)))
            )
            .parse()
            .unwrap(),
        );
        assert!(auth.authenticated(&headers));
        headers.remove(header::AUTHORIZATION);
        headers.insert(
            header::COOKIE,
            format!("btmux_auth_8004={}", "a".repeat(64))
                .parse()
                .unwrap(),
        );
        assert!(auth.authenticated(&headers));
    }
}

#[cfg(test)]
mod middleware_tests {
    use super::*;
    use tower::ServiceExt;
    #[tokio::test]
    async fn protects_http_files_mcp_and_websocket_upgrades() {
        let auth = Arc::new(
            Auth::new(
                "a".repeat(64),
                "127.0.0.1",
                8004,
                &["https://terminal.example".into()],
            )
            .unwrap(),
        );
        let app = axum::Router::new()
            .fallback(|| async { "ok" })
            .layer(axum::middleware::from_fn_with_state(auth, protect));
        for path in [
            "/",
            "/api/sessions",
            "/api/file?path=/etc/passwd",
            "/ws/control",
            "/ws/files",
            "/ws/pane/id",
            "/ws/forward/id?port=80",
            "/mcp",
        ] {
            let request = Request::builder()
                .uri(path)
                .header(header::HOST, "localhost:8004")
                .body(axum::body::Body::empty())
                .unwrap();
            assert_eq!(
                app.clone().oneshot(request).await.unwrap().status(),
                StatusCode::UNAUTHORIZED,
                "{path}"
            );
        }
        let request = Request::builder()
            .uri("/api/sessions")
            .header(header::HOST, "terminal.example")
            .header(header::AUTHORIZATION, format!("Bearer {}", "a".repeat(64)))
            .body(axum::body::Body::empty())
            .unwrap();
        let response = app.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let cookie = response.headers()[header::SET_COOKIE].to_str().unwrap();
        assert!(
            cookie.contains("HttpOnly")
                && cookie.contains("SameSite=Strict")
                && cookie.contains("Secure")
                && cookie.contains("Max-Age=")
        );
    }

    #[tokio::test]
    async fn sign_in_callbacks_load_again_from_this_site() {
        let auth = Arc::new(Auth::new("a".repeat(64), "127.0.0.1", 8004, &[]).unwrap());
        let app = axum::Router::new()
            .fallback(|| async { "ok" })
            .layer(axum::middleware::from_fn_with_state(auth, protect));
        let get = |uri: &str, site: &str| {
            Request::builder()
                .uri(uri)
                .header(header::HOST, "localhost:8004")
                .header("sec-fetch-site", site)
                .header("sec-fetch-dest", "document")
                .body(axum::body::Body::empty())
                .unwrap()
        };
        let uri = "/vscode/callback?vscode-scheme=code&vscode-authority=vscode.github-authentication&code=x%20y";
        let response = app.clone().oneshot(get(uri, "cross-site")).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()[header::REFRESH], format!("0; url={uri}"));
        // From this site, or anywhere else, it takes credentials as usual.
        for (uri, site) in [(uri, "same-origin"), ("/vscode/", "cross-site")] {
            let response = app.clone().oneshot(get(uri, site)).await.unwrap();
            assert_eq!(response.status(), StatusCode::UNAUTHORIZED, "{uri} {site}");
        }
    }

    #[tokio::test]
    async fn pairing_codes_sign_in_once() {
        let auth = Arc::new(Auth::new("a".repeat(64), "127.0.0.1", 8004, &[]).unwrap());
        let app = routes(auth.clone())
            .fallback(|| async { "ok" })
            .layer(axum::middleware::from_fn_with_state(auth.clone(), protect));
        let get = |uri: &str| {
            Request::builder()
                .uri(uri)
                .header(header::HOST, "localhost:8004")
                .body(axum::body::Body::empty())
                .unwrap()
        };
        let issue = Request::builder()
            .method(Method::POST)
            .uri("/api/pair")
            .header(header::HOST, "localhost:8004")
            .body(axum::body::Body::empty())
            .unwrap();
        assert_eq!(
            app.clone().oneshot(issue).await.unwrap().status(),
            StatusCode::UNAUTHORIZED
        );
        let code = auth.pairing_code();
        for uri in ["/pair", "/pair?code=", "/pair?code=bogus"] {
            let response = app.clone().oneshot(get(uri)).await.unwrap();
            assert_eq!(response.status(), StatusCode::UNAUTHORIZED, "{uri}");
        }
        let post = |body: String, origin: Option<&str>| {
            let mut request = Request::builder()
                .method(Method::POST)
                .uri("/pair")
                .header(header::HOST, "localhost:8004")
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded");
            if let Some(origin) = origin {
                request = request.header(header::ORIGIN, origin);
            }
            request.body(axum::body::Body::from(body)).unwrap()
        };
        // Fetching the link (a preview, a scanner) leaves the code unspent.
        for _ in 0..2 {
            let response = app
                .clone()
                .oneshot(get(&format!("/pair?code={code}")))
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK);
            assert!(!response.headers().contains_key(header::SET_COOKIE));
            let page = axum::body::to_bytes(response.into_body(), usize::MAX)
                .await
                .unwrap();
            let page = String::from_utf8(page.to_vec()).unwrap();
            assert!(page.contains(&format!(r#"name="code" value="{code}""#)));
        }
        // Posts need our Origin.
        for origin in [None, Some("https://elsewhere.test")] {
            let response = app
                .clone()
                .oneshot(post(format!("code={code}"), origin))
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::FORBIDDEN, "{origin:?}");
        }
        let origin = Some("http://localhost:8004");
        let response = app
            .clone()
            .oneshot(post("code=bogus".into(), origin))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        let response = app
            .clone()
            .oneshot(post(format!("code={code}"), origin))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::SEE_OTHER);
        assert_eq!(response.headers()[header::LOCATION], "/");
        let cookie = response.headers()[header::SET_COOKIE].to_str().unwrap();
        assert!(cookie.starts_with(&format!("btmux_auth_8004={};", "a".repeat(64))));
        let response = app
            .clone()
            .oneshot(post(format!("code={code}"), origin))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        let response = app
            .oneshot(get(&format!("/pair?code={code}")))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[test]
    fn pairing_codes_expire_and_are_bounded() {
        let auth = Auth::new("a".repeat(64), "127.0.0.1", 8004, &[]).unwrap();
        let first = auth.pairing_code();
        for _ in 0..MAX_PAIRING_CODES {
            auth.pairing_code();
        }
        assert!(!auth.redeem_pairing_code(&first));
        let code = auth.pairing_code();
        auth.pairing_codes.lock().unwrap().last_mut().unwrap().1 = Instant::now();
        assert!(!auth.redeem_pairing_code(&code));
    }
    #[test]
    fn guest_tokens_reach_only_pane_routes() {
        let pane = uuid::Uuid::new_v4();
        let route = |method: Method, path: String| guest_route(&method, &path);
        assert_eq!(route(Method::POST, "/api/notify".into()), Some(None));
        for path in [
            "notify",
            "telemetry/v1/logs",
            "telemetry/v1/metrics",
            "widget",
            "popup",
        ] {
            assert_eq!(
                route(Method::POST, format!("/api/panes/{pane}/{path}")),
                Some(Some(pane))
            );
        }
        assert_eq!(
            route(Method::DELETE, format!("/api/panes/{pane}/agent-status")),
            Some(Some(pane))
        );
        for path in ["input", "prompt", "output", "open-editor", "split", "focus"] {
            assert_eq!(
                route(Method::POST, format!("/api/panes/{pane}/{path}")),
                None
            );
        }
        assert_eq!(
            route(Method::GET, format!("/api/panes/{pane}/notify")),
            None
        );
        let popup = uuid::Uuid::new_v4();
        assert_eq!(
            route(Method::GET, format!("/api/panes/{pane}/popup/{popup}")),
            Some(Some(pane))
        );
        for path in [
            format!("popup/{popup}/x"),
            "popup/x".into(),
            "popup/".into(),
        ] {
            assert_eq!(
                route(Method::GET, format!("/api/panes/{pane}/{path}")),
                None
            );
        }
        assert_eq!(
            route(Method::POST, format!("/api/panes/{pane}/popup/{popup}")),
            None
        );
        assert_eq!(
            route(Method::POST, "/api/panes/not-a-uuid/notify".into()),
            None
        );
        assert_eq!(route(Method::GET, "/api/sessions".into()), None);
        assert_eq!(route(Method::POST, "/api/multipass/bridge".into()), None);
        assert_eq!(route(Method::GET, "/ws/control".into()), None);
        assert_eq!(route(Method::POST, "/mcp".into()), None);
    }
}
