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

    pub(super) fn set_current(&self, key: Arc<CertifiedKey>) {
        *self.current.write().unwrap() = key;
    }

    pub(super) fn set_challenge(&self, key: Option<Arc<CertifiedKey>>) {
        *self.challenge.write().unwrap() = key;
    }
}

impl ResolvesServerCert for Certificates {
    fn resolve(&self, hello: ClientHello<'_>) -> Option<Arc<CertifiedKey>> {
        let acme = hello
            .alpn()
            .is_some_and(|mut protocols| protocols.any(|p| p == ACME_TLS_ALPN));
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

/// The ACME account and what it last issued, kept owner-only beside the
/// registration (`state.acme`).
#[derive(Serialize, Deserialize)]
struct Stored {
    directory: String,
    /// `AccountCredentials`, which isn't `Clone`.
    account: serde_json::Value,
    /// The registration's certificate came from `directory`.
    issued: bool,
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
}

impl From<String> for Failure {
    fn from(message: String) -> Self {
        Self {
            message,
            rate_limited: false,
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
    scheduled: std::sync::Mutex<Option<Scheduled>>,
    /// Bounds a whole attempt, since requests themselves have no deadline.
    attempt: Duration,
    /// Tests substitute a CA.
    http: Option<Arc<dyn instant_acme::HttpClient>>,
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

const CHECK: Duration = Duration::from_secs(6 * 3600);
const MIN_RETRY: Duration = Duration::from_secs(60);
const RATE_LIMITED_RETRY: Duration = Duration::from_secs(3 * 3600);
/// Bounds on a CA's renewal info refresh interval (RFC 9773, section 4.3.2).
const ARI_REFRESH: (Duration, Duration) = (Duration::from_secs(60), Duration::from_secs(24 * 3600));
const ATTEMPT: Duration = Duration::from_secs(10 * 60);

/// Delegates to a shared client, since builders take ownership.
struct SharedHttp(Arc<dyn instant_acme::HttpClient>);

impl instant_acme::HttpClient for SharedHttp {
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
        self.0.request(request)
    }
}

impl Maintainer {
    pub fn new(
        directory: String,
        hostname: String,
        state_file: PathBuf,
        certificates: Arc<Certificates>,
        up: watch::Receiver<bool>,
    ) -> Self {
        Self {
            directory,
            hostname: hostname.to_ascii_lowercase(),
            state_file,
            certificates,
            up,
            scheduled: std::sync::Mutex::default(),
            attempt: ATTEMPT,
            http: None,
        }
    }

    pub async fn run(mut self) {
        let mut retry = MIN_RETRY;
        loop {
            if self.up.wait_for(|up| *up).await.is_err() {
                return;
            }
            let wait = match self.attempt().await {
                Ok(next) => {
                    retry = MIN_RETRY;
                    next
                }
                Err(failure) => {
                    let wait = if failure.rate_limited {
                        retry.max(RATE_LIMITED_RETRY)
                    } else {
                        retry
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
    /// otherwise stop renewal for good. Cancelling withdraws any challenge.
    async fn attempt(&self) -> Result<Duration, Failure> {
        tokio::time::timeout(self.attempt, self.step())
            .await
            .unwrap_or_else(|_| Err("the CA didn't answer in time".into()))
    }

    fn builder(&self) -> Result<instant_acme::AccountBuilder, Failure> {
        Ok(match &self.http {
            Some(http) => Account::builder_with_http(Box::new(SharedHttp(http.clone()))),
            None => Account::builder()?,
        })
    }

    /// Issues a certificate if one is due; returns the time to check again.
    async fn step(&self) -> Result<Duration, Failure> {
        let path = path(&self.state_file);
        let (account, mut stored) = self.account(&path).await?;
        let registration =
            Registration::load(&self.state_file)?.ok_or("the registration is gone")?;
        let plan = if stored.issued {
            self.plan(&account, &registration.cert).await
        } else {
            Plan {
                due: Due::Now("a certificate from another CA"),
                recheck: CHECK,
                replaces: None,
            }
        };
        let reason = match plan.due {
            Due::Now(reason) => reason,
            Due::At(at) => match at.duration_since(SystemTime::now()) {
                Ok(wait) if !wait.is_zero() => return Ok(wait.min(plan.recheck)),
                _ => "renewal time",
            },
        };
        tracing::info!(
            hostname = self.hostname,
            "btmux connect certificate: ordering ({reason})"
        );
        let (chain, key) = self.issue(&account, plan.replaces).await?;
        let certified = certified_key(&chain, &key)?;
        // Reread, so a concurrent `connect-dev` isn't undone.
        let mut registration =
            Registration::load(&self.state_file)?.ok_or("the registration is gone")?;
        if registration.hostname.to_ascii_lowercase() != self.hostname {
            return Err("the registration changed hostname; restart btmux"
                .to_owned()
                .into());
        }
        registration.cert = chain;
        registration.key = key;
        registration.save(&self.state_file)?;
        self.certificates.set_current(certified);
        stored.issued = true;
        stored.save(&path)?;
        *self.scheduled.lock().unwrap() = None;
        tracing::info!(
            hostname = self.hostname,
            "btmux connect certificate: issued"
        );
        Ok(MIN_RETRY)
    }

    /// The stored account for this directory, or a new one. A new account
    /// (or directory) hasn't issued the current certificate.
    async fn account(&self, path: &Path) -> Result<(Account, Stored), Failure> {
        if let Some(stored) = Stored::load(path).filter(|s| s.directory == self.directory) {
            let credentials: AccountCredentials =
                serde_json::from_value(stored.account.clone()).map_err(|e| e.to_string())?;
            let account = self.builder()?.from_credentials(credentials).await?;
            return Ok((account, stored));
        }
        // The registration's `acme` exists only with the owner's agreement to
        // the CA's terms (`connect-dev --accept-acme-terms`).
        let (account, credentials) = self
            .builder()?
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
            issued: false,
        };
        stored.save(path)?;
        tracing::info!(
            directory = self.directory,
            "btmux connect: created an ACME account"
        );
        Ok((account, stored))
    }

    /// When the current certificate is due: in the CA's renewal window (ACME
    /// Renewal Info) when it offers one, else at two thirds of its lifetime.
    async fn plan(&self, account: &Account, cert_pem: &str) -> Plan {
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
        &self,
        cert_pem: &str,
        window: (time::OffsetDateTime, time::OffsetDateTime),
    ) -> SystemTime {
        let mut scheduled = self.scheduled.lock().unwrap();
        if let Some(s) = scheduled
            .as_ref()
            .filter(|s| s.cert == cert_pem && s.window == window)
        {
            return s.at;
        }
        let span = (window.1 - window.0).whole_seconds().max(0) as u64;
        let at = window.0 + time::Duration::seconds(random_below(span.max(1)) as i64);
        let at = SystemTime::UNIX_EPOCH + Duration::from_secs(at.unix_timestamp().max(0) as u64);
        *scheduled = Some(Scheduled {
            cert: cert_pem.to_owned(),
            window,
            at,
        });
        at
    }

    async fn issue(
        &self,
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
        // Cleared however this returns, or is cancelled.
        let challenge = Challenge(&self.certificates);
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
        let validation = RetryPolicy::new()
            .initial_delay(Duration::from_secs(1))
            .backoff(1.5)
            .timeout(Duration::from_secs(120));
        let status = order.poll_ready(&validation).await?;
        drop(challenge);
        if status != OrderStatus::Ready {
            return Err(format!("validation failed: the order is {status:?}").into());
        }
        let key = order.finalize().await?;
        let chain = order.poll_certificate(&validation).await?;
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
    fn served(certificates: &Arc<Certificates>, name: &str, alpn: &[u8]) -> Option<Vec<u8>> {
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
        client.alpn_protocols = vec![alpn.to_vec()];
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
        let (cert, key) = issued(&["laptop.test"], 1, 90);
        let certificates = Arc::new(Certificates::new(&registration(cert, key)).unwrap());
        let current = certificates.current.read().unwrap().cert[0].to_vec();
        assert_eq!(
            served(&certificates, "laptop.test", b"http/1.1"),
            Some(current.clone())
        );
        // No challenge pending: ACME handshakes fail.
        assert_eq!(served(&certificates, "laptop.test", ACME_TLS_ALPN), None);

        let challenge = challenge_certificate("laptop.test", &[1u8; 32]).unwrap();
        certificates.set_challenge(Some(challenge.clone()));
        assert_eq!(
            served(&certificates, "LAPTOP.test", ACME_TLS_ALPN),
            Some(challenge.cert[0].to_vec())
        );
        assert_eq!(served(&certificates, "other.test", ACME_TLS_ALPN), None);
        assert_eq!(
            served(&certificates, "laptop.test", b"http/1.1"),
            Some(current)
        );
        drop(Challenge(&certificates));
        assert_eq!(served(&certificates, "laptop.test", ACME_TLS_ALPN), None);
    }

    /// Just enough of an ACME CA: a directory, nonces, accounts, renewal
    /// info with a settable window and `Retry-After`, and new orders, which
    /// it records and refuses.
    struct MockCa {
        window: std::sync::Mutex<(time::OffsetDateTime, time::OffsetDateTime)>,
        retry_after: u64,
        orders: Arc<std::sync::Mutex<Vec<serde_json::Value>>>,
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
                        (
                            403,
                            vec![],
                            r#"{"type":"urn:ietf:params:acme:error:unauthorized","detail":"mock"}"#
                                .into(),
                        )
                    }
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

    fn maintainer(state_file: &Path, cert: String, key: String) -> Maintainer {
        let mut registration = registration(cert, key);
        registration.acme = Some(format!("{CA}/dir"));
        registration.save(state_file).unwrap();
        let certificates = Arc::new(Certificates::new(&registration).unwrap());
        let (_up, up) = watch::channel(true);
        Maintainer::new(
            format!("{CA}/dir"),
            "laptop.test".into(),
            state_file.to_owned(),
            certificates,
            up,
        )
    }

    fn mark_issued(state_file: &Path) {
        let path = path(state_file);
        let mut stored = Stored::load(&path).unwrap();
        stored.issued = true;
        stored.save(&path).unwrap();
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
            orders: Arc::default(),
        });
        let state_file = temp_state();
        let (cert, key) = issued(&["laptop.test"], 1, 90);
        let mut maintainer = maintainer(&state_file, cert.clone(), key);
        maintainer.http = Some(ca.clone());

        // A new account hasn't issued the current certificate: a fresh order.
        assert!(maintainer.step().await.is_err(), "the mock refuses orders");
        assert_eq!(ca.orders.lock().unwrap().len(), 1);
        assert!(ca.orders.lock().unwrap()[0].get("replaces").is_none());
        mark_issued(&state_file);

        // A window tomorrow: ask again when the CA says, not hours later.
        let wait = maintainer.step().await.ok().unwrap();
        assert_eq!(wait, Duration::from_secs(90));
        let at = maintainer.scheduled.lock().unwrap().as_ref().unwrap().at;
        for _ in 0..5 {
            maintainer.step().await.ok().unwrap();
            assert_eq!(
                maintainer.scheduled.lock().unwrap().as_ref().unwrap().at,
                at
            );
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
        let self_signed = rcgen::generate_simple_self_signed(vec!["laptop.test".into()]).unwrap();
        let mut maintainer = maintainer(
            &state_file,
            self_signed.cert.pem(),
            self_signed.signing_key.serialize_pem(),
        );
        maintainer.directory = format!("https://{addr}/dir");
        maintainer.attempt = Duration::from_millis(300);
        let started = std::time::Instant::now();
        let failure = maintainer.attempt().await.err().unwrap();
        assert!(failure.message.contains("in time"), "{}", failure.message);
        assert!(started.elapsed() < Duration::from_secs(5));
        std::fs::remove_dir_all(state_file.parent().unwrap()).unwrap();
    }
}
