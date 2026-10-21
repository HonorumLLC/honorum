//! Google Calendar: one Google account per Honorum account, in two
//! independent directions, publishing the planner to a calendar Honorum
//! creates ([`publish`]) and showing events from calendars the student picks
//! ([`events`]). Each direction asks for its own permission when the student
//! turns it on. The refresh token is sealed (scope `google`) and never leaves
//! the server; devices receive connection status and events, never a
//! credential. Access tokens live only in this process's memory.

pub(crate) mod events;
pub(crate) mod publish;
#[cfg(test)]
mod tests;

use std::collections::HashMap;
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

use axum::body::Body;
use axum::extract::{Query, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::Response;
use base64::Engine;
use rand::RngExt;
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};

use crate::Server;
use crate::api::{ApiResult, empty, fail, ok, read_json};
use crate::config::Config;
use crate::db::{self, Lease, Sql};
use crate::lms::path_escape;
use crate::session::{Auth, new_id};
use crate::status::html_escape;

/// Creating and writing the Honorum calendar.
pub(crate) const GOOGLE_SCOPE_PUBLISH: &str = "https://www.googleapis.com/auth/calendar.app.created";
/// Listing the student's calendars.
pub(crate) const GOOGLE_SCOPE_LIST: &str = "https://www.googleapis.com/auth/calendar.calendarlist.readonly";
/// Reading events of shown calendars.
pub(crate) const GOOGLE_SCOPE_EVENTS: &str = "https://www.googleapis.com/auth/calendar.events.readonly";
/// How long a consent attempt (`google_states` row) stays valid.
pub(crate) const GOOGLE_STATE_TTL: Duration = Duration::from_secs(10 * 60);

/// Statements that remove everything an account's Google connection keeps;
/// each takes the account as its one parameter.
pub(crate) const GOOGLE_DELETES: [&str; 4] = [
    "DELETE FROM google_connections WHERE account=$1",
    "DELETE FROM google_published WHERE account=$1",
    "DELETE FROM google_calendars WHERE account=$1",
    "DELETE FROM google_states WHERE account=$1",
];

const BUSY: &str = "Google Calendar is busy; try again";
const READ_FAILED: &str = "could not read the Google Calendar connection";

/// Google's OAuth and Calendar endpoints; tests point them at a fake with
/// `GoogleState::set_endpoints`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct GoogleEndpoints {
    pub(crate) auth: String,
    pub(crate) token: String,
    pub(crate) revoke: String,
    pub(crate) api: String,
}

impl Default for GoogleEndpoints {
    fn default() -> Self {
        Self {
            auth: "https://accounts.google.com/o/oauth2/v2/auth".into(),
            token: "https://oauth2.googleapis.com/token".into(),
            revoke: "https://oauth2.googleapis.com/revoke".into(),
            api: "https://www.googleapis.com/calendar/v3".into(),
        }
    }
}

/// A cached access token.
#[derive(Clone, Debug)]
pub(crate) struct GoogleToken {
    pub(crate) access: String,
    pub(crate) expires: Instant,
}

/// The per-process Google client: endpoints, 64 striped account locks, the
/// worker's wakeup, and short-lived access tokens keyed `account \0 id`.
pub(crate) struct GoogleState {
    endpoints: RwLock<GoogleEndpoints>,
    pub(crate) http: reqwest::Client,
    pub(crate) locks: [tokio::sync::Mutex<()>; 64],
    /// Wakes `run_google`; wakeups while it is busy collapse into one.
    pub(crate) wake: tokio::sync::Notify,
    pub(crate) tokens: Mutex<HashMap<String, GoogleToken>>,
}

impl GoogleState {
    /// The client for a configuration, with Google's real endpoints and a
    /// 20-second request timeout.
    pub(crate) fn new(_cfg: &Config) -> Self {
        Self {
            endpoints: RwLock::new(GoogleEndpoints::default()),
            http: reqwest::Client::builder()
                .timeout(Duration::from_secs(20))
                .build()
                .expect("the Google client configuration is valid"),
            locks: std::array::from_fn(|_| tokio::sync::Mutex::new(())),
            wake: tokio::sync::Notify::new(),
            tokens: Mutex::new(HashMap::new()),
        }
    }

    /// The current endpoints.
    pub(crate) fn endpoints(&self) -> GoogleEndpoints {
        self.endpoints.read().expect("google endpoints lock").clone()
    }

    /// Replaces the endpoints, as tests do.
    #[cfg(test)]
    pub(crate) fn set_endpoints(&self, endpoints: GoogleEndpoints) {
        *self.endpoints.write().expect("google endpoints lock") = endpoints;
    }
}

impl Default for GoogleState {
    fn default() -> Self {
        Self::new(&Config::default())
    }
}

/// An account's Google connection row. The sealed refresh token is
/// `credential`.
#[derive(Clone, Debug, Default)]
pub(crate) struct GoogleConnection {
    pub(crate) id: String,
    pub(crate) subject: String,
    pub(crate) email: String,
    /// Space-separated granted scopes.
    pub(crate) scopes: String,
    /// Set when access was removed; work for the connection stops.
    pub(crate) error: String,
    pub(crate) publish: bool,
    pub(crate) tasks: bool,
    pub(crate) show: bool,
    /// The calendar Honorum created, once it exists.
    pub(crate) calendar: String,
    pub(crate) published_at: i64,
    pub(crate) publish_error: String,
    pub(crate) version: i64,
    pub(crate) credential: Vec<u8>,
}

impl GoogleConnection {
    /// Reports whether every scope in `scopes` was granted.
    pub(crate) fn granted(&self, scopes: &[&str]) -> bool {
        let have: Vec<&str> = self.scopes.split_whitespace().collect();
        scopes.iter().all(|scope| have.contains(scope))
    }
}

/// Google's token endpoint reply.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct GoogleTokenReply {
    pub(crate) access_token: String,
    pub(crate) expires_in: i64,
    pub(crate) refresh_token: String,
    pub(crate) scope: String,
    pub(crate) id_token: String,
    pub(crate) error: String,
}

/// A Calendar API failure: the status and the first error reason.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct GoogleApiError {
    pub(crate) status: u16,
    pub(crate) reason: String,
    pub(crate) message: String,
}

impl GoogleApiError {
    /// Rate and usage limits and Google's own failures, worth retrying later.
    pub(crate) fn throttled(&self) -> bool {
        match self.status {
            429 => true,
            s if s >= 500 => true,
            403 => matches!(
                self.reason.as_str(),
                "rateLimitExceeded" | "userRateLimitExceeded" | "quotaExceeded" | "usageLimits"
            ),
            _ => false,
        }
    }

    fn failure(status: u16, reason: &str) -> GoogleError {
        GoogleError::Api(Self { status, reason: reason.into(), message: String::new() })
    }
}

/// A Google failure. Display texts are the messages clients show.
#[derive(Debug)]
pub(crate) enum GoogleError {
    /// "Access was removed or expired" (`invalid_grant`).
    Revoked,
    /// "The connection is unavailable": the refresh token cannot be opened.
    Unsealed,
    /// "Google could not be reached".
    Unreachable,
    /// "Google answered N" from the token endpoint.
    Token(u16),
    /// "Google Calendar answered N REASON".
    Api(GoogleApiError),
    /// Anything else, such as "unexpected reply from Google Calendar" or a
    /// database failure.
    Other(anyhow::Error),
}

impl GoogleError {
    /// The Calendar API status, or 0 for other failures.
    pub(crate) fn status(&self) -> u16 {
        match self {
            Self::Api(e) => e.status,
            _ => 0,
        }
    }

    /// Reports a throttled Calendar API failure.
    pub(crate) fn throttled(&self) -> bool {
        matches!(self, Self::Api(e) if e.throttled())
    }
}

impl std::fmt::Display for GoogleError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Revoked => f.write_str("Access was removed or expired"),
            Self::Unsealed => f.write_str("The connection is unavailable"),
            Self::Unreachable => f.write_str("Google could not be reached"),
            Self::Token(status) => write!(f, "Google answered {status}"),
            Self::Api(e) => write!(f, "Google Calendar answered {} {}", e.status, e.reason),
            Self::Other(err) => err.fmt(f),
        }
    }
}

impl std::error::Error for GoogleError {}

impl From<db::Error> for GoogleError {
    fn from(err: db::Error) -> Self {
        Self::Other(err.into())
    }
}

/// The account's Google lock: a striped local mutex, then the `google`
/// advisory lock. Long work should stop when `lease.lost()` fires. Fields
/// drop in order, so the advisory lock is released before the local one.
pub(crate) struct GoogleLock<'a> {
    pub(crate) lease: Lease,
    pub(crate) _local: tokio::sync::MutexGuard<'a, ()>,
}

/// A random URL-safe token of `n` bytes, unpadded base64.
pub(crate) fn random_token(n: usize) -> String {
    let mut rng = rand::rng();
    let bytes: Vec<u8> = (0..n).map(|_| rng.random::<u8>()).collect();
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

/// `/calendars/{calendar}` followed by `rest`, each part escaped by
/// [`path_escape`].
pub(crate) fn calendar_path(calendar: &str, rest: &[&str]) -> String {
    let mut path = format!("/calendars/{}", path_escape(calendar));
    for part in rest {
        path.push('/');
        path.push_str(&path_escape(part));
    }
    path
}

/// A form or query component: everything but ASCII letters, digits, and
/// `-_.~` is percent-encoded with uppercase hex, and a space becomes `+`.
fn query_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for &c in s.as_bytes() {
        if c.is_ascii_alphanumeric() || b"-_.~".contains(&c) {
            out.push(c as char);
        } else if c == b' ' {
            out.push('+');
        } else {
            out.push_str(&format!("%{c:02X}"));
        }
    }
    out
}

/// Form and query encoding: pairs sorted by key (stable for repeated keys),
/// each part escaped by [`query_escape`], joined with `&`.
pub(crate) fn encode_query(pairs: &[(&str, &str)]) -> String {
    let mut sorted = pairs.to_vec();
    sorted.sort_by(|a, b| a.0.cmp(b.0));
    sorted.iter().map(|(k, v)| format!("{}={}", query_escape(k), query_escape(v))).collect::<Vec<_>>().join("&")
}

/// A JSON object field: the exact name, else a case-insensitive match.
fn field<'a>(v: &'a Value, name: &str) -> Option<&'a Value> {
    let object = v.as_object()?;
    object.get(name).or_else(|| object.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v))
}

fn field_str<'a>(v: &'a Value, name: &str) -> &'a str {
    field(v, name).and_then(Value::as_str).unwrap_or_default()
}

/// The `sub` and `email` claims of an ID token from Google's token endpoint
/// (read without a signature check, as OpenID Connect allows for a token
/// received directly over TLS); empty on any parse failure.
pub(crate) fn id_token_claims(token: &str) -> (String, String) {
    let parts: Vec<&str> = token.split('.').collect();
    if parts.len() != 3 {
        return (String::new(), String::new());
    }
    let Ok(raw) = base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(parts[1]) else {
        return (String::new(), String::new());
    };
    let claims: Value = serde_json::from_slice(&raw).unwrap_or(Value::Null);
    (field_str(&claims, "sub").to_owned(), field_str(&claims, "email").to_owned())
}

/// A small HTML page with an escaped message, for the OAuth callback.
pub(crate) fn google_page(status: u16, message: &str) -> Response {
    let escaped = html_escape(message);
    let body = format!(
        r#"<!doctype html><meta charset="utf-8"><meta name="viewport" content="width=device-width"><title>Honorum</title><p style="font:16px system-ui;margin:3em auto;max-width:28em;text-align:center">{escaped}</p>"#
    );
    let mut response = Response::new(Body::from(body));
    *response.status_mut() = StatusCode::from_u16(status).expect("a valid status");
    response.headers_mut().insert(header::CONTENT_TYPE, HeaderValue::from_static("text/html; charset=utf-8"));
    response
}

/// A `302 Found` to `location`.
fn redirect(location: &str) -> Response {
    let mut response = empty(302);
    if let Ok(value) = HeaderValue::from_str(location) {
        response.headers_mut().insert(header::LOCATION, value);
    }
    response
}

/// Reads at most `limit` bytes of a reply body; the rest is dropped.
async fn read_limited(mut resp: reqwest::Response, limit: usize) -> reqwest::Result<Vec<u8>> {
    let mut out = Vec::new();
    while let Some(chunk) = resp.chunk().await? {
        let room = limit - out.len();
        out.extend_from_slice(&chunk[..chunk.len().min(room)]);
        if out.len() >= limit {
            break;
        }
    }
    Ok(out)
}

fn flag(row: &db::Row, i: usize) -> Result<bool, tokio_postgres::Error> {
    Ok(row.try_get::<_, i64>(i)? != 0)
}

impl Server {
    /// Reports whether Google Calendar can be connected: an OAuth client and
    /// the credential key.
    pub(crate) fn google_available(&self) -> bool {
        !self.cfg.google.client_id.is_empty()
            && !self.cfg.google.client_secret.is_empty()
            && self.integrations_available()
    }

    /// The OAuth redirect URI: `BASE/api/google/callback`.
    pub(crate) fn google_redirect(&self) -> String {
        format!("{}/api/google/callback", self.cfg.base_url.trim_end_matches('/'))
    }

    /// Wakes the Google worker soon.
    pub(crate) fn kick_google(&self) {
        self.google.wake.notify_one();
    }

    /// Takes the account's Google lock. Cancel by dropping the future.
    pub(crate) async fn lock_google(&self, account: &str) -> db::Result<GoogleLock<'_>> {
        let hash = Sha256::digest(account.as_bytes());
        let local = self.google.locks[usize::from(hash[0]) % self.google.locks.len()].lock().await;
        let lease = self.db.lock("google", account).await?;
        Ok(GoogleLock { lease, _local: local })
    }

    /// Reads the account's connection, `None` when there is none.
    pub(crate) async fn google_connection(&self, account: &str) -> db::Result<Option<GoogleConnection>> {
        let Some(row) = self
            .db
            .fetch_opt(
                "SELECT id,subject,email,scopes,error,publish,tasks,calendar,published_seq,published_at,publish_retry,publish_error,failures,show,listed_at,version,credential FROM google_connections WHERE account=$1",
                &[&account],
            )
            .await?
        else {
            return Ok(None);
        };
        Ok(Some(GoogleConnection {
            id: row.try_get(0)?,
            subject: row.try_get(1)?,
            email: row.try_get(2)?,
            scopes: row.try_get(3)?,
            error: row.try_get(4)?,
            publish: flag(&row, 5)?,
            tasks: flag(&row, 6)?,
            calendar: row.try_get(7)?,
            published_at: row.try_get(9)?,
            publish_error: row.try_get(11)?,
            show: flag(&row, 13)?,
            version: row.try_get(15)?,
            credential: row.try_get(16)?,
        }))
    }

    /// Posts a form (plus client credentials) to the token endpoint.
    pub(crate) async fn google_token_request(&self, form: &[(&str, &str)]) -> Result<GoogleTokenReply, GoogleError> {
        let mut fields: Vec<(&str, &str)> = form.to_vec();
        fields.push(("client_id", &self.cfg.google.client_id));
        fields.push(("client_secret", &self.cfg.google.client_secret));
        let request = self
            .google
            .http
            .post(self.google.endpoints().token)
            .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
            .body(encode_query(&fields));
        let resp = match request.send().await {
            Ok(resp) => resp,
            Err(err) if err.is_builder() => return Err(GoogleError::Other(err.into())),
            Err(_) => return Err(GoogleError::Unreachable),
        };
        let status = resp.status().as_u16();
        let data = read_limited(resp, 1 << 20).await.unwrap_or_default();
        let reply: GoogleTokenReply =
            serde_json::Deserializer::from_slice(&data).into_iter().next().and_then(Result::ok).unwrap_or_default();
        if reply.error == "invalid_grant" {
            return Err(GoogleError::Revoked);
        }
        if status != 200 || reply.access_token.is_empty() {
            return Err(GoogleError::Token(status));
        }
        Ok(reply)
    }

    /// A current access token, refreshed when within a minute of expiry or
    /// when `fresh`. A revoked grant records the error on the connection
    /// (and in `c`); changed scopes are recorded too.
    pub(crate) async fn google_access(
        &self,
        account: &str,
        c: &mut GoogleConnection,
        fresh: bool,
    ) -> Result<String, GoogleError> {
        let key = format!("{account}\0{}", c.id);
        let cached = self.google.tokens.lock().expect("google tokens lock").get(&key).cloned();
        if let Some(t) = cached
            && !fresh
            && t.expires.saturating_duration_since(Instant::now()) > Duration::from_secs(60)
        {
            return Ok(t.access);
        }
        let refresh = self.open(account, &c.id, "google", &c.credential).map_err(|_| GoogleError::Unsealed)?;
        let reply =
            match self.google_token_request(&[("grant_type", "refresh_token"), ("refresh_token", &refresh)]).await {
                Ok(reply) => reply,
                Err(err) => {
                    if matches!(err, GoogleError::Revoked) {
                        let message = err.to_string();
                        self.exec_logged(
                            "UPDATE google_connections SET error=$1 WHERE account=$2 AND id=$3",
                            &[&message, &account, &c.id],
                        )
                        .await;
                        c.error = message;
                    }
                    return Err(err);
                }
            };
        let expires = Instant::now() + Duration::from_secs(reply.expires_in.max(0) as u64);
        self.google
            .tokens
            .lock()
            .expect("google tokens lock")
            .insert(key, GoogleToken { access: reply.access_token.clone(), expires });
        // A refresh names the permissions still granted; the student may have
        // removed one in their Google account since connecting.
        if !reply.scope.is_empty() && reply.scope != c.scopes {
            c.scopes = reply.scope.clone();
            self.exec_logged(
                "UPDATE google_connections SET scopes=$1 WHERE account=$2 AND id=$3",
                &[&reply.scope, &account, &c.id],
            )
            .await;
        }
        Ok(reply.access_token)
    }

    /// Drops the account's cached access tokens.
    pub(crate) fn forget_google_tokens(&self, account: &str) {
        let prefix = format!("{account}\0");
        self.google.tokens.lock().expect("google tokens lock").retain(|key, _| !key.starts_with(&prefix));
    }

    /// One Calendar API request (`path` under the API endpoint, `query`
    /// encoded), retried once with a fresh token after a 401. A non-empty
    /// reply decodes into `T` ("unexpected reply from Google Calendar" on
    /// failure); an empty one yields `T::default()`. Use
    /// [`Server::google_send`] when the reply does not matter.
    pub(crate) async fn google_call<T: DeserializeOwned + Default>(
        &self,
        account: &str,
        c: &mut GoogleConnection,
        method: reqwest::Method,
        path: &str,
        query: &[(&str, &str)],
        body: Option<&Value>,
    ) -> Result<T, GoogleError> {
        let data = self.google_send(account, c, method, path, query, body).await?;
        if data.is_empty() {
            return Ok(T::default());
        }
        serde_json::from_slice(&data)
            .map_err(|_| GoogleError::Other(anyhow::anyhow!("unexpected reply from Google Calendar")))
    }

    /// [`Server::google_call`] without decoding: the reply body (at most
    /// 8 MiB) of a successful request.
    pub(crate) async fn google_send(
        &self,
        account: &str,
        c: &mut GoogleConnection,
        method: reqwest::Method,
        path: &str,
        query: &[(&str, &str)],
        body: Option<&Value>,
    ) -> Result<Vec<u8>, GoogleError> {
        let payload = body.map(|b| serde_json::to_vec(b).expect("JSON values serialize"));
        let mut attempt = 0;
        loop {
            let token = self.google_access(account, c, attempt > 0).await?;
            let mut target = format!("{}{path}", self.google.endpoints().api);
            if !query.is_empty() {
                target.push('?');
                target.push_str(&encode_query(query));
            }
            let mut request = self.google.http.request(method.clone(), &target).bearer_auth(&token);
            if let Some(payload) = &payload {
                request = request.header(header::CONTENT_TYPE, "application/json").body(payload.clone());
            }
            let resp = match request.send().await {
                Ok(resp) => resp,
                Err(err) if err.is_builder() => return Err(GoogleError::Other(err.into())),
                Err(_) => return Err(GoogleApiError::failure(503, "unreachable")),
            };
            let status = resp.status().as_u16();
            let Ok(data) = read_limited(resp, 8 << 20).await else {
                return Err(GoogleApiError::failure(503, "unreadable"));
            };
            if status == 401 && attempt == 0 {
                attempt += 1;
                continue;
            }
            if status >= 300 {
                let reply: Value = serde_json::from_slice(&data).unwrap_or(Value::Null);
                let error = field(&reply, "error").cloned().unwrap_or(Value::Null);
                let reason = field(&error, "errors")
                    .and_then(Value::as_array)
                    .and_then(|errors| errors.first())
                    .map(|first| field_str(first, "reason").to_owned())
                    .unwrap_or_default();
                return Err(GoogleError::Api(GoogleApiError {
                    status,
                    reason,
                    message: field_str(&error, "message").to_owned(),
                }));
            }
            return Ok(data);
        }
    }

    /// Where the browser goes after Google answers: the web app
    /// (`BASE/?google_KEY=VALUE#settings/schedule`) or the iPhone app
    /// (`honorum://google?KEY=VALUE`).
    pub(crate) fn google_return(&self, back: &str, key: &str, value: &str) -> String {
        if back == "ios" {
            return format!("honorum://google?{}", encode_query(&[(key, value)]));
        }
        let name = format!("google_{key}");
        format!(
            "{}/?{}#settings/schedule",
            self.cfg.base_url.trim_end_matches('/'),
            encode_query(&[(name.as_str(), value)])
        )
    }

    /// The connection status clients see: `available`, `connected`,
    /// `email`, `error`, `publish {on, granted, tasks, synced, error}`, and
    /// `show {on, granted, calendars}`. Never includes a credential.
    pub(crate) async fn google_out(&self, account: &str) -> db::Result<Map<String, Value>> {
        let mut out = Map::new();
        out.insert("available".into(), json!(self.google_available()));
        out.insert("connected".into(), json!(false));
        out.insert("email".into(), json!(""));
        out.insert("error".into(), json!(""));
        out.insert("publish".into(), json!({"on": false, "granted": false, "tasks": true, "synced": 0, "error": ""}));
        out.insert("show".into(), json!({"on": false, "granted": false, "calendars": []}));
        let Some(c) = self.google_connection(account).await? else { return Ok(out) };
        let rows = self
            .db
            .fetch_all(
                "SELECT id,name,color,main,selected,synced,error FROM google_calendars WHERE account=$1 ORDER BY main DESC,name,id",
                &[&account],
            )
            .await?;
        let mut calendars = Vec::with_capacity(rows.len());
        for row in &rows {
            calendars.push(json!({
                "id": row.try_get::<_, String>(0)?,
                "name": row.try_get::<_, String>(1)?,
                "color": row.try_get::<_, String>(2)?,
                "primary": flag(row, 3)?,
                "on": flag(row, 4)?,
                "synced": row.try_get::<_, i64>(5)?,
                "error": row.try_get::<_, String>(6)?,
            }));
        }
        out.insert("connected".into(), json!(true));
        out.insert("email".into(), json!(c.email));
        out.insert("error".into(), json!(c.error));
        out.insert(
            "publish".into(),
            json!({"on": c.publish, "granted": c.granted(&[GOOGLE_SCOPE_PUBLISH]), "tasks": c.tasks, "synced": c.published_at, "error": c.publish_error}),
        );
        out.insert(
            "show".into(),
            json!({"on": c.show, "granted": c.granted(&[GOOGLE_SCOPE_LIST, GOOGLE_SCOPE_EVENTS]), "calendars": calendars}),
        );
        Ok(out)
    }

    async fn write_google(&self, account: &str) -> ApiResult {
        match self.google_out(account).await {
            Ok(out) => ok(&out),
            Err(_) => fail(500, READ_FAILED),
        }
    }

    /// Cleans up at Google as far as it can within 15 seconds (deletes the
    /// Honorum calendar, stops channels, revokes the token), then deletes
    /// every record of the connection regardless. The caller holds the
    /// account's Google lock.
    pub(crate) async fn forget_google(&self, account: &str, c: &mut GoogleConnection) {
        let cleanup = async {
            if c.error.is_empty() {
                if !c.calendar.is_empty() && c.granted(&[GOOGLE_SCOPE_PUBLISH]) {
                    let path = calendar_path(&c.calendar, &[]);
                    let _ = self.google_send(account, c, reqwest::Method::DELETE, &path, &[], None).await;
                }
                if let Ok(rows) = self
                    .db
                    .fetch_all(
                        "SELECT channel,resource FROM google_calendars WHERE account=$1 AND channel<>''",
                        &[&account],
                    )
                    .await
                {
                    let channels: Vec<(String, String)> =
                        rows.iter().filter_map(|r| Some((r.try_get(0).ok()?, r.try_get(1).ok()?))).collect();
                    for (id, resource) in channels {
                        let body = json!({"id": id, "resourceId": resource});
                        let _ = self
                            .google_send(account, c, reqwest::Method::POST, "/channels/stop", &[], Some(&body))
                            .await;
                    }
                }
            }
            if let Ok(refresh) = self.open(account, &c.id, "google", &c.credential) {
                let _ = self
                    .google
                    .http
                    .post(self.google.endpoints().revoke)
                    .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                    .body(encode_query(&[("token", &refresh)]))
                    .send()
                    .await;
            }
        };
        let _ = tokio::time::timeout(Duration::from_secs(15), cleanup).await;
        for q in GOOGLE_DELETES {
            self.exec_logged(q, &[&account]).await;
        }
        self.forget_google_tokens(account);
    }
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct StartIn {
    #[serde(alias = "Want")]
    want: String,
    #[serde(alias = "Back")]
    back: String,
}

/// `POST /api/me/google/start`: begins consent (`want`: publish, show, or
/// reconnect; `back`: web or ios; 1 KiB) and returns `{"url"}`. Reconnecting
/// asks for every direction that is on. The consent uses PKCE (S256) and
/// `include_granted_scopes`, so permissions granted earlier are kept.
pub(crate) async fn handle_google_start(State(s): State<Arc<Server>>, Auth(se): Auth, body: Body) -> ApiResult {
    let input: Option<StartIn> = read_json(body, 1024).await;
    let Some(mut input) = input.filter(|i| matches!(i.want.as_str(), "publish" | "show" | "reconnect")) else {
        return fail(400, "want must be publish, show, or reconnect");
    };
    if input.back.is_empty() {
        input.back = "web".into();
    }
    if input.back != "web" && input.back != "ios" {
        return fail(400, "back must be web or ios");
    }
    if !s.google_available() {
        return fail(503, "Google Calendar is not configured on this server");
    }
    let Ok(existing) = s.google_connection(&se.account).await else { return fail(500, READ_FAILED) };
    let (mut publish, mut show) = (input.want == "publish", input.want == "show");
    if input.want == "reconnect" {
        match &existing {
            Some(e) if e.publish || e.show => (publish, show) = (e.publish, e.show),
            _ => return fail(400, "nothing to reconnect"),
        }
    }
    let mut scopes = vec!["openid", "email"];
    if publish {
        scopes.push(GOOGLE_SCOPE_PUBLISH);
    }
    if show {
        scopes.extend([GOOGLE_SCOPE_LIST, GOOGLE_SCOPE_EVENTS]);
    }
    let (state, verifier) = (random_token(32), random_token(48));
    let saved =
        s.db.exec(
            "INSERT INTO google_states(state,account,session,verifier,want,back,created_at) VALUES($1,$2,$3,$4,$5,$6,$7)",
            &[&state, &se.account, &se.id, &verifier, &input.want, &input.back, &s.now().timestamp()],
        )
        .await;
    if saved.is_err() {
        return fail(500, "could not start connecting Google Calendar");
    }
    let challenge = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
    let redirect_uri = s.google_redirect();
    let scope = scopes.join(" ");
    let mut q: Vec<(&str, &str)> = vec![
        ("client_id", &s.cfg.google.client_id),
        ("redirect_uri", &redirect_uri),
        ("response_type", "code"),
        ("scope", &scope),
        ("access_type", "offline"),
        ("include_granted_scopes", "true"),
        ("prompt", "consent"),
        ("state", &state),
        ("code_challenge", &challenge),
        ("code_challenge_method", "S256"),
    ];
    if let Some(e) = existing.as_ref().filter(|e| !e.email.is_empty()) {
        q.push(("login_hint", &e.email));
    }
    let url = format!("{}?{}", s.google.endpoints().auth, encode_query(&q));
    ok(&json!({ "url": url }))
}

/// `GET /api/google/callback`: exchanges Google's code, stores the sealed
/// refresh token under a one-time handoff, and redirects (302) back. It
/// connects nothing: the session that started consent finishes it, so a
/// consent link someone else started cannot attach this Google account to
/// theirs.
pub(crate) async fn handle_google_callback(
    State(s): State<Arc<Server>>,
    Query(query): Query<HashMap<String, String>>,
) -> ApiResult {
    let get = |k: &str| query.get(k).map(String::as_str).unwrap_or_default();
    let state = get("state");
    let row =
        s.db.fetch_opt(
            "SELECT account,verifier,back,created_at FROM google_states WHERE state=$1 AND handoff=''",
            &[&state],
        )
        .await;
    let read = |row: db::Row| -> Option<(String, String, String, i64)> {
        Some((row.try_get(0).ok()?, row.try_get(1).ok()?, row.try_get(2).ok()?, row.try_get(3).ok()?))
    };
    let now = s.now().timestamp();
    let Some((account, verifier, back, _)) = row
        .ok()
        .flatten()
        .and_then(read)
        .filter(|(_, _, _, created)| now - created <= GOOGLE_STATE_TTL.as_secs() as i64)
    else {
        return Ok(google_page(400, "This Google sign-in has expired. Return to Honorum and try again."));
    };
    let failed = async |reason: &str| -> ApiResult {
        s.exec_logged("DELETE FROM google_states WHERE state=$1", &[&state]).await;
        Ok(redirect(&s.google_return(&back, "error", reason)))
    };
    let error = get("error");
    if !error.is_empty() {
        return failed(if error == "access_denied" { "denied" } else { "failed" }).await;
    }
    let redirect_uri = s.google_redirect();
    let reply = s
        .google_token_request(&[
            ("grant_type", "authorization_code"),
            ("code", get("code")),
            ("redirect_uri", &redirect_uri),
            ("code_verifier", &verifier),
        ])
        .await;
    let Some(reply) = reply.ok().filter(|r| !r.refresh_token.is_empty()) else { return failed("failed").await };
    let (subject, email) = id_token_claims(&reply.id_token);
    let Ok(sealed) = s.seal(&account, state, "google-pending", &reply.refresh_token) else {
        return failed("failed").await;
    };
    let handoff = random_token(32);
    let saved =
        s.db.exec(
            "UPDATE google_states SET handoff=$1,credential=$2,scopes=$3,subject=$4,email=$5 WHERE state=$6",
            &[&handoff, &sealed, &reply.scope, &subject, &email, &state],
        )
        .await;
    if saved.is_err() {
        return failed("failed").await;
    }
    Ok(redirect(&s.google_return(&back, "handoff", &handoff)))
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct FinishIn {
    #[serde(alias = "Handoff")]
    handoff: String,
}

/// `POST /api/me/google/finish`: connects the Google account a callback
/// returned, for the session that started it (`handoff`; 1 KiB). A
/// different Google account replaces the connected one, and its calendars go
/// with it. The reply is the connection status plus `granted`, whether
/// Google allowed what was asked.
pub(crate) async fn handle_google_finish(State(s): State<Arc<Server>>, Auth(se): Auth, body: Body) -> ApiResult {
    let input: Option<FinishIn> = read_json(body, 1024).await;
    let Some(input) = input.filter(|i| !i.handoff.is_empty()) else { return fail(400, "handoff is required") };
    let row = s
        .db
        .fetch_opt(
            "SELECT state,session,want,created_at,credential,scopes,subject,email FROM google_states WHERE handoff=$1 AND account=$2",
            &[&input.handoff, &se.account],
        )
        .await;
    type Pending = (String, String, String, i64, Option<Vec<u8>>, String, String, String);
    let read = |row: db::Row| -> Option<Pending> {
        Some((
            row.try_get(0).ok()?,
            row.try_get(1).ok()?,
            row.try_get(2).ok()?,
            row.try_get(3).ok()?,
            row.try_get(4).ok()?,
            row.try_get(5).ok()?,
            row.try_get(6).ok()?,
            row.try_get(7).ok()?,
        ))
    };
    let now = s.now().timestamp();
    let Some((state, _, want, _, sealed, scopes, subject, email)) =
        row.ok().flatten().and_then(read).filter(|p| p.1 == se.id && now - p.3 <= GOOGLE_STATE_TTL.as_secs() as i64)
    else {
        return fail(404, "This sign-in has expired");
    };
    s.exec_logged("DELETE FROM google_states WHERE state=$1", &[&state]).await;
    let Ok(refresh) = s.open(&se.account, &state, "google-pending", &sealed.unwrap_or_default()) else {
        return fail(500, "could not read Google's answer");
    };
    let Ok(lock) = s.lock_google(&se.account).await else { return fail(503, BUSY) };
    let Ok(mut existing) = s.google_connection(&se.account).await else {
        drop(lock);
        return fail(500, READ_FAILED);
    };
    if let Some(e) = existing.as_mut()
        && !e.subject.is_empty()
        && !subject.is_empty()
        && e.subject != subject
    {
        s.forget_google(&se.account, e).await;
        existing = None;
    }
    let id = existing.as_ref().map(|e| e.id.clone()).unwrap_or_else(new_id);
    let Ok(credential) = s.seal(&se.account, &id, "google", &refresh) else {
        drop(lock);
        return fail(500, "could not protect the Google Calendar connection");
    };
    let mut result = s
        .db
        .exec(
            "INSERT INTO google_connections(account,id,subject,email,credential,scopes,created_at) VALUES($1,$2,$3,$4,$5,$6,$7)
 ON CONFLICT(account) DO UPDATE SET subject=excluded.subject,email=excluded.email,credential=excluded.credential,scopes=excluded.scopes,error=''",
            &[&se.account, &id, &subject, &email, &credential, &scopes, &s.now().timestamp()],
        )
        .await
        .map(drop);
    // `granted` says whether Google allowed what was asked; a student can
    // clear a permission on Google's consent screen.
    let c = GoogleConnection { scopes, ..Default::default() };
    let mut granted = false;
    if result.is_ok() {
        match want.as_str() {
            "publish" => {
                granted = c.granted(&[GOOGLE_SCOPE_PUBLISH]);
                if granted {
                    result = s
                        .db
                        .exec(
                            "UPDATE google_connections SET publish=1,published_seq=-1,publish_error='',publish_retry=0,failures=0 WHERE account=$1",
                            &[&se.account],
                        )
                        .await
                        .map(drop);
                }
            }
            "show" => {
                granted = c.granted(&[GOOGLE_SCOPE_LIST, GOOGLE_SCOPE_EVENTS]);
                if granted {
                    result =
                        s.db.exec("UPDATE google_connections SET show=1,listed_at=0 WHERE account=$1", &[&se.account])
                            .await
                            .map(drop);
                }
            }
            "reconnect" => {
                let e = existing.as_ref();
                granted = e.is_none_or(|e| !e.publish || c.granted(&[GOOGLE_SCOPE_PUBLISH]))
                    && e.is_none_or(|e| !e.show || c.granted(&[GOOGLE_SCOPE_LIST, GOOGLE_SCOPE_EVENTS]));
                result = s
                    .db
                    .exec(
                        "UPDATE google_connections SET published_seq=-1,publish_error='',publish_retry=0,failures=0,listed_at=0 WHERE account=$1",
                        &[&se.account],
                    )
                    .await
                    .map(drop);
            }
            _ => {}
        }
    }
    drop(lock);
    if result.is_err() {
        return fail(500, "could not save the Google Calendar connection");
    }
    s.forget_google_tokens(&se.account);
    s.kick_google();
    let Ok(mut out) = s.google_out(&se.account).await else { return fail(500, READ_FAILED) };
    out.insert("granted".into(), json!(granted));
    ok(&out)
}

/// `GET /api/me/google`: the connection status.
pub(crate) async fn handle_google(State(s): State<Arc<Server>>, Auth(se): Auth) -> ApiResult {
    s.write_google(&se.account).await
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct PatchIn {
    #[serde(alias = "Publish")]
    publish: Option<bool>,
    #[serde(alias = "Tasks")]
    tasks: Option<bool>,
    #[serde(alias = "Show")]
    show: Option<bool>,
    #[serde(alias = "Calendars", deserialize_with = "crate::api::null_default")]
    calendars: HashMap<String, bool>,
}

/// `PATCH /api/me/google`: turns directions, due dates, and shown calendars
/// on or off (`publish`, `tasks`, `show`, `calendars`; 16 KiB). Turning a
/// direction on needs its permission (403 without it; the client starts
/// consent instead).
pub(crate) async fn handle_google_patch(State(s): State<Arc<Server>>, Auth(se): Auth, body: Body) -> ApiResult {
    let Some(input) = read_json::<PatchIn>(body, 16 << 10).await else { return fail(400, "bad json") };
    let Ok(_lock) = s.lock_google(&se.account).await else { return fail(503, BUSY) };
    let c = match s.google_connection(&se.account).await {
        Err(_) => return fail(500, READ_FAILED),
        Ok(None) => return fail(404, "Google Calendar is not connected"),
        Ok(Some(c)) => c,
    };
    if (input.publish == Some(true) && !c.granted(&[GOOGLE_SCOPE_PUBLISH]))
        || (input.show == Some(true) && !c.granted(&[GOOGLE_SCOPE_LIST, GOOGLE_SCOPE_EVENTS]))
    {
        return fail(403, "Google Calendar needs your permission for this");
    }
    let saved: db::Result<()> = async {
        let mut client = s.db.client().await?;
        let tx = client.transaction().await?;
        let account = &se.account;
        if let Some(on) = input.publish {
            tx.exec(
                "UPDATE google_connections SET publish=$1,published_seq=-1,publish_error='',publish_retry=0,failures=0 WHERE account=$2",
                &[&i64::from(on), account],
            )
            .await?;
        }
        if let Some(on) = input.tasks {
            tx.exec("UPDATE google_connections SET tasks=$1,published_seq=-1 WHERE account=$2", &[&i64::from(on), account])
                .await?;
        }
        if let Some(on) = input.show {
            tx.exec(
                "UPDATE google_connections SET show=$1,listed_at=0,version=version+1 WHERE account=$2",
                &[&i64::from(on), account],
            )
            .await?;
        }
        for (id, on) in &input.calendars {
            tx.exec(
                "UPDATE google_calendars SET selected=$1,dirty=1 WHERE account=$2 AND id=$3",
                &[&i64::from(*on), account, id],
            )
            .await?;
        }
        if !input.calendars.is_empty() {
            tx.exec("UPDATE google_connections SET version=version+1 WHERE account=$1", &[account]).await?;
        }
        tx.commit().await?;
        Ok(())
    }
    .await;
    if saved.is_err() {
        return fail(500, "could not save Google Calendar settings");
    }
    s.kick_google();
    s.changed(&se.account);
    s.write_google(&se.account).await
}

/// `DELETE /api/me/google`: disconnects: the Honorum calendar is removed
/// from Google, notifications stop, the token is revoked, and nothing is
/// kept.
pub(crate) async fn handle_google_disconnect(State(s): State<Arc<Server>>, Auth(se): Auth) -> ApiResult {
    let Ok(lock) = s.lock_google(&se.account).await else { return fail(503, BUSY) };
    let read = s.google_connection(&se.account).await;
    if let Ok(Some(c)) = &read {
        s.forget_google(&se.account, &mut c.clone()).await;
    }
    drop(lock);
    if read.is_err() {
        return fail(500, READ_FAILED);
    }
    s.changed(&se.account);
    s.write_google(&se.account).await
}
