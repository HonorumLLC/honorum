//! Phone sign-in, sign-out, and the hourly sweep of expired records.
//! Session lookup and the [`crate::session::Auth`] extractor are in
//! [`crate::session`].

use std::sync::Arc;

use axum::body::Body;
use axum::extract::State;
use axum::http::StatusCode;
use axum::http::header::SET_COOKIE;
use serde::Deserialize;
use serde_json::{Map, Value};

use crate::Server;
use crate::accounts::account_details;
use crate::analytics::METRICS_KEPT;
use crate::api::{ApiError, ApiResult, ClientIp, empty, fail, json, null_default, read_json};
use crate::db::Sql;
use crate::metrics::metric_day;
use crate::monitor::term_ended;
use crate::phone::{CODE_PATTERN, PHONE_PATTERN, fail_phone};
use crate::session::{Auth, MAX_DEVICES, SESSION_TTL, Session, hash_token, new_id, new_token};

/// The SMS terms version a code request must accept; requests naming any
/// other version are refused with "Review the SMS terms, then request a
/// code."
pub(crate) const SMS_CONSENT_VERSION: &str = "2026-09-08-v2";

/// The largest sign-in request body (4 KiB).
const AUTH_BODY_MAX: usize = 4 << 10;

/// A device name as stored: trimmed, at most 60 bytes, "Unknown device"
/// when empty. A cut never splits a character.
pub(crate) fn clean_device(name: &str) -> String {
    let name = name.trim();
    if name.is_empty() {
        return "Unknown device".into();
    }
    let mut end = name.len().min(60);
    while !name.is_char_boundary(end) {
        end -= 1;
    }
    name[..end].to_owned()
}

impl Server {
    /// Creates the account for `phone` if needed and a session for this
    /// device in one transaction, retiring expired sessions and the least
    /// recently used ones beyond [`crate::session::MAX_DEVICES`]. Returns the
    /// account details (see [`crate::accounts::account_details`]) and the new
    /// bearer token. Counts `signins`, and `signups` for a new account.
    pub(crate) async fn start_session(
        &self,
        phone: &str,
        device: &str,
    ) -> anyhow::Result<(Map<String, Value>, String)> {
        let mut client = self.db.client().await?;
        let tx = client.transaction().await?;
        let now = self.now().timestamp();
        let created = tx
            .exec(
                "INSERT INTO accounts(id,phone,created_at) VALUES($1,$2,$3) ON CONFLICT(phone) DO NOTHING",
                &[&new_id(), &phone, &now],
            )
            .await?;
        let account: String = tx.fetch_one("SELECT id FROM accounts WHERE phone=$1", &[&phone]).await?.try_get(0)?;
        // Serialize the device cap across API replicas for this account.
        tx.exec("UPDATE accounts SET seq=seq WHERE id=$1", &[&account]).await?;
        tx.exec(
            "DELETE FROM sessions WHERE account=$1 AND last_seen<=$2",
            &[&account, &(now - SESSION_TTL.as_secs() as i64)],
        )
        .await?;
        // A full device list must not lock its owner out. Retire the least
        // recently used sessions, leaving room for this device, in the same
        // transaction.
        tx.exec(
            "DELETE FROM sessions WHERE id IN (SELECT id FROM sessions WHERE account=$1 ORDER BY last_seen DESC,created_at DESC,id LIMIT 2147483647 OFFSET $2)",
            &[&account, &(MAX_DEVICES - 1)],
        )
        .await?;
        let (token, id) = (new_token(), new_id());
        tx.exec(
            "INSERT INTO sessions(id,account,token_hash,name,created_at,last_seen) VALUES($1,$2,$3,$4,$5,$6)",
            &[&id, &account, &hash_token(&token), &clean_device(device), &now, &now],
        )
        .await?;
        let se = Session { id, account, ..Session::default() };
        let out = account_details(&tx, &se, self.now()).await?;
        tx.commit().await?;
        self.count("signins", 1);
        if created > 0 {
            self.count("signups", 1);
        }
        Ok((out, token))
    }

    /// Deletes expired records: delivered deliveries older than 30 days,
    /// sessions past their TTL, SMS consents older than a year, shares idle
    /// 180 days, document history older than 30 days, metrics older than
    /// `METRICS_KEPT` days, and lapsed rate limits; then expires seat alerts
    /// and queues streak maintenance. Writes are logged, never fatal.
    ///
    /// Offline devices still need document tombstones, so those are kept.
    pub(crate) async fn sweep(&self) {
        let now = self.now().timestamp();
        self.exec_logged("DELETE FROM deliveries WHERE delivered_at<>0 AND due_at < $1", &[&(now - 30 * 86400)]).await;
        self.exec_logged("DELETE FROM sessions WHERE last_seen <= $1", &[&(now - SESSION_TTL.as_secs() as i64)]).await;
        self.exec_logged("DELETE FROM sms_consents WHERE consented_at <= $1", &[&(now - 365 * 86400)]).await;
        // Shared schedules: 180 days idle.
        self.exec_logged("DELETE FROM sync WHERE updated_at < $1", &[&(now - 180 * 86400)]).await;
        self.exec_logged("DELETE FROM doc_history WHERE archived_at < $1", &[&(now - 30 * 86400)]).await;
        let kept = metric_day(self.now() - chrono::Duration::days(METRICS_KEPT));
        self.exec_logged("DELETE FROM daily_metrics WHERE day < $1", &[&kept]).await;
        self.exec_logged("DELETE FROM rate_limits WHERE expires<=$1", &[&now]).await;
        self.expire_watches(now).await;
        self.freeze_streaks().await;
    }

    /// Removes seat alerts for terms that ended (see
    /// [`crate::monitor::term_ended`]) and any alert a year old (`now` in
    /// Unix seconds), which covers schools that publish no term dates,
    /// waking the affected accounts.
    pub(crate) async fn expire_watches(&self, now: i64) {
        const STALE: &str = r#"created_at < $1 OR EXISTS (SELECT 1 FROM terms t WHERE t.inst=watches.inst AND t.id=watches.term AND t."end"<>'' AND t."end"<$2)"#;
        let (cutoff, ended) = (now - 365 * 86400, term_ended(self.now()));
        let mut accounts = Vec::new();
        let query = format!("SELECT DISTINCT account FROM watches WHERE account<>'' AND ({STALE})");
        if let Ok(rows) = self.db.fetch_all(&query, &[&cutoff, &ended]).await {
            accounts.extend(rows.iter().filter_map(|row| row.try_get::<_, String>(0).ok()));
        }
        if let Err(err) = self.db.exec(&format!("DELETE FROM watches WHERE {STALE}"), &[&cutoff, &ended]).await {
            tracing::error!(%err, "expired seat alerts could not be removed");
            return;
        }
        for account in accounts {
            self.changed(&account);
        }
    }
}

#[derive(Default, Deserialize)]
#[serde(default)]
struct CodeRequest {
    #[serde(alias = "Phone", deserialize_with = "null_default")]
    phone: String,
    #[serde(alias = "Consent", deserialize_with = "null_default")]
    consent: String,
}

#[derive(Default, Deserialize)]
#[serde(default)]
struct VerifyRequest {
    #[serde(alias = "Phone", deserialize_with = "null_default")]
    phone: String,
    #[serde(alias = "Code", deserialize_with = "null_default")]
    code: String,
    #[serde(alias = "Device", deserialize_with = "null_default")]
    device: String,
}

/// Counts one request against a sign-in limit: 503 when limits cannot be
/// read, 429 with `Retry-After` and `message` when the budget is spent.
async fn limit(s: &Server, key: &str, budget: i32, period: i64, retry: u64, message: &str) -> ApiResult<()> {
    match s.rate_allow(key, budget, period).await {
        Err(_) => fail(503, "request limits unavailable; try again shortly"),
        Ok(false) => Err(ApiError::new(429, message).retry_after(retry)),
        Ok(true) => Ok(()),
    }
}

/// `POST /api/auth/code`: records SMS consent and sends a code (`phone`,
/// `consent`; 4 KiB). Rate limits: 100 per address per ten minutes, one per
/// number per 30 seconds, five per number per ten minutes. Answers 202
/// `{"retry_after": 30}`.
pub(crate) async fn handle_auth_code(State(s): State<Arc<Server>>, ClientIp(ip): ClientIp, body: Body) -> ApiResult {
    let input = match read_json::<CodeRequest>(body, AUTH_BODY_MAX).await {
        Some(input) if PHONE_PATTERN.is_match(&input.phone) => input,
        _ => return fail(400, "Enter a mobile number with its country code."),
    };
    if input.consent != SMS_CONSENT_VERSION {
        return fail(400, "Review the SMS terms, then request a code.");
    }
    let Some(verifier) = s.verifier() else {
        return fail(503, "Phone sign-in is not configured on this server.");
    };
    // Per-number limits prevent repeated texts. The wider address budget lets
    // students on a shared campus network sign in independently, and it
    // comes first so one address cannot fill the limiter with numbers.
    let many = "Too many codes requested. Try again in ten minutes.";
    limit(&s, &format!("sms-ip:{ip}"), 100, 600, 600, many).await?;
    let cooldown = "Wait thirty seconds before requesting another code.";
    limit(&s, &format!("sms-cooldown:{}", input.phone), 1, 30, 30, cooldown).await?;
    limit(&s, &format!("sms-number:{}", input.phone), 5, 600, 600, many).await?;
    // Save the explicit request before contacting the provider. A failed or
    // cancelled send still represents a consent event, never proof of delivery.
    let saved =
        s.db.exec(
            "INSERT INTO sms_consents(id,phone,consented_at,version) VALUES($1,$2,$3,$4)",
            &[&new_id(), &input.phone, &s.now().timestamp(), &input.consent],
        )
        .await;
    if let Err(err) = saved {
        tracing::error!(%err, "SMS consent could not be recorded");
        return fail(503, "Your code could not be requested. Try again shortly.");
    }
    verifier.send(&input.phone).await.map_err(fail_phone)?;
    s.count("codes", 1);
    Ok(json(StatusCode::ACCEPTED, &serde_json::json!({ "retry_after": 30 })))
}

/// `POST /api/auth/verify`: checks a code (`phone`, `code`, `device`; 4 KiB;
/// ten checks per number per ten minutes), starts a session, sets the
/// cookie, and returns the account details with `token`.
pub(crate) async fn handle_auth_verify(State(s): State<Arc<Server>>, body: Body) -> ApiResult {
    let input = match read_json::<VerifyRequest>(body, AUTH_BODY_MAX).await {
        Some(input) if PHONE_PATTERN.is_match(&input.phone) && CODE_PATTERN.is_match(&input.code) => input,
        _ => return fail(400, "Enter your mobile number and six-digit code."),
    };
    let Some(verifier) = s.verifier() else {
        return fail(503, "Phone sign-in is not configured on this server.");
    };
    let attempts = "Too many verification attempts. Try again in ten minutes.";
    limit(&s, &format!("sms-check:{}", input.phone), 10, 600, 600, attempts).await?;
    // Finish the provider call before opening the session transaction.
    verifier.check(&input.phone, &input.code).await.map_err(fail_phone)?;
    let (mut out, token) = match s.start_session(&input.phone, &input.device).await {
        Ok(started) => started,
        Err(err) => {
            tracing::error!(%err, "session creation failed");
            // The provider has consumed the code. Never claim it is still reusable.
            return fail(503, "Sign-in could not finish. Request another code and try again.");
        }
    };
    let cookie = s.session_cookie(&token, SESSION_TTL.as_secs() as i64);
    out.insert("token".into(), Value::String(token));
    let mut response = json(StatusCode::OK, &out);
    response.headers_mut().insert(SET_COOKIE, cookie);
    Ok(response)
}

/// `POST /api/auth/logout`: deletes this session and clears the cookie (204).
pub(crate) async fn handle_logout(State(s): State<Arc<Server>>, Auth(se): Auth) -> ApiResult {
    if s.db.exec("DELETE FROM sessions WHERE id=$1", &[&se.id]).await.is_err() {
        return fail(500, "could not sign out");
    }
    let mut response = empty(204);
    response.headers_mut().insert(SET_COOKIE, s.session_cookie("", -1));
    Ok(response)
}
