//! LMS relays for guests: the browser cannot call Canvas or fetch a calendar
//! feed cross-origin, so the server relays those requests. Tokens and feed
//! URLs arrive with each request and are never stored. Account imports
//! reuse the Canvas client here.
//!
//! URL checks and the guarded HTTP client live in [`crate::outbound`].

use std::collections::HashMap;
use std::future::Future;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::Duration;

use axum::body::Body;
use axum::extract::State;
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use percent_encoding::{AsciiSet, NON_ALPHANUMERIC, utf8_percent_encode};
use regex::Regex;
use reqwest::Method;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::{Map, Value, json};
use tokio::sync::Semaphore;
use tokio_util::sync::CancellationToken;

use crate::Server;
use crate::api::{ApiError, ApiResult, fail, null_default, ok, read_json};
use crate::outbound::{Outbound, same_origin};

/// Per-course module lookups run at most this many at once; the module test
/// pins peak concurrency to it.
pub(crate) const CANVAS_CONCURRENCY: usize = 4;
/// The planner's date range (−7 to +120 days) is walked as this many
/// contiguous windows at once.
pub(crate) const PLANNER_WINDOWS: usize = 4;
/// The course list and every planner window run at once; the ceiling on
/// in-flight Canvas requests per import.
pub(crate) const PLANNER_CONCURRENCY: usize = PLANNER_WINDOWS + 1;
/// Requests one Canvas operation may make, including retries.
pub(crate) const CANVAS_REQUEST_BUDGET: usize = 100;
/// Bytes one Canvas operation may read (16 MiB).
pub(crate) const CANVAS_BYTE_BUDGET: usize = 16 << 20;
/// The largest single relayed response (4 MiB).
pub(crate) const RELAY_MAX_BODY: usize = 4 << 20;

/// The normalized task shape every client stores.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct Task {
    pub(crate) uid: String,
    pub(crate) title: String,
    /// The LMS course's display name.
    pub(crate) course: String,
    /// Stable course id, so the person can map it to a class once.
    #[serde(rename = "courseId", skip_serializing_if = "String::is_empty")]
    pub(crate) course_id: String,
    /// For example "CS 1213-010", the best hint for matching a class.
    #[serde(rename = "courseCode", skip_serializing_if = "String::is_empty")]
    pub(crate) course_code: String,
    /// RFC 3339, or empty when undated.
    pub(crate) due: String,
    pub(crate) done: bool,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub(crate) url: String,
    /// The Canvas planner override, once the person has marked it done or not.
    #[serde(rename = "overrideId", skip_serializing_if = "String::is_empty")]
    pub(crate) override_id: String,
    /// `moduleId:itemId` when a module requires marking this item done.
    #[serde(rename = "moduleItem", skip_serializing_if = "String::is_empty")]
    pub(crate) module_item: String,
    /// Why Canvas counts it done: `submitted`, `graded`, or `marked`.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub(crate) reason: String,
}

/// One relayed response.
#[derive(Clone, Debug)]
pub(crate) struct RelayReply {
    pub(crate) body: bytes::Bytes,
    pub(crate) headers: reqwest::header::HeaderMap,
    pub(crate) status: u16,
}

/// Performs one relayed request with an optional JSON body and returns the
/// status for the caller to judge. Only rejected credentials (401/403:
/// "the service rejected the token or feed url") and an oversized body
/// ("the service response is too large", over [`RELAY_MAX_BODY`]) are
/// errors besides transport failures. Redirects stay within three
/// requests, pass `safe_url`, and, when a token is sent, the original
/// origin ("Canvas redirected to a different origin"). Each request has a
/// 25-second limit.
pub(crate) async fn relay_raw(
    outbound: &Outbound,
    method: reqwest::Method,
    url: &url::Url,
    token: &str,
    payload: Option<&serde_json::Value>,
) -> anyhow::Result<RelayReply> {
    let mut body = payload.map(serde_json::to_vec).transpose()?;
    let client = outbound.client();
    let op = url_error_op(&method);
    let mut method = method;
    let mut current = url.clone();
    let mut requests = 0;
    let response = loop {
        let mut request = client.request(method.clone(), current.clone()).header(header::USER_AGENT, "Honorum");
        if let Some(body) = &body {
            request = request.header(header::CONTENT_TYPE, "application/json").body(body.clone());
        }
        if !token.is_empty() {
            request = request.header(header::AUTHORIZATION, format!("Bearer {token}"));
        }
        let response = request.send().await.map_err(|e| url_error(&op, &current, e.without_url()))?;
        requests += 1;
        let status = response.status().as_u16();
        let location = response.headers().get(header::LOCATION).and_then(|v| v.to_str().ok()).map(str::to_owned);
        let (true, Some(location)) = (matches!(status, 301 | 302 | 303 | 307 | 308), location) else {
            break response;
        };
        let next = current
            .join(&location)
            .map_err(|e| url_error(&op, &current, format!("failed to parse Location header {location:?}: {e}")))?;
        // 301, 302, and 303 continue as a GET without a body (HEAD stays
        // HEAD); 307 and 308 repeat the request.
        if matches!(status, 301..=303) {
            body = None;
            if method != Method::GET && method != Method::HEAD {
                method = Method::GET;
            }
        }
        if requests >= 3 {
            return Err(url_error(&op, &next, "too many redirects"));
        }
        if !token.is_empty() && !same_origin(url, &next) {
            return Err(url_error(&op, &next, "Canvas redirected to a different origin"));
        }
        if let Err(err) = outbound.safe_url(next.as_str()) {
            return Err(url_error(&op, &next, err));
        }
        current = next;
    };
    let status = response.status().as_u16();
    if status == 401 || status == 403 {
        anyhow::bail!("the service rejected the token or feed url");
    }
    let headers = response.headers().clone();
    let mut response = response;
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        body.extend_from_slice(&chunk);
        if body.len() > RELAY_MAX_BODY {
            anyhow::bail!("the service response is too large");
        }
    }
    Ok(RelayReply { body: body.into(), headers, status })
}

/// The operation name in transport errors: "Get", "Post", "Put", ….
fn url_error_op(method: &Method) -> String {
    let m = method.as_str();
    let mut op = m.get(..1).unwrap_or_default().to_owned();
    op.push_str(&m.get(1..).unwrap_or_default().to_lowercase());
    op
}

/// A transport error naming the request: `Get "https://…": reason`.
fn url_error(op: &str, u: &url::Url, err: impl std::fmt::Display) -> anyhow::Error {
    anyhow::anyhow!("{op} {:?}: {err}", u.as_str())
}

/// A completion request for a Canvas planner item: the guest endpoint's
/// body, and what an account completion builds from a task document.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub(crate) struct CanvasCompletion {
    #[serde(alias = "Base", deserialize_with = "null_default")]
    pub(crate) base: String,
    #[serde(alias = "Token", deserialize_with = "null_default")]
    pub(crate) token: String,
    #[serde(rename = "type", alias = "Type", deserialize_with = "null_default")]
    pub(crate) kind: String,
    #[serde(alias = "ID", alias = "Id", deserialize_with = "null_default")]
    pub(crate) id: String,
    #[serde(alias = "OverrideID", alias = "overrideID", deserialize_with = "null_default")]
    pub(crate) override_id: String,
    #[serde(alias = "CourseID", alias = "courseID", deserialize_with = "null_default")]
    pub(crate) course_id: String,
    #[serde(alias = "ModuleItem", deserialize_with = "null_default")]
    pub(crate) module_item: String,
    #[serde(alias = "Done", deserialize_with = "null_default")]
    pub(crate) done: bool,
}

/// A module item that must be marked done: its `moduleId:itemId` key and
/// whether the student completed it.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct ModuleDone {
    pub(crate) key: String,
    pub(crate) completed: bool,
}

/// A JSON number kept as its text: a number, a string holding a number, or
/// null (empty). Canvas IDs arrive in either form.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct NumberText(pub String);

impl std::fmt::Display for NumberText {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for NumberText {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        match Value::deserialize(d)? {
            Value::Null => Ok(Self::default()),
            Value::Number(n) => Ok(Self(n.to_string())),
            Value::String(s) if serde_json::from_str::<serde_json::Number>(&s).is_ok() => Ok(Self(s)),
            other => Err(serde::de::Error::custom(format!("invalid number {other}"))),
        }
    }
}

/// Counters one Canvas operation shares across its parallel requests.
#[derive(Default)]
struct Budget {
    requests: usize,
    bytes: usize,
    failure: Option<String>,
}

/// One Canvas operation: a base origin, a token, and a shared request and
/// byte budget (including module lookups). At most [`PLANNER_CONCURRENCY`]
/// requests are in flight. The first failure in [`CanvasClient::parallel`]
/// cancels the rest. The caller bounds total time by wrapping the operation
/// in a timeout.
pub(crate) struct CanvasClient {
    outbound: Outbound,
    base: url::Url,
    token: String,
    slots: Semaphore,
    budget: Mutex<Budget>,
    cancel: CancellationToken,
}

/// Link headers name the next page this way.
static RE_LINK: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"<([^>]+)>;\s*rel="next""#).expect("valid pattern"));

/// One URL path segment: everything but ASCII letters, digits, and
/// `-._~$&+:=@` is percent-encoded with uppercase hex.
const PATH_SEGMENT: &AsciiSet = &NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'.')
    .remove(b'_')
    .remove(b'~')
    .remove(b'$')
    .remove(b'&')
    .remove(b'+')
    .remove(b':')
    .remove(b'=')
    .remove(b'@');

/// Escapes `s` as one URL path segment (see [`PATH_SEGMENT`]).
pub(crate) fn path_escape(s: &str) -> String {
    utf8_percent_encode(s, PATH_SEGMENT).to_string()
}

/// The error for work stopped by a cancelled operation.
fn canceled() -> anyhow::Error {
    anyhow::anyhow!("context canceled")
}

/// The error for an operation out of time.
fn deadline_exceeded() -> anyhow::Error {
    anyhow::anyhow!("context deadline exceeded")
}

const TOO_MANY_REQUESTS: &str = "Canvas required too many requests; the operation could not be completed";
const TOO_MUCH_DATA: &str = "Canvas returned too much data; the operation could not be completed";

impl CanvasClient {
    /// A client for one operation against `base` with `token`.
    pub(crate) fn new(outbound: Outbound, base: url::Url, token: String) -> Self {
        Self {
            outbound,
            base,
            token,
            slots: Semaphore::new(PLANNER_CONCURRENCY),
            budget: Mutex::new(Budget::default()),
            cancel: CancellationToken::new(),
        }
    }

    /// Runs `f(0..n)` on at most `limit` workers. The first error is kept,
    /// cancels the operation, and is returned; otherwise an earlier
    /// cancellation is returned as an error. Nested collections share the
    /// request slots and budget, so they cannot multiply outbound traffic.
    pub(crate) async fn parallel<F, Fut>(&self, n: usize, limit: usize, f: F) -> anyhow::Result<()>
    where
        F: Fn(usize) -> Fut,
        Fut: Future<Output = anyhow::Result<()>>,
    {
        let next = AtomicUsize::new(0);
        let worker = || async {
            loop {
                let i = next.fetch_add(1, Ordering::Relaxed);
                if i >= n {
                    return;
                }
                let result = if self.cancel.is_cancelled() { Err(canceled()) } else { f(i).await };
                if let Err(err) = result {
                    let mut budget = self.budget.lock().expect("canvas budget lock");
                    if budget.failure.is_none() {
                        budget.failure = Some(err.to_string());
                    }
                    drop(budget);
                    self.cancel.cancel();
                    return;
                }
            }
        };
        futures::future::join_all((0..n.min(limit)).map(|_| worker())).await;
        let failure = self.budget.lock().expect("canvas budget lock").failure.clone();
        match failure {
            Some(message) => Err(anyhow::Error::msg(message)),
            None if self.cancel.is_cancelled() => Err(canceled()),
            None => Ok(()),
        }
    }

    /// One request to a path or URL relative to the base, which must stay on
    /// the base's origin ("Canvas returned a page on a different origin").
    /// GETs answered 429 retry twice, waiting 250 ms doubling or the
    /// `Retry-After` value up to five seconds; longer waits return the 429.
    /// Every attempt counts against the budget. Budget overruns fail with
    /// "Canvas required too many requests; the operation could not be
    /// completed" or "Canvas returned too much data; the operation could not
    /// be completed".
    pub(crate) async fn request(
        &self,
        method: reqwest::Method,
        path: &str,
        payload: Option<&serde_json::Value>,
    ) -> anyhow::Result<RelayReply> {
        let u = self.base.join(path)?;
        let u = self.outbound.safe_url(u.as_str()).map_err(anyhow::Error::msg)?;
        if !same_origin(&self.base, &u) {
            anyhow::bail!("Canvas returned a page on a different origin");
        }
        let mut attempt = 0;
        loop {
            let reply = self.request_once(&method, &u, payload).await?;
            // Only reads can be retried safely. Long throttle delays are left
            // to the next sync the student starts.
            if method != Method::GET || reply.status != 429 || attempt >= 2 {
                return Ok(reply);
            }
            let mut delay = Duration::from_millis(250 << attempt);
            let retry_after = reply.headers.get(header::RETRY_AFTER).and_then(|v| v.to_str().ok()).unwrap_or_default();
            if !retry_after.is_empty() {
                if let Ok(seconds) = retry_after.parse::<i64>()
                    && seconds >= 0
                {
                    if seconds > 5 {
                        return Ok(reply);
                    }
                    delay = Duration::from_secs(seconds as u64);
                } else if let Ok(when) = chrono::DateTime::parse_from_rfc2822(retry_after) {
                    delay = (when.with_timezone(&chrono::Utc) - chrono::Utc::now()).to_std().unwrap_or_default();
                }
            }
            if delay > Duration::from_secs(5) {
                return Ok(reply);
            }
            tokio::select! {
                () = self.cancel.cancelled() => return Err(canceled()),
                () = tokio::time::sleep(delay) => {}
            }
            attempt += 1;
        }
    }

    async fn request_once(
        &self,
        method: &Method,
        u: &url::Url,
        payload: Option<&serde_json::Value>,
    ) -> anyhow::Result<RelayReply> {
        let _slot = tokio::select! {
            slot = self.slots.acquire() => slot?,
            () = self.cancel.cancelled() => return Err(canceled()),
        };
        if self.cancel.is_cancelled() {
            return Err(canceled());
        }
        {
            let mut budget = self.budget.lock().expect("canvas budget lock");
            if budget.requests >= CANVAS_REQUEST_BUDGET {
                anyhow::bail!(TOO_MANY_REQUESTS);
            }
            if budget.bytes > CANVAS_BYTE_BUDGET {
                anyhow::bail!(TOO_MUCH_DATA);
            }
            budget.requests += 1;
        }
        let result = tokio::select! {
            result = relay_raw(&self.outbound, method.clone(), u, &self.token, payload) => result,
            () = self.cancel.cancelled() => Err(canceled()),
        };
        let read = result.as_ref().map(|r| r.body.len()).unwrap_or(0);
        let too_large = {
            let mut budget = self.budget.lock().expect("canvas budget lock");
            budget.bytes += read;
            budget.bytes > CANVAS_BYTE_BUDGET
        };
        if too_large {
            anyhow::bail!(TOO_MUCH_DATA);
        }
        result
    }

    /// Reads a paginated Canvas list, following `Link: rel="next"` for at
    /// most ten pages ("Canvas returned too many pages; the operation could
    /// not be completed"). A non-200 status fails with "Canvas answered N";
    /// a body that is not a JSON array with "unexpected reply from Canvas".
    pub(crate) async fn list<T: DeserializeOwned>(&self, path: &str) -> anyhow::Result<Vec<T>> {
        let mut all = Vec::new();
        let mut path = path.to_owned();
        let mut page = 0;
        while !path.is_empty() && page < 10 {
            let reply = self.request(Method::GET, &path, None).await?;
            if reply.status != 200 {
                anyhow::bail!("Canvas answered {}", reply.status);
            }
            let items: Option<Vec<T>> =
                serde_json::from_slice(&reply.body).map_err(|_| anyhow::anyhow!("unexpected reply from Canvas"))?;
            all.extend(items.ok_or_else(|| anyhow::anyhow!("unexpected reply from Canvas"))?);
            let link = reply.headers.get(header::LINK).and_then(|v| v.to_str().ok()).unwrap_or_default();
            let mut next = String::new();
            if let Some(m) = RE_LINK.captures(link) {
                let current = self.base.join(&path)?;
                next = current.join(&m[1])?.to_string();
            }
            path = next;
            page += 1;
        }
        if !path.is_empty() {
            anyhow::bail!("Canvas returned too many pages; the operation could not be completed");
        }
        Ok(all)
    }

    /// Loads one course's modules, including separately paginated item
    /// lists, and returns the items that must be marked done, keyed by
    /// `Type:contentId` (or `Page:slug`).
    pub(crate) async fn module_items(&self, course_id: &str) -> anyhow::Result<HashMap<String, ModuleDone>> {
        let root = format!("/api/v1/courses/{}", path_escape(course_id));
        let modules: Vec<CanvasModule> = self.list(&format!("{root}/modules?include[]=items&per_page=100")).await?;
        // Canvas omits items it deems too numerous to inline. Fetch those
        // lists together, then fold them in the original order so duplicate
        // requirements resolve the same way on every sync.
        let missing: Vec<usize> = (0..modules.len()).filter(|&i| modules[i].items.is_none()).collect();
        let fetched: Mutex<HashMap<usize, Vec<ModuleItem>>> = Mutex::new(HashMap::new());
        self.parallel(missing.len(), CANVAS_CONCURRENCY, |i| {
            let index = missing[i];
            let path = format!("{root}/modules/{}/items?per_page=100", path_escape(&modules[index].id.0));
            let fetched = &fetched;
            async move {
                let list = self.list::<ModuleItem>(&path).await?;
                fetched.lock().expect("module items lock").insert(index, list);
                Ok(())
            }
        })
        .await?;
        let fetched = fetched.into_inner().expect("module items lock");
        let mut out = HashMap::new();
        for (index, module) in modules.iter().enumerate() {
            let items = match &module.items {
                Some(items) => items.as_slice(),
                None => fetched.get(&index).map(Vec::as_slice).unwrap_or_default(),
            };
            for item in items {
                let Some(requirement) = &item.requirement else { continue };
                if requirement.kind != "must_mark_done" {
                    continue;
                }
                let reference = if item.kind == "Page" { item.page_url.clone() } else { item.content_id.0.clone() };
                out.insert(
                    format!("{}:{reference}", item.kind),
                    ModuleDone { key: format!("{}:{}", module.id, item.id), completed: requirement.completed },
                );
            }
        }
        Ok(out)
    }
}

#[derive(Default, Deserialize)]
#[serde(default)]
struct CanvasModule {
    id: NumberText,
    items: Option<Vec<ModuleItem>>,
}

#[derive(Default, Deserialize)]
#[serde(default)]
struct ModuleItem {
    id: NumberText,
    #[serde(rename = "type", deserialize_with = "null_default")]
    kind: String,
    content_id: NumberText,
    #[serde(deserialize_with = "null_default")]
    page_url: String,
    #[serde(rename = "completion_requirement")]
    requirement: Option<Requirement>,
}

#[derive(Default, Deserialize)]
#[serde(default)]
struct Requirement {
    #[serde(rename = "type", deserialize_with = "null_default")]
    kind: String,
    #[serde(deserialize_with = "null_default")]
    completed: bool,
}

#[derive(Default, Deserialize)]
#[serde(default)]
struct CanvasCourse {
    id: NumberText,
    #[serde(deserialize_with = "null_default")]
    name: String,
    #[serde(rename = "course_code", deserialize_with = "null_default")]
    code: String,
}

#[derive(Default, Deserialize)]
#[serde(default)]
struct PlannerItem {
    #[serde(rename = "plannable_type", deserialize_with = "null_default")]
    kind: String,
    #[serde(deserialize_with = "null_default")]
    context_name: String,
    course_id: NumberText,
    #[serde(rename = "html_url", deserialize_with = "null_default")]
    url: String,
    #[serde(deserialize_with = "null_default")]
    plannable: Plannable,
    submissions: Option<Value>,
    #[serde(rename = "planner_override")]
    override_: Option<PlannerOverride>,
}

#[derive(Default, Deserialize)]
#[serde(default)]
struct Plannable {
    id: NumberText,
    #[serde(deserialize_with = "null_default")]
    title: String,
    #[serde(deserialize_with = "null_default")]
    due_at: String,
    #[serde(deserialize_with = "null_default")]
    todo_date: String,
    /// Calendar events carry a start instead of a due time.
    #[serde(deserialize_with = "null_default")]
    start_at: String,
    #[serde(deserialize_with = "null_default")]
    url: String,
}

#[derive(Default, Deserialize)]
#[serde(default)]
struct PlannerOverride {
    id: NumberText,
    #[serde(rename = "marked_complete", deserialize_with = "null_default")]
    complete: bool,
}

#[derive(Default, Deserialize)]
#[serde(default)]
struct Override {
    id: NumberText,
    #[serde(rename = "plannable_type", deserialize_with = "null_default")]
    kind: String,
    #[serde(rename = "plannable_id")]
    plannable: NumberText,
    #[serde(rename = "marked_complete", deserialize_with = "null_default")]
    complete: bool,
}

/// Planner types whose items a module can require marking done, and the
/// module item type each appears as.
fn module_type(kind: &str) -> Option<&'static str> {
    match kind {
        "assignment" => Some("Assignment"),
        "quiz" => Some("Quiz"),
        "discussion_topic" => Some("Discussion"),
        "wiki_page" => Some("Page"),
        _ => None,
    }
}

/// A boolean field of a submission object: the exact name, else a
/// case-insensitive match; false when the submissions value is not an
/// object.
fn submission_flag(submissions: Option<&Value>, name: &str) -> bool {
    let Some(Value::Object(fields)) = submissions else { return false };
    fields
        .get(name)
        .or_else(|| fields.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v))
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

/// `scheme://host[:port]` of a URL, the prefix Canvas's relative links take.
fn origin_prefix(base: &url::Url) -> String {
    base[..url::Position::BeforePath].to_owned()
}

impl Server {
    /// Relays one GET and requires a 200 ("the service answered N").
    pub(crate) async fn relay(
        &self,
        url: &url::Url,
        token: &str,
    ) -> anyhow::Result<(bytes::Bytes, reqwest::header::HeaderMap)> {
        let reply = relay_raw(&self.outbound, Method::GET, url, token, None).await?;
        if reply.status != 200 {
            anyhow::bail!("the service answered {}", reply.status);
        }
        Ok((reply.body, reply.headers))
    }

    /// Reads the student's Canvas planner into tasks within a 30-second
    /// budget, holding one of the provider host's import slots. With
    /// `modules` false the per-course module lookups are skipped. When
    /// `preview` is given, the planner tasks are sent on it before module
    /// lookups start; a closed receiver aborts the import.
    ///
    /// Module lookups are the slow part of a refresh: one or more requests
    /// per course. Planner items already carry the title, due date,
    /// submission state, and to-do checkmark, so a fast refresh needs only
    /// them; module routing (for readings a student marks done in a module)
    /// is resolved on connect, on an explicit rescan, and daily.
    pub(crate) async fn fetch_canvas_tasks(
        &self,
        base: &url::Url,
        token: &str,
        preview: Option<tokio::sync::mpsc::UnboundedSender<Vec<Task>>>,
        modules: bool,
    ) -> anyhow::Result<Vec<Task>> {
        tokio::time::timeout(Duration::from_secs(30), self.read_canvas_tasks(base, token, preview, modules))
            .await
            .unwrap_or_else(|_| Err(deadline_exceeded()))
    }

    async fn read_canvas_tasks(
        &self,
        base: &url::Url,
        token: &str,
        preview: Option<tokio::sync::mpsc::UnboundedSender<Vec<Task>>>,
        modules: bool,
    ) -> anyhow::Result<Vec<Task>> {
        let host = base.host_str().unwrap_or_default().trim_start_matches('[').trim_end_matches(']').to_lowercase();
        let _slot = self.provider_slot(&host).await?;
        let client = CanvasClient::new(self.outbound.clone(), base.clone(), token.to_owned());
        let from = self.now() - chrono::Duration::days(7);
        // Days from -7 through +120, walked as contiguous windows. Windows
        // share their boundary day (both bounds are inclusive); duplicate
        // boundary items are removed below by identity.
        const SPAN: i64 = 127;
        let windows = PLANNER_WINDOWS as i64;
        let day = |offset: i64| (from + chrono::Duration::days(offset)).format("%Y-%m-%d").to_string();
        let bounds: Vec<(String, String)> =
            (0..windows).map(|i| (day(SPAN * i / windows), day(SPAN * (i + 1) / windows))).collect();
        let courses: Mutex<Vec<CanvasCourse>> = Mutex::new(Vec::new());
        let parts: Mutex<Vec<Vec<PlannerItem>>> = Mutex::new((0..PLANNER_WINDOWS).map(|_| Vec::new()).collect());
        // Job 0 fetches the active courses; jobs 1..N each walk one window.
        client
            .parallel(PLANNER_WINDOWS + 1, PLANNER_CONCURRENCY, |i| {
                let (client, courses, parts, bounds) = (&client, &courses, &parts, &bounds);
                async move {
                    if i == 0 {
                        let list = client.list("/api/v1/courses?enrollment_state=active&per_page=100").await?;
                        *courses.lock().expect("courses lock") = list;
                        return Ok(());
                    }
                    let (start, end) = &bounds[i - 1];
                    let part = client
                        .list(&format!("/api/v1/planner/items?per_page=100&start_date={start}&end_date={end}"))
                        .await?;
                    parts.lock().expect("planner lock")[i - 1] = part;
                    Ok(())
                }
            })
            .await?;
        // Merge the windows, keeping one copy of each planner item. Identity
        // is the plannable type and id, the same key the task UID uses.
        let mut seen_items = std::collections::HashSet::new();
        let items: Vec<PlannerItem> = parts
            .into_inner()
            .expect("planner lock")
            .into_iter()
            .flatten()
            .filter(|it| seen_items.insert(format!("{}:{}", it.kind, it.plannable.id)))
            .collect();
        let courses: HashMap<String, CanvasCourse> =
            courses.into_inner().expect("courses lock").into_iter().map(|c| (c.id.0.clone(), c)).collect();
        let mut tasks = Vec::with_capacity(items.len());
        // The module requirement each task may have: (course, Type:id).
        let mut refs: Vec<Option<(String, String)>> = Vec::with_capacity(items.len());
        let mut course_ids: Vec<String> = Vec::new();
        for it in items {
            if it.kind == "announcement" {
                continue;
            }
            // A course calendar event (an exam an instructor posted, say) is
            // due when it starts; an all-day one starts at midnight, campus time.
            let p = &it.plannable;
            let due = [&p.due_at, &p.todo_date, &p.start_at].into_iter().find(|v| !v.is_empty()).cloned();
            let mut t = Task {
                uid: format!("canvas:{}:{}", it.kind, p.id),
                title: p.title.clone(),
                course: it.context_name.clone(),
                course_id: it.course_id.0.clone(),
                due: due.unwrap_or_default(),
                url: it.url.clone(),
                ..Task::default()
            };
            if let Some(c) = courses.get(&t.course_id) {
                t.course_code = c.code.clone();
                if t.course.is_empty() {
                    t.course = c.name.clone();
                }
            }
            if t.url.starts_with('/') {
                t.url = origin_prefix(base) + &t.url;
            }
            let overridden = it.override_.as_ref();
            if submission_flag(it.submissions.as_ref(), "submitted") {
                (t.done, t.reason) = (true, "submitted".into());
            } else if submission_flag(it.submissions.as_ref(), "graded") {
                (t.done, t.reason) = (true, "graded".into());
            } else if overridden.is_some_and(|o| o.complete) {
                (t.done, t.reason) = (true, "marked".into());
            }
            if let Some(o) = overridden {
                t.override_id = o.id.0.clone();
            }
            let mut reference = None;
            if let Some(mtype) = module_type(&it.kind)
                && !t.course_id.is_empty()
            {
                let id = if mtype == "Page" { p.url.clone() } else { p.id.0.clone() };
                reference = Some((t.course_id.clone(), format!("{mtype}:{id}")));
                if !course_ids.contains(&t.course_id) {
                    course_ids.push(t.course_id.clone());
                }
            }
            refs.push(reference);
            tasks.push(t);
        }
        if let Some(preview) = preview {
            preview.send(tasks.clone()).map_err(|_| anyhow::anyhow!("the client went away"))?;
        }
        if !modules || course_ids.is_empty() {
            return Ok(tasks);
        }
        let lists: Mutex<HashMap<String, HashMap<String, ModuleDone>>> = Mutex::new(HashMap::new());
        client
            .parallel(course_ids.len(), CANVAS_CONCURRENCY, |i| {
                let (client, lists, course) = (&client, &lists, &course_ids[i]);
                async move {
                    let found = client.module_items(course).await?;
                    lists.lock().expect("module lists lock").insert(course.clone(), found);
                    Ok(())
                }
            })
            .await?;
        let by_course = lists.into_inner().expect("module lists lock");
        for (task, reference) in tasks.iter_mut().zip(&refs) {
            let Some((course, key)) = reference else { continue };
            if let Some(mi) = by_course.get(course).and_then(|m| m.get(key)) {
                task.module_item = mi.key.clone();
                if mi.completed && !task.done {
                    (task.done, task.reason) = (true, "marked".into());
                }
            }
        }
        Ok(tasks)
    }

    /// Marks a planner item done or not done in Canvas (creating or updating
    /// its planner override) and, when the item has one, its module's "Mark
    /// as done" requirement. Returns `{"overrideId", "done", "module"}`.
    pub(crate) async fn complete_canvas(
        &self,
        base: &url::Url,
        input: &CanvasCompletion,
    ) -> anyhow::Result<serde_json::Map<String, serde_json::Value>> {
        let client = CanvasClient::new(self.outbound.clone(), base.clone(), input.token.clone());
        tokio::time::timeout(Duration::from_secs(30), complete_with(&client, input))
            .await
            .unwrap_or_else(|_| Err(deadline_exceeded()))
    }
}

/// Canvas keeps a to-do checkmark as a "planner override": created once,
/// updated after.
async fn complete_with(client: &CanvasClient, input: &CanvasCompletion) -> anyhow::Result<Map<String, Value>> {
    const API: &str = "/api/v1/planner/overrides";
    let update = |id: String| async move {
        let reply = client
            .request(
                Method::PUT,
                &format!("{API}/{}", path_escape(&id)),
                Some(&json!({ "marked_complete": input.done })),
            )
            .await?;
        anyhow::Ok(reply)
    };
    let reply = if !input.override_id.is_empty() {
        update(input.override_id.clone()).await?
    } else {
        let created = client
            .request(
                Method::POST,
                API,
                Some(&json!({ "plannable_type": input.kind, "plannable_id": input.id, "marked_complete": input.done })),
            )
            .await?;
        let mut reply = created;
        if reply.status == 400 {
            let existing: Vec<Override> = client.list(&format!("{API}?per_page=100")).await?;
            let normalize = |t: &str| t.replace('_', "");
            if let Some(o) = existing
                .iter()
                .find(|o| normalize(&o.kind).eq_ignore_ascii_case(&normalize(&input.kind)) && o.plannable.0 == input.id)
            {
                reply = update(o.id.0.clone()).await?;
            }
        }
        reply
    };
    if !(200..=299).contains(&reply.status) {
        anyhow::bail!("Canvas answered {}", reply.status);
    }
    let o: Override = serde_json::from_slice(&reply.body)
        .ok()
        .filter(|o: &Override| !o.id.0.is_empty())
        .ok_or_else(|| anyhow::anyhow!("unexpected reply from Canvas"))?;
    // The module's own "Mark as done" requirement, when the item has one.
    let mut module_ok = true;
    if !input.module_item.is_empty() && !input.course_id.is_empty() {
        module_ok = false;
        if let Some((module, item)) = input.module_item.split_once(':') {
            let path = format!(
                "/api/v1/courses/{}/modules/{}/items/{}/done",
                path_escape(&input.course_id),
                path_escape(module),
                path_escape(item)
            );
            let method = if input.done { Method::PUT } else { Method::DELETE };
            module_ok = matches!(client.request(method, &path, None).await, Ok(r) if (200..300).contains(&r.status));
        }
    }
    let mut out = Map::new();
    out.insert("overrideId".into(), Value::String(o.id.0));
    out.insert("done".into(), Value::Bool(o.complete));
    out.insert("module".into(), Value::Bool(module_ok));
    Ok(out)
}

#[derive(Default, Deserialize)]
#[serde(default)]
struct FeedRequest {
    #[serde(alias = "URL", alias = "Url", deserialize_with = "null_default")]
    url: String,
}

/// `POST /api/lms/feed`: fetches an iCalendar feed and returns it unchanged
/// as `text/calendar` (4 KiB body limit).
pub(crate) async fn handle_feed(State(s): State<Arc<Server>>, body: Body) -> ApiResult {
    let Some(input) = read_json::<FeedRequest>(body, 4 << 10).await else {
        return fail(400, "bad json");
    };
    let u = s.outbound.safe_url(&input.url).map_err(|e| ApiError::new(400, e))?;
    let (body, _) = s.relay(&u, "").await.map_err(|e| ApiError::new(502, e.to_string()))?;
    let head = &body[..body.len().min(200)];
    if !head.windows(b"BEGIN:VCALENDAR".len()).any(|w| w == b"BEGIN:VCALENDAR") {
        return fail(502, "that url is not a calendar feed");
    }
    let mut response = Response::new(Body::from(body));
    response.headers_mut().insert(header::CONTENT_TYPE, HeaderValue::from_static("text/calendar; charset=utf-8"));
    Ok(response)
}

#[derive(Default, Deserialize)]
#[serde(default)]
struct GuestCanvas {
    #[serde(alias = "Base", deserialize_with = "null_default")]
    base: String,
    #[serde(alias = "Token", deserialize_with = "null_default")]
    token: String,
}

/// Reports whether an `Accept` header asks for NDJSON with a positive weight.
fn accepts_ndjson(headers: &HeaderMap) -> bool {
    let accept = headers.get(header::ACCEPT).and_then(|v| v.to_str().ok()).unwrap_or_default();
    accept.split(',').any(|item| {
        let mut parts = item.trim().split(';');
        let media = parts.next().unwrap_or_default().trim().to_ascii_lowercase();
        if media != "application/x-ndjson" {
            return false;
        }
        let mut weight = None;
        for param in parts {
            let Some((key, value)) = param.split_once('=') else { return false };
            if key.trim().eq_ignore_ascii_case("q") {
                weight = Some(value.trim().trim_matches('"').to_owned());
            }
        }
        match weight {
            None => true,
            Some(q) => q.parse::<f64>().is_ok_and(|w| w > 0.0 && w <= 1.0),
        }
    })
}

/// One NDJSON event: `{"type": kind, "tasks": [...]}` or, with a message,
/// `{"type": kind, "error": message}`.
fn event_line(kind: &str, tasks: &[Task], message: &str) -> bytes::Bytes {
    let event = if message.is_empty() {
        json!({ "type": kind, "tasks": tasks })
    } else {
        json!({ "type": kind, "error": message })
    };
    let mut line = serde_json::to_vec(&event).expect("events serialize");
    line.push(b'\n');
    line.into()
}

/// Aborts a spawned import when the response that streams it is dropped.
struct AbortOnDrop<T>(tokio::task::JoinHandle<T>);

impl<T> Drop for AbortOnDrop<T> {
    fn drop(&mut self) {
        self.0.abort();
    }
}

/// `POST /api/lms/canvas`: reads a guest's Canvas planner (assignments,
/// quizzes, discussions, notes, and course calendar events such as exams)
/// with the token in the body. With `Accept: application/x-ndjson` it
/// streams `tasks`, then `complete` (or `error`) events.
pub(crate) async fn handle_canvas(State(s): State<Arc<Server>>, headers: HeaderMap, body: Body) -> ApiResult {
    let mut response = canvas_response(s, &headers, body).await.unwrap_or_else(IntoResponse::into_response);
    let h = response.headers_mut();
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    h.append(header::VARY, HeaderValue::from_static("Accept"));
    Ok(response)
}

async fn canvas_response(s: Arc<Server>, headers: &HeaderMap, body: Body) -> ApiResult {
    let input = read_json::<GuestCanvas>(body, 4 << 10).await.filter(|i| !i.token.is_empty());
    let Some(input) = input else { return fail(400, "base url and token are required") };
    let base = s.outbound.safe_url(&input.base).map_err(|e| ApiError::new(400, e))?;
    // A guest import is a one-shot the student started, with no background
    // refresh to fill modules in later, so it always resolves module routing.
    if !accepts_ndjson(headers) {
        let tasks = s.fetch_canvas_tasks(&base, &input.token, None, true).await;
        return ok(&tasks.map_err(|e| ApiError::new(502, e.to_string()))?);
    }
    let (sender, mut preview) = tokio::sync::mpsc::unbounded_channel();
    let import = {
        let s = s.clone();
        AbortOnDrop(tokio::spawn(async move { s.fetch_canvas_tasks(&base, &input.token, Some(sender), true).await }))
    };
    let mut import = import;
    let first = tokio::select! {
        biased;
        Some(tasks) = preview.recv() => tasks,
        result = &mut import.0 => {
            // The import ended before any tasks were ready: a failure.
            let result = result.unwrap_or_else(|e| Err(anyhow::anyhow!("the import stopped: {e}")));
            return match result {
                Ok(tasks) => stream_response(vec![event_line("complete", &tasks, "")], None),
                Err(err) => fail(502, err.to_string()),
            };
        }
    };
    stream_response(vec![event_line("tasks", &first, "")], Some(import))
}

/// A 200 NDJSON response: `lines`, then the final event of `import` if any.
fn stream_response(lines: Vec<bytes::Bytes>, import: Option<AbortOnDrop<anyhow::Result<Vec<Task>>>>) -> ApiResult {
    let state = (lines.into_iter(), import);
    let stream = futures::stream::unfold(state, |(mut lines, mut import)| async move {
        if let Some(line) = lines.next() {
            return Some((Ok::<_, std::convert::Infallible>(line), (lines, import)));
        }
        let mut running = import.take()?;
        let result = (&mut running.0).await.unwrap_or_else(|e| Err(anyhow::anyhow!("the import stopped: {e}")));
        let line = match result {
            Ok(tasks) => event_line("complete", &tasks, ""),
            Err(err) => event_line("error", &[], &err.to_string()),
        };
        Some((Ok(line), (lines, None)))
    });
    let mut response = Response::new(Body::from_stream(stream));
    *response.status_mut() = StatusCode::OK;
    let h = response.headers_mut();
    h.insert(header::CONTENT_TYPE, HeaderValue::from_static("application/x-ndjson; charset=utf-8"));
    h.insert("x-accel-buffering", HeaderValue::from_static("no"));
    Ok(response)
}

/// `POST /api/lms/canvas/complete`: marks a guest's planner item done or
/// not, the same as ticking it in Canvas's own to-do list, so Honorum and
/// Canvas agree about work that involves no submission (readings, notes).
pub(crate) async fn handle_canvas_complete(State(s): State<Arc<Server>>, body: Body) -> ApiResult {
    let input = read_json::<CanvasCompletion>(body, 4 << 10)
        .await
        .filter(|i| !i.token.is_empty() && !i.kind.is_empty() && !i.id.is_empty());
    let Some(input) = input else { return fail(400, "base url, token, type and id are required") };
    let base = s.outbound.safe_url(&input.base).map_err(|e| ApiError::new(400, e))?;
    let out = s.complete_canvas(&base, &input).await.map_err(|e| ApiError::new(502, e.to_string()))?;
    ok(&out)
}

#[cfg(test)]
#[path = "lms_tests.rs"]
pub(crate) mod tests;
