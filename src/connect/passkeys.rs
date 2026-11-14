//! Passkey sign-in for relayed connections. The access token never works over
//! a relay (`auth::protect_relayed`); instead a passkey enrolled on this
//! machine signs a browser in, and gets a session cookie (`session.rs`).
//!
//! Enrollment takes a one-time code, issued to whoever already has access: the
//! token locally, or a freshly signed-in relayed session. Passkeys live
//! beside the state file (`state.passkeys`, owner-only); removing one ends its
//! sessions.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

use axum::{
    extract::{DefaultBodyLimit, Path as UrlPath, State},
    http::{header, HeaderMap, HeaderValue, StatusCode},
    response::{Html, IntoResponse, Response},
    routing::{delete, get, post},
    Extension, Json, Router,
};
use base64::Engine;
use serde::{Deserialize, Serialize};
use webauthn_rs::prelude::*;

use super::session::{self, Keys, Session};
use crate::auth::Auth;

const CODE_TTL: Duration = Duration::from_secs(5 * 60);
const MAX_CODES: usize = 16;
/// A registration outlives a cancelled prompt, so the page can retry.
const REGISTRATION_TTL: Duration = Duration::from_secs(5 * 60);
const MAX_REGISTRATIONS: usize = 16;
const CEREMONY_TTL: u64 = 2 * 60;
/// Enrolling, removing passkeys, and signing out everywhere over the relay
/// take a sign-in this recent, so a stolen session can't become permanent.
const FRESH: u64 = 5 * 60;
/// Unauthenticated ceremony starts, across all clients: the relay hides
/// their addresses.
const STARTS_PER_MINUTE: u32 = 60;
/// Sign-in state names every enrolled passkey and comes back in the finish
/// request, so the cap on passkeys keeps that request well under the limit.
const MAX_PASSKEYS: usize = 10;
const BODY_LIMIT: usize = 64 * 1024;

#[derive(Serialize, Deserialize, Clone)]
pub struct Credential {
    /// The credential ID, base64url.
    pub id: String,
    pub label: String,
    pub created: u64,
    pub last_used: Option<u64>,
    passkey: Passkey,
}

#[derive(Serialize, Deserialize)]
struct Store {
    /// Passkeys are bound to this hostname; another registration's are ignored.
    rp_id: String,
    user_id: Uuid,
    credentials: Vec<Credential>,
}

/// A signed-in relayed request, for handlers that need a fresh sign-in.
#[derive(Clone, Copy)]
pub struct RelayedSession(pub Session);

pub struct Passkeys {
    webauthn: Webauthn,
    hostname: String,
    origin: String,
    state_file: PathBuf,
    /// Written under this lock, so concurrent sign-ins can't roll a counter back.
    store: Mutex<Store>,
    keys: RwLock<Keys>,
    codes: Mutex<Vec<(String, Instant)>>,
    registrations: Mutex<HashMap<String, (PasskeyRegistration, Instant)>>,
    /// Sign-in ceremonies already finished, until they expire.
    used: Mutex<HashMap<String, u64>>,
    starts: Mutex<(Instant, u32)>,
    events: Option<tokio::sync::broadcast::Sender<String>>,
    /// Sign-in state is sealed for this process only, so a restart or an
    /// upgrade can't make a finished sign-in usable again.
    salt: [u8; 16],
    /// Connections signed in with a session, to cut when it ends: a removed
    /// passkey, everyone signed out, or expiry. Weak, so finished connections
    /// drop out.
    connections: Mutex<HashMap<usize, (Session, std::sync::Weak<super::Live>)>>,
}

fn store_path(state_file: &Path) -> PathBuf {
    state_file.with_extension("passkeys")
}

fn b64() -> base64::engine::GeneralPurpose {
    base64::engine::general_purpose::URL_SAFE_NO_PAD
}

impl Passkeys {
    pub fn new(
        registration: &super::tunnel::Registration,
        state_file: &Path,
        events: Option<tokio::sync::broadcast::Sender<String>>,
    ) -> Result<Self, String> {
        let origin = Url::parse(&registration.public_url).map_err(|e| e.to_string())?;
        // User verification is always required; no subdomains, no other ports.
        let webauthn = WebauthnBuilder::new(&registration.hostname, &origin)
            .and_then(|builder| builder.rp_name("btmux").build())
            .map_err(|e| format!("passkeys for {}: {e}", registration.hostname))?;
        let rp_id = registration.hostname.clone();
        let path = store_path(state_file);
        let store = match crate::discovery::read_private_file(&path, 1024 * 1024) {
            Ok(text) => serde_json::from_str::<Store>(&text)
                .map_err(|e| format!("{}: {e}", path.display()))?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Store {
                rp_id: rp_id.clone(),
                user_id: Uuid::new_v4(),
                credentials: Vec::new(),
            },
            Err(error) => return Err(format!("{}: {error}", path.display())),
        };
        let store = if store.rp_id == rp_id {
            store
        } else {
            Store {
                rp_id: rp_id.clone(),
                user_id: store.user_id,
                credentials: Vec::new(),
            }
        };
        let mut salt = [0u8; 16];
        aws_lc_rs::rand::fill(&mut salt).map_err(|_| "no system randomness".to_owned())?;
        Ok(Self {
            webauthn,
            hostname: registration.hostname.clone(),
            origin: origin.origin().ascii_serialization(),
            state_file: state_file.to_owned(),
            store: Mutex::new(store),
            keys: RwLock::new(Keys::load(state_file, &rp_id, salt)?),
            codes: Mutex::default(),
            registrations: Mutex::default(),
            used: Mutex::default(),
            starts: Mutex::new((Instant::now(), 0)),
            events,
            salt,
            connections: Mutex::default(),
        })
    }

    fn save(&self, store: &Store) -> Result<(), String> {
        let json = serde_json::to_vec_pretty(store).map_err(|e| e.to_string())?;
        super::write_private(&store_path(&self.state_file), &json)
    }

    /// The request's session, if its cookie is valid and its passkey still enrolled.
    pub fn session(&self, headers: &HeaderMap) -> Option<Session> {
        let cookie = session::session_from(headers)?;
        let session = self.keys.read().unwrap().verify(cookie, session::now())?;
        let enrolled = self.store.lock().unwrap().credentials.iter().any(|c| {
            b64()
                .decode(&c.id)
                .is_ok_and(|id| session::credential_digest(&id) == session.credential)
        });
        enrolled.then_some(session)
    }

    /// Registers the connection a signed-in request arrived on, so ending its
    /// session cuts it along with any terminal it opened.
    pub fn track(&self, session: Session, connection: &super::RelayedConnection) {
        let key = Arc::as_ptr(&connection.0) as usize;
        let mut connections = self.connections.lock().unwrap();
        connections.retain(|_, (_, live)| live.strong_count() > 0);
        connections.insert(key, (session, Arc::downgrade(&connection.0)));
    }

    /// Cuts the tracked connections whose session `ended`.
    fn cut(&self, ended: impl Fn(&Session) -> bool) -> usize {
        let mut connections = self.connections.lock().unwrap();
        let mut cut = 0;
        connections.retain(|_, (session, live)| {
            let Some(live) = live.upgrade() else {
                return false;
            };
            if !ended(session) {
                return true;
            }
            live.cut();
            cut += 1;
            false
        });
        cut
    }

    /// Cuts connections whose sessions expired; run periodically.
    pub fn cut_expired(&self) {
        let now = session::now();
        self.cut(|s| s.issued + session::SESSION_LIFETIME.as_secs() <= now);
    }

    /// Bounds unauthenticated ceremony starts.
    fn admit(&self) -> Result<(), Refusal> {
        let mut starts = self.starts.lock().unwrap();
        if starts.0.elapsed() > Duration::from_secs(60) {
            *starts = (Instant::now(), 0);
        }
        starts.1 += 1;
        if starts.1 > STARTS_PER_MINUTE {
            return Err(Refusal(
                StatusCode::TOO_MANY_REQUESTS,
                "Too many sign-in attempts; try again in a minute.".into(),
            ));
        }
        Ok(())
    }

    pub fn issue_code(&self) -> (String, String) {
        let code = session::random_id();
        let now = Instant::now();
        let mut codes = self.codes.lock().unwrap();
        codes.retain(|(_, issued)| issued.elapsed() < CODE_TTL);
        if codes.len() >= MAX_CODES {
            codes.remove(0);
        }
        codes.push((code.clone(), now));
        let url = format!("{}/connect/enroll#code={code}", self.origin);
        (code, url)
    }

    fn redeem(&self, code: &str) -> bool {
        let mut codes = self.codes.lock().unwrap();
        codes.retain(|(_, issued)| issued.elapsed() < CODE_TTL);
        let found = codes.iter().position(|(valid, _)| {
            crate::auth::constant_time_eq(valid.as_bytes(), code.as_bytes())
        });
        found.map(|index| codes.swap_remove(index)).is_some()
    }

    fn register_start(&self, code: &str) -> Result<(String, CreationChallengeResponse), Refusal> {
        self.admit()?;
        self.room_for_one()?;
        if !self.redeem(code) {
            return Err(Refusal::unauthorized(
                "This enrollment link was used or has expired. Ask for a new one.",
            ));
        }
        let (user_id, exclude) = {
            let store = self.store.lock().unwrap();
            let exclude = store
                .credentials
                .iter()
                .map(|c| c.passkey.cred_id().clone())
                .collect::<Vec<_>>();
            (store.user_id, exclude)
        };
        let (options, state) = self
            .webauthn
            .start_passkey_registration(user_id, "btmux", &self.hostname, Some(exclude))
            .map_err(Refusal::from)?;
        let id = session::random_id();
        let mut registrations = self.registrations.lock().unwrap();
        registrations.retain(|_, (_, started)| started.elapsed() < REGISTRATION_TTL);
        if registrations.len() >= MAX_REGISTRATIONS {
            return Err(Refusal(
                StatusCode::TOO_MANY_REQUESTS,
                "Too many enrollments in progress.".into(),
            ));
        }
        registrations.insert(id.clone(), (state, Instant::now()));
        Ok((id, options))
    }

    fn register_finish(
        &self,
        id: &str,
        credential: &RegisterPublicKeyCredential,
        label: String,
    ) -> Result<String, Refusal> {
        let state = {
            let mut registrations = self.registrations.lock().unwrap();
            registrations.retain(|_, (_, started)| started.elapsed() < REGISTRATION_TTL);
            registrations.remove(id)
        }
        .ok_or_else(|| Refusal::unauthorized("This enrollment expired. Ask for a new link."))?
        .0;
        let passkey = self
            .webauthn
            .finish_passkey_registration(credential, &state)
            .map_err(Refusal::from)?;
        let id = b64().encode(passkey.cred_id());
        let now = session::now();
        {
            let mut store = self.store.lock().unwrap();
            if store.credentials.iter().any(|c| c.id == id) {
                return Err(Refusal::unauthorized("This passkey is already enrolled."));
            }
            if store.credentials.len() >= MAX_PASSKEYS {
                return Err(too_many_passkeys());
            }
            store.credentials.push(Credential {
                id: id.clone(),
                label: label.clone(),
                created: now,
                last_used: Some(now),
                passkey: passkey.clone(),
            });
            if let Err(error) = self.save(&store) {
                store.credentials.pop();
                return Err(Refusal(StatusCode::INTERNAL_SERVER_ERROR, error));
            }
        }
        tracing::info!(label, "btmux connect: passkey enrolled");
        self.notify(format!("A passkey was added: {label}"));
        Ok(self
            .keys
            .read()
            .unwrap()
            .sign(session::credential_digest(passkey.cred_id()), now))
    }

    fn login_start(&self) -> Result<(String, RequestChallengeResponse), Refusal> {
        self.admit()?;
        let passkeys: Vec<Passkey> = self
            .store
            .lock()
            .unwrap()
            .credentials
            .iter()
            .map(|c| c.passkey.clone())
            .collect();
        if passkeys.is_empty() {
            return Err(Refusal::unauthorized(
                "No passkey is enrolled on this machine yet. Run `btmux connect-enroll` there.",
            ));
        }
        let (options, state) = self
            .webauthn
            .start_passkey_authentication(&passkeys)
            .map_err(Refusal::from)?;
        // No server-side state that unauthenticated starts could crowd out.
        let ceremony = Ceremony {
            id: session::random_id(),
            expires: session::now() + CEREMONY_TTL,
            state,
        };
        let sealed = self
            .keys
            .read()
            .unwrap()
            .seal(&serde_json::to_vec(&ceremony).unwrap());
        Ok((sealed, options))
    }

    fn login_finish(
        &self,
        sealed: &str,
        credential: &PublicKeyCredential,
    ) -> Result<String, Refusal> {
        let expired = || Refusal::unauthorized("This sign-in expired. Try again.");
        let ceremony: Ceremony = self
            .keys
            .read()
            .unwrap()
            .open(sealed)
            .and_then(|plain| serde_json::from_slice(&plain).ok())
            .ok_or_else(expired)?;
        let now = session::now();
        if ceremony.expires <= now {
            return Err(expired());
        }
        let result = self
            .webauthn
            .finish_passkey_authentication(credential, &ceremony.state)
            .map_err(Refusal::from)?;
        {
            let mut used = self.used.lock().unwrap();
            used.retain(|_, expires| *expires > now);
            if used.insert(ceremony.id, ceremony.expires).is_some() {
                return Err(expired());
            }
        }
        let mut store = self.store.lock().unwrap();
        let Some(entry) = store
            .credentials
            .iter_mut()
            .find(|c| c.passkey.cred_id() == result.cred_id())
        else {
            return Err(Refusal::unauthorized("This passkey was removed."));
        };
        entry.passkey.update_credential(&result);
        entry.last_used = Some(now);
        let label = entry.label.clone();
        // A failed write keeps the old counter on disk, which only makes the
        // next check stricter.
        if let Err(error) = self.save(&store) {
            tracing::warn!("btmux connect: {error}");
        }
        drop(store);
        tracing::info!(label, "btmux connect: signed in");
        Ok(self
            .keys
            .read()
            .unwrap()
            .sign(session::credential_digest(result.cred_id()), now))
    }

    pub fn list(&self) -> Vec<PasskeySummary> {
        self.store
            .lock()
            .unwrap()
            .credentials
            .iter()
            .map(|c| PasskeySummary {
                id: c.id.clone(),
                label: c.label.clone(),
                created: c.created,
                last_used: c.last_used,
            })
            .collect()
    }

    pub fn remove(&self, id: &str) -> Result<bool, String> {
        let mut store = self.store.lock().unwrap();
        let Some(index) = store.credentials.iter().position(|c| c.id == id) else {
            return Ok(false);
        };
        let removed = store.credentials.remove(index);
        if let Err(error) = self.save(&store) {
            store.credentials.insert(index, removed);
            return Err(error);
        }
        drop(store);
        if let Ok(id) = b64().decode(&removed.id) {
            let digest = session::credential_digest(&id);
            self.cut(|s| s.credential == digest);
        }
        self.notify(format!("A passkey was removed: {}", removed.label));
        Ok(true)
    }

    pub fn sign_out_everywhere(&self) -> Result<(), String> {
        let rp_id = self.store.lock().unwrap().rp_id.clone();
        *self.keys.write().unwrap() = Keys::rotate(&self.state_file, &rp_id, self.salt)?;
        self.cut(|_| true);
        self.notify("Every relayed browser was signed out.".into());
        Ok(())
    }

    /// Refuses enrollment once the cap on passkeys is reached.
    fn room_for_one(&self) -> Result<(), Refusal> {
        if self.store.lock().unwrap().credentials.len() >= MAX_PASSKEYS {
            return Err(too_many_passkeys());
        }
        Ok(())
    }

    fn notify(&self, message: String) {
        if let Some(events) = &self.events {
            let toast = crate::ws::control::ServerMessage::Toast {
                message,
                level: crate::session::NotificationLevel::Attention,
                persistent: false,
            };
            let _ = events.send(serde_json::to_string(&toast).unwrap());
        }
    }

    pub fn login_page(&self) -> Response {
        page(
            "Sign in",
            &format!(
                "<h1>Sign in to {}</h1>\
                 <p>Use a passkey enrolled on this machine.</p>\
                 <button id=\"login\" type=\"button\">Sign in with a passkey</button>",
                escape(&self.hostname)
            ),
        )
    }

    fn enroll_page(&self) -> Response {
        page(
            "Add a passkey",
            &format!(
                "<h1>Add a passkey for {}</h1>\
                 <p>The passkey you create here signs this browser in to a shell on that \
                 machine. Continue only if you asked for this link.</p>\
                 <button id=\"enroll\" type=\"button\">Create a passkey</button>",
                escape(&self.hostname)
            ),
        )
    }
}

#[derive(Serialize, Deserialize)]
struct Ceremony {
    id: String,
    expires: u64,
    state: PasskeyAuthentication,
}

#[derive(Serialize)]
pub struct PasskeySummary {
    pub id: String,
    pub label: String,
    pub created: u64,
    pub last_used: Option<u64>,
}

fn too_many_passkeys() -> Refusal {
    Refusal(
        StatusCode::CONFLICT,
        format!(
            "This machine already has {MAX_PASSKEYS} passkeys, the most it allows. \
             Remove one with `btmux connect-passkeys --remove` there."
        ),
    )
}

/// A refused ceremony step, with a message the page shows.
pub struct Refusal(StatusCode, String);

impl Refusal {
    fn unauthorized(message: &str) -> Self {
        Self(StatusCode::UNAUTHORIZED, message.into())
    }
}

impl From<WebauthnError> for Refusal {
    fn from(error: WebauthnError) -> Self {
        tracing::debug!("btmux connect: passkey ceremony failed: {error:?}");
        Self(
            StatusCode::UNAUTHORIZED,
            "The passkey wasn't accepted.".into(),
        )
    }
}

impl IntoResponse for Refusal {
    fn into_response(self) -> Response {
        (self.0, [(header::CACHE_CONTROL, "no-store")], self.1).into_response()
    }
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

const PAGE_POLICY: &str = "default-src 'none'; script-src 'self'; style-src 'unsafe-inline'; \
    connect-src 'self'; img-src 'self'; form-action 'none'; frame-ancestors 'none'; base-uri 'none'";

/// A sign-in page: no inline script, nothing framed, nothing cached.
fn page(title: &str, body: &str) -> Response {
    let html = format!(
        "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\">\
         <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\
         <title>{title} · btmux</title><style>{STYLE}</style>\
         <script src=\"/connect/signin.js\" defer></script></head>\
         <body><main>{body}<p id=\"status\" role=\"status\"></p></main></body></html>"
    );
    (
        [
            (header::CONTENT_SECURITY_POLICY, PAGE_POLICY),
            (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
            (header::CACHE_CONTROL, "no-store"),
            (header::REFERRER_POLICY, "no-referrer"),
        ],
        Html(html),
    )
        .into_response()
}

const STYLE: &str = "body{font:16px/1.5 system-ui,sans-serif;margin:0;min-height:100vh;\
    display:grid;place-items:center;background:#f6f5f2;color:#222}\
    @media (prefers-color-scheme:dark){body{background:#17171a;color:#e8e8e8}}\
    main{max-width:26rem;padding:1.5rem}h1{font-size:1.4rem;margin:0 0 .5rem}\
    button{font:inherit;padding:.6rem 1.1rem;border-radius:.5rem;border:0;\
    background:#3b6fd8;color:#fff;cursor:pointer}button:disabled{opacity:.6}\
    #status{min-height:1.5em;opacity:.8}";

/// Paths a relayed browser reaches before signing in.
pub fn public_route(method: &axum::http::Method, path: &str) -> bool {
    use axum::http::Method;
    match *method {
        Method::GET => matches!(
            path,
            "/connect/login" | "/connect/enroll" | "/connect/signin.js" | "/connect/session"
        ),
        Method::POST => matches!(
            path,
            "/connect/logout"
                | "/connect/passkeys/register/start"
                | "/connect/passkeys/register/finish"
                | "/connect/passkeys/login/start"
                | "/connect/passkeys/login/finish"
        ),
        _ => false,
    }
}

fn passkeys(auth: &Auth) -> Result<&Arc<Passkeys>, Refusal> {
    auth.passkeys()
        .ok_or_else(|| Refusal(StatusCode::NOT_FOUND, "btmux connect isn't set up.".into()))
}

fn signed_in(cookie: String) -> Response {
    (
        StatusCode::NO_CONTENT,
        [
            (
                header::SET_COOKIE,
                HeaderValue::from_str(&session::session_cookie(&cookie)).unwrap(),
            ),
            (header::CACHE_CONTROL, HeaderValue::from_static("no-store")),
        ],
    )
        .into_response()
}

/// Over the relay, changes to how this machine is reached take a fresh sign-in.
fn require_fresh(session: Option<Extension<RelayedSession>>) -> Result<(), Refusal> {
    match session {
        Some(Extension(RelayedSession(session)))
            if session::now().saturating_sub(session.issued) > FRESH =>
        {
            Err(Refusal(
                StatusCode::FORBIDDEN,
                "Sign in again at /connect/login?reauth, then try again.".into(),
            ))
        }
        _ => Ok(()),
    }
}

/// A short name for the device a passkey was made on.
fn device_label(headers: &HeaderMap) -> String {
    let agent = headers
        .get(header::USER_AGENT)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default();
    let browser = [
        ("Edg/", "Edge"),
        ("Firefox/", "Firefox"),
        ("Chrome/", "Chrome"),
        ("Safari/", "Safari"),
    ]
    .into_iter()
    .find_map(|(mark, name)| agent.contains(mark).then_some(name));
    let system = [
        ("iPhone", "iPhone"),
        ("iPad", "iPad"),
        ("Android", "Android"),
        ("Mac OS X", "macOS"),
        ("Windows", "Windows"),
        ("Linux", "Linux"),
    ]
    .into_iter()
    .find_map(|(mark, name)| agent.contains(mark).then_some(name));
    match (browser, system) {
        (Some(browser), Some(system)) => format!("{browser} on {system}"),
        (Some(name), None) | (None, Some(name)) => name.to_owned(),
        (None, None) => "Passkey".to_owned(),
    }
}

#[derive(Deserialize)]
struct RegisterStart {
    code: String,
}

#[derive(Deserialize)]
struct RegisterFinish {
    id: String,
    credential: RegisterPublicKeyCredential,
}

#[derive(Deserialize)]
struct LoginFinish {
    ceremony: String,
    credential: PublicKeyCredential,
}

pub fn routes(auth: Arc<Auth>) -> Router {
    let ceremonies = Router::new()
        .route(
            "/connect/passkeys/register/start",
            post(
                |State(auth): State<Arc<Auth>>, Json(body): Json<RegisterStart>| async move {
                    let (id, options) = passkeys(&auth)?.register_start(&body.code)?;
                    Ok::<_, Refusal>(Json(serde_json::json!({ "id": id, "options": options })))
                },
            ),
        )
        .route(
            "/connect/passkeys/register/finish",
            post(
                |State(auth): State<Arc<Auth>>,
                 headers: HeaderMap,
                 Json(body): Json<RegisterFinish>| async move {
                    let label = device_label(&headers);
                    let cookie =
                        passkeys(&auth)?.register_finish(&body.id, &body.credential, label)?;
                    Ok::<_, Refusal>(signed_in(cookie))
                },
            ),
        )
        .route(
            "/connect/passkeys/login/start",
            post(|State(auth): State<Arc<Auth>>| async move {
                let (ceremony, options) = passkeys(&auth)?.login_start()?;
                Ok::<_, Refusal>(Json(
                    serde_json::json!({ "ceremony": ceremony, "options": options }),
                ))
            }),
        )
        .route(
            "/connect/passkeys/login/finish",
            post(
                |State(auth): State<Arc<Auth>>, Json(body): Json<LoginFinish>| async move {
                    let cookie = passkeys(&auth)?.login_finish(&body.ceremony, &body.credential)?;
                    Ok::<_, Refusal>(signed_in(cookie))
                },
            ),
        )
        .layer(DefaultBodyLimit::max(BODY_LIMIT));
    Router::new()
        .route(
            "/connect/login",
            get(|State(auth): State<Arc<Auth>>| async move {
                Ok::<_, Refusal>(passkeys(&auth)?.login_page())
            }),
        )
        .route(
            "/connect/enroll",
            get(|State(auth): State<Arc<Auth>>| async move {
                Ok::<_, Refusal>(passkeys(&auth)?.enroll_page())
            }),
        )
        .route(
            "/connect/signin.js",
            get(|| async {
                (
                    [
                        (header::CONTENT_TYPE, "text/javascript; charset=utf-8"),
                        (header::CACHE_CONTROL, "no-cache"),
                        (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
                    ],
                    include_str!("signin.js"),
                )
            }),
        )
        .route(
            "/connect/session",
            get(|State(auth): State<Arc<Auth>>, headers: HeaderMap| async move {
                let signed_in = auth.passkeys().and_then(|p| p.session(&headers)).is_some();
                if signed_in {
                    StatusCode::NO_CONTENT
                } else {
                    StatusCode::UNAUTHORIZED
                }
            }),
        )
        .route(
            "/connect/logout",
            post(|| async {
                (
                    StatusCode::NO_CONTENT,
                    [(header::SET_COOKIE, session::clear_session_cookie())],
                )
            }),
        )
        .merge(ceremonies)
        .route(
            "/api/connect/enrollments",
            post(
                |State(auth): State<Arc<Auth>>,
                 relayed: Option<Extension<RelayedSession>>| async move {
                    require_fresh(relayed)?;
                    let (_, url) = passkeys(&auth)?.issue_code();
                    Ok::<_, Refusal>(Json(serde_json::json!({
                        "url": url,
                        "expires_in": CODE_TTL.as_secs(),
                    })))
                },
            ),
        )
        .route(
            "/api/connect/passkeys",
            get(|State(auth): State<Arc<Auth>>| async move {
                Ok::<_, Refusal>(Json(passkeys(&auth)?.list()))
            }),
        )
        .route(
            "/api/connect/passkeys/{id}",
            delete(
                |State(auth): State<Arc<Auth>>,
                 relayed: Option<Extension<RelayedSession>>,
                 UrlPath(id): UrlPath<String>| async move {
                    require_fresh(relayed)?;
                    match passkeys(&auth)?.remove(&id) {
                        Ok(true) => Ok(StatusCode::NO_CONTENT),
                        Ok(false) => Err(Refusal(StatusCode::NOT_FOUND, "No such passkey.".into())),
                        Err(error) => Err(Refusal(StatusCode::INTERNAL_SERVER_ERROR, error)),
                    }
                },
            ),
        )
        .route(
            "/api/connect/sign-out-everywhere",
            post(
                |State(auth): State<Arc<Auth>>,
                 relayed: Option<Extension<RelayedSession>>| async move {
                    require_fresh(relayed)?;
                    passkeys(&auth)?
                        .sign_out_everywhere()
                        .map_err(|e| Refusal(StatusCode::INTERNAL_SERVER_ERROR, e))?;
                    Ok::<_, Refusal>(StatusCode::NO_CONTENT)
                },
            ),
        )
        .with_state(auth)
}

/// `btmux connect-enroll`: print a one-time enrollment link.
pub async fn enroll_cli(profile: Option<&str>) -> i32 {
    let response = crate::api_client::request(
        profile,
        reqwest::Method::POST,
        "/api/connect/enrollments",
        None,
        Duration::from_secs(5),
    )
    .await
    .and_then(|body| serde_json::from_str::<serde_json::Value>(&body).map_err(|e| e.to_string()));
    match response {
        Ok(body) => {
            println!("{}", body["url"].as_str().unwrap_or_default());
            eprintln!(
                "Open it within {} minutes to add a passkey. Anyone with this link gets a shell on this machine.",
                body["expires_in"].as_u64().unwrap_or(0) / 60
            );
            0
        }
        Err(error) => {
            eprintln!("btmux: {error}");
            1
        }
    }
}

/// `btmux connect-passkeys`: list passkeys, remove one, or sign out everywhere.
#[derive(clap::Args, Clone, Debug)]
pub struct PasskeysArgs {
    /// Remove the passkey with this ID; its sessions end.
    #[arg(long, value_name = "ID", allow_hyphen_values = true)]
    remove: Option<String>,
    /// End every relayed session.
    #[arg(long)]
    sign_out_everywhere: bool,
}

pub async fn passkeys_cli(args: &PasskeysArgs, profile: Option<&str>) -> i32 {
    use reqwest::Method;
    let timeout = Duration::from_secs(5);
    let result = if let Some(id) = &args.remove {
        crate::api_client::request(
            profile,
            Method::DELETE,
            &format!("/api/connect/passkeys/{id}"),
            None,
            timeout,
        )
        .await
        .map(|_| eprintln!("btmux: removed"))
    } else if args.sign_out_everywhere {
        crate::api_client::request(
            profile,
            Method::POST,
            "/api/connect/sign-out-everywhere",
            None,
            timeout,
        )
        .await
        .map(|_| eprintln!("btmux: every relayed session ended"))
    } else {
        crate::api_client::request(profile, Method::GET, "/api/connect/passkeys", None, timeout)
            .await
            .map(|body| println!("{body}"))
    };
    match result {
        Ok(()) => 0,
        Err(error) => {
            eprintln!("btmux: {error}");
            1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn passkeys() -> (Passkeys, PathBuf) {
        let dir = std::env::temp_dir().join(format!("btmux-passkeys-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        (passkeys_in(&dir), dir)
    }

    /// Another process with the same state, as after a restart or an upgrade.
    fn passkeys_in(dir: &Path) -> Passkeys {
        let registration = super::super::tunnel::Registration {
            relay: "relay.test:4433".into(),
            relay_name: "relay.test".into(),
            relay_ca: String::new(),
            hostname: "laptop.btmux.test".into(),
            public_url: "https://laptop.btmux.test".into(),
            token: String::new(),
            cert: String::new(),
            key: String::new(),
            acme: None,
        };
        Passkeys::new(&registration, &dir.join("state.json"), None).unwrap()
    }

    fn signed_in(credential: &[u8], issued: u64) -> Session {
        Session {
            credential: session::credential_digest(credential),
            issued,
        }
    }

    #[test]
    fn ending_a_session_cuts_its_connections() {
        let (passkeys, dir) = passkeys();
        let now = session::now();
        let phone = super::super::RelayedConnection::new();
        let laptop = super::super::RelayedConnection::new();
        let stale = super::super::RelayedConnection::new();
        passkeys.track(signed_in(b"phone", now), &phone);
        passkeys.track(signed_in(b"laptop", now), &laptop);
        passkeys.track(
            signed_in(b"laptop", now - session::SESSION_LIFETIME.as_secs()),
            &stale,
        );

        passkeys.cut_expired();
        assert!(stale.is_cut() && !phone.is_cut() && !laptop.is_cut());
        // A removed passkey's connections, and only those.
        let digest = session::credential_digest(b"phone");
        assert_eq!(passkeys.cut(|s| s.credential == digest), 1);
        assert!(phone.is_cut() && !laptop.is_cut());
        passkeys.sign_out_everywhere().unwrap();
        assert!(laptop.is_cut());

        // Finished connections drop out instead of piling up.
        let gone = super::super::RelayedConnection::new();
        passkeys.track(signed_in(b"laptop", now), &gone);
        drop(gone);
        assert_eq!(passkeys.cut(|_| true), 0);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn finished_sign_ins_stay_finished_after_a_restart() {
        let (first, dir) = passkeys();
        let sealed = first.keys.read().unwrap().seal(b"ceremony");
        let cookie = first
            .keys
            .read()
            .unwrap()
            .sign(session::credential_digest(b"c"), session::now());
        let restarted = passkeys_in(&dir);
        let keys = restarted.keys.read().unwrap();
        assert!(
            keys.open(&sealed).is_none(),
            "sealed state outlived its process"
        );
        assert!(
            keys.verify(&cookie, session::now()).is_some(),
            "sessions should survive"
        );
        drop(keys);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn credential_ids_may_start_with_a_hyphen() {
        use clap::Parser;
        #[derive(Parser)]
        struct Cli {
            #[command(flatten)]
            args: PasskeysArgs,
        }
        let cli = Cli::try_parse_from(["btmux", "--remove", "-fAbc_123"]).unwrap();
        assert_eq!(cli.args.remove.as_deref(), Some("-fAbc_123"));
    }

    #[test]
    fn enrollment_codes_work_once_and_are_bounded() {
        let (passkeys, dir) = passkeys();
        let (code, url) = passkeys.issue_code();
        assert_eq!(
            url,
            format!("https://laptop.btmux.test/connect/enroll#code={code}")
        );
        assert!(passkeys.register_start(&code).is_ok());
        assert!(passkeys.register_start(&code).is_err());
        assert!(passkeys.register_start("guess").is_err());
        let first = passkeys.issue_code().0;
        for _ in 0..MAX_CODES {
            passkeys.issue_code();
        }
        assert!(
            !passkeys.redeem(&first),
            "the oldest code should be dropped"
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn sign_in_needs_an_enrolled_passkey() {
        let (passkeys, dir) = passkeys();
        assert!(passkeys.login_start().is_err());
        let mut headers = HeaderMap::new();
        let cookie = passkeys.keys.read().unwrap().sign(
            session::credential_digest(b"never enrolled"),
            session::now(),
        );
        headers.insert(
            header::COOKIE,
            format!("{}={cookie}", session::SESSION_COOKIE)
                .parse()
                .unwrap(),
        );
        assert!(passkeys.session(&headers).is_none());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn ceremony_starts_are_rate_limited() {
        let (passkeys, dir) = passkeys();
        for _ in 0..STARTS_PER_MINUTE {
            assert!(passkeys.admit().is_ok());
        }
        assert!(passkeys.admit().is_err());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn device_labels_name_browser_and_system() {
        let label = |agent: &str| {
            let mut headers = HeaderMap::new();
            headers.insert(header::USER_AGENT, agent.parse().unwrap());
            device_label(&headers)
        };
        assert_eq!(
            label("Mozilla/5.0 (iPhone; CPU iPhone OS 18_0 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/18.0 Mobile/15E148 Safari/604.1"),
            "Safari on iPhone"
        );
        assert_eq!(
            label("Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0 Safari/537.36"),
            "Chrome on macOS"
        );
        assert_eq!(label(""), "Passkey");
    }

    #[test]
    fn relayed_changes_take_a_fresh_sign_in() {
        let session = |age: u64| {
            Some(Extension(RelayedSession(Session {
                credential: [0; 16],
                issued: session::now() - age,
            })))
        };
        // Local requests (the token) and recent sign-ins may.
        assert!(require_fresh(None).is_ok());
        assert!(require_fresh(session(60)).is_ok());
        // A session signed in long ago may not.
        assert!(require_fresh(session(FRESH + 60)).is_err());
    }

    #[test]
    fn page_names_are_escaped() {
        assert_eq!(
            escape("<a href=\"x\">&"),
            "&lt;a href=&quot;x&quot;&gt;&amp;"
        );
    }
}
