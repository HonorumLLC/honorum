//! Sealing account credentials: Canvas tokens, calendar feed URLs, and
//! Google refresh tokens, with AES-256-GCM under `HONORUM_CANVAS_KEY`.
//!
//! A sealed value is `nonce || ciphertext` (12-byte nonce). The associated
//! data is `account \0 connection \0 scope`, so a row cannot be moved to
//! another account, connection, or purpose. Values sealed by earlier
//! releases must keep opening, so the layout and associated data are fixed.

use aes_gcm::aead::{Aead, KeyInit, Payload};
use aes_gcm::{Aes256Gcm, Nonce};
use base64::Engine;
use rand::RngExt;

use crate::Server;

/// A 32-byte AES-256 key.
#[derive(Clone)]
pub(crate) struct Key(pub [u8; 32]);

impl std::fmt::Debug for Key {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Key(..)")
    }
}

/// Decodes `HONORUM_CANVAS_KEY`: empty yields `None`; anything but
/// standard base64 of exactly 32 bytes fails with "HONORUM_CANVAS_KEY must
/// be a base64-encoded 32-byte key".
pub(crate) fn decode_canvas_key(value: &str) -> anyhow::Result<Option<Key>> {
    if value.is_empty() {
        return Ok(None);
    }
    let bytes = base64::engine::general_purpose::STANDARD.decode(value).unwrap_or_default();
    let key: [u8; 32] =
        bytes.try_into().map_err(|_| anyhow::anyhow!("HONORUM_CANVAS_KEY must be a base64-encoded 32-byte key"))?;
    Ok(Some(Key(key)))
}

impl Server {
    /// Reports whether account integrations (Canvas, feeds, Google) can
    /// store credentials: a 32-byte key is configured.
    pub(crate) fn integrations_available(&self) -> bool {
        self.canvas_key.is_some()
    }

    /// Seals `secret` for one account connection and scope. Fails with
    /// "account integrations are not configured" without a key.
    pub(crate) fn seal(&self, account: &str, id: &str, scope: &str, secret: &str) -> anyhow::Result<Vec<u8>> {
        let aead = self.cipher()?;
        let nonce: [u8; NONCE_SIZE] = rand::rng().random();
        let aad = associated_data(account, id, scope);
        let sealed = aead
            .encrypt(&Nonce::from(nonce), Payload { msg: secret.as_bytes(), aad: aad.as_bytes() })
            .map_err(|_| anyhow::anyhow!("could not seal the credential"))?;
        let mut out = Vec::with_capacity(NONCE_SIZE + sealed.len());
        out.extend_from_slice(&nonce);
        out.extend_from_slice(&sealed);
        Ok(out)
    }

    /// Opens a value sealed by [`Server::seal`] with the same account,
    /// connection, and scope. Fails without a key, with "the connection has
    /// no credential" for a value shorter than a nonce, and on any
    /// authentication failure.
    pub(crate) fn open(&self, account: &str, id: &str, scope: &str, credential: &[u8]) -> anyhow::Result<String> {
        let aead = self.cipher()?;
        if credential.len() < NONCE_SIZE {
            anyhow::bail!("the connection has no credential");
        }
        let (nonce, sealed) = credential.split_at(NONCE_SIZE);
        let nonce: [u8; NONCE_SIZE] = nonce.try_into().expect("a nonce-sized slice");
        let aad = associated_data(account, id, scope);
        let plain = aead
            .decrypt(&Nonce::from(nonce), Payload { msg: sealed, aad: aad.as_bytes() })
            .map_err(|_| anyhow::anyhow!("cipher: message authentication failed"))?;
        // Secrets are always sealed as UTF-8; invalid bytes are replaced
        // rather than rejected.
        Ok(String::from_utf8_lossy(&plain).into_owned())
    }

    fn cipher(&self) -> anyhow::Result<Aes256Gcm> {
        let key = self.canvas_key.as_ref().ok_or_else(|| anyhow::anyhow!("account integrations are not configured"))?;
        Ok(Aes256Gcm::new(&key.0.into()))
    }
}

/// The standard GCM nonce length.
const NONCE_SIZE: usize = 12;

/// `account \0 id \0 scope`: binds a sealed value to its row and purpose.
fn associated_data(account: &str, id: &str, scope: &str) -> String {
    format!("{account}\0{id}\0{scope}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{TEST_CANVAS_KEY, test_server_with};

    /// A value sealed by an earlier release under the all-zero key, account
    /// "a", connection "c", and scope "https://canvas.example.edu", with the
    /// secret "token". Opening it proves the layout and associated data match
    /// production ciphertext.
    const SEALED_BY_EARLIER_RELEASE: &str = "AAECAwQFBgcICQoL/KRYN26V/7CH/XlmEs8OGXtnE3U0";

    #[tokio::test]
    async fn sealed_values_open_only_for_their_row() {
        let t = test_server_with(|cfg| cfg.canvas_key = TEST_CANVAS_KEY.into()).await;
        let s = &t.server;
        let sealed = s.seal("a", "c", "scope", "secret").unwrap();
        assert_eq!(sealed.len(), NONCE_SIZE + "secret".len() + 16);
        assert_eq!(s.open("a", "c", "scope", &sealed).unwrap(), "secret");
        assert!(s.open("b", "c", "scope", &sealed).is_err());
        assert!(s.open("a", "d", "scope", &sealed).is_err());
        assert!(s.open("a", "c", "other", &sealed).is_err());
        assert_eq!(s.open("a", "c", "scope", &[1, 2]).unwrap_err().to_string(), "the connection has no credential");
        let earlier = base64::engine::general_purpose::STANDARD.decode(SEALED_BY_EARLIER_RELEASE).unwrap();
        assert_eq!(s.open("a", "c", "https://canvas.example.edu", &earlier).unwrap(), "token");
    }

    #[tokio::test]
    async fn sealing_needs_a_key() {
        let t = crate::test_support::test_server().await;
        assert!(!t.server.integrations_available());
        assert_eq!(
            t.server.seal("a", "c", "s", "x").unwrap_err().to_string(),
            "account integrations are not configured"
        );
        assert!(decode_canvas_key("not-a-key").is_err());
        assert!(decode_canvas_key("").unwrap().is_none());
    }
}
