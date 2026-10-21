//! Device sessions: reading a request's credential, and the [`Auth`]
//! extractor for handlers that require a signed-in account.
//!
//! Browsers carry an HttpOnly cookie and must name the account they expect in
//! `X-Honorum-Account`, so one tab cannot act on an account another tab chose.
//! Native clients send `Authorization: Bearer <token>` and may omit the
//! header; if they send it, it must match.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::extract::FromRequestParts;
use axum::http::header::{AUTHORIZATION, COOKIE};
use axum::http::request::Parts;
use axum::http::{HeaderMap, HeaderValue};
use base64::Engine;
use rand::RngExt;
use sha2::{Digest, Sha256};
use tokio_util::sync::CancellationToken;

use crate::Server;
use crate::api::ApiError;
use crate::db::Sql;

pub(crate) const SESSION_COOKIE: &str = "hn_session";
/// Sessions expire after a year without use; the cookie lasts a year.
pub(crate) const SESSION_TTL: Duration = Duration::from_secs(365 * 24 * 3600);
/// The most sessions one account keeps; a new sign-in retires the least
/// recently used.
pub(crate) const MAX_DEVICES: i64 = 20;
/// Seconds of activity a session's `last_seen` may lag before a request
/// refreshes it.
const SEEN_GRANULARITY: i64 = 300;
/// How often refreshed `last_seen` times are written, together.
const SEEN_FLUSH: Duration = Duration::from_secs(60);
/// Sessions one `last_seen` write updates at most.
const SEEN_BATCH: usize = 1000;

pub(crate) const ACCOUNT_CHANGED: &str = "the signed-in account changed; sign in again";

/// A signed-in device.
#[derive(Clone, Debug, Default)]
pub(crate) struct Session {
    pub(crate) id: String,
    pub(crate) account: String,
    pub(crate) last_seen: i64,
}

/// Why a credential could not be resolved.
#[derive(Clone, Debug)]
pub(crate) enum SessionError {
    /// The cookie's account differs from the one the page expects.
    AccountChanged,
    /// The session could not be read.
    Database,
}

impl From<SessionError> for ApiError {
    fn from(err: SessionError) -> Self {
        match err {
            SessionError::AccountChanged => ApiError::new(409, ACCOUNT_CHANGED),
            SessionError::Database => ApiError::new(500, "could not read session"),
        }
    }
}

/// A session already read while choosing the request's rate limit.
#[derive(Clone)]
pub(crate) struct ResolvedSession(pub Arc<Result<Option<Session>, SessionError>>);

/// SHA-256 of a token, hex encoded; only hashes are stored.
pub(crate) fn hash_token(token: &str) -> String {
    hex::encode(Sha256::digest(token.as_bytes()))
}

/// A new bearer token: 32 random bytes, URL-safe base64.
pub(crate) fn new_token() -> String {
    let bytes: [u8; 32] = rand::rng().random();
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

/// A new random identifier: 16 bytes, hex encoded.
pub(crate) fn new_id() -> String {
    let bytes: [u8; 16] = rand::rng().random();
    hex::encode(bytes)
}

/// Reports whether the request carries a cookie or bearer credential.
pub(crate) fn has_credential(headers: &HeaderMap) -> bool {
    cookie(headers, SESSION_COOKIE).is_some() || bearer(headers).is_some()
}

fn bearer(headers: &HeaderMap) -> Option<&str> {
    headers.get(AUTHORIZATION)?.to_str().ok()?.strip_prefix("Bearer ")
}

/// The value of the named cookie.
pub(crate) fn cookie<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers.get_all(COOKIE).iter().filter_map(|v| v.to_str().ok()).flat_map(|v| v.split(';')).find_map(|pair| {
        let (k, v) = pair.trim().split_once('=')?;
        (k == name).then(|| v.trim_matches('"'))
    })
}

impl Server {
    /// Reads the session a request's credential names. `Ok(None)` means no
    /// credential, an unknown one, or an expired one.
    pub(crate) async fn lookup_session(&self, headers: &HeaderMap) -> Result<Option<Session>, SessionError> {
        let mut token = cookie(headers, SESSION_COOKIE).unwrap_or_default();
        let is_bearer = bearer(headers).is_some();
        if let Some(t) = bearer(headers) {
            token = t;
        }
        if token.is_empty() {
            return Ok(None);
        }
        let row = self
            .db
            .fetch_opt("SELECT id,account,name,push,last_seen FROM sessions WHERE token_hash=$1", &[&hash_token(token)])
            .await
            .map_err(|_| SessionError::Database)?;
        let Some(row) = row else { return Ok(None) };
        let read = || -> Result<Session, tokio_postgres::Error> {
            Ok(Session {
                id: row.try_get(0)?,
                account: row.try_get(1)?,
                last_seen: row.try_get::<_, Option<i64>>(4)?.unwrap_or_default(),
            })
        };
        let se = read().map_err(|_| SessionError::Database)?;
        let expected: Vec<&HeaderValue> = headers.get_all("x-honorum-account").iter().collect();
        if (!is_bearer || !expected.is_empty())
            && (expected.len() != 1 || expected[0].as_bytes() != se.account.as_bytes())
        {
            return Err(SessionError::AccountChanged);
        }
        let now = self.now().timestamp();
        if now - se.last_seen >= SESSION_TTL.as_secs() as i64 {
            self.exec_logged("DELETE FROM sessions WHERE id=$1", &[&se.id]).await;
            return Ok(None);
        }
        if now - se.last_seen > SEEN_GRANULARITY {
            self.seen.mark(&se.id, now);
        }
        Ok(Some(se))
    }

    /// Writes refreshed `last_seen` times every minute, and once more when
    /// `stop` fires, so requests never wait on that write.
    pub async fn run_sessions(self: &Arc<Self>, stop: CancellationToken) {
        let mut ticker = tokio::time::interval(SEEN_FLUSH);
        ticker.tick().await;
        loop {
            tokio::select! {
                _ = stop.cancelled() => {
                    self.flush_seen().await;
                    return;
                }
                _ = ticker.tick() => self.flush_seen().await,
            }
        }
    }

    /// Writes the `last_seen` times requests have refreshed since the last
    /// write. Times that cannot be written are kept for the next one.
    pub(crate) async fn flush_seen(&self) {
        let seen = self.seen.take();
        let mut entries: Vec<(String, i64)> = seen.into_iter().collect();
        while !entries.is_empty() {
            let batch: Vec<(String, i64)> = entries.drain(..entries.len().min(SEEN_BATCH)).collect();
            let (ids, times): (Vec<&str>, Vec<i64>) = batch.iter().map(|(id, t)| (id.as_str(), *t)).unzip();
            let written = self
                .db
                .exec(
                    "UPDATE sessions SET last_seen=v.seen FROM unnest($1::text[],$2::bigint[]) AS v(id,seen) WHERE sessions.id=v.id AND sessions.last_seen<v.seen",
                    &[&ids, &times],
                )
                .await;
            if let Err(err) = written {
                tracing::warn!(%err, "session activity will be written later");
                for (id, t) in batch.into_iter().chain(entries) {
                    self.seen.mark(&id, t);
                }
                return;
            }
        }
    }

    /// The session for a request, reusing one read by the rate limiter.
    pub(crate) async fn session(&self, parts: &Parts) -> Result<Option<Session>, SessionError> {
        if let Some(resolved) = parts.extensions.get::<ResolvedSession>() {
            return (*resolved.0).clone();
        }
        self.lookup_session(&parts.headers).await
    }

    /// A `Set-Cookie` value for the session cookie. An empty token with
    /// `max_age` below zero clears it.
    pub(crate) fn session_cookie(&self, token: &str, max_age: i64) -> HeaderValue {
        let secure = if self.cfg.base_url.starts_with("https://") { "; Secure" } else { "" };
        let age = if max_age < 0 { "; Max-Age=0".to_owned() } else { format!("; Max-Age={max_age}") };
        HeaderValue::from_str(&format!("{SESSION_COOKIE}={token}; Path=/{age}; HttpOnly{secure}; SameSite=Lax"))
            .expect("session tokens are header-safe")
    }
}

/// Sessions whose `last_seen` requests have refreshed, waiting for
/// [`Server::flush_seen`].
#[derive(Default)]
pub(crate) struct SeenSessions(Mutex<HashMap<String, i64>>);

impl SeenSessions {
    fn mark(&self, id: &str, at: i64) {
        let mut seen = self.0.lock().expect("seen lock");
        let t = seen.entry(id.to_owned()).or_default();
        *t = (*t).max(at);
    }

    fn take(&self) -> HashMap<String, i64> {
        std::mem::take(&mut *self.0.lock().expect("seen lock"))
    }
}

/// Requires a signed-in session: 401 without one, 409 when the page expects
/// another account.
pub(crate) struct Auth(pub Session);

impl FromRequestParts<Arc<Server>> for Auth {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, s: &Arc<Server>) -> Result<Self, ApiError> {
        match s.session(parts).await? {
            Some(se) => Ok(Auth(se)),
            None => Err(ApiError::new(401, "sign in required")),
        }
    }
}
