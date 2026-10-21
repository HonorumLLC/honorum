//! Canvas imports and the LMS relay: concurrent collection, streaming,
//! request and byte budgets, throttling, pagination, origin checks, and a
//! latency benchmark, plus the fake upstream server the Canvas tests share.

use std::future::Future;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use std::time::{Duration, Instant};

use axum::body::Body;
use axum::extract::Request;
use axum::http::{HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;
use tower::ServiceExt;

use super::*;
use crate::test_support::TestResponse;
use crate::test_support::{TestServer, test_server};

/// A loopback HTTP server standing in for Canvas or a calendar host.
/// Dropping it stops accepting connections.
pub(crate) struct Upstream {
    pub(crate) url: String,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for Upstream {
    fn drop(&mut self) {
        self.task.abort();
    }
}

/// Serves every request with `handler`.
pub(crate) async fn upstream<F, Fut>(handler: F) -> Upstream
where
    F: Fn(Request) -> Fut + Clone + Send + Sync + 'static,
    Fut: Future<Output = Response> + Send + 'static,
{
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let app = axum::Router::new().fallback(move |request: Request| handler(request));
    let task = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    Upstream { url, task }
}

/// A response with `status` and a plain body.
pub(crate) fn reply(status: u16, body: impl Into<String>) -> Response {
    (StatusCode::from_u16(status).unwrap(), body.into()).into_response()
}

/// `http.Error(w, message, status)`.
pub(crate) fn error(status: u16, message: &str) -> Response {
    reply(status, format!("{message}\n"))
}

/// A response with one extra header.
pub(crate) fn with_header(mut response: Response, name: &'static str, value: &str) -> Response {
    response.headers_mut().insert(name, HeaderValue::from_str(value).unwrap());
    response
}

/// The query parameter `name` of a request, or "".
pub(crate) fn query(request: &Request, name: &str) -> String {
    let q = request.uri().query().unwrap_or_default();
    url::form_urlencoded::parse(q.as_bytes()).find(|(k, _)| k == name).map(|(_, v)| v.into_owned()).unwrap_or_default()
}

/// Fires its token when dropped: a handler holding one learns that the
/// request was abandoned.
pub(crate) struct OnDrop(pub CancellationToken);

impl Drop for OnDrop {
    fn drop(&mut self) {
        self.0.cancel();
    }
}

/// A test server whose outbound requests may reach loopback hosts.
async fn local_server() -> TestServer {
    let t = test_server().await;
    t.server.outbound.set_allow_local(true);
    t
}

/// Posts a guest request to the full router.
async fn post(t: &TestServer, path: &str, body: &str, accept: Option<&str>) -> TestResponse {
    let mut headers = vec![("content-type", "application/json")];
    if let Some(accept) = accept {
        headers.push(("accept", accept));
    }
    t.client().send("POST", path, &headers, Body::from(body.to_owned())).await
}

fn canvas_body(base: &str, token: &str) -> String {
    json!({ "base": base, "token": token }).to_string()
}

fn tasks_of(res: &TestResponse) -> Vec<Task> {
    serde_json::from_slice(&res.body).unwrap_or_else(|e| panic!("{e}: {} {}", res.status, res.text()))
}

#[tokio::test]
async fn canvas_fetches_courses_and_planner_together() {
    let started = Arc::new(AtomicI32::new(0));
    let both = CancellationToken::new();
    let up = upstream({
        let (started, both) = (started.clone(), both.clone());
        move |_req: Request| {
            let (started, both) = (started.clone(), both.clone());
            async move {
                if started.fetch_add(1, Ordering::SeqCst) + 1 == 2 {
                    both.cancel();
                }
                both.cancelled().await;
                reply(200, "[]")
            }
        }
    })
    .await;
    let t = local_server().await;
    let res =
        tokio::time::timeout(Duration::from_secs(2), post(&t, "/api/lms/canvas", &canvas_body(&up.url, "tok"), None))
            .await
            .expect("independent collections did not overlap");
    assert_eq!((res.status.as_u16(), res.text().as_str()), (200, "[]\n"), "independent collections did not overlap");
}

#[tokio::test]
async fn canvas_streams_assignments_before_module_completion() {
    let module_started = CancellationToken::new();
    let finish_module = CancellationToken::new();
    let up = upstream({
        let (module_started, finish_module) = (module_started.clone(), finish_module.clone());
        move |req: Request| {
            let (module_started, finish_module) = (module_started.clone(), finish_module.clone());
            async move {
                match req.uri().path() {
                    "/api/v1/courses" => reply(200, r#"[{"id":1,"name":"Programming","course_code":"CS 101"}]"#),
                    "/api/v1/planner/items" => reply(
                        200,
                        r#"[{"plannable_type":"wiki_page","course_id":1,"plannable":{"id":40,"title":"Reading","url":"reading"}}]"#,
                    ),
                    "/api/v1/courses/1/modules" => {
                        module_started.cancel();
                        finish_module.cancelled().await;
                        reply(
                            200,
                            r#"[{"id":9,"items":[{"id":42,"type":"Page","page_url":"reading","completion_requirement":{"type":"must_mark_done","completed":true}}]}]"#,
                        )
                    }
                    _ => error(404, "404 page not found"),
                }
            }
        }
    })
    .await;
    let t = local_server().await;
    let request = axum::http::Request::post("/api/lms/canvas")
        .header("content-type", "application/json")
        .header("accept", "application/x-ndjson")
        .body(Body::from(canvas_body(&up.url, "tok")))
        .unwrap();
    let response = tokio::time::timeout(Duration::from_secs(2), crate::router(t.server.clone()).oneshot(request))
        .await
        .expect("assignments waited for blocked module scan")
        .unwrap();
    let header = |name: &str| response.headers().get(name).and_then(|v| v.to_str().ok()).unwrap_or_default().to_owned();
    assert!(
        response.status() == 200
            && header("cache-control") == "no-store"
            && header("x-accel-buffering") == "no"
            && header("content-type").starts_with("application/x-ndjson"),
        "unexpected streaming headers: {:?}",
        response.headers()
    );
    let mut body = response.into_body();
    let mut buffered = Vec::new();
    // Reads the next NDJSON event, or None at the end of the body.
    async fn next_event(body: &mut Body, buffered: &mut Vec<u8>) -> Option<Value> {
        loop {
            if let Some(end) = buffered.iter().position(|&b| b == b'\n') {
                let line: Vec<u8> = buffered.drain(..=end).collect();
                return Some(serde_json::from_slice(&line).unwrap());
            }
            match tokio::time::timeout(Duration::from_secs(2), body.frame()).await.expect("event was not flushed") {
                Some(frame) => {
                    if let Ok(data) = frame.unwrap().into_data() {
                        buffered.extend_from_slice(&data);
                    }
                }
                None => {
                    assert!(buffered.is_empty(), "partial event");
                    return None;
                }
            }
        }
    }
    let event = next_event(&mut body, &mut buffered).await.expect("assignments were not flushed");
    let tasks: Vec<Task> = serde_json::from_value(event["tasks"].clone()).unwrap();
    assert!(
        event["type"] == "tasks"
            && tasks.len() == 1
            && tasks[0].course_code == "CS 101"
            && !tasks[0].done
            && tasks[0].module_item.is_empty(),
        "unexpected initial assignments: {event}"
    );
    tokio::time::timeout(Duration::from_secs(2), module_started.cancelled()).await.expect("module scan did not start");
    finish_module.cancel();
    let event = next_event(&mut body, &mut buffered).await.unwrap();
    let tasks: Vec<Task> = serde_json::from_value(event["tasks"].clone()).unwrap();
    assert!(
        event["type"] == "complete"
            && tasks.len() == 1
            && tasks[0].done
            && tasks[0].module_item == "9:42"
            && tasks[0].reason == "marked",
        "module completion was not streamed: {event}"
    );
    assert!(next_event(&mut body, &mut buffered).await.is_none(), "unexpected event after completion");
}

#[tokio::test]
async fn canvas_stream_reports_failures_without_completing() {
    for failed in ["/api/v1/courses", "/api/v1/planner/items", "/api/v1/courses/1/modules"] {
        let up = upstream(move |req: Request| async move {
            let path = req.uri().path();
            if path == failed {
                error(503, "unavailable")
            } else if path == "/api/v1/planner/items" {
                reply(
                    200,
                    r#"[{"plannable_type":"assignment","course_id":1,"plannable":{"id":1,"title":"Assignment"}}]"#,
                )
            } else {
                reply(200, "[]")
            }
        })
        .await;
        let t = local_server().await;
        let res = post(&t, "/api/lms/canvas", &canvas_body(&up.url, "tok"), Some("application/x-ndjson")).await;
        if failed != "/api/v1/courses/1/modules" {
            assert!(
                res.status == 502 && !res.text().contains(r#""tasks""#),
                "{failed}: failed planner was streamed: {} {}",
                res.status,
                res.text()
            );
            continue;
        }
        let events: Vec<Value> = res.text().lines().map(|l| serde_json::from_str(l).unwrap()).collect();
        assert!(
            res.status == 200
                && events.len() == 2
                && events[0]["type"] == "tasks"
                && events[1]["type"] == "error"
                && events[1]["error"].as_str().unwrap_or_default().contains("503"),
            "module failure was not reported: {} {events:?}",
            res.status
        );
    }
}

#[tokio::test]
async fn canvas_module_requests_share_concurrency_limit() {
    for course_count in [1usize, 6] {
        let active = Arc::new(AtomicI32::new(0));
        let peak = Arc::new(AtomicI32::new(0));
        let requests = Arc::new(AtomicI32::new(0));
        let started = CancellationToken::new();
        let up = upstream({
            let (active, peak, requests, started) = (active.clone(), peak.clone(), requests.clone(), started.clone());
            move |req: Request| {
                let (active, peak, requests, started) = (active.clone(), peak.clone(), requests.clone(), started.clone());
                async move {
                    let path = req.uri().path().to_owned();
                    match path.as_str() {
                        "/api/v1/courses" => return reply(200, "[]"),
                        "/api/v1/planner/items" => {
                            let items: Vec<Value> = (1..=course_count)
                                .map(|i| json!({"plannable_type":"assignment","course_id":i,"plannable":{"id":i,"title":"Assignment"}}))
                                .collect();
                            return reply(200, serde_json::to_string(&items).unwrap());
                        }
                        _ => {}
                    }
                    requests.fetch_add(1, Ordering::SeqCst);
                    let n = active.fetch_add(1, Ordering::SeqCst) + 1;
                    struct Leave(Arc<AtomicI32>);
                    impl Drop for Leave {
                        fn drop(&mut self) {
                            self.0.fetch_sub(1, Ordering::SeqCst);
                        }
                    }
                    let _leave = Leave(active.clone());
                    peak.fetch_max(n, Ordering::SeqCst);
                    if n == CANVAS_CONCURRENCY as i32 {
                        started.cancel();
                    }
                    // A single course must first return its module collection
                    // before its item lists can overlap. Several courses
                    // overlap immediately.
                    if course_count > 1 || path.ends_with("/items") {
                        started.cancelled().await;
                    }
                    if path.ends_with("/modules") {
                        return reply(200, r#"[{"id":1},{"id":2},{"id":3},{"id":4}]"#);
                    }
                    let parts: Vec<&str> = path.split('/').collect();
                    // Duplicate references in several modules must keep
                    // Canvas's collection order, whatever order requests
                    // complete in.
                    reply(
                        200,
                        format!(
                            r#"[{{"id":42,"type":"Assignment","content_id":{},"completion_requirement":{{"type":"must_mark_done","completed":true}}}}]"#,
                            parts[4]
                        ),
                    )
                }
            }
        })
        .await;
        let t = local_server().await;
        let res = tokio::time::timeout(
            Duration::from_secs(2),
            post(&t, "/api/lms/canvas", &canvas_body(&up.url, "tok"), None),
        )
        .await
        .expect("failed concurrent import");
        let tasks = tasks_of(&res);
        assert!(
            res.status == 200
                && peak.load(Ordering::SeqCst) == CANVAS_CONCURRENCY as i32
                && requests.load(Ordering::SeqCst) == (course_count * 5) as i32
                && tasks.len() == course_count,
            "courses={course_count}: wrong concurrency or incomplete module scan: peak={} requests={}, {} {}",
            peak.load(Ordering::SeqCst),
            requests.load(Ordering::SeqCst),
            res.status,
            res.text()
        );
        for (i, task) in tasks.iter().enumerate() {
            assert!(
                task.course_id == (i + 1).to_string() && task.module_item == "4:42" && task.done,
                "parallel import changed task or module order: {tasks:?}"
            );
        }
    }
}

#[tokio::test]
async fn canvas_cancels_other_collections_after_failure() {
    let planner_started = CancellationToken::new();
    let planner_canceled = CancellationToken::new();
    let up = upstream({
        let (planner_started, planner_canceled) = (planner_started.clone(), planner_canceled.clone());
        move |req: Request| {
            let (planner_started, planner_canceled) = (planner_started.clone(), planner_canceled.clone());
            async move {
                // The planner is fetched as several concurrent windows; any
                // one starting is enough, and each hangs until the failed
                // course request cancels the import.
                if req.uri().path() == "/api/v1/planner/items" {
                    planner_started.cancel();
                    let _canceled = OnDrop(planner_canceled);
                    std::future::pending::<()>().await;
                }
                planner_started.cancelled().await;
                error(503, "unavailable")
            }
        }
    })
    .await;
    let t = local_server().await;
    let res =
        tokio::time::timeout(Duration::from_secs(2), post(&t, "/api/lms/canvas", &canvas_body(&up.url, "tok"), None))
            .await
            .expect("the import did not finish");
    tokio::time::timeout(Duration::from_secs(2), planner_canceled.cancelled())
        .await
        .expect("failed import left the other collection running");
    assert!(
        res.status == 502 && res.text().contains("503"),
        "the original failure was lost: {} {}",
        res.status,
        res.text()
    );
}

#[tokio::test]
async fn canvas_concurrent_requests_share_byte_budget() {
    let requests = Arc::new(AtomicI32::new(0));
    let body = "x".repeat(3 << 20);
    let up = upstream({
        let requests = requests.clone();
        move |_req: Request| {
            let (requests, body) = (requests.clone(), body.clone());
            async move {
                requests.fetch_add(1, Ordering::SeqCst);
                reply(200, body)
            }
        }
    })
    .await;
    let outbound = Outbound::new();
    outbound.set_allow_local(true);
    let c = CanvasClient::new(outbound, url::Url::parse(&up.url).unwrap(), "tok".into());
    let result = c
        .parallel(20, CANVAS_CONCURRENCY, |_| async {
            c.request(Method::GET, "/", None).await?;
            Ok(())
        })
        .await;
    // Five 3 MiB responses fit; the sixth crosses the budget, and up to one
    // per worker can already be in flight when the import aborts.
    let n = requests.load(Ordering::SeqCst);
    assert!(
        result.as_ref().is_err_and(|e| e.to_string().contains("too much data")) && n <= (5 + CANVAS_CONCURRENCY) as i32,
        "shared byte budget was ignored: requests={n} err={result:?}"
    );
}

#[tokio::test]
async fn canvas_retries_only_throttled_reads() {
    for (name, method, retry_after, success_on, want_requests, want_status) in [
        ("read recovers", Method::GET, "0", 3, 3, 200),
        ("read retry limit", Method::GET, "0", 0, 3, 429),
        ("long throttle", Method::GET, "60", 0, 1, 429),
        ("write is not retried", Method::PUT, "0", 2, 1, 429),
    ] {
        let requests = Arc::new(AtomicI32::new(0));
        let up = upstream({
            let requests = requests.clone();
            move |_req: Request| {
                let requests = requests.clone();
                async move {
                    if requests.fetch_add(1, Ordering::SeqCst) + 1 == success_on {
                        return reply(200, "[]");
                    }
                    with_header(reply(429, ""), "retry-after", retry_after)
                }
            }
        })
        .await;
        let outbound = Outbound::new();
        outbound.set_allow_local(true);
        let c = CanvasClient::new(outbound, url::Url::parse(&up.url).unwrap(), "tok".into());
        let result = c.request(method, "/", None).await;
        let budget = c.budget.lock().unwrap().requests;
        let n = requests.load(Ordering::SeqCst);
        assert!(
            result.as_ref().is_ok_and(|r| r.status == want_status)
                && n == want_requests
                && budget == want_requests as usize,
            "{name}: wrong throttle behavior: status={:?} requests={n} budget={budget}",
            result.map(|r| r.status)
        );
    }
}

#[tokio::test]
async fn canvas_throttle_wait_honors_cancellation() {
    let requests = Arc::new(AtomicI32::new(0));
    let up = upstream({
        let requests = requests.clone();
        move |_req: Request| {
            let requests = requests.clone();
            async move {
                requests.fetch_add(1, Ordering::SeqCst);
                with_header(reply(429, ""), "retry-after", "1")
            }
        }
    })
    .await;
    let outbound = Outbound::new();
    outbound.set_allow_local(true);
    let c = CanvasClient::new(outbound, url::Url::parse(&up.url).unwrap(), "tok".into());
    // A 50 ms deadline: the operation is dropped when it expires.
    let result = tokio::time::timeout(Duration::from_millis(50), c.request(Method::GET, "/", None)).await;
    assert!(
        result.is_err() && requests.load(Ordering::SeqCst) == 1,
        "throttle wait ignored cancellation: requests={}",
        requests.load(Ordering::SeqCst)
    );
}

#[tokio::test]
async fn canvas_paginates_course_and_module_metadata() {
    for failed in ["", "/api/v1/courses", "/api/v1/courses/1/modules", "/api/v1/courses/1/modules/9/items"] {
        let up = upstream(move |req: Request| async move {
            let path = req.uri().path().to_owned();
            if path == "/api/v1/planner/items" {
                return reply(
                    200,
                    r#"[{"plannable_type":"wiki_page","course_id":1,"plannable":{"id":40,"title":"Reading","url":"reading"}}]"#,
                );
            }
            if query(&req, "page").is_empty() {
                return with_header(reply(200, "[]"), "link", r#"<?page=2>; rel="next""#);
            }
            if path == failed {
                return error(503, "upstream unavailable");
            }
            match path.as_str() {
                "/api/v1/courses" => reply(200, r#"[{"id":1,"name":"Programming","course_code":"CS 101"}]"#),
                "/api/v1/courses/1/modules" => reply(200, r#"[{"id":9}]"#),
                "/api/v1/courses/1/modules/9/items" => reply(
                    200,
                    r#"[{"id":42,"type":"Page","page_url":"reading","completion_requirement":{"type":"must_mark_done","completed":true}}]"#,
                ),
                _ => error(404, "404 page not found"),
            }
        })
        .await;
        let t = local_server().await;
        let res = post(&t, "/api/lms/canvas", &canvas_body(&up.url, "test-token"), None).await;
        if !failed.is_empty() {
            assert_eq!(res.status, 502, "{failed}: incomplete metadata returned as successful import: {}", res.text());
            continue;
        }
        let tasks = tasks_of(&res);
        assert!(
            res.status == 200
                && tasks.len() == 1
                && tasks[0].course_code == "CS 101"
                && tasks[0].course == "Programming"
                && tasks[0].module_item == "9:42"
                && tasks[0].done,
            "later metadata pages were omitted: {} {}",
            res.status,
            res.text()
        );
    }
}

#[tokio::test]
async fn canvas_finds_existing_override_on_later_page() {
    let updates = Arc::new(AtomicI32::new(0));
    let wrong = Arc::new(AtomicBool::new(false));
    let up = upstream({
        let (updates, wrong) = (updates.clone(), wrong.clone());
        move |req: Request| {
            let (updates, wrong) = (updates.clone(), wrong.clone());
            async move {
                match req.method().as_str() {
                    "POST" => reply(400, ""),
                    "GET" => {
                        if query(&req, "page").is_empty() {
                            let host = req.headers().get("host").unwrap().to_str().unwrap().to_owned();
                            let next = format!(r#"<http://{host}{}?page=2>; rel="next""#, req.uri().path());
                            with_header(reply(200, "[]"), "link", &next)
                        } else {
                            reply(
                                200,
                                r#"[{"id":55,"plannable_type":"wiki_page","plannable_id":40,"marked_complete":false}]"#,
                            )
                        }
                    }
                    "PUT" => {
                        if req.uri().path() != "/api/v1/planner/overrides/55" {
                            wrong.store(true, Ordering::SeqCst);
                        }
                        updates.fetch_add(1, Ordering::SeqCst);
                        reply(200, r#"{"id":55,"marked_complete":true}"#)
                    }
                    _ => reply(405, ""),
                }
            }
        }
    })
    .await;
    let t = local_server().await;
    let body =
        json!({"base": up.url, "token": "test-token", "type": "wiki_page", "id": "40", "done": true}).to_string();
    let res = post(&t, "/api/lms/canvas/complete", &body, None).await;
    assert!(!wrong.load(Ordering::SeqCst), "wrong override updated");
    assert!(
        res.status == 200 && updates.load(Ordering::SeqCst) == 1 && res.text().contains(r#""overrideId":"55""#),
        "existing override on later page was missed: {} {}",
        res.status,
        res.text()
    );
}

#[tokio::test]
async fn canvas_bounds_total_module_requests() {
    let requests = Arc::new(AtomicI32::new(0));
    let up = upstream({
        let requests = requests.clone();
        move |req: Request| {
            let requests = requests.clone();
            async move {
                requests.fetch_add(1, Ordering::SeqCst);
                if req.uri().path() == "/api/v1/planner/items" {
                    let items: Vec<Value> = (1..=101)
                        .map(|i| json!({"plannable_type":"assignment","course_id":i,"plannable":{"id":i,"title":"Assignment"}}))
                        .collect();
                    return reply(200, serde_json::to_string(&items).unwrap());
                }
                reply(200, "[]")
            }
        }
    })
    .await;
    let t = local_server().await;
    let res = post(&t, "/api/lms/canvas", &canvas_body(&up.url, "test-token"), None).await;
    assert!(
        res.status == 502 && requests.load(Ordering::SeqCst) <= 100 && res.text().contains("too many requests"),
        "unbounded or partial Canvas import: requests={}, {} {}",
        requests.load(Ordering::SeqCst),
        res.status,
        res.text()
    );
}

#[tokio::test]
async fn canvas_keeps_tokens_on_original_origin() {
    let followed = Arc::new(AtomicBool::new(false));
    let other = upstream({
        let followed = followed.clone();
        move |_req: Request| {
            let followed = followed.clone();
            async move {
                followed.store(true, Ordering::SeqCst);
                reply(200, "[]")
            }
        }
    })
    .await;
    for kind in ["redirect", "pagination", "page limit"] {
        let other_url = other.url.clone();
        let up = upstream(move |req: Request| {
            let other_url = other_url.clone();
            async move {
                if req.uri().path() == "/api/v1/planner/items" {
                    match kind {
                        "redirect" => return with_header(reply(302, ""), "location", &other_url),
                        "pagination" => {
                            return with_header(reply(200, "[]"), "link", &format!(r#"<{other_url}>; rel="next""#));
                        }
                        _ => {
                            let host = req.headers().get("host").unwrap().to_str().unwrap().to_owned();
                            let next = format!(r#"<http://{host}/api/v1/planner/items?page=2>; rel="next""#);
                            return with_header(reply(200, "[]"), "link", &next);
                        }
                    }
                }
                reply(200, "[]")
            }
        })
        .await;
        let t = local_server().await;
        let res = post(&t, "/api/lms/canvas", &canvas_body(&up.url, "test-token"), None).await;
        assert_eq!(res.status, 502, "{kind} should fail without returning a partial import: {}", res.text());
    }
    assert!(!followed.load(Ordering::SeqCst), "Canvas followed a different origin");
}

#[tokio::test]
async fn lms_rejects_truncated_response() {
    let up = upstream(|_req: Request| async { reply(200, "x".repeat((4 << 20) + 1)) }).await;
    let t = local_server().await;
    let u = url::Url::parse(&up.url).unwrap();
    let result = t.server.relay(&u, "").await;
    assert!(
        result.as_ref().is_err_and(|e| e.to_string().contains("too large")),
        "oversized response was silently truncated: {:?}",
        result.err()
    );
}

#[tokio::test]
async fn lms_relay() {
    let up = upstream(|req: Request| async move {
        let method = req.method().as_str().to_owned();
        let path = req.uri().path().to_owned();
        let page = query(&req, "page");
        let host = req.headers().get("host").and_then(|v| v.to_str().ok()).unwrap_or_default().to_owned();
        let authorization =
            req.headers().get("authorization").and_then(|v| v.to_str().ok()).unwrap_or_default().to_owned();
        let body = req.into_body().collect().await.unwrap().to_bytes();
        let input: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
        match (method.as_str(), path.as_str()) {
            (_, "/feed.ics") => reply(
                200,
                "BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nUID:1\r\nSUMMARY:HW 1 [CS 101]\r\nDTSTART:20260910T235900Z\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n",
            ),
            ("POST", "/api/v1/planner/overrides") => {
                // Pretend an override exists for item 9.
                if input["plannable_id"] == "9" {
                    return reply(400, "");
                }
                reply(200, r#"{"id":77,"plannable_type":"quiz","plannable_id":3,"marked_complete":true}"#)
            }
            ("GET", "/api/v1/planner/overrides") => {
                reply(200, r#"[{"id":55,"plannable_type":"assignment","plannable_id":9,"marked_complete":false}]"#)
            }
            ("PUT", p) if p.starts_with("/api/v1/planner/overrides/") => reply(
                200,
                format!(
                    r#"{{"id":{},"marked_complete":{}}}"#,
                    p.trim_start_matches("/api/v1/planner/overrides/"),
                    input["marked_complete"]
                ),
            ),
            (_, "/api/v1/courses") => reply(200, r#"[{"id":1,"name":"Intro to Programming","course_code":"CS-1213-010"}]"#),
            // Module 5 inlines its items; module 6 leaves them out, so they
            // are fetched separately.
            (_, "/api/v1/courses/1/modules") => reply(
                200,
                r#"[{"id":5,"items":[{"id":11,"type":"Assignment","content_id":9,"completion_requirement":{"type":"must_mark_done","completed":false}},{"id":12,"type":"Page","page_url":"week-1-reading","completion_requirement":{"type":"must_view","completed":true}}]},{"id":6}]"#,
            ),
            (_, "/api/v1/courses/1/modules/6/items") => reply(
                200,
                r#"[{"id":13,"type":"Page","page_url":"syllabus","completion_requirement":{"type":"must_mark_done","completed":true}}]"#,
            ),
            ("PUT" | "DELETE", "/api/v1/courses/1/modules/5/items/11/done") => reply(200, "{}"),
            (_, "/api/v1/planner/items") => {
                if authorization != "Bearer tok" {
                    return error(401, "no");
                }
                if page.is_empty() {
                    with_header(
                        reply(
                            200,
                            r#"[{"plannable_type":"assignment","course_id":1,"context_name":"CS 1213-010","html_url":"/courses/1/assignments/9","plannable":{"id":9,"title":"Project 1","due_at":"2026-09-20T04:59:00Z"},"submissions":{"submitted":true}},{"plannable_type":"wiki_page","course_id":1,"context_name":"CS 1213-010","plannable":{"id":40,"title":"Syllabus","todo_date":"2026-09-21T04:59:00Z","url":"syllabus"},"submissions":false},{"plannable_type":"announcement","plannable":{"id":1,"title":"skip"}}]"#,
                        ),
                        "link",
                        &format!(r#"<http://{host}/api/v1/planner/items?page=2>; rel="next""#),
                    )
                } else {
                    reply(
                        200,
                        r#"[{"plannable_type":"quiz","context_name":"MATH 1914","html_url":"https://x/q","plannable":{"id":3,"title":"Quiz 2","due_at":"2026-09-22T04:59:00Z"},"submissions":false,"planner_override":{"marked_complete":true}}]"#,
                    )
                }
            }
            _ => error(404, "404 page not found"),
        }
    })
    .await;
    let t = local_server().await;
    let base = &up.url;
    let res = post(&t, "/api/lms/feed", &json!({"url": format!("{base}/feed.ics")}).to_string(), None).await;
    assert!(res.status == 200 && res.text().contains("HW 1"), "feed: {} {}", res.status, res.text());
    let res = post(&t, "/api/lms/feed", &json!({"url": format!("{base}/nope")}).to_string(), None).await;
    assert_eq!(res.status, 502, "missing feed should fail");
    // Marking done: a new override is created; when Canvas says one exists,
    // it is found and updated.
    let complete = |extra: Value| {
        let mut body = json!({"base": base, "token": "tok"});
        body.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
        body.to_string()
    };
    let res = post(&t, "/api/lms/canvas/complete", &complete(json!({"type":"quiz","id":"3","done":true})), None).await;
    assert!(res.status == 200 && res.text().contains(r#""overrideId":"77""#), "complete (create): {}", res.text());
    let res =
        post(&t, "/api/lms/canvas/complete", &complete(json!({"type":"assignment","id":"9","done":true})), None).await;
    assert!(res.status == 200 && res.text().contains(r#""overrideId":"55""#), "complete (existing): {}", res.text());
    let res = post(
        &t,
        "/api/lms/canvas/complete",
        &complete(json!({"type":"quiz","id":"3","done":false,"overrideId":"77"})),
        None,
    )
    .await;
    assert!(res.status == 200 && res.text().contains(r#""done":false"#), "complete (update): {}", res.text());
    let res = post(&t, "/api/lms/canvas", &canvas_body(base, "tok"), None).await;
    let tasks: Vec<Task> = serde_json::from_slice(&res.body).unwrap_or_default();
    assert!(
        res.status == 200
            && tasks.len() == 3
            && tasks[0].title == "Project 1"
            && tasks[0].done
            && tasks[0].course == "CS 1213-010"
            && tasks[0].url.starts_with(base.as_str())
            && tasks[2].uid == "canvas:quiz:3"
            && tasks[2].done,
        "canvas: {} {}",
        res.status,
        res.text()
    );
    // Course code and module items come along; a page marked done in its
    // module counts as done.
    assert!(
        tasks[0].course_code == "CS-1213-010" && tasks[0].module_item == "5:11" && tasks[0].reason == "submitted",
        "module/course info: {:?}",
        tasks[0]
    );
    assert!(
        tasks[1].uid == "canvas:wiki_page:40"
            && tasks[1].module_item == "6:13"
            && tasks[1].done
            && tasks[1].reason == "marked",
        "page marked done in module: {:?}",
        tasks[1]
    );
    assert!(tasks[2].reason == "marked" && tasks[2].override_id.is_empty(), "override reason: {:?}", tasks[2]);
    // Ticking an item with a module requirement updates the module too.
    let res = post(
        &t,
        "/api/lms/canvas/complete",
        &complete(json!({"type":"quiz","id":"3","done":true,"courseId":"1","moduleItem":"5:11"})),
        None,
    )
    .await;
    assert!(res.status == 200 && res.text().contains(r#""module":true"#), "complete (module): {}", res.text());
    let res = post(&t, "/api/lms/canvas", &canvas_body(base, "bad"), None).await;
    assert_eq!(res.status, 502, "bad token");
    t.server.outbound.set_allow_local(false);
    let res = post(&t, "/api/lms/feed", r#"{"url":"http://127.0.0.1/feed.ics"}"#, None).await;
    assert_eq!(res.status, 400, "private/plain-http url must be refused");
}

/// A course calendar event has a start rather than a due time, which is
/// when its task is due.
#[tokio::test]
async fn canvas_calendar_events_are_due_when_they_start() {
    let up = upstream(|req: Request| async move {
        if req.uri().path().contains("/planner/items") {
            return reply(
                200,
                r#"[{"plannable_type":"calendar_event","course_id":1,"context_name":"Chemistry","html_url":"/calendar?event_id=7","plannable":{"id":7,"title":"Midterm 1","start_at":"2026-10-08T00:00:00Z","end_at":"2026-10-08T01:20:00Z"}},{"plannable_type":"assignment","course_id":1,"context_name":"Chemistry","plannable":{"id":8,"title":"Lab 2","due_at":"2026-10-09T04:59:00Z"}}]"#,
            );
        }
        reply(200, r#"[{"id":1,"name":"Chemistry","course_code":"CHEM 1315"}]"#)
    })
    .await;
    let t = local_server().await;
    let res = post(&t, "/api/lms/canvas", &canvas_body(&up.url, "tok"), None).await;
    assert_eq!(res.status, 200, "canvas: {}", res.text());
    let tasks = tasks_of(&res);
    assert_eq!(tasks.len(), 2, "tasks: {}", res.text());
    assert!(
        tasks[0].uid == "canvas:calendar_event:7"
            && tasks[0].due == "2026-10-08T00:00:00Z"
            && tasks[0].course_code == "CHEM 1315"
            && tasks[0].url == format!("{}/calendar?event_id=7", up.url),
        "calendar event should be due at its start: {:?}",
        tasks[0]
    );
    assert_eq!(tasks[1].due, "2026-10-09T04:59:00Z", "assignment due time changed: {:?}", tasks[1]);
}

/// Canvas sync latency: six courses and three separately
/// fetched module item lists per course, 26 responses each delayed by
/// 20 ms. Reports when the first tasks arrive. Run with `--ignored`.
#[tokio::test]
#[ignore = "a timing benchmark"]
async fn canvas_sync_latency() {
    let up = upstream(|req: Request| async move {
        tokio::time::sleep(Duration::from_millis(20)).await;
        let path = req.uri().path();
        if path == "/api/v1/planner/items" {
            let items: Vec<Value> = (1..=6)
                .map(|i| json!({"plannable_type":"assignment","course_id":i,"plannable":{"id":i,"title":"Assignment"}}))
                .collect();
            return reply(200, serde_json::to_string(&items).unwrap());
        }
        if path.ends_with("/modules") {
            return reply(200, r#"[{"id":1},{"id":2},{"id":3}]"#);
        }
        reply(200, "[]")
    })
    .await;
    let t = local_server().await;
    let runs = 10;
    let mut first_total = Duration::ZERO;
    let mut total = Duration::ZERO;
    for _ in 0..runs {
        let started = Instant::now();
        let request = axum::http::Request::post("/api/lms/canvas")
            .header("content-type", "application/json")
            .header("accept", "application/x-ndjson")
            .body(Body::from(canvas_body(&up.url, "tok")))
            .unwrap();
        let response = crate::router(t.server.clone()).oneshot(request).await.unwrap();
        assert_eq!(response.status(), 200);
        let mut body = response.into_body();
        let mut text = Vec::new();
        let mut first = None;
        while let Some(frame) = body.frame().await {
            if let Ok(data) = frame.unwrap().into_data() {
                first.get_or_insert_with(Instant::now);
                text.extend_from_slice(&data);
            }
        }
        assert!(String::from_utf8_lossy(&text).contains(r#""canvas:assignment:6""#), "incomplete import");
        first_total += first.unwrap_or_else(Instant::now) - started;
        total += started.elapsed();
    }
    println!(
        "first-tasks-ms {:.1}, total-ms {:.1}",
        first_total.as_secs_f64() * 1000.0 / runs as f64,
        total.as_secs_f64() * 1000.0 / runs as f64
    );
}
