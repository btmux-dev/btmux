//! Certificates for a btmux connect machine from an ACME CA (Let's Encrypt),
//! validated with TLS-ALPN-01 (RFC 8737) through the relay: the CA connects
//! to the machine's hostname, the relay routes it here by that name, and the
//! tunnel answers with a challenge certificate. No DNS credentials are
//! involved, and the key never leaves the machine.

use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};
use std::time::{Duration, SystemTime};

use instant_acme::{
    Account, AccountCredentials, AuthorizationStatus, ChallengeType, Identifier, NewAccount,
    NewOrder, OrderStatus, RetryPolicy,
};
use rustls::pki_types::{pem::PemObject, CertificateDer, PrivateKeyDer};
use rustls::server::{ClientHello, ResolvesServerCert};
use rustls::sign::CertifiedKey;
use serde::{Deserialize, Serialize};
use tokio::sync::watch;

use super::tunnel::Registration;

/// The TLS-ALPN-01 protocol; handshakes that negotiate it never reach the app.
pub const ACME_TLS_ALPN: &[u8] = b"acme-tls/1";

pub const LETS_ENCRYPT: &str = "https://acme-v02.api.letsencrypt.org/directory";
pub const LETS_ENCRYPT_STAGING: &str = "https://acme-staging-v02.api.letsencrypt.org/directory";
pub const LETS_ENCRYPT_TERMS: &str = "https://letsencrypt.org/repository/";

/// Serves the machine's certificate, or to an ACME CA validating it, the
/// challenge certificate.
#[derive(Debug)]
pub struct Certificates {
    hostname: String,
    current: RwLock<Arc<CertifiedKey>>,
    challenge: RwLock<Option<Arc<CertifiedKey>>>,
}

impl Certificates {
    pub fn new(registration: &Registration) -> Result<Self, String> {
        Ok(Self {
            hostname: registration.hostname.to_ascii_lowercase(),
            current: RwLock::new(certified_key(&registration.cert, &registration.key)?),
            challenge: RwLock::default(),
        })
    }

    /// As `new`, but with ACME a self-signed placeholder stands in for an
    /// unusable certificate until one is issued.
    pub fn load(registration: &Registration) -> Result<Self, String> {
        match Self::new(registration) {
            Err(error) if registration.acme.is_some() => {
                tracing::warn!(
                    hostname = registration.hostname,
                    "btmux connect certificate: {error}; self-signed until one is issued"
                );
                let cert = rcgen::generate_simple_self_signed(vec![registration.hostname.clone()])
                    .map_err(|e| e.to_string())?;
                let mut placeholder = registration.clone();
                placeholder.cert = cert.cert.pem();
                placeholder.key = cert.signing_key.serialize_pem();
                Self::new(&placeholder)
            }
            loaded => loaded,
        }
    }

    pub(super) fn set_current(&self, key: Arc<CertifiedKey>) {
        *self.current.write().unwrap() = key;
    }

    pub(super) fn set_challenge(&self, key: Option<Arc<CertifiedKey>>) {
        *self.challenge.write().unwrap() = key;
    }
}

impl ResolvesServerCert for Certificates {
    fn resolve(&self, hello: ClientHello<'_>) -> Option<Arc<CertifiedKey>> {
        // A CA offers only this protocol (RFC 8737, section 3), which is
        // then what's negotiated.
        let acme = hello.alpn().is_some_and(|protocols| {
            let mut protocols = protocols;
            protocols.next() == Some(ACME_TLS_ALPN) && protocols.next().is_none()
        });
        if !acme {
            return Some(self.current.read().unwrap().clone());
        }
        let named = hello
            .server_name()
            .is_some_and(|name| name.eq_ignore_ascii_case(&self.hostname));
        if named {
            self.challenge.read().unwrap().clone()
        } else {
            None
        }
    }
}

/// A certificate chain and its key, checked to match.
pub fn certified_key(cert_pem: &str, key_pem: &str) -> Result<Arc<CertifiedKey>, String> {
    let certified = unchecked_key(cert_pem, key_pem)?;
    certified
        .keys_match()
        .map_err(|e| format!("the certificate and its key: {e}"))?;
    Ok(certified)
}

/// A certificate chain and its key, not checked: the check parses the
/// certificate with webpki, which refuses a challenge certificate's
/// critical acmeIdentifier.
fn unchecked_key(cert_pem: &str, key_pem: &str) -> Result<Arc<CertifiedKey>, String> {
    let certs = CertificateDer::pem_slice_iter(cert_pem.as_bytes())
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("the certificate: {e}"))?;
    if certs.is_empty() {
        return Err("no certificate".into());
    }
    let key = PrivateKeyDer::from_pem_slice(key_pem.as_bytes())
        .map_err(|e| format!("the certificate's key: {e}"))?;
    let key = super::tunnel::provider()
        .key_provider
        .load_private_key(key)
        .map_err(|e| format!("the certificate's key: {e}"))?;
    Ok(Arc::new(CertifiedKey::new(certs, key)))
}

/// A self-signed certificate for `hostname` carrying the key authorization's
/// digest (RFC 8737, section 3).
fn challenge_certificate(hostname: &str, digest: &[u8]) -> Result<Arc<CertifiedKey>, String> {
    let mut params =
        rcgen::CertificateParams::new(vec![hostname.to_owned()]).map_err(|e| e.to_string())?;
    params.custom_extensions = vec![rcgen::CustomExtension::new_acme_identifier(digest)];
    let key = rcgen::KeyPair::generate().map_err(|e| e.to_string())?;
    let cert = params.self_signed(&key).map_err(|e| e.to_string())?;
    // Signed with `key` just now.
    unchecked_key(&cert.pem(), &key.serialize_pem())
}

/// When a certificate wants replacing.
#[derive(Debug, PartialEq)]
enum Due {
    Now(&'static str),
    At(SystemTime),
}

/// Replaces a certificate that's self-signed, for another name, or past two
/// thirds of its lifetime.
fn due(cert_pem: &str, hostname: &str) -> Due {
    let Some(Ok(der)) = CertificateDer::pem_slice_iter(cert_pem.as_bytes()).next() else {
        return Due::Now("no certificate");
    };
    let Ok((_, cert)) = x509_parser::parse_x509_certificate(&der) else {
        return Due::Now("an unreadable certificate");
    };
    if cert.issuer() == cert.subject() {
        return Due::Now("a self-signed certificate");
    }
    let named = cert
        .subject_alternative_name()
        .ok()
        .flatten()
        .is_some_and(|san| {
            san.value.general_names.iter().any(|name| {
                matches!(name, x509_parser::extensions::GeneralName::DNSName(dns)
                if dns.eq_ignore_ascii_case(hostname))
            })
        });
    if !named {
        return Due::Now("a certificate for another name");
    }
    let validity = cert.validity();
    let (start, end) = (
        validity.not_before.timestamp(),
        validity.not_after.timestamp(),
    );
    let renew = end - (end - start) / 3;
    SystemTime::UNIX_EPOCH
        .checked_add(Duration::from_secs(renew.max(0) as u64))
        .map_or(Due::Now("an unreadable validity"), Due::At)
}

/// The ACME account, kept owner-only beside the registration (`state.acme`).
#[derive(Serialize, Deserialize)]
struct Stored {
    directory: String,
    /// `AccountCredentials`, which isn't `Clone`.
    account: serde_json::Value,
    /// An order being finalized, so an interrupted attempt collects its
    /// certificate instead of ordering another.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pending: Option<Pending>,
}

#[derive(Serialize, Deserialize, Clone)]
struct Pending {
    order: String,
    hostname: String,
    /// The certificate's key (PEM).
    key: String,
}

pub fn path(state_file: &Path) -> PathBuf {
    state_file.with_extension("acme")
}

impl Stored {
    fn load(path: &Path) -> Option<Self> {
        let bytes = std::fs::read(path).ok()?;
        match serde_json::from_slice(&bytes) {
            Ok(stored) => Some(stored),
            Err(error) => {
                tracing::warn!("{}: {error}; starting a new ACME account", path.display());
                None
            }
        }
    }

    fn save(&self, path: &Path) -> Result<(), String> {
        let json = serde_json::to_vec_pretty(self).map_err(|e| e.to_string())?;
        super::write_private(path, &json)
    }
}

/// How an attempt failed.
struct Failure {
    message: String,
    /// The CA's rate limit: retrying soon only adds to it.
    rate_limited: bool,
    /// When the CA said to try again.
    retry_after: Option<Duration>,
}

impl From<String> for Failure {
    fn from(message: String) -> Self {
        Self {
            message,
            rate_limited: false,
            retry_after: None,
        }
    }
}

impl From<&str> for Failure {
    fn from(message: &str) -> Self {
        message.to_owned().into()
    }
}

impl From<instant_acme::Error> for Failure {
    fn from(error: instant_acme::Error) -> Self {
        let rate_limited = matches!(&error, instant_acme::Error::Api(problem)
            if problem.r#type.as_deref() == Some("urn:ietf:params:acme:error:rateLimited"));
        Self {
            message: error.to_string(),
            rate_limited,
            retry_after: None,
        }
    }
}

/// Keeps the machine's certificate issued and renewed for as long as btmux
/// runs. Orders wait for the tunnel (`up`), since validation reaches the
/// machine through it.
pub struct Maintainer {
    directory: String,
    hostname: String,
    state_file: PathBuf,
    certificates: Arc<Certificates>,
    up: watch::Receiver<bool>,
    /// The renewal time drawn from the CA's window, kept while the
    /// certificate and window stay the same.
    scheduled: Option<Scheduled>,
    /// Bounds a whole attempt, since requests themselves have no deadline.
    attempt: Duration,
    /// Built on first use; tests substitute a CA.
    http: Option<Arc<CaHttp>>,
    /// The account, kept until an attempt fails.
    session: Option<Session>,
    /// Settings' view.
    status: Arc<super::status::Status>,
}

struct Session {
    account: Account,
    stored: Stored,
}

struct Scheduled {
    cert: String,
    window: (time::OffsetDateTime, time::OffsetDateTime),
    at: SystemTime,
}

/// What to do about the current certificate.
struct Plan {
    due: Due,
    /// When to ask the CA again (its `Retry-After`), whatever `due` says.
    recheck: Duration,
    /// The certificate a renewal replaces, for a CA offering ARI.
    replaces: Option<instant_acme::CertificateIdentifier<'static>>,
}

impl Plan {
    fn now(reason: &'static str) -> Self {
        Self {
            due: Due::Now(reason),
            recheck: CHECK,
            replaces: None,
        }
    }
}

const CHECK: Duration = Duration::from_secs(6 * 3600);
const MIN_RETRY: Duration = Duration::from_secs(60);
const RATE_LIMITED_RETRY: Duration = Duration::from_secs(3 * 3600);
/// The longest a CA's `Retry-After` is followed.
const MAX_RETRY_AFTER: Duration = Duration::from_secs(8 * 86400);
/// Bounds on a CA's renewal info refresh interval (RFC 9773, section 4.3.2).
const ARI_REFRESH: (Duration, Duration) = (Duration::from_secs(60), Duration::from_secs(24 * 3600));
const ATTEMPT: Duration = Duration::from_secs(10 * 60);

type Responding = std::pin::Pin<
    Box<
        dyn std::future::Future<Output = Result<instant_acme::BytesResponse, instant_acme::Error>>
            + Send,
    >,
>;

/// The CA's HTTP client, noting the `Retry-After` of its last refusal,
/// which `instant_acme::Error` drops.
struct CaHttp {
    client: Arc<dyn instant_acme::HttpClient>,
    refused: std::sync::Mutex<Option<SystemTime>>,
}

impl CaHttp {
    fn new(client: Arc<dyn instant_acme::HttpClient>) -> Arc<Self> {
        Arc::new(Self {
            client,
            refused: std::sync::Mutex::default(),
        })
    }

    /// instant-acme's default client, trusting the platform's roots.
    fn platform() -> Result<Arc<Self>, String> {
        let https = hyper_rustls::HttpsConnectorBuilder::new()
            .try_with_platform_verifier()
            .map_err(|e| e.to_string())?
            .https_only()
            .enable_http1()
            .enable_http2()
            .build();
        let client =
            hyper_util::client::legacy::Client::builder(hyper_util::rt::TokioExecutor::new())
                .build::<_, instant_acme::BodyWrapper<axum::body::Bytes>>(https);
        Ok(Self::new(Arc::new(client)))
    }
}

/// Shares a `CaHttp`, since builders take ownership.
struct SharedHttp(Arc<CaHttp>);

impl instant_acme::HttpClient for SharedHttp {
    fn request(
        &self,
        request: axum::http::Request<instant_acme::BodyWrapper<axum::body::Bytes>>,
    ) -> Responding {
        let response = self.0.client.request(request);
        let http = self.0.clone();
        Box::pin(async move {
            let response = response.await?;
            if !response.parts.status.is_success() {
                *http.refused.lock().unwrap() = retry_after(&response.parts.headers);
            }
            Ok(response)
        })
    }
}

/// A `Retry-After` header: seconds or an HTTP date.
fn retry_after(headers: &axum::http::HeaderMap) -> Option<SystemTime> {
    let value = headers
        .get(axum::http::header::RETRY_AFTER)?
        .to_str()
        .ok()?
        .trim();
    match value.parse::<u64>() {
        Ok(seconds) => Some(SystemTime::now() + Duration::from_secs(seconds)),
        Err(_) => httpdate::parse_http_date(value).ok(),
    }
}

/// How long to wait for validation and issuance.
fn polling() -> RetryPolicy {
    RetryPolicy::new()
        .initial_delay(Duration::from_secs(1))
        .backoff(1.5)
        .timeout(Duration::from_secs(120))
}

/// A signing request for `hostname` with the key in `key_pem`.
fn csr(hostname: &str, key_pem: &str) -> Result<Vec<u8>, String> {
    let key = rcgen::KeyPair::from_pem(key_pem).map_err(|e| e.to_string())?;
    let mut params =
        rcgen::CertificateParams::new(vec![hostname.to_owned()]).map_err(|e| e.to_string())?;
    params.distinguished_name = rcgen::DistinguishedName::new();
    let csr = params.serialize_request(&key).map_err(|e| e.to_string())?;
    Ok(csr.der().to_vec())
}

impl Maintainer {
    pub fn new(
        directory: String,
        hostname: String,
        state_file: PathBuf,
        certificates: Arc<Certificates>,
        up: watch::Receiver<bool>,
        status: Arc<super::status::Status>,
    ) -> Self {
        Self {
            directory,
            hostname: hostname.to_ascii_lowercase(),
            state_file,
            certificates,
            up,
            scheduled: None,
            attempt: ATTEMPT,
            http: None,
            session: None,
            status,
        }
    }

    pub async fn run(mut self) {
        let mut retry = MIN_RETRY;
        loop {
            if self.up.wait_for(|up| *up).await.is_err() {
                return;
            }
            let wait = match self.attempt().await {
                Ok(Some(next)) => {
                    self.status.update_acme(|acme| acme.error = None);
                    retry = MIN_RETRY;
                    next
                }
                Ok(None) => {
                    self.status.update_acme(|acme| acme.stopped = true);
                    tracing::info!(
                        hostname = self.hostname,
                        "btmux connect certificate: the registration changed; restart btmux"
                    );
                    return;
                }
                Err(failure) => {
                    self.status
                        .update_acme(|acme| acme.error = Some(failure.message.clone()));
                    // Starts over from what's saved.
                    self.session = None;
                    let wait = match failure.retry_after {
                        Some(after) => after.clamp(MIN_RETRY, MAX_RETRY_AFTER),
                        None if failure.rate_limited => retry.max(RATE_LIMITED_RETRY),
                        None => retry,
                    };
                    tracing::warn!(
                        hostname = self.hostname,
                        "btmux connect certificate: {}; retrying in {}s",
                        failure.message,
                        wait.as_secs()
                    );
                    retry = (retry * 2).min(CHECK);
                    wait
                }
            };
            tokio::time::sleep(wait).await;
        }
    }

    /// One `step`, bounded: a CA that accepts a connection and stalls would
    /// otherwise stop renewal for good. Canceling withdraws any challenge.
    async fn attempt(&mut self) -> Result<Option<Duration>, Failure> {
        if let Some(http) = &self.http {
            *http.refused.lock().unwrap() = None;
        }
        let limit = self.attempt;
        let result = tokio::time::timeout(limit, self.step())
            .await
            .unwrap_or_else(|_| Err("the CA didn't answer in time".into()));
        result.map_err(|mut failure| {
            if failure.rate_limited {
                failure.retry_after = self
                    .http
                    .as_ref()
                    .and_then(|http| *http.refused.lock().unwrap())
                    .and_then(|at| at.duration_since(SystemTime::now()).ok());
            }
            failure
        })
    }

    /// The registration, unless it changed name or CA since btmux started
    /// (`connect-dev`), which takes a restart.
    fn registration(&self) -> Result<Option<Registration>, Failure> {
        Ok(
            Registration::load(&self.state_file)?.filter(|registration| {
                registration.hostname.eq_ignore_ascii_case(&self.hostname)
                    && registration.acme.as_deref() == Some(self.directory.as_str())
            }),
        )
    }

    /// Issues a certificate if one is due; returns the time to check again,
    /// or `None` once the registration no longer asks for one.
    async fn step(&mut self) -> Result<Option<Duration>, Failure> {
        let Some(registration) = self.registration()? else {
            return Ok(None);
        };
        let account = self.account().await?;
        if let Some((chain, key)) = self.collect(&account).await? {
            return self.install(&account, chain, key);
        }
        let plan = if certified_key(&registration.cert, &registration.key).is_err() {
            Plan::now("an unusable certificate or key")
        } else if registration.issued_to.as_deref() != Some(account.id()) {
            match due(&registration.cert, &self.hostname) {
                Due::Now(reason) => Plan::now(reason),
                Due::At(_) => Plan::now("a certificate from another CA or account"),
            }
        } else {
            self.plan(&account, &registration.cert).await
        };
        let reason = match plan.due {
            Due::Now(reason) => reason,
            Due::At(at) => {
                self.status.update_acme(|acme| {
                    acme.renews_at = Some(super::status::seconds(at));
                });
                match at.duration_since(SystemTime::now()) {
                    Ok(wait) if !wait.is_zero() => return Ok(Some(wait.min(plan.recheck))),
                    _ => "renewal time",
                }
            }
        };
        tracing::info!(
            hostname = self.hostname,
            "btmux connect certificate: ordering ({reason})"
        );
        let (chain, key) = self.issue(&account, plan.replaces).await?;
        self.install(&account, chain, key)
    }

    /// Serves and saves a new certificate.
    fn install(
        &mut self,
        account: &Account,
        chain: String,
        key: String,
    ) -> Result<Option<Duration>, Failure> {
        let certified = match certified_key(&chain, &key) {
            Ok(certified) => certified,
            Err(error) => {
                // Collecting it again wouldn't help.
                self.save_pending(None)?;
                return Err(format!("the CA's certificate: {error}").into());
            }
        };
        // Served at once; a failed save is retried from `pending`.
        self.certificates.set_current(certified);
        // Reread, so a concurrent `connect-dev` isn't undone.
        let Some(mut registration) = self.registration()? else {
            self.save_pending(None)?;
            return Ok(None);
        };
        let certificate = super::status::certificate_info(&chain);
        self.status.update(|s| {
            s.certificate = certificate;
            if let Some(acme) = &mut s.acme {
                acme.renews_at = None;
            }
        });
        registration.cert = chain;
        registration.key = key;
        registration.issued_to = Some(account.id().to_owned());
        registration.save(&self.state_file)?;
        self.save_pending(None)?;
        self.scheduled = None;
        tracing::info!(
            hostname = self.hostname,
            "btmux connect certificate: issued"
        );
        Ok(Some(MIN_RETRY))
    }

    /// The account from `state.acme`, or a new one.
    async fn account(&mut self) -> Result<Account, Failure> {
        if let Some(session) = &self.session {
            return Ok(session.account.clone());
        }
        let http = match &self.http {
            Some(http) => http.clone(),
            None => self.http.insert(CaHttp::platform()?).clone(),
        };
        let builder = || Account::builder_with_http(Box::new(SharedHttp(http.clone())));
        let path = path(&self.state_file);
        let (account, stored) = match Stored::load(&path).filter(|s| s.directory == self.directory)
        {
            Some(stored) => {
                let credentials: AccountCredentials =
                    serde_json::from_value(stored.account.clone()).map_err(|e| e.to_string())?;
                (builder().from_credentials(credentials).await?, stored)
            }
            None => {
                // The registration's `acme` exists only with the owner's
                // agreement to the CA's terms (`connect-dev --accept-acme-terms`).
                let (account, credentials) = builder()
                    .create(
                        &NewAccount {
                            contact: &[],
                            terms_of_service_agreed: true,
                            only_return_existing: false,
                        },
                        self.directory.clone(),
                        None,
                    )
                    .await?;
                let stored = Stored {
                    directory: self.directory.clone(),
                    account: serde_json::to_value(&credentials).map_err(|e| e.to_string())?,
                    pending: None,
                };
                stored.save(&path)?;
                tracing::info!(
                    directory = self.directory,
                    "btmux connect: created an ACME account"
                );
                (account, stored)
            }
        };
        self.session = Some(Session {
            account: account.clone(),
            stored,
        });
        Ok(account)
    }

    fn save_pending(&mut self, pending: Option<Pending>) -> Result<(), String> {
        let session = self.session.as_mut().ok_or("no ACME account")?;
        session.stored.pending = pending;
        session.stored.save(&path(&self.state_file))
    }

    /// The certificate and key of an order an earlier attempt finalized; an
    /// order the CA won't issue is forgotten.
    async fn collect(&mut self, account: &Account) -> Result<Option<(String, String)>, Failure> {
        let Some(pending) = self
            .session
            .as_ref()
            .and_then(|session| session.stored.pending.clone())
        else {
            return Ok(None);
        };
        if pending.hostname != self.hostname {
            self.save_pending(None)?;
            return Ok(None);
        }
        let collected = async {
            let mut order = account.order(pending.order.clone()).await?;
            if order.state().status == OrderStatus::Ready {
                let csr = csr(&self.hostname, &pending.key)
                    .map_err(|_| instant_acme::Error::Str("an unreadable key"))?;
                order.finalize_csr(&csr).await?;
            }
            order.poll_certificate(&polling()).await
        }
        .await;
        match collected {
            Ok(chain) => Ok(Some((chain, pending.key))),
            Err(error @ (instant_acme::Error::Api(_) | instant_acme::Error::Str(_))) => {
                tracing::warn!(
                    hostname = self.hostname,
                    "btmux connect certificate: dropping an unfinished order: {error}"
                );
                self.save_pending(None)?;
                Ok(None)
            }
            Err(error) => Err(error.into()),
        }
    }

    /// When the current certificate is due: in the CA's renewal window (ACME
    /// Renewal Info) when it offers one, else at two thirds of its lifetime.
    async fn plan(&mut self, account: &Account, cert_pem: &str) -> Plan {
        let fallback = Plan {
            due: due(cert_pem, &self.hostname),
            recheck: CHECK,
            replaces: None,
        };
        let Due::At(_) = fallback.due else {
            return fallback;
        };
        let Some(Ok(der)) = CertificateDer::pem_slice_iter(cert_pem.as_bytes()).next() else {
            return fallback;
        };
        let Ok(id) = instant_acme::CertificateIdentifier::try_from(&der) else {
            return fallback;
        };
        match account.renewal_info(&id).await {
            Ok((info, retry_after)) => {
                let window = (info.suggested_window.start, info.suggested_window.end);
                Plan {
                    due: Due::At(self.scheduled_at(cert_pem, window)),
                    recheck: retry_after.clamp(ARI_REFRESH.0, ARI_REFRESH.1),
                    replaces: Some(id),
                }
            }
            Err(_) => fallback,
        }
    }

    /// A random time in the window, which spreads renewals out (RFC 9773),
    /// drawn once per certificate and window.
    fn scheduled_at(
        &mut self,
        cert_pem: &str,
        window: (time::OffsetDateTime, time::OffsetDateTime),
    ) -> SystemTime {
        if let Some(s) = self
            .scheduled
            .as_ref()
            .filter(|s| s.cert == cert_pem && s.window == window)
        {
            return s.at;
        }
        let span = (window.1 - window.0).whole_seconds().max(0) as u64;
        let at = window.0 + time::Duration::seconds(random_below(span.max(1)) as i64);
        let at = SystemTime::UNIX_EPOCH + Duration::from_secs(at.unix_timestamp().max(0) as u64);
        self.scheduled = Some(Scheduled {
            cert: cert_pem.to_owned(),
            window,
            at,
        });
        at
    }

    async fn issue(
        &mut self,
        account: &Account,
        replaces: Option<instant_acme::CertificateIdentifier<'static>>,
    ) -> Result<(String, String), Failure> {
        let identifiers = [Identifier::Dns(self.hostname.clone())];
        let mut order = match replaces {
            // Names the predecessor, so the CA treats it as a renewal (and
            // exempts it from rate limits).
            Some(replaces) => {
                let ordered = account
                    .new_order(&NewOrder::new(&identifiers).replaces(replaces))
                    .await;
                match ordered {
                    Err(instant_acme::Error::Api(problem))
                        if problem.r#type.as_deref()
                            == Some("urn:ietf:params:acme:error:alreadyReplaced") =>
                    {
                        account.new_order(&NewOrder::new(&identifiers)).await?
                    }
                    ordered => ordered?,
                }
            }
            None => account.new_order(&NewOrder::new(&identifiers)).await?,
        };
        let certificates = self.certificates.clone();
        // Cleared however this returns, or is canceled.
        let challenge = Challenge(&certificates);
        let mut authorizations = order.authorizations();
        while let Some(authorization) = authorizations.next().await {
            let mut authorization = authorization?;
            match authorization.status {
                AuthorizationStatus::Valid => continue,
                AuthorizationStatus::Pending => {}
                status => return Err(format!("the authorization is {status:?}").into()),
            }
            let mut handle = authorization
                .challenge(ChallengeType::TlsAlpn01)
                .ok_or("the CA offers no tls-alpn-01 challenge")?;
            let digest = handle.key_authorization().digest();
            challenge.0.set_challenge(Some(challenge_certificate(
                &self.hostname,
                digest.as_ref(),
            )?));
            handle.set_ready().await?;
        }
        let status = order.poll_ready(&polling()).await?;
        drop(challenge);
        if status != OrderStatus::Ready {
            return Err(format!("validation failed: the order is {status:?}").into());
        }
        let key = rcgen::KeyPair::generate()
            .map_err(|e| e.to_string())?
            .serialize_pem();
        // Saved first, so an interrupted attempt collects this order.
        self.save_pending(Some(Pending {
            order: order.url().to_owned(),
            hostname: self.hostname.clone(),
            key: key.clone(),
        }))?;
        order.finalize_csr(&csr(&self.hostname, &key)?).await?;
        let chain = order.poll_certificate(&polling()).await?;
        Ok((chain, key))
    }
}

/// Withdraws the challenge certificate when dropped.
struct Challenge<'a>(&'a Certificates);

impl Drop for Challenge<'_> {
    fn drop(&mut self) {
        self.0.set_challenge(None);
    }
}

fn random_below(bound: u64) -> u64 {
    let mut bytes = [0u8; 8];
    aws_lc_rs::rand::fill(&mut bytes).expect("system randomness");
    u64::from_le_bytes(bytes) % bound
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;

    /// A challenge certificate for `hostname`.
    pub(in crate::connect) fn challenge(hostname: &str) -> Arc<CertifiedKey> {
        challenge_certificate(hostname, &[1u8; 32]).unwrap()
    }

    fn registration(cert: String, key: String) -> Registration {
        Registration {
            relay: "relay.test:443".into(),
            relay_name: "relay.test".into(),
            relay_ca: String::new(),
            hostname: "laptop.test".into(),
            public_url: "https://laptop.test".into(),
            token: "t".into(),
            cert,
            key,
            acme: None,
            issued_to: None,
        }
    }

    /// A certificate for `names` signed by a throwaway CA, valid `days`
    /// starting `ago` days ago.
    fn issued(names: &[&str], ago: i64, days: i64) -> (String, String) {
        let ca_key = rcgen::KeyPair::generate().unwrap();
        let mut ca = rcgen::CertificateParams::new(Vec::<String>::new()).unwrap();
        ca.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
        ca.distinguished_name
            .push(rcgen::DnType::CommonName, "test CA");
        let issuer = rcgen::Issuer::new(ca, ca_key);
        let key = rcgen::KeyPair::generate().unwrap();
        let mut params =
            rcgen::CertificateParams::new(names.iter().map(|n| n.to_string()).collect::<Vec<_>>())
                .unwrap();
        // ARI identifies certificates by it.
        params.use_authority_key_identifier_extension = true;
        let now = time::OffsetDateTime::now_utc();
        params.not_before = now - time::Duration::days(ago);
        params.not_after = params.not_before + time::Duration::days(days);
        let cert = params.signed_by(&key, &issuer).unwrap();
        (cert.pem(), key.serialize_pem())
    }

    #[test]
    fn renews_self_signed_foreign_and_aging_certificates() {
        let self_signed = rcgen::generate_simple_self_signed(vec!["laptop.test".into()]).unwrap();
        assert_eq!(
            due(&self_signed.cert.pem(), "laptop.test"),
            Due::Now("a self-signed certificate")
        );
        let (other, _) = issued(&["other.test"], 1, 90);
        assert_eq!(
            due(&other, "laptop.test"),
            Due::Now("a certificate for another name")
        );
        assert_eq!(due("", "laptop.test"), Due::Now("no certificate"));

        let (fresh, _) = issued(&["laptop.test"], 1, 90);
        let Due::At(at) = due(&fresh, "LAPTOP.test") else {
            panic!()
        };
        let left = at.duration_since(SystemTime::now()).unwrap();
        // Two thirds of 90 days, less the day already gone.
        assert!(left > Duration::from_secs(58 * 86400) && left < Duration::from_secs(60 * 86400));

        let (aging, _) = issued(&["laptop.test"], 61, 90);
        let Due::At(at) = due(&aging, "laptop.test") else {
            panic!()
        };
        assert!(at <= SystemTime::now());
    }

    #[test]
    fn challenge_certificates_carry_the_key_authorization() {
        let digest = [7u8; 32];
        let key = challenge_certificate("laptop.test", &digest).unwrap();
        let (_, cert) = x509_parser::parse_x509_certificate(&key.cert[0]).unwrap();
        // id-pe-acmeIdentifier, 1.3.6.1.5.5.7.1.31
        let ext = cert
            .extensions()
            .iter()
            .find(|e| e.oid.to_id_string() == "1.3.6.1.5.5.7.1.31")
            .expect("acmeIdentifier");
        assert!(ext.critical);
        // An OCTET STRING holding the digest.
        assert_eq!(&ext.value[..2], &[0x04, 32]);
        assert_eq!(&ext.value[2..], &digest);
        let names = cert.subject_alternative_name().unwrap().unwrap();
        assert_eq!(
            names.value.general_names,
            vec![x509_parser::extensions::GeneralName::DNSName("laptop.test")]
        );
    }

    /// Answers a handshake offering `alpn` for `name` and returns the
    /// certificate served, if any.
    fn served(certificates: &Arc<Certificates>, name: &str, alpn: &[&[u8]]) -> Option<Vec<u8>> {
        let mut server =
            rustls::ServerConfig::builder_with_provider(super::super::tunnel::provider())
                .with_safe_default_protocol_versions()
                .unwrap()
                .with_no_client_auth()
                .with_cert_resolver(certificates.clone());
        server.alpn_protocols = vec![b"http/1.1".to_vec(), ACME_TLS_ALPN.to_vec()];
        let mut client =
            rustls::ClientConfig::builder_with_provider(super::super::tunnel::provider())
                .with_safe_default_protocol_versions()
                .unwrap()
                .dangerous()
                .with_custom_certificate_verifier(Arc::new(AcceptAny))
                .with_no_client_auth();
        client.alpn_protocols = alpn.iter().map(|p| p.to_vec()).collect();
        let mut client =
            rustls::ClientConnection::new(Arc::new(client), name.to_owned().try_into().unwrap())
                .unwrap();
        let mut server = rustls::ServerConnection::new(Arc::new(server)).unwrap();
        for _ in 0..4 {
            let mut buf = Vec::new();
            client.write_tls(&mut buf).unwrap();
            if server.read_tls(&mut buf.as_slice()).is_err()
                || server.process_new_packets().is_err()
            {
                return None;
            }
            let mut buf = Vec::new();
            server.write_tls(&mut buf).unwrap();
            client.read_tls(&mut buf.as_slice()).unwrap();
            if client.process_new_packets().is_err() {
                return None;
            }
        }
        client.peer_certificates().map(|c| c[0].to_vec())
    }

    #[derive(Debug)]
    pub(in crate::connect) struct AcceptAny;

    impl rustls::client::danger::ServerCertVerifier for AcceptAny {
        fn verify_server_cert(
            &self,
            _: &CertificateDer<'_>,
            _: &[CertificateDer<'_>],
            _: &rustls::pki_types::ServerName<'_>,
            _: &[u8],
            _: rustls::pki_types::UnixTime,
        ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
            Ok(rustls::client::danger::ServerCertVerified::assertion())
        }
        fn verify_tls12_signature(
            &self,
            _: &[u8],
            _: &CertificateDer<'_>,
            _: &rustls::DigitallySignedStruct,
        ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
            Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
        }
        fn verify_tls13_signature(
            &self,
            _: &[u8],
            _: &CertificateDer<'_>,
            _: &rustls::DigitallySignedStruct,
        ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
            Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
        }
        fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
            super::super::tunnel::provider()
                .signature_verification_algorithms
                .supported_schemes()
        }
    }

    #[test]
    fn serves_the_challenge_only_to_acme_for_this_machine() {
        const HTTP: &[u8] = b"http/1.1";
        let (cert, key) = issued(&["laptop.test"], 1, 90);
        let certificates = Arc::new(Certificates::new(&registration(cert, key)).unwrap());
        let current = certificates.current.read().unwrap().cert[0].to_vec();
        assert_eq!(
            served(&certificates, "laptop.test", &[HTTP]),
            Some(current.clone())
        );
        // No challenge pending: ACME handshakes fail.
        assert_eq!(served(&certificates, "laptop.test", &[ACME_TLS_ALPN]), None);

        let challenge = challenge_certificate("laptop.test", &[1u8; 32]).unwrap();
        certificates.set_challenge(Some(challenge.clone()));
        assert_eq!(
            served(&certificates, "LAPTOP.test", &[ACME_TLS_ALPN]),
            Some(challenge.cert[0].to_vec())
        );
        assert_eq!(served(&certificates, "other.test", &[ACME_TLS_ALPN]), None);
        assert_eq!(
            served(&certificates, "laptop.test", &[HTTP]),
            Some(current.clone())
        );
        // Offering HTTP too negotiates HTTP, so it gets the real certificate.
        for alpn in [[HTTP, ACME_TLS_ALPN], [ACME_TLS_ALPN, HTTP]] {
            assert_eq!(
                served(&certificates, "laptop.test", &alpn),
                Some(current.clone())
            );
        }
        drop(Challenge(&certificates));
        assert_eq!(served(&certificates, "laptop.test", &[ACME_TLS_ALPN]), None);
    }

    #[test]
    fn acme_stands_in_for_an_unusable_certificate() {
        let mut registration = registration(String::new(), String::new());
        assert!(Certificates::load(&registration).is_err());
        registration.acme = Some(LETS_ENCRYPT_STAGING.into());
        let certificates = Certificates::load(&registration).unwrap();
        let current = certificates.current.read().unwrap().clone();
        let (_, cert) = x509_parser::parse_x509_certificate(&current.cert[0]).unwrap();
        assert_eq!(cert.issuer(), cert.subject());
    }

    /// Just enough of an ACME CA: a directory, nonces, accounts, renewal
    /// info with a settable window and `Retry-After`, new orders, which it
    /// records and refuses (rate-limited, if set), and one issued order.
    struct MockCa {
        window: std::sync::Mutex<(time::OffsetDateTime, time::OffsetDateTime)>,
        retry_after: u64,
        rate_limited: bool,
        orders: Arc<std::sync::Mutex<Vec<serde_json::Value>>>,
        /// The certificate of `/order/1`.
        issued: String,
    }

    impl Default for MockCa {
        fn default() -> Self {
            let epoch = time::OffsetDateTime::UNIX_EPOCH;
            Self {
                window: std::sync::Mutex::new((epoch, epoch)),
                retry_after: 0,
                rate_limited: false,
                orders: Arc::default(),
                issued: String::new(),
            }
        }
    }

    const CA: &str = "https://ca.test";

    impl instant_acme::HttpClient for MockCa {
        fn request(
            &self,
            request: axum::http::Request<instant_acme::BodyWrapper<axum::body::Bytes>>,
        ) -> std::pin::Pin<
            Box<
                dyn std::future::Future<
                        Output = Result<instant_acme::BytesResponse, instant_acme::Error>,
                    > + Send,
            >,
        > {
            use base64::Engine;
            let (start, end) = *self.window.lock().unwrap();
            let retry_after = self.retry_after;
            let rate_limited = self.rate_limited;
            let issued = self.issued.clone();
            let path = request.uri().path().to_owned();
            let body = axum::body::Body::new(request.into_body());
            let orders = self.orders.clone();
            Box::pin(async move {
                let sent = axum::body::to_bytes(body, usize::MAX)
                    .await
                    .unwrap_or_default();
                let rfc3339 = time::format_description::well_known::Rfc3339;
                let (status, headers, reply): (u16, Vec<(&str, String)>, String) = match path
                    .as_str()
                {
                    "/dir" => (
                        200,
                        vec![],
                        serde_json::json!({
                            "newNonce": format!("{CA}/nonce"),
                            "newAccount": format!("{CA}/account"),
                            "newOrder": format!("{CA}/order"),
                            "renewalInfo": format!("{CA}/ari"),
                        })
                        .to_string(),
                    ),
                    "/nonce" => (200, vec![], String::new()),
                    "/account" => (201, vec![("location", format!("{CA}/acct/1"))], "{}".into()),
                    "/order" => {
                        let jws: serde_json::Value = serde_json::from_slice(&sent).unwrap();
                        let payload = base64::engine::general_purpose::URL_SAFE_NO_PAD
                            .decode(jws["payload"].as_str().unwrap())
                            .unwrap();
                        orders
                            .lock()
                            .unwrap()
                            .push(serde_json::from_slice(&payload).unwrap());
                        if rate_limited {
                            (
                                429,
                                vec![("retry-after", "86400".into())],
                                r#"{"type":"urn:ietf:params:acme:error:rateLimited","detail":"mock"}"#
                                    .into(),
                            )
                        } else {
                            (
                                403,
                                vec![],
                                r#"{"type":"urn:ietf:params:acme:error:unauthorized","detail":"mock"}"#
                                    .into(),
                            )
                        }
                    }
                    "/order/1" => (
                        200,
                        vec![],
                        serde_json::json!({
                            "status": "valid",
                            "identifiers": [{"type": "dns", "value": "laptop.test"}],
                            "authorizations": [],
                            "finalize": format!("{CA}/finalize/1"),
                            "certificate": format!("{CA}/cert/1"),
                        })
                        .to_string(),
                    ),
                    "/cert/1" => (200, vec![], issued),
                    ari if ari.starts_with("/ari/") => (
                        200,
                        vec![("retry-after", retry_after.to_string())],
                        serde_json::json!({"suggestedWindow": {
                            "start": start.format(&rfc3339).unwrap(),
                            "end": end.format(&rfc3339).unwrap(),
                        }})
                        .to_string(),
                    ),
                    other => panic!("unexpected request for {other}"),
                };
                let mut response = axum::http::Response::builder()
                    .status(status)
                    .header("replay-nonce", "nonce");
                for (name, value) in headers {
                    response = response.header(name, value);
                }
                let (parts, ()) = response.body(()).unwrap().into_parts();
                Ok(instant_acme::BytesResponse {
                    parts,
                    body: Box::new(axum::body::Bytes::from(reply)),
                })
            })
        }
    }

    fn temp_state() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("btmux-acme-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        dir.join("state.json")
    }

    /// A maintainer for `directory`, with the mock CA unless that's
    /// elsewhere.
    fn maintainer(
        state_file: &Path,
        directory: &str,
        ca: &Arc<MockCa>,
        cert: String,
        key: String,
    ) -> Maintainer {
        let mut registration = registration(cert, key);
        registration.acme = Some(directory.to_owned());
        registration.save(state_file).unwrap();
        let certificates = Arc::new(Certificates::new(&registration).unwrap());
        let (_up, up) = watch::channel(true);
        let mut maintainer = Maintainer::new(
            directory.to_owned(),
            "laptop.test".into(),
            state_file.to_owned(),
            certificates,
            up,
            super::super::status::Status::new(&registration, None),
        );
        if directory.starts_with(CA) {
            maintainer.http = Some(CaHttp::new(ca.clone()));
        }
        maintainer
    }

    const MOCK_DIRECTORY: &str = "https://ca.test/dir";
    const MOCK_ACCOUNT: &str = "https://ca.test/acct/1";

    fn change_registration(state_file: &Path, change: impl FnOnce(&mut Registration)) {
        let mut registration = Registration::load(state_file).unwrap().unwrap();
        change(&mut registration);
        registration.save(state_file).unwrap();
    }

    fn self_signed() -> (String, String) {
        let cert = rcgen::generate_simple_self_signed(vec!["laptop.test".into()]).unwrap();
        (cert.cert.pem(), cert.signing_key.serialize_pem())
    }

    /// Follows the CA's renewal info: its refresh interval, one renewal time
    /// per window, and orders naming the certificate they replace.
    #[tokio::test]
    async fn renewals_follow_the_cas_renewal_info() {
        let now = time::OffsetDateTime::now_utc();
        let ca = Arc::new(MockCa {
            window: std::sync::Mutex::new((
                now + time::Duration::days(1),
                now + time::Duration::days(2),
            )),
            retry_after: 90,
            ..MockCa::default()
        });
        let state_file = temp_state();
        let (cert, key) = issued(&["laptop.test"], 1, 90);
        let mut maintainer = maintainer(&state_file, MOCK_DIRECTORY, &ca, cert.clone(), key);

        // Not issued to this account: a fresh order.
        assert!(maintainer.step().await.is_err(), "the mock refuses orders");
        assert_eq!(ca.orders.lock().unwrap().len(), 1);
        assert!(ca.orders.lock().unwrap()[0].get("replaces").is_none());
        change_registration(&state_file, |r| r.issued_to = Some(MOCK_ACCOUNT.into()));

        // A window tomorrow: ask again when the CA says, not hours later.
        // The CA's 90 s, less the moment since the client read it.
        let wait = maintainer.step().await.ok().unwrap().unwrap();
        assert!(
            wait > Duration::from_secs(85) && wait <= Duration::from_secs(90),
            "{wait:?}"
        );
        let at = maintainer.scheduled.as_ref().unwrap().at;
        let acme = maintainer.status.get().acme.unwrap();
        assert_eq!(acme.renews_at, Some(super::super::status::seconds(at)));
        for _ in 0..5 {
            maintainer.step().await.ok().unwrap();
            assert_eq!(maintainer.scheduled.as_ref().unwrap().at, at);
        }
        // A moved window draws a new time.
        *ca.window.lock().unwrap() = (
            now - time::Duration::hours(2),
            now - time::Duration::hours(1),
        );
        assert!(
            maintainer.step().await.is_err(),
            "due now; the mock refuses orders"
        );
        let der = CertificateDer::pem_slice_iter(cert.as_bytes())
            .next()
            .unwrap()
            .unwrap();
        let id = instant_acme::CertificateIdentifier::try_from(&der).unwrap();
        let orders = ca.orders.lock().unwrap();
        assert_eq!(orders.len(), 2);
        assert_eq!(orders[1]["replaces"], id.to_string());
        std::fs::remove_dir_all(state_file.parent().unwrap()).unwrap();
    }

    /// A CA that accepts a connection and never answers can't stop renewal:
    /// the attempt gives up and the retry path runs.
    #[tokio::test]
    async fn a_stalled_ca_times_out() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let mut held = Vec::new();
            while let Ok((tcp, _)) = listener.accept().await {
                held.push(tcp);
            }
        });
        let state_file = temp_state();
        let (cert, key) = self_signed();
        let directory = format!("https://{addr}/dir");
        let mut maintainer = maintainer(&state_file, &directory, &Arc::default(), cert, key);
        maintainer.attempt = Duration::from_millis(300);
        let started = std::time::Instant::now();
        let failure = maintainer.attempt().await.err().unwrap();
        assert!(failure.message.contains("in time"), "{}", failure.message);
        assert!(started.elapsed() < Duration::from_secs(5));
        std::fs::remove_dir_all(state_file.parent().unwrap()).unwrap();
    }

    /// A registration changed by `connect-dev` stops the maintainer before
    /// it orders anything.
    #[tokio::test]
    async fn stops_once_the_registration_changes() {
        let ca = Arc::new(MockCa::default());
        let state_file = temp_state();
        let (cert, key) = self_signed();
        let mut maintainer = maintainer(&state_file, MOCK_DIRECTORY, &ca, cert, key);
        change_registration(&state_file, |r| r.hostname = "desktop.test".into());
        assert_eq!(maintainer.step().await.ok().unwrap(), None);
        change_registration(&state_file, |r| {
            r.hostname = "laptop.test".into();
            r.acme = None;
        });
        assert_eq!(maintainer.step().await.ok().unwrap(), None);
        assert!(ca.orders.lock().unwrap().is_empty());
        std::fs::remove_dir_all(state_file.parent().unwrap()).unwrap();
    }

    #[tokio::test]
    async fn rate_limits_wait_for_the_cas_retry_after() {
        let ca = Arc::new(MockCa {
            rate_limited: true,
            ..MockCa::default()
        });
        let state_file = temp_state();
        let (cert, key) = self_signed();
        let mut maintainer = maintainer(&state_file, MOCK_DIRECTORY, &ca, cert, key);
        let failure = maintainer.attempt().await.err().unwrap();
        assert!(failure.rate_limited);
        let after = failure.retry_after.unwrap();
        assert!(after > Duration::from_secs(86000) && after <= Duration::from_secs(86400));
        std::fs::remove_dir_all(state_file.parent().unwrap()).unwrap();
    }

    /// An order finalized by an interrupted attempt is collected, not
    /// ordered again, and its certificate saved with the account it was
    /// issued to.
    #[tokio::test]
    async fn collects_an_interrupted_order() {
        let (chain, key) = issued(&["laptop.test"], 1, 90);
        let ca = Arc::new(MockCa {
            issued: chain.clone(),
            ..MockCa::default()
        });
        let state_file = temp_state();
        let (cert, placeholder_key) = self_signed();
        let mut maintainer = maintainer(&state_file, MOCK_DIRECTORY, &ca, cert, placeholder_key);
        // Creates the account; the mock refuses the order.
        assert!(maintainer.step().await.is_err());
        maintainer
            .save_pending(Some(Pending {
                order: format!("{CA}/order/1"),
                hostname: "laptop.test".into(),
                key: key.clone(),
            }))
            .unwrap();
        // As after a restart.
        maintainer.session = None;
        assert_eq!(maintainer.step().await.ok().unwrap(), Some(MIN_RETRY));
        assert_eq!(ca.orders.lock().unwrap().len(), 1);
        let registration = Registration::load(&state_file).unwrap().unwrap();
        assert_eq!((registration.cert, registration.key), (chain.clone(), key));
        assert_eq!(registration.issued_to.as_deref(), Some(MOCK_ACCOUNT));
        assert!(Stored::load(&path(&state_file)).unwrap().pending.is_none());
        let served = maintainer.certificates.current.read().unwrap().clone();
        let der = CertificateDer::pem_slice_iter(chain.as_bytes())
            .next()
            .unwrap()
            .unwrap();
        assert_eq!(served.cert[0], der);
        // Settings shows the CA's certificate.
        let certificate = maintainer.status.get().certificate.unwrap();
        assert_eq!(certificate.issuer, "test CA");
        assert!(!certificate.self_signed);
        std::fs::remove_dir_all(state_file.parent().unwrap()).unwrap();
    }
}
