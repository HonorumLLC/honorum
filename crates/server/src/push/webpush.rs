//! Web Push: VAPID keys and message encryption (RFC 8291 `aes128gcm`) with
//! VAPID authorization (RFC 8292).
//!
//! VAPID keys stored in `kv` by earlier releases stay valid: keys use the
//! encoding described on [`VapidKeys`], and subscription and VAPID keys are
//! decoded leniently.

use aes_gcm::aead::Aead;
use aes_gcm::{Aes128Gcm, KeyInit};
use base64::Engine;
use base64::alphabet;
use base64::engine::DecodePaddingMode;
use base64::engine::general_purpose::{GeneralPurpose, GeneralPurposeConfig, URL_SAFE_NO_PAD};
use hkdf::Hkdf;
use p256::ecdsa::signature::Signer;
use p256::ecdsa::{Signature, SigningKey};
use p256::elliptic_curve::sec1::ToSec1Point;
use serde::Deserialize;
use sha2::Sha256;

use crate::config::Config;
use crate::db::{Db, Sql};

/// The VAPID key pair: the public key as unpadded URL-safe base64 of the
/// uncompressed P-256 point (65 bytes), the private key as unpadded
/// URL-safe base64 of the 32-byte scalar. Keys stored in `kv` use this
/// encoding.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct VapidKeys {
    pub(crate) public: String,
    pub(crate) private: String,
}

/// A browser's push subscription as the client serializes it.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct Subscription {
    #[serde(deserialize_with = "crate::api::null_default", alias = "Endpoint")]
    pub(crate) endpoint: String,
    #[serde(deserialize_with = "crate::api::null_default", alias = "Keys")]
    pub(crate) keys: SubscriptionKeys,
}

/// The subscription's client keys, unpadded URL-safe base64 (padded or
/// standard base64 is also accepted).
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct SubscriptionKeys {
    #[serde(deserialize_with = "crate::api::null_default", alias = "P256dh", alias = "P256DH")]
    pub(crate) p256dh: String,
    #[serde(deserialize_with = "crate::api::null_default", alias = "Auth")]
    pub(crate) auth: String,
}

/// The record size every message declares and is padded to.
pub(crate) const RECORD_SIZE: u32 = 4096;

/// The `aes128gcm` header length for a 65-byte key ID: salt (16), record
/// size (4), key ID length (1), and the key ID.
const HEADER_LEN: usize = 16 + 4 + 1 + 65;

/// Base64 that tolerates nonzero trailing bits and either padding.
const fn lenient(alphabet: &alphabet::Alphabet) -> GeneralPurpose {
    GeneralPurpose::new(
        alphabet,
        GeneralPurposeConfig::new()
            .with_decode_allow_trailing_bits(true)
            .with_decode_padding_mode(DecodePaddingMode::Indifferent),
    )
}
const STANDARD_LENIENT: GeneralPurpose = lenient(&alphabet::STANDARD);
const URL_LENIENT: GeneralPurpose = lenient(&alphabet::URL_SAFE);

/// Decodes a subscription key: padded to a multiple of four, then standard
/// base64, else URL-safe base64.
fn decode_subscription_key(key: &str) -> anyhow::Result<Vec<u8>> {
    let mut padded = key.to_owned();
    let rem = key.len() % 4;
    if rem != 0 {
        padded.push_str(&"=".repeat(4 - rem));
    }
    STANDARD_LENIENT
        .decode(&padded)
        .or_else(|_| URL_LENIENT.decode(&padded))
        .map_err(|e| anyhow::anyhow!("illegal base64 data in subscription key: {e}"))
}

/// Decodes a VAPID key: padded URL-safe base64, else unpadded.
fn decode_vapid_key(key: &str) -> anyhow::Result<Vec<u8>> {
    URL_LENIENT.decode(key).map_err(|e| anyhow::anyhow!("illegal base64 data in VAPID key: {e}"))
}

/// Encrypts one `aes128gcm` record (RFC 8188, keyed per RFC 8291) for the
/// receiver's public key `p256dh` and `auth` secret, using the sender's
/// ephemeral key `local`. The plaintext is `message`, the `0x02` final
/// record delimiter, and zeros up to `padded_len` bytes. Returns the header
/// (salt, `record_size`, key ID = `local`'s uncompressed public key)
/// followed by the ciphertext. Fails when `p256dh` is not an uncompressed
/// P-256 point or the message does not fit in `padded_len`.
pub(crate) fn encrypt_record(
    message: &[u8],
    p256dh: &[u8],
    auth: &[u8],
    salt: &[u8; 16],
    local: &p256::SecretKey,
    record_size: u32,
    padded_len: usize,
) -> anyhow::Result<Vec<u8>> {
    if p256dh.len() != 65 || p256dh[0] != 4 {
        anyhow::bail!("Unmarshal Error: Public key is not a valid point on the curve");
    }
    let receiver = p256::PublicKey::from_sec1_bytes(p256dh)
        .map_err(|_| anyhow::anyhow!("Unmarshal Error: Public key is not a valid point on the curve"))?;
    let local_public = local.public_key().to_sec1_point(false);
    let local_public = local_public.as_bytes();
    let shared = p256::ecdh::diffie_hellman(local.to_nonzero_scalar(), receiver.as_affine());

    let mut key_info = b"WebPush: info\x00".to_vec();
    key_info.extend_from_slice(p256dh);
    key_info.extend_from_slice(local_public);
    let mut ikm = [0u8; 32];
    Hkdf::<Sha256>::new(Some(auth), shared.raw_secret_bytes().as_slice())
        .expand(&key_info, &mut ikm)
        .map_err(|_| anyhow::anyhow!("hkdf expansion failed"))?;
    let content = Hkdf::<Sha256>::new(Some(salt), &ikm);
    let mut cek = [0u8; 16];
    let mut nonce = [0u8; 12];
    content
        .expand(b"Content-Encoding: aes128gcm\x00", &mut cek)
        .and_then(|()| content.expand(b"Content-Encoding: nonce\x00", &mut nonce))
        .map_err(|_| anyhow::anyhow!("hkdf expansion failed"))?;

    if message.len() + 1 > padded_len {
        anyhow::bail!("payload has exceeded the maximum length");
    }
    let mut plaintext = Vec::with_capacity(padded_len);
    plaintext.extend_from_slice(message);
    plaintext.push(2);
    plaintext.resize(padded_len, 0);
    let cipher = Aes128Gcm::new(&cek.into());
    let sealed = cipher
        .encrypt(&nonce.into(), plaintext.as_slice())
        .map_err(|_| anyhow::anyhow!("push message encryption failed"))?;

    let mut body = Vec::with_capacity(HEADER_LEN + sealed.len());
    body.extend_from_slice(salt);
    body.extend_from_slice(&record_size.to_be_bytes());
    body.push(local_public.len() as u8);
    body.extend_from_slice(local_public);
    body.extend_from_slice(&sealed);
    Ok(body)
}

/// A random P-256 secret key.
fn random_secret() -> p256::SecretKey {
    let mut scalar = [0u8; 32];
    loop {
        rand::fill(&mut scalar);
        if let Ok(secret) = p256::SecretKey::from_slice(&scalar) {
            return secret;
        }
    }
}

/// The VAPID `Authorization` value for `endpoint` (RFC 8292): an ES256 JWT
/// with the endpoint's origin as audience, `exp` 12 hours after `now_unix`,
/// and `subscriber` as subject, plus the public key.
///
/// A subject that is neither an `https:` URL nor a `mailto:` address gets a
/// `mailto:` prefix; a `mailto:` address is used as is.
fn vapid_authorization(endpoint: &str, subscriber: &str, keys: &VapidKeys, now_unix: i64) -> anyhow::Result<String> {
    let url = url::Url::parse(endpoint)?;
    let host = match (url.host_str(), url.port()) {
        (Some(host), Some(port)) => format!("{host}:{port}"),
        (Some(host), None) => host.to_owned(),
        (None, _) => String::new(),
    };
    let subscriber = if subscriber.starts_with("https:") || subscriber.starts_with("mailto:") {
        subscriber.to_owned()
    } else {
        format!("mailto:{subscriber}")
    };
    let encode = |v: &serde_json::Value| URL_SAFE_NO_PAD.encode(serde_json::to_vec(v).expect("JSON values serialize"));
    let signing = format!(
        "{}.{}",
        encode(&serde_json::json!({"alg": "ES256", "typ": "JWT"})),
        encode(
            &serde_json::json!({"aud": format!("{}://{host}", url.scheme()), "exp": now_unix + 12 * 3600, "sub": subscriber})
        ),
    );
    let private = decode_vapid_key(&keys.private)?;
    let key = SigningKey::from_slice(&private).map_err(|e| anyhow::anyhow!("invalid VAPID private key: {e}"))?;
    let signature: Signature = key.sign(signing.as_bytes());
    let public = decode_vapid_key(&keys.public)?;
    Ok(format!(
        "vapid t={signing}.{}, k={}",
        URL_SAFE_NO_PAD.encode(signature.to_bytes()),
        URL_SAFE_NO_PAD.encode(public)
    ))
}

/// Generates a VAPID key pair.
pub(crate) fn generate_vapid_keys() -> VapidKeys {
    let mut scalar = [0u8; 32];
    loop {
        rand::fill(&mut scalar);
        if let Ok(secret) = p256::SecretKey::from_slice(&scalar) {
            let public = secret.public_key().to_sec1_point(false);
            return VapidKeys {
                public: URL_SAFE_NO_PAD.encode(public.as_bytes()),
                private: URL_SAFE_NO_PAD.encode(scalar),
            };
        }
    }
}

/// The server's VAPID keys: the configured pair, or the pair stored in
/// `kv` (`vapid_pub`, `vapid_priv`), generating and storing one under an
/// advisory transaction lock when there is none. A stored pair with one
/// half missing fails with "stored VAPID key pair is incomplete".
pub(crate) async fn initialize(db: &Db, cfg: &Config) -> anyhow::Result<VapidKeys> {
    if !cfg.vapid_public.is_empty() {
        return Ok(VapidKeys { public: cfg.vapid_public.clone(), private: cfg.vapid_private.clone() });
    }
    let mut client = db.client().await?;
    let tx = client.transaction().await?;
    tx.exec("SELECT pg_advisory_xact_lock(hashtextextended('vapid-keys',0))", &[]).await?;
    let mut keys = VapidKeys::default();
    for (key, value) in [("vapid_pub", &mut keys.public), ("vapid_priv", &mut keys.private)] {
        if let Some(row) = tx.fetch_opt("SELECT v FROM kv WHERE k=$1", &[&key]).await? {
            *value = row.try_get::<_, Option<String>>(0)?.unwrap_or_default();
        }
    }
    if keys.public.is_empty() != keys.private.is_empty() {
        anyhow::bail!("stored VAPID key pair is incomplete");
    }
    let generated = keys.public.is_empty();
    if generated {
        keys = generate_vapid_keys();
        tx.exec(
            "INSERT INTO kv(k,v) VALUES('vapid_pub',$1),('vapid_priv',$2) ON CONFLICT(k) DO UPDATE SET v=excluded.v",
            &[&keys.public, &keys.private],
        )
        .await?;
    }
    tx.commit().await?;
    if generated {
        tracing::warn!(
            public = %keys.public,
            "generated new VAPID keys; set HONORUM_VAPID_PUBLIC/PRIVATE to keep them across database resets"
        );
    }
    Ok(keys)
}

/// Encrypts `payload` for `subscription` and posts it with a VAPID JWT
/// (subject `contact`, 12-hour expiry) and `TTL: ttl`. Returns the push
/// service's status; only transport and key failures are errors.
pub(crate) async fn send_notification(
    client: &reqwest::Client,
    payload: &[u8],
    subscription: &Subscription,
    contact: &str,
    keys: &VapidKeys,
    ttl: u32,
) -> anyhow::Result<u16> {
    let auth = decode_subscription_key(&subscription.keys.auth)?;
    let p256dh = decode_subscription_key(&subscription.keys.p256dh)?;
    let mut salt = [0u8; 16];
    rand::fill(&mut salt);
    let padded_len = RECORD_SIZE as usize - 16 - HEADER_LEN;
    let body = encrypt_record(payload, &p256dh, &auth, &salt, &random_secret(), RECORD_SIZE, padded_len)?;
    // The JWT expiry follows the wall clock, not the server's clock.
    let authorization = vapid_authorization(&subscription.endpoint, contact, keys, chrono::Utc::now().timestamp())?;
    let response = client
        .post(&subscription.endpoint)
        .header("content-encoding", "aes128gcm")
        .header("content-type", "application/octet-stream")
        .header("ttl", ttl.to_string())
        .header("authorization", authorization)
        .body(body)
        .send()
        .await?;
    Ok(response.status().as_u16())
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use aes_gcm::aead::Aead;
    use aes_gcm::{Aes128Gcm, KeyInit};
    use axum::body::Bytes;
    use axum::http::HeaderMap;
    use base64::Engine;
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;
    use hkdf::Hkdf;
    use p256::ecdsa::signature::Verifier;
    use p256::ecdsa::{Signature, VerifyingKey};
    use p256::elliptic_curve::sec1::ToSec1Point;
    use sha2::Sha256;

    use super::*;

    fn b64(s: &str) -> Vec<u8> {
        URL_SAFE_NO_PAD.decode(s).unwrap()
    }

    /// Decrypts a single-record `aes128gcm` body as a user agent would,
    /// returning the message without its delimiter and padding.
    fn decrypt(body: &[u8], receiver: &p256::SecretKey, auth: &[u8]) -> Vec<u8> {
        let salt = &body[..16];
        let record_size = u32::from_be_bytes(body[16..20].try_into().unwrap());
        assert_eq!(record_size, RECORD_SIZE);
        let id_len = body[20] as usize;
        let sender = p256::PublicKey::from_sec1_bytes(&body[21..21 + id_len]).unwrap();
        let ciphertext = &body[21 + id_len..];
        let shared = p256::ecdh::diffie_hellman(receiver.to_nonzero_scalar(), sender.as_affine());
        let mut info = b"WebPush: info\x00".to_vec();
        info.extend_from_slice(receiver.public_key().to_sec1_point(false).as_bytes());
        info.extend_from_slice(&body[21..21 + id_len]);
        let mut ikm = [0u8; 32];
        Hkdf::<Sha256>::new(Some(auth), shared.raw_secret_bytes().as_slice()).expand(&info, &mut ikm).unwrap();
        let content = Hkdf::<Sha256>::new(Some(salt), &ikm);
        let (mut cek, mut nonce) = ([0u8; 16], [0u8; 12]);
        content.expand(b"Content-Encoding: aes128gcm\x00", &mut cek).unwrap();
        content.expand(b"Content-Encoding: nonce\x00", &mut nonce).unwrap();
        let mut plain = Aes128Gcm::new(&cek.into()).decrypt(&nonce.into(), ciphertext).unwrap();
        let end = plain.iter().rposition(|b| *b != 0).unwrap();
        assert_eq!(plain[end], 2, "final record delimiter");
        plain.truncate(end);
        plain
    }

    #[test]
    fn rfc8291_example_encrypts_identically() {
        // RFC 8291 section 5: the example message, keys, and salt, with no padding.
        let message = b"When I grow up, I want to be a watermelon";
        let sender = p256::SecretKey::from_slice(&b64("yfWPiYE-n46HLnH0KqZOF1fJJU3MYrct3AELtAQ-oRw")).unwrap();
        assert_eq!(
            URL_SAFE_NO_PAD.encode(sender.public_key().to_sec1_point(false).as_bytes()),
            "BP4z9KsN6nGRTbVYI_c7VJSPQTBtkgcy27mlmlMoZIIgDll6e3vCYLocInmYWAmS6TlzAC8wEqKK6PBru3jl7A8"
        );
        let receiver = p256::SecretKey::from_slice(&b64("q1dXpw3UpT5VOmu_cf_v6ih07Aems3njxI-JWgLcM94")).unwrap();
        let receiver_public =
            b64("BCVxsr7N_eNgVRqvHtD0zTZsEc6-VV-JvLexhqUzORcxaOzi6-AYWXvTBHm4bjyPjs7Vd8pZGH6SRpkNtoIAiw4");
        assert_eq!(receiver.public_key().to_sec1_point(false).as_bytes(), receiver_public.as_slice());
        let auth = b64("BTBZMqHH6r4Tts7J_aSIgg");
        let salt: [u8; 16] = b64("DGv6ra1nlYgDCS1FRnbzlw").try_into().unwrap();
        let body = encrypt_record(message, &receiver_public, &auth, &salt, &sender, 4096, message.len() + 1).unwrap();
        assert_eq!(
            URL_SAFE_NO_PAD.encode(&body),
            "DGv6ra1nlYgDCS1FRnbzlwAAEABBBP4z9KsN6nGRTbVYI_c7VJSPQTBtkgcy27mlmlMoZIIgDll6e3vCYLocInmYWAmS6TlzAC8wEqKK6PBru3jl7A_yl95bQpu6cVPTpK4Mqgkf1CXztLVBSt2Ks3oZwbuwXPXLWyouBWLVWGNWQexSgSxsj_Qulcy4a-fN"
        );
        assert_eq!(decrypt_unpadded_rs(&body, &receiver, &auth), message);
    }

    /// [`decrypt`] for a body whose record size need not be [`RECORD_SIZE`].
    fn decrypt_unpadded_rs(body: &[u8], receiver: &p256::SecretKey, auth: &[u8]) -> Vec<u8> {
        decrypt(body, receiver, auth)
    }

    #[test]
    fn messages_pad_to_the_record_size_and_overflow_fails() {
        let receiver = random_secret();
        let public = receiver.public_key().to_sec1_point(false);
        let auth = [7u8; 16];
        let padded = RECORD_SIZE as usize - 16 - HEADER_LEN;
        let body = encrypt_record(
            b"{\"title\":\"x\"}",
            public.as_bytes(),
            &auth,
            &[1; 16],
            &random_secret(),
            RECORD_SIZE,
            padded,
        )
        .unwrap();
        assert_eq!(body.len(), RECORD_SIZE as usize);
        assert_eq!(decrypt(&body, &receiver, &auth), b"{\"title\":\"x\"}");
        let too_long = vec![b'a'; padded];
        let err = encrypt_record(&too_long, public.as_bytes(), &auth, &[1; 16], &random_secret(), RECORD_SIZE, padded)
            .unwrap_err();
        assert_eq!(err.to_string(), "payload has exceeded the maximum length");
        assert!(encrypt_record(b"x", &[4; 33], &auth, &[1; 16], &random_secret(), RECORD_SIZE, padded).is_err());
    }

    #[test]
    fn subscription_keys_decode_in_every_base64_form() {
        let bytes: Vec<u8> = (0u8..65).collect();
        for engine in [
            &base64::engine::general_purpose::STANDARD,
            &base64::engine::general_purpose::STANDARD_NO_PAD,
            &base64::engine::general_purpose::URL_SAFE,
            &base64::engine::general_purpose::URL_SAFE_NO_PAD,
        ] {
            assert_eq!(decode_subscription_key(&engine.encode(&bytes)).unwrap(), bytes);
        }
    }

    #[tokio::test]
    async fn notifications_post_encrypted_bodies_with_vapid_authorization() {
        // A key pair in the form stored in kv by earlier releases.
        let keys = VapidKeys {
            private: "Z4nB5ugpyozUjmVVS5ejwpLY9jqzwQIEcumd9o7-rNg".into(),
            public: "BNvUqFLleFDHxWZ-Y1i3aSC2L5vLhIJ8ah3KnCVEbeRDIc1iL7zfuUmOm85W5OKNGpEVwyqE8h4OZF3do-Fsll4".into(),
        };
        let seen: Arc<Mutex<Option<(HeaderMap, Bytes)>>> = Arc::default();
        let app = axum::Router::new().route(
            "/push/abc",
            axum::routing::post({
                let seen = seen.clone();
                move |headers: HeaderMap, body: Bytes| async move {
                    *seen.lock().unwrap() = Some((headers, body));
                    axum::http::StatusCode::CREATED
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await });

        let receiver = random_secret();
        let auth = [9u8; 16];
        let subscription: Subscription = serde_json::from_value(serde_json::json!({
            "endpoint": format!("http://{addr}/push/abc"),
            "keys": {
                "p256dh": URL_SAFE_NO_PAD.encode(receiver.public_key().to_sec1_point(false).as_bytes()),
                "auth": base64::engine::general_purpose::STANDARD.encode(auth),
            },
        }))
        .unwrap();
        let status = send_notification(&reqwest::Client::new(), b"hello", &subscription, "mailto:t@x", &keys, 3600)
            .await
            .unwrap();
        assert_eq!(status, 201);

        let (headers, body) = seen.lock().unwrap().take().unwrap();
        let header = |name: &str| headers.get(name).unwrap().to_str().unwrap().to_owned();
        assert_eq!(header("ttl"), "3600");
        assert_eq!(header("content-encoding"), "aes128gcm");
        assert_eq!(header("content-type"), "application/octet-stream");
        assert!(headers.get("urgency").is_none() && headers.get("topic").is_none());
        assert_eq!(body.len(), RECORD_SIZE as usize);
        assert_eq!(decrypt(&body, &receiver, &auth), b"hello");

        let authorization = header("authorization");
        let rest = authorization.strip_prefix("vapid t=").unwrap();
        let (jwt, k) = rest.split_once(", k=").unwrap();
        assert_eq!(k, keys.public);
        let parts: Vec<&str> = jwt.split('.').collect();
        assert_eq!(parts.len(), 3);
        let head: serde_json::Value = serde_json::from_slice(&b64(parts[0])).unwrap();
        assert_eq!(head, serde_json::json!({"alg": "ES256", "typ": "JWT"}));
        let claims: serde_json::Value = serde_json::from_slice(&b64(parts[1])).unwrap();
        assert_eq!(claims["aud"], format!("http://{addr}"));
        // A `mailto:` subject is used as is.
        assert_eq!(claims["sub"], "mailto:t@x");
        let exp = claims["exp"].as_i64().unwrap() - chrono::Utc::now().timestamp();
        assert!((12 * 3600 - 60..=12 * 3600).contains(&exp), "exp in {exp}s");
        let verifier = VerifyingKey::from_sec1_bytes(&b64(&keys.public)).unwrap();
        let signature = Signature::from_slice(&b64(parts[2])).unwrap();
        verifier.verify(format!("{}.{}", parts[0], parts[1]).as_bytes(), &signature).unwrap();
    }
}
