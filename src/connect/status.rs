//! What Settings shows about btmux connect: the tunnel, the certificate, and
//! the passkeys. Pushed to every control socket on connect and on change.

use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use serde::Serialize;
use tokio::sync::broadcast;

use super::passkeys::PasskeySummary;
use super::tunnel::Registration;
use crate::ws::control::ServerMessage;

#[derive(Clone, PartialEq, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct ConnectStatus {
    pub hostname: String,
    pub public_url: String,
    pub relay: String,
    pub tunnel: TunnelState,
    /// `None` until a usable certificate loads.
    pub certificate: Option<CertificateInfo>,
    /// Set when certificates come from an ACME CA.
    pub acme: Option<AcmeStatus>,
    pub passkeys: Vec<PasskeySummary>,
}

#[derive(Clone, PartialEq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
#[cfg_attr(test, derive(ts_rs::TS))]
pub enum TunnelState {
    Connecting,
    Up,
    Down {
        error: String,
    },
    /// Another connection registered this hostname.
    Replaced,
    /// Couldn't start, as with an unusable relay CA; no retries.
    Failed {
        error: String,
    },
}

#[derive(Clone, PartialEq, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct CertificateInfo {
    /// The issuer's organization, else its common name.
    pub issuer: String,
    pub self_signed: bool,
    /// Unix seconds.
    #[cfg_attr(test, ts(type = "number"))]
    pub not_after: u64,
}

#[derive(Clone, PartialEq, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct AcmeStatus {
    /// The CA, by name when known.
    pub directory: String,
    /// The last attempt's failure, until one succeeds.
    pub error: Option<String>,
    /// When the certificate is to be renewed (unix seconds).
    #[cfg_attr(test, ts(type = "number | null"))]
    pub renews_at: Option<u64>,
    /// The registration changed; renewal waits for a restart.
    pub stopped: bool,
}

/// The current status, broadcast whenever it changes.
pub struct Status {
    current: Mutex<ConnectStatus>,
    events: Option<broadcast::Sender<String>>,
    /// The newest passkey list's revision (`publish_passkeys`).
    passkeys_revision: Mutex<u64>,
}

impl Status {
    pub fn new(
        registration: &Registration,
        events: Option<broadcast::Sender<String>>,
    ) -> Arc<Self> {
        let certificate = super::acme::certified_key(&registration.cert, &registration.key)
            .ok()
            .and_then(|_| certificate_info(&registration.cert));
        Arc::new(Self {
            current: Mutex::new(ConnectStatus {
                hostname: registration.hostname.clone(),
                public_url: registration.public_url.clone(),
                relay: registration.relay_name.clone(),
                tunnel: TunnelState::Connecting,
                certificate,
                acme: registration.acme.as_deref().map(|directory| AcmeStatus {
                    directory: directory_name(directory),
                    error: None,
                    renews_at: None,
                    stopped: false,
                }),
                passkeys: Vec::new(),
            }),
            events,
            passkeys_revision: Mutex::default(),
        })
    }

    pub fn get(&self) -> ConnectStatus {
        self.current.lock().unwrap().clone()
    }

    pub fn events(&self) -> Option<&broadcast::Sender<String>> {
        self.events.as_ref()
    }

    /// Applies `change`, broadcasting the result if it changed anything.
    pub fn update(&self, change: impl FnOnce(&mut ConnectStatus)) {
        let mut current = self.current.lock().unwrap();
        let before = current.clone();
        change(&mut current);
        if *current == before {
            return;
        }
        if let Some(events) = &self.events {
            let status = current.clone();
            let _ = events.send(serde_json::to_string(&ServerMessage::Connect { status }).unwrap());
        }
    }

    /// Shows the passkeys as of `revision`, unless a later list was shown
    /// already: publishers number their lists in the order of changes, but
    /// may arrive out of it.
    pub fn publish_passkeys(&self, revision: u64, passkeys: Vec<PasskeySummary>) {
        let mut newest = self.passkeys_revision.lock().unwrap();
        if revision <= *newest {
            return;
        }
        *newest = revision;
        self.update(|s| s.passkeys = passkeys);
    }

    /// Applies `change` to the ACME status, if certificates come from a CA.
    pub fn update_acme(&self, change: impl FnOnce(&mut AcmeStatus)) {
        self.update(|status| {
            if let Some(acme) = &mut status.acme {
                change(acme);
            }
        });
    }
}

fn directory_name(directory: &str) -> String {
    match directory {
        super::acme::LETS_ENCRYPT => "Let's Encrypt".into(),
        super::acme::LETS_ENCRYPT_STAGING => "Let's Encrypt (staging)".into(),
        other => other.into(),
    }
}

/// The leaf certificate's issuer and expiry.
pub fn certificate_info(cert_pem: &str) -> Option<CertificateInfo> {
    use rustls::pki_types::{pem::PemObject, CertificateDer};
    let der = CertificateDer::pem_slice_iter(cert_pem.as_bytes())
        .next()?
        .ok()?;
    let (_, cert) = x509_parser::parse_x509_certificate(&der).ok()?;
    let issuer = cert.issuer();
    let name = issuer
        .iter_organization()
        .chain(issuer.iter_common_name())
        .find_map(|attribute| attribute.as_str().ok())
        .unwrap_or_default()
        .to_owned();
    Some(CertificateInfo {
        issuer: name,
        self_signed: cert.issuer() == cert.subject(),
        not_after: cert.validity().not_after.timestamp().max(0) as u64,
    })
}

/// Unix seconds.
pub fn seconds(time: SystemTime) -> u64 {
    time.duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |since| since.as_secs())
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(in crate::connect) fn registration(cert: String, key: String) -> Registration {
        Registration {
            relay: "relay.test:443".into(),
            relay_name: "relay.test".into(),
            relay_ca: String::new(),
            hostname: "laptop.test".into(),
            public_url: "https://laptop.test".into(),
            token: "t".into(),
            cert,
            key,
            acme: Some(super::super::acme::LETS_ENCRYPT.into()),
            issued_to: None,
        }
    }

    #[test]
    fn broadcasts_only_changes() {
        let cert = rcgen::generate_simple_self_signed(vec!["laptop.test".into()]).unwrap();
        let (events, mut received) = broadcast::channel(8);
        let status = Status::new(
            &registration(cert.cert.pem(), cert.signing_key.serialize_pem()),
            Some(events),
        );
        let initial = status.get();
        assert!(initial.certificate.as_ref().unwrap().self_signed);
        assert_eq!(initial.acme.as_ref().unwrap().directory, "Let's Encrypt");

        status.update(|s| s.tunnel = TunnelState::Up);
        status.update(|s| s.tunnel = TunnelState::Up);
        status.update_acme(|acme| acme.error = Some("refused".into()));
        let first: serde_json::Value = serde_json::from_str(&received.try_recv().unwrap()).unwrap();
        assert_eq!(first["type"], "connect");
        assert_eq!(first["status"]["tunnel"]["state"], "up");
        let second: serde_json::Value =
            serde_json::from_str(&received.try_recv().unwrap()).unwrap();
        assert_eq!(second["status"]["acme"]["error"], "refused");
        assert!(received.try_recv().is_err(), "an unchanged update was sent");
    }

    /// A publisher that numbered its list first but arrives last doesn't
    /// bring back a removed passkey.
    #[test]
    fn older_passkey_lists_arriving_late_are_ignored() {
        let status = Status::new(&registration(String::new(), String::new()), None);
        let passkey = |id: &str| PasskeySummary {
            id: id.into(),
            label: "Phone".into(),
            created: 1,
            last_used: None,
        };
        // Revision 1 lists X; revision 2, after X's removal, doesn't.
        status.publish_passkeys(2, vec![passkey("y")]);
        status.publish_passkeys(1, vec![passkey("x"), passkey("y")]);
        assert_eq!(status.get().passkeys, vec![passkey("y")]);
        status.publish_passkeys(3, vec![]);
        assert!(status.get().passkeys.is_empty());
    }

    #[test]
    fn unusable_certificates_have_no_info() {
        let status = Status::new(&registration(String::new(), String::new()), None);
        assert!(status.get().certificate.is_none());
    }
}
