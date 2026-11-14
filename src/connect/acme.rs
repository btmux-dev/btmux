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
    pub directory: String,
    pub hostname: String,
    pub state_file: PathBuf,
    pub certificates: Arc<Certificates>,
    pub up: watch::Receiver<bool>,
}

const CHECK: Duration = Duration::from_secs(6 * 3600);
const MIN_RETRY: Duration = Duration::from_secs(60);
const RATE_LIMITED_RETRY: Duration = Duration::from_secs(3 * 3600);

impl Maintainer {
    pub async fn run(mut self) {
        let mut retry = MIN_RETRY;
        loop {
            if self.up.wait_for(|up| *up).await.is_err() {
                return;
            }
            let wait = match self.step().await {
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

    /// Issues a certificate if one is due; returns the time to check again.
    async fn step(&self) -> Result<Duration, Failure> {
        let path = path(&self.state_file);
        let (account, mut stored) = self.account(&path).await?;
        let registration =
            Registration::load(&self.state_file)?.ok_or("the registration is gone")?;
        let reason = if stored.issued {
            match self.when(&account, &registration.cert).await {
                Due::Now(reason) => reason,
                Due::At(at) => {
                    let wait = at
                        .duration_since(SystemTime::now())
                        .unwrap_or(Duration::ZERO);
                    if !wait.is_zero() {
                        return Ok(wait.min(CHECK));
                    }
                    "renewal time"
                }
            }
        } else {
            "a certificate from another CA"
        };
        tracing::info!(
            hostname = self.hostname,
            "btmux connect certificate: ordering ({reason})"
        );
        let (chain, key) = self.issue(&account).await?;
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
            let account = Account::builder()?.from_credentials(credentials).await?;
            return Ok((account, stored));
        }
        // The registration's `acme` exists only with the owner's agreement to
        // the CA's terms (`connect-dev --accept-acme-terms`).
        let (account, credentials) = Account::builder()?
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

    /// When the current certificate is due: the CA's renewal window (ACME
    /// Renewal Info) when it offers one, else two thirds of its lifetime.
    async fn when(&self, account: &Account, cert_pem: &str) -> Due {
        let due = due(cert_pem, &self.hostname);
        let Due::At(_) = due else { return due };
        let Some(Ok(der)) = CertificateDer::pem_slice_iter(cert_pem.as_bytes()).next() else {
            return due;
        };
        let Ok(id) = instant_acme::CertificateIdentifier::try_from(&der) else {
            return due;
        };
        match account.renewal_info(&id).await {
            Ok((info, _)) => {
                // A random time in the window spreads renewals out (RFC 9773).
                let window = info.suggested_window;
                let span = (window.end - window.start).whole_seconds().max(0) as u64;
                let offset = random_below(span.max(1));
                let at = window.start + time::Duration::seconds(offset as i64);
                SystemTime::UNIX_EPOCH
                    .checked_add(Duration::from_secs(at.unix_timestamp().max(0) as u64))
                    .map_or(due, Due::At)
            }
            Err(_) => due,
        }
    }

    async fn issue(&self, account: &Account) -> Result<(String, String), Failure> {
        let identifiers = [Identifier::Dns(self.hostname.clone())];
        let mut order = account.new_order(&NewOrder::new(&identifiers)).await?;
        // Cleared however this returns.
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
}
