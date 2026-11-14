//! Relayed sign-in state that needs no server-side store: a session cookie
//! signed with HMAC-SHA256, and passkey ceremony state sealed with AES-GCM,
//! both under keys derived from one secret beside the state file
//! (`state.session-key`). So sessions survive restarts and upgrades, and
//! replacing the secret signs everyone out.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use aws_lc_rs::{aead, digest, hmac, rand};
use base64::Engine;

pub const SESSION_COOKIE: &str = "__Host-btmux_session";
pub const SESSION_LIFETIME: Duration = Duration::from_secs(7 * 24 * 60 * 60);
const VERSION: u8 = 1;

pub fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

fn b64() -> base64::engine::GeneralPurpose {
    base64::engine::general_purpose::URL_SAFE_NO_PAD
}

/// Identifies a credential in a cookie without carrying its full ID.
pub fn credential_digest(credential_id: &[u8]) -> [u8; 16] {
    let hash = digest::digest(&digest::SHA256, credential_id);
    hash.as_ref()[..16].try_into().unwrap()
}

/// A verified session: which passkey signed in, and when.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Session {
    pub credential: [u8; 16],
    pub issued: u64,
}

pub struct Keys {
    session: hmac::Key,
    ceremony: aead::LessSafeKey,
    /// Bound into every MAC and seal, so nothing carries over to another
    /// hostname's registration.
    rp_id: String,
}

impl Keys {
    fn derive(secret: &[u8], rp_id: &str) -> Self {
        let root = hmac::Key::new(hmac::HMAC_SHA256, secret);
        let derive = |label: &[u8]| hmac::sign(&root, label);
        let session = hmac::Key::new(hmac::HMAC_SHA256, derive(b"btmux session v1").as_ref());
        let ceremony = aead::LessSafeKey::new(
            aead::UnboundKey::new(&aead::AES_256_GCM, derive(b"btmux ceremony v1").as_ref())
                .unwrap(),
        );
        Self {
            session,
            ceremony,
            rp_id: rp_id.to_owned(),
        }
    }

    /// Loads the secret, creating it if missing.
    pub fn load(state_file: &Path, rp_id: &str) -> Result<Self, String> {
        let path = key_path(state_file);
        let secret = match crate::discovery::read_private_file(&path, 1024) {
            Ok(text) => b64()
                .decode(text.trim())
                .map_err(|e| format!("{}: {e}", path.display()))?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => write_secret(&path)?,
            Err(error) => return Err(format!("{}: {error}", path.display())),
        };
        if secret.len() < 32 {
            return Err(format!("{}: too short", path.display()));
        }
        Ok(Self::derive(&secret, rp_id))
    }

    /// Replaces the secret: every session and ceremony in flight ends.
    pub fn rotate(state_file: &Path, rp_id: &str) -> Result<Self, String> {
        let secret = write_secret(&key_path(state_file))?;
        Ok(Self::derive(&secret, rp_id))
    }

    fn mac_input(&self, payload: &[u8]) -> Vec<u8> {
        let mut input = Vec::with_capacity(32 + self.rp_id.len() + payload.len());
        input.extend_from_slice(b"btmux-session\0");
        input.extend_from_slice(self.rp_id.as_bytes());
        input.push(0);
        input.extend_from_slice(payload);
        input
    }

    /// `version | credential digest | issued | expires | mac`, base64url.
    pub fn sign(&self, credential: [u8; 16], issued: u64) -> String {
        let mut payload = Vec::with_capacity(1 + 16 + 16 + 32);
        payload.push(VERSION);
        payload.extend_from_slice(&credential);
        payload.extend_from_slice(&issued.to_be_bytes());
        payload.extend_from_slice(&(issued + SESSION_LIFETIME.as_secs()).to_be_bytes());
        let tag = hmac::sign(&self.session, &self.mac_input(&payload));
        payload.extend_from_slice(tag.as_ref());
        b64().encode(payload)
    }

    pub fn verify(&self, cookie: &str, now: u64) -> Option<Session> {
        let bytes = b64().decode(cookie).ok()?;
        if bytes.len() != 1 + 16 + 16 + 32 || bytes[0] != VERSION {
            return None;
        }
        let (payload, tag) = bytes.split_at(1 + 16 + 16);
        hmac::verify(&self.session, &self.mac_input(payload), tag).ok()?;
        let credential = payload[1..17].try_into().ok()?;
        let issued = u64::from_be_bytes(payload[17..25].try_into().ok()?);
        let expires = u64::from_be_bytes(payload[25..33].try_into().ok()?);
        (issued <= now + 60 && now < expires).then_some(Session { credential, issued })
    }

    fn aad(&self) -> aead::Aad<Vec<u8>> {
        let mut aad = b"btmux-ceremony\0".to_vec();
        aad.extend_from_slice(self.rp_id.as_bytes());
        aead::Aad::from(aad)
    }

    /// Seals `plaintext` for the browser to hand back; only this server can
    /// open it, and it can't be altered.
    pub fn seal(&self, plaintext: &[u8]) -> String {
        let mut nonce = [0u8; aead::NONCE_LEN];
        rand::fill(&mut nonce).expect("system randomness");
        let mut sealed = plaintext.to_vec();
        self.ceremony
            .seal_in_place_append_tag(
                aead::Nonce::assume_unique_for_key(nonce),
                self.aad(),
                &mut sealed,
            )
            .expect("AES-GCM seal");
        let mut out = nonce.to_vec();
        out.extend_from_slice(&sealed);
        b64().encode(out)
    }

    pub fn open(&self, sealed: &str) -> Option<Vec<u8>> {
        let bytes = b64().decode(sealed).ok()?;
        if bytes.len() < aead::NONCE_LEN {
            return None;
        }
        let (nonce, sealed) = bytes.split_at(aead::NONCE_LEN);
        let nonce = aead::Nonce::try_assume_unique_for_key(nonce).ok()?;
        let mut sealed = sealed.to_vec();
        let plaintext = self
            .ceremony
            .open_in_place(nonce, self.aad(), &mut sealed)
            .ok()?;
        Some(plaintext.to_vec())
    }
}

pub fn key_path(state_file: &Path) -> PathBuf {
    state_file.with_extension("session-key")
}

/// 32 random bytes as base64url, owner-only, replacing any previous secret
/// atomically.
fn write_secret(path: &Path) -> Result<Vec<u8>, String> {
    let mut secret = vec![0u8; 32];
    rand::fill(&mut secret).map_err(|_| "no system randomness".to_owned())?;
    super::write_private(path, b64().encode(&secret).as_bytes())?;
    Ok(secret)
}

/// Random bytes as URL-safe text, for codes and ceremony IDs.
pub fn random_id() -> String {
    let mut bytes = [0u8; 16];
    rand::fill(&mut bytes).expect("system randomness");
    b64().encode(bytes)
}

/// The cookie header for a new session.
pub fn session_cookie(value: &str) -> String {
    format!(
        "{SESSION_COOKIE}={value}; Path=/; Max-Age={}; HttpOnly; Secure; SameSite=Strict",
        SESSION_LIFETIME.as_secs()
    )
}

pub fn clear_session_cookie() -> String {
    format!("{SESSION_COOKIE}=; Path=/; Max-Age=0; HttpOnly; Secure; SameSite=Strict")
}

/// The session cookie's value from a request, if sent.
pub fn session_from(headers: &axum::http::HeaderMap) -> Option<&str> {
    headers
        .get_all(axum::http::header::COOKIE)
        .iter()
        .filter_map(|h| h.to_str().ok())
        .flat_map(|h| h.split(';'))
        .find_map(|cookie| {
            let (name, value) = cookie.trim().split_once('=')?;
            (name == SESSION_COOKIE).then_some(value)
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keys(rp_id: &str) -> Keys {
        Keys::derive(&[7u8; 32], rp_id)
    }

    #[test]
    fn sessions_verify_until_they_expire() {
        let keys = keys("laptop.btmux.test");
        let credential = credential_digest(b"credential");
        let cookie = keys.sign(credential, 1_000);
        let session = keys.verify(&cookie, 1_000).unwrap();
        assert_eq!(
            session,
            Session {
                credential,
                issued: 1_000
            }
        );
        let last = 1_000 + SESSION_LIFETIME.as_secs() - 1;
        assert!(keys.verify(&cookie, last).is_some());
        assert!(keys.verify(&cookie, last + 1).is_none());
        // Issued well in the future: not from this clock.
        assert!(keys.verify(&keys.sign(credential, 5_000), 1_000).is_none());
    }

    #[test]
    fn sessions_resist_tampering_and_other_keys() {
        let keys = keys("laptop.btmux.test");
        let cookie = keys.sign(credential_digest(b"credential"), 1_000);
        let mut bytes = b64().decode(&cookie).unwrap();
        bytes[5] ^= 1;
        assert!(keys.verify(&b64().encode(&bytes), 1_000).is_none());
        assert!(keys.verify("not base64!", 1_000).is_none());
        assert!(keys.verify("", 1_000).is_none());
        // Another hostname's registration, or a rotated secret.
        assert!(self::keys("other.btmux.test")
            .verify(&cookie, 1_000)
            .is_none());
        assert!(Keys::derive(&[8u8; 32], "laptop.btmux.test")
            .verify(&cookie, 1_000)
            .is_none());
    }

    #[test]
    fn sealed_state_opens_only_unaltered_here() {
        let keys = keys("laptop.btmux.test");
        let sealed = keys.seal(b"ceremony state");
        assert_eq!(keys.open(&sealed).unwrap(), b"ceremony state");
        assert_ne!(keys.seal(b"ceremony state"), sealed, "nonces repeat");
        let mut bytes = b64().decode(&sealed).unwrap();
        let last = bytes.len() - 1;
        bytes[last] ^= 1;
        assert!(keys.open(&b64().encode(&bytes)).is_none());
        assert!(self::keys("other.btmux.test").open(&sealed).is_none());
        assert!(keys.open("short").is_none());
    }

    #[test]
    fn secrets_are_private_and_rotate() {
        let dir = std::env::temp_dir().join(format!("btmux-session-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let state = dir.join("state.json");
        let first = Keys::load(&state, "laptop.btmux.test").unwrap();
        let cookie = first.sign(credential_digest(b"c"), 1_000);
        let again = Keys::load(&state, "laptop.btmux.test").unwrap();
        assert!(again.verify(&cookie, 1_000).is_some());
        let rotated = Keys::rotate(&state, "laptop.btmux.test").unwrap();
        assert!(rotated.verify(&cookie, 1_000).is_none());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(key_path(&state))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        std::fs::remove_dir_all(dir).unwrap();
    }
}
