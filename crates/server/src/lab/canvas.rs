//! A simulated Canvas for the performance and capacity labs: courses,
//! planner items, and module items for seeded users, every response delayed
//! by a fixed latency, with request counts by path.

use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::Router;
use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderValue, Method, StatusCode, Uri, header};
use axum::response::{IntoResponse, Response};
use chrono::{DateTime, Days, NaiveDate, SecondsFormat, TimeDelta, Timelike, Utc};
use serde_json::{Map, Value, json};

/// Answers the Canvas API calls an import makes with a fixed student: six
/// courses, 150 planner items from a week ago to four months ahead, and
/// modules whose pages must be marked done. Every response waits
/// `latency` first; a request abandoned during the wait gets no answer.
pub(super) struct PerfCanvas {
    latency: Duration,
    /// Requests received, including abandoned ones.
    pub(crate) requests: AtomicI64,
    /// The most recent 100 requests as `METHOD /path?query`, for checking
    /// what an import asked for.
    paths: Mutex<Vec<String>>,
    items: Vec<Value>,
    due: Vec<DateTime<Utc>>,
}

impl PerfCanvas {
    /// Builds the fixture relative to the current time.
    pub(super) fn new(latency: Duration) -> Arc<Self> {
        let types = ["assignment", "assignment", "quiz", "assignment", "discussion_topic", "wiki_page"];
        let now = Utc::now();
        let start = now - TimeDelta::days(7);
        let (mut items, mut due_at) = (Vec::with_capacity(150), Vec::with_capacity(150));
        for i in 0..150i64 {
            let course = 1001 + i % 6;
            let due = start + TimeDelta::hours(i * 127 * 24 / 150);
            let due =
                due.with_minute(0).and_then(|d| d.with_second(0)).and_then(|d| d.with_nanosecond(0)).unwrap_or(due);
            let kind = types[((i / 6) as usize) % types.len()];
            let due_text = due.to_rfc3339_opts(SecondsFormat::Secs, true);
            let mut plannable = Map::new();
            plannable.insert("id".into(), json!(5000 + i));
            plannable.insert("title".into(), json!(format!("{} {}", kind.replace('_', " "), i + 1)));
            if kind == "wiki_page" {
                plannable.insert("url".into(), json!(format!("reading-{i}")));
                plannable.insert("todo_date".into(), json!(due_text));
            } else {
                plannable.insert("due_at".into(), json!(due_text));
            }
            items.push(json!({
                "plannable_type": kind,
                "course_id": course,
                "context_name": format!("Course {course}"),
                "html_url": format!("/courses/{course}/{kind}s/{}", 5000 + i),
                "plannable": plannable,
                "submissions": {"submitted": due < now},
                "planner_override": null,
            }));
            due_at.push(due);
        }
        Arc::new(Self { latency, requests: AtomicI64::new(0), paths: Mutex::new(Vec::new()), items, due: due_at })
    }

    /// The recorded recent requests, oldest first.
    pub(super) fn paths(&self) -> Vec<String> {
        self.paths.lock().expect("paths lock").clone()
    }

    /// The fixture as an HTTP service.
    pub(super) fn router(self: &Arc<Self>) -> Router {
        Router::new().fallback(serve).with_state(self.clone())
    }

    /// Serves the fixture on `listener` until the returned task is aborted.
    pub(super) fn spawn(self: &Arc<Self>, listener: tokio::net::TcpListener) -> tokio::task::JoinHandle<()> {
        let app = self.router();
        tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        })
    }
}

/// A JSON body: compact JSON and a trailing newline, labeled `text/plain;
/// charset=utf-8` like every simulated response.
fn encoded(value: &Value) -> Response {
    let mut body = serde_json::to_vec(value).unwrap_or_default();
    body.push(b'\n');
    plain(body)
}

fn plain(body: impl Into<axum::body::Body>) -> Response {
    let mut response = Response::new(body.into());
    response.headers_mut().insert(header::CONTENT_TYPE, HeaderValue::from_static("text/plain; charset=utf-8"));
    response
}

async fn serve(State(c): State<Arc<PerfCanvas>>, method: Method, uri: Uri, body: Bytes) -> Response {
    c.requests.fetch_add(1, Ordering::Relaxed);
    let request_uri = uri.path_and_query().map(|p| p.as_str().to_owned()).unwrap_or_else(|| uri.path().to_owned());
    {
        let mut paths = c.paths.lock().expect("paths lock");
        let keep = paths.len().saturating_sub(99);
        paths.drain(..keep);
        paths.push(format!("{method} {request_uri}"));
    }
    tokio::time::sleep(c.latency).await;
    let path = uri.path();
    let query: Vec<(String, String)> =
        url::form_urlencoded::parse(uri.query().unwrap_or_default().as_bytes()).into_owned().collect();
    let get = |key: &str| query.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str()).unwrap_or_default();
    if path == "/api/v1/courses" {
        let out: Vec<Value> = (1001..=1006)
            .map(|id| json!({"id": id, "name": format!("Course {id}"), "course_code": format!("PERF {id}-001")}))
            .collect();
        return encoded(&Value::Array(out));
    }
    if path == "/api/v1/planner/items" {
        // An unparsable date is earlier than every item.
        let date = |key| NaiveDate::parse_from_str(get(key), "%Y-%m-%d").unwrap_or(NaiveDate::MIN);
        let from = date("start_date").and_time(Default::default()).and_utc();
        let to =
            date("end_date").checked_add_days(Days::new(1)).unwrap_or_default().and_time(Default::default()).and_utc();
        let per = get("per_page").parse::<usize>().ok().filter(|&n| n > 0).unwrap_or(10);
        let page = get("page").parse::<usize>().unwrap_or(0);
        let matching: Vec<&Value> =
            c.items.iter().zip(&c.due).filter(|(_, due)| **due >= from && **due < to).map(|(it, _)| it).collect();
        let end = (page + 1) * per;
        let mut response;
        if end < matching.len() {
            let page_items: Vec<Value> = matching[page * per..end].iter().map(|v| (*v).clone()).collect();
            response = encoded(&Value::Array(page_items));
            // The next-page query: keys sorted, each value form-encoded.
            let mut next: Vec<(String, String)> = query.iter().filter(|(k, _)| k != "page").cloned().collect();
            next.push(("page".into(), (page + 1).to_string()));
            next.sort_by(|a, b| a.0.cmp(&b.0));
            let encoded_query = url::form_urlencoded::Serializer::new(String::new()).extend_pairs(next).finish();
            let link = format!("<{path}?{encoded_query}>; rel=\"next\"");
            if let Ok(value) = HeaderValue::from_str(&link) {
                response.headers_mut().insert(header::LINK, value);
            }
        } else {
            let start = (page * per).min(matching.len());
            let rest: Vec<Value> = matching[start..].iter().map(|v| (*v).clone()).collect();
            response = encoded(&Value::Array(rest));
        }
        return response;
    }
    if path.ends_with("/modules") {
        let course: i64 = path.split('/').nth(4).and_then(|s| s.parse().ok()).unwrap_or(0);
        let items: Vec<Value> = c
            .items
            .iter()
            .enumerate()
            .filter(|(_, it)| it["course_id"] == course && it["plannable_type"] == "wiki_page")
            .map(|(i, it)| {
                json!({
                    "id": 9000 + i,
                    "type": "Page",
                    "page_url": it["plannable"]["url"],
                    "completion_requirement": {"type": "must_mark_done", "completed": false},
                })
            })
            .collect();
        return encoded(&json!([{"id": course * 10 + 1, "items": items}]));
    }
    if path.starts_with("/api/v1/planner/overrides") {
        let mut input: Map<String, Value> = serde_json::from_slice(&body).unwrap_or_default();
        input.insert("id".into(), json!(77));
        return encoded(&Value::Object(input));
    }
    if path.ends_with("/done") {
        return StatusCode::NO_CONTENT.into_response();
    }
    plain("[]")
}
