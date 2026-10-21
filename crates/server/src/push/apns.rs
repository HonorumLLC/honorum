//! Apple Push Notification service delivery with a `.p8` signing key
//! (ES256 provider tokens).

use std::sync::{LazyLock, Mutex};
use std::time::Duration;

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use chrono::{DateTime, TimeDelta, Utc};
use p256::ecdsa::signature::Signer;
use p256::ecdsa::{Signature, SigningKey};
use p256::pkcs8::DecodePrivateKey;
use regex::Regex;

use super::{Alert, SendOutcome};
use crate::config::Apns;

/// APNs device tokens: 32 to 200 hex digits.
pub(crate) static APNS_TOKEN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[0-9a-fA-F]{32,200}$").expect("valid pattern"));

/// An APNs client for one key, team, and topic. `host` is Apple's
/// production or sandbox origin; tests point it at a fake.
pub(crate) struct ApnsSender {
    pub(crate) cfg: Apns,
    pub(crate) key: SigningKey,
    pub(crate) host: String,
    pub(crate) client: reqwest::Client,
    /// The current provider token and when it was issued.
    token: Mutex<Option<(String, DateTime<Utc>)>>,
}

impl ApnsSender {
    /// The sender for `cfg`: `None` when nothing is configured; an error
    /// when only some settings are present ("APNs needs HONORUM_APNS_KEY,
    /// HONORUM_APNS_KEY_ID, HONORUM_APNS_TEAM_ID, and HONORUM_APNS_TOPIC
    /// together"), when the key is not PEM ("HONORUM_APNS_KEY is not a PEM
    /// key"), or not a PKCS#8 P-256 key. Literal `\n` in the key reads as a
    /// newline.
    pub(crate) fn new(cfg: &Apns) -> anyhow::Result<Option<Self>> {
        if cfg.key.is_empty() && cfg.key_id.is_empty() && cfg.team_id.is_empty() && cfg.topic.is_empty() {
            return Ok(None);
        }
        if cfg.key.is_empty() || cfg.key_id.is_empty() || cfg.team_id.is_empty() || cfg.topic.is_empty() {
            anyhow::bail!(
                "APNs needs HONORUM_APNS_KEY, HONORUM_APNS_KEY_ID, HONORUM_APNS_TEAM_ID, and HONORUM_APNS_TOPIC together"
            );
        }
        let pem = cfg.key.replace("\\n", "\n");
        if !pem.contains("-----BEGIN ") {
            anyhow::bail!("HONORUM_APNS_KEY is not a PEM key");
        }
        let key = SigningKey::from_pkcs8_pem(pem.trim()).map_err(|e| anyhow::anyhow!("HONORUM_APNS_KEY: {e}"))?;
        let host = if cfg.sandbox { "https://api.sandbox.push.apple.com" } else { "https://api.push.apple.com" };
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(20))
            .build()
            .expect("the APNs client configuration is valid");
        Ok(Some(Self { cfg: cfg.clone(), key, host: host.to_owned(), client, token: Mutex::new(None) }))
    }

    /// A provider token issued at `now`, reused for 50 minutes (Apple's
    /// limit is an hour).
    pub(crate) fn jwt(&self, now: DateTime<Utc>) -> anyhow::Result<String> {
        let mut cached = self.token.lock().expect("APNs token lock");
        if let Some((token, issued)) = cached.as_ref()
            && now - *issued < TimeDelta::minutes(50)
        {
            return Ok(token.clone());
        }
        let encode =
            |v: &serde_json::Value| URL_SAFE_NO_PAD.encode(serde_json::to_vec(v).expect("JSON values serialize"));
        let signing = format!(
            "{}.{}",
            encode(&serde_json::json!({"alg": "ES256", "kid": self.cfg.key_id})),
            encode(&serde_json::json!({"iss": self.cfg.team_id, "iat": now.timestamp()})),
        );
        let signature: Signature = self.key.try_sign(signing.as_bytes())?;
        let token = format!("{signing}.{}", URL_SAFE_NO_PAD.encode(signature.to_bytes()));
        *cached = Some((token.clone(), now));
        Ok(token)
    }

    /// Delivers one alert to a device token. `gone` reports a token Apple no
    /// longer accepts (410, `BadDeviceToken`, `Unregistered`,
    /// `DeviceTokenNotForTopic`) or one that is malformed.
    pub(crate) async fn send(&self, device_token: &str, alert: &Alert, now: DateTime<Utc>) -> SendOutcome {
        if !APNS_TOKEN.is_match(device_token) {
            return (true, Err(anyhow::anyhow!("invalid device token")));
        }
        let token = match self.jwt(now) {
            Ok(token) => token,
            Err(err) => return (false, Err(err)),
        };
        let payload = serde_json::json!({
            "aps": {"alert": {"title": alert.title, "body": alert.body}, "sound": "default"},
            "url": alert.url,
        });
        let response = self
            .client
            .post(format!("{}/3/device/{}", self.host, device_token.to_lowercase()))
            .header("authorization", format!("bearer {token}"))
            .header("apns-topic", &self.cfg.topic)
            .header("apns-push-type", "alert")
            .header("apns-priority", "10")
            .header("apns-expiration", (now + TimeDelta::hours(1)).timestamp().to_string())
            .body(serde_json::to_vec(&payload).expect("JSON values serialize"))
            .send()
            .await;
        let response = match response {
            Ok(response) => response,
            Err(err) => return (false, Err(err.into())),
        };
        let status = response.status().as_u16();
        if status == 200 {
            return (false, Ok(()));
        }
        let mut body = response.bytes().await.unwrap_or_default().to_vec();
        body.truncate(4 << 10);
        let reason = reply_reason(&body);
        if status == 410 || matches!(reason.as_str(), "BadDeviceToken" | "Unregistered" | "DeviceTokenNotForTopic") {
            return (true, Err(anyhow::anyhow!("apns token gone: {status} {reason}")));
        }
        (false, Err(anyhow::anyhow!("apns failed: {status} {reason}")))
    }
}

/// The `reason` of an APNs error reply: the exact key, else a
/// case-insensitive match; empty when the body is not such a reply.
fn reply_reason(body: &[u8]) -> String {
    let Ok(serde_json::Value::Object(reply)) = serde_json::Deserializer::from_slice(body)
        .into_iter::<serde_json::Value>()
        .next()
        .unwrap_or(Ok(serde_json::Value::Null))
    else {
        return String::new();
    };
    let reason =
        reply.get("reason").or_else(|| reply.iter().find(|(k, _)| k.eq_ignore_ascii_case("reason")).map(|(_, v)| v));
    reason.and_then(|v| v.as_str()).unwrap_or_default().to_owned()
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use axum::http::{HeaderMap, StatusCode, Uri};
    use p256::pkcs8::EncodePrivateKey;

    use super::*;
    use crate::db::Sql;
    use crate::push::{PushTarget, push_target};
    use crate::session::{hash_token, new_token};
    use crate::test_support::test_server;

    fn test_apns_key() -> String {
        let mut scalar = [0u8; 32];
        let key = loop {
            rand::fill(&mut scalar);
            if let Ok(key) = p256::SecretKey::from_slice(&scalar) {
                break key;
            }
        };
        key.to_pkcs8_pem(Default::default()).unwrap().to_string()
    }

    #[tokio::test]
    async fn apns_delivery_and_registration() {
        assert!(ApnsSender::new(&Apns { key: test_apns_key(), ..Apns::default() }).is_err(), "partial configuration");
        let mut sender = ApnsSender::new(&Apns {
            key: test_apns_key(),
            key_id: "KEY1".into(),
            team_id: "TEAM1".into(),
            topic: "app.honorum.ios".into(),
            sandbox: false,
        })
        .unwrap()
        .unwrap();

        let seen: Arc<Mutex<Vec<String>>> = Arc::default();
        let status = Arc::new(Mutex::new(200u16));
        let app = axum::Router::new().fallback({
            let (seen, status) = (seen.clone(), status.clone());
            move |uri: Uri, headers: HeaderMap, body: axum::body::Bytes| async move {
                let payload: serde_json::Value = serde_json::from_slice(&body).unwrap_or_default();
                let header =
                    |name: &str| headers.get(name).and_then(|v| v.to_str().ok()).unwrap_or_default().to_owned();
                seen.lock().unwrap().push(format!(
                    "{}|{}|{}",
                    uri.path(),
                    header("apns-topic"),
                    header("apns-push-type")
                ));
                let auth = header("authorization");
                assert!(
                    auth.starts_with("bearer ") && auth["bearer ".len()..].split('.').count() == 3,
                    "provider token"
                );
                assert_eq!(payload["aps"]["alert"]["title"], "Seat open", "payload: {payload}");
                let code = *status.lock().unwrap();
                let body = if code == 200 { String::new() } else { r#"{"reason":"BadDeviceToken"}"#.to_owned() };
                (StatusCode::from_u16(code).unwrap(), body)
            }
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, app).await });
        sender.host = url.clone();
        sender.client = reqwest::Client::new();

        let now = Utc::now();
        let token = "ab".repeat(32);
        let alert = Alert { title: "Seat open".into(), body: "CS 101".into(), url: String::new() };
        let (gone, result) = sender.send(&token, &alert, now).await;
        assert!(!gone && result.is_ok(), "send: gone={gone} {result:?}");
        *status.lock().unwrap() = 400;
        let (gone, _) = sender.send(&token, &Alert { title: "Seat open".into(), ..Alert::default() }, now).await;
        assert!(gone, "bad token not reported as gone");
        {
            let seen = seen.lock().unwrap();
            assert_eq!(seen.len(), 2, "requests: {seen:?}");
            assert_eq!(seen[0], format!("/3/device/{token}|app.honorum.ios|alert"));
        }
        let (gone, _) = sender.send("not hex", &Alert::default(), now).await;
        assert!(gone, "invalid token accepted");
        let first = sender.jwt(now).unwrap();
        let second = sender.jwt(now + TimeDelta::minutes(1)).unwrap();
        assert_eq!(first, second, "provider token not reused");
        assert_ne!(first, sender.jwt(now + TimeDelta::minutes(50)).unwrap(), "provider token renewed");
        assert_eq!(
            push_target(&format!(r#"{{"apns":"{}"}}"#, token.to_uppercase())),
            Some(PushTarget::Apns(token.clone()))
        );
        assert!(matches!(push_target(r#"{"endpoint":"https://push.example/x"}"#), Some(PushTarget::Web(_))));

        let t = test_server().await;
        let s = &t.server;
        let bearer = new_token();
        s.db.exec("INSERT INTO accounts(id,phone,created_at) VALUES('a','+12015550123',0)", &[]).await.unwrap();
        s.db.exec(
            "INSERT INTO sessions(id,account,token_hash,name,created_at,last_seen) VALUES('d','a',$1,'Test',0,$2)",
            &[&hash_token(&bearer), &s.now().timestamp()],
        )
        .await
        .unwrap();
        let c = t.client().bearer(&bearer);
        let put = |body: String| {
            let c = c.clone();
            async move { c.put("/api/me/push", &serde_json::from_str(&body).unwrap()).await.status.as_u16() }
        };
        assert_eq!(put(format!(r#"{{"apns":"{token}"}}"#)).await, 503);
        let sender = Arc::new(sender);
        s.set_apns(Some(sender.clone()));
        assert_eq!(put(r#"{"apns":"zz"}"#.into()).await, 400);
        assert_eq!(put(format!(r#"{{"apns":"{}"}}"#, token.to_uppercase())).await, 204);
        let stored: String = s.db.fetch_one("SELECT push FROM sessions", &[]).await.unwrap().get(0);
        assert_eq!(stored, format!(r#"{{"apns":"{token}"}}"#));
        *status.lock().unwrap() = 200;
        let (gone, result) = s.deliver(&stored, &Alert { title: "Seat open".into(), ..Alert::default() }).await;
        assert!(!gone && result.is_ok(), "deliver: {gone} {result:?}");
    }
}
