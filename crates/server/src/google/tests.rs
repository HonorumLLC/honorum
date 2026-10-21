//! Google Calendar: the consent round trip, publishing, showing
//! calendars, and disconnecting, against a fake Google on a loopback
//! listener.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use axum::body::Body;
use axum::extract::{Request, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use base64::Engine;
use chrono::{TimeDelta, TimeZone, Utc};
use serde_json::{Map, Value, json};
use tokio_util::sync::CancellationToken;

use super::events::clip;
use super::publish::{google_event_id, google_occurrence};
use super::{GOOGLE_SCOPE_EVENTS, GOOGLE_SCOPE_LIST, GOOGLE_SCOPE_PUBLISH, GoogleEndpoints};
use crate::config::GoogleOauth;
use crate::db::Sql;
use crate::reminders::ReminderItem;
use crate::session::{SESSION_COOKIE, hash_token, new_id, new_token};
use crate::test_support::{TEST_CANVAS_KEY, TestServer, test_server_with};
use crate::test_support::{TestClient, TestResponse};

type Event = Map<String, Value>;

/// Enough of Google's OAuth and Calendar API for the integration: a token
/// endpoint, one Google account's calendars and events, and change
/// channels. Deleted events stay behind as cancelled, and inserting a used
/// ID is a conflict, as at Google.
#[derive(Default)]
struct Fake {
    /// Granted by the next code exchange.
    scopes: String,
    subject: String,
    /// Refresh tokens stop working.
    revoked: bool,
    refreshes: usize,
    revokes: Vec<String>,
    /// Calendar → event ID → event.
    calendars: HashMap<String, HashMap<String, Event>>,
    /// The calendar list.
    list: Vec<Value>,
    /// Events of calendars the student shows.
    shown: HashMap<String, Vec<Value>>,
    /// Method and path of every event write.
    writes: Vec<String>,
    watches: Vec<Value>,
    stops: Vec<String>,
    next_cal: usize,
}

#[derive(Clone)]
struct FakeGoogle(Arc<Mutex<Fake>>);

impl FakeGoogle {
    fn with<R>(&self, f: impl FnOnce(&mut Fake) -> R) -> R {
        f(&mut self.0.lock().unwrap())
    }

    fn event(&self, matches: impl Fn(&Event) -> bool) -> Option<Event> {
        self.with(|g| g.calendars.values().flat_map(|c| c.values()).find(|e| matches(e)).cloned())
    }

    fn update(&self, matches: impl Fn(&Event) -> bool, change: impl FnOnce(&mut Event)) {
        self.with(|g| {
            let e = g.calendars.values_mut().flat_map(|c| c.values_mut()).find(|e| matches(e)).expect("an event");
            change(e);
        })
    }

    fn write_count(&self) -> usize {
        self.with(|g| g.writes.len())
    }
}

fn summary_is(summary: &'static str) -> impl Fn(&Event) -> bool {
    move |e| e.get("summary") == Some(&json!(summary))
}

/// A test server with Google configured, the all-zero credential key, and
/// endpoints pointing at a fresh fake.
async fn google_server(base_url: Option<&str>) -> (TestServer, FakeGoogle) {
    let base = base_url.map(str::to_owned);
    let t = test_server_with(|cfg| {
        cfg.google = GoogleOauth { client_id: "client".into(), client_secret: "secret".into() };
        cfg.canvas_key = TEST_CANVAS_KEY.into();
        if let Some(base) = base {
            cfg.base_url = base;
        }
    })
    .await;
    let g = FakeGoogle(Arc::new(Mutex::new(Fake { subject: "google-1".into(), ..Default::default() })));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let app = axum::Router::new().fallback(serve_fake).with_state(g.clone());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    t.server.google.set_endpoints(GoogleEndpoints {
        auth: format!("{url}/auth"),
        token: format!("{url}/token"),
        revoke: format!("{url}/revoke"),
        api: format!("{url}/calendar/v3"),
    });
    (t, g)
}

fn jwt(claims: Value) -> String {
    format!("e30.{}.sig", base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(claims.to_string()))
}

fn reply(code: u16, v: Value) -> Response {
    (StatusCode::from_u16(code).unwrap(), [("content-type", "application/json")], format!("{v}\n")).into_response()
}

fn failure(code: u16, reason: &str) -> Response {
    reply(code, json!({"error": {"code": code, "message": reason, "errors": [{"reason": reason}]}}))
}

fn status(code: u16) -> Response {
    StatusCode::from_u16(code).unwrap().into_response()
}

async fn serve_fake(State(g): State<FakeGoogle>, request: Request) -> Response {
    let (parts, body) = request.into_parts();
    let body = axum::body::to_bytes(body, 1 << 24).await.unwrap();
    let mut guard = g.0.lock().unwrap();
    let g = &mut *guard;
    let raw_path = parts.uri.path().to_owned();
    let query: HashMap<String, String> =
        url::form_urlencoded::parse(parts.uri.query().unwrap_or_default().as_bytes()).into_owned().collect();
    match raw_path.as_str() {
        "/token" => {
            let form: HashMap<String, String> = url::form_urlencoded::parse(&body).into_owned().collect();
            let get = |k: &str| form.get(k).map(String::as_str).unwrap_or_default();
            if get("client_id") != "client" || get("client_secret") != "secret" {
                return failure(401, "invalid_client");
            }
            return match get("grant_type") {
                "authorization_code" => {
                    if get("code") != "good-code"
                        || get("code_verifier").is_empty()
                        || !get("redirect_uri").ends_with("/api/google/callback")
                    {
                        return reply(400, json!({"error": "invalid_grant"}));
                    }
                    g.revoked = false;
                    reply(
                        200,
                        json!({"access_token": "access-0", "expires_in": 3600, "refresh_token": format!("refresh-{}", g.subject), "scope": g.scopes,
                            "id_token": jwt(json!({"sub": g.subject, "email": format!("{}@example.com", g.subject)}))}),
                    )
                }
                "refresh_token" => {
                    if g.revoked || get("refresh_token") != format!("refresh-{}", g.subject) {
                        return reply(400, json!({"error": "invalid_grant"}));
                    }
                    g.refreshes += 1;
                    reply(
                        200,
                        json!({"access_token": format!("access-{}", g.refreshes), "expires_in": 3600, "scope": g.scopes}),
                    )
                }
                _ => status(200),
            };
        }
        "/revoke" => {
            let form: HashMap<String, String> = url::form_urlencoded::parse(&body).into_owned().collect();
            g.revokes.push(form.get("token").cloned().unwrap_or_default());
            return status(200);
        }
        _ => {}
    }
    let authorization = parts.headers.get("authorization").and_then(|v| v.to_str().ok()).unwrap_or_default();
    if !authorization.starts_with("Bearer access-") {
        return failure(401, "authError");
    }
    let path = raw_path.strip_prefix("/calendar/v3").unwrap_or(&raw_path).to_owned();
    let segments: Vec<String> = path
        .trim_matches('/')
        .split('/')
        .map(|p| percent_encoding::percent_decode_str(p).decode_utf8_lossy().into_owned())
        .collect();
    let mut body: Map<String, Value> = serde_json::from_slice(&body).unwrap_or_default();
    let method = parts.method.as_str();
    let n = segments.len();
    if path == "/calendars" && method == "POST" {
        g.next_cal += 1;
        let id = format!("honorum-{}@group.calendar.google.com", g.next_cal);
        g.calendars.insert(id.clone(), HashMap::new());
        return reply(200, json!({"id": id, "summary": body.get("summary"), "timeZone": body.get("timeZone")}));
    }
    if path == "/users/me/calendarList" {
        return reply(200, json!({"items": g.list}));
    }
    if path == "/channels/stop" {
        let id = match body.get("id") {
            Some(Value::String(s)) => s.clone(),
            other => format!("{other:?}"),
        };
        g.stops.push(id);
        return status(204);
    }
    if n == 2 && segments[0] == "calendars" && method == "DELETE" {
        if g.calendars.remove(&segments[1]).is_none() {
            return failure(404, "notFound");
        }
        return status(204);
    }
    if n == 4 && segments[2] == "events" && segments[3] == "watch" {
        g.watches.push(Value::Object(body.clone()));
        let expiration = (Utc::now() + TimeDelta::days(7)).timestamp_millis().to_string();
        return reply(
            200,
            json!({"id": body.get("id"), "resourceId": format!("resource-{}", segments[1]), "expiration": expiration}),
        );
    }
    if n >= 3 && segments[0] == "calendars" && segments[2] == "events" {
        if let Some(events) = g.shown.get(&segments[1])
            && method == "GET"
        {
            if query.get("singleEvents").map(String::as_str) != Some("true") {
                return failure(400, "badRequest");
            }
            return reply(200, json!({"items": events}));
        }
        if !g.calendars.contains_key(&segments[1]) {
            return failure(404, "notFound");
        }
        if method != "GET" {
            g.writes.push(format!("{method} {}", segments[3..].join("/")));
        }
        let etag = format!("\"{}\"", g.writes.len());
        let cal = g.calendars.get_mut(&segments[1]).unwrap();
        return match (method, n) {
            ("GET", 3) => {
                let items: Vec<Value> = cal
                    .iter()
                    .map(|(id, e)| json!({"id": id, "status": e.get("status"), "etag": e.get("etag")}))
                    .collect();
                reply(200, json!({"items": items}))
            }
            ("POST", 3) => {
                let id = body.get("id").and_then(Value::as_str).unwrap_or_default().to_owned();
                if cal.contains_key(&id) {
                    return failure(409, "duplicate");
                }
                body.insert("etag".into(), json!(etag));
                cal.insert(id, body.clone());
                reply(200, Value::Object(body))
            }
            ("PUT", 4) => {
                if !cal.contains_key(&segments[3]) {
                    return failure(404, "notFound");
                }
                body.insert("id".into(), json!(segments[3]));
                body.insert("etag".into(), json!(etag));
                cal.insert(segments[3].clone(), body.clone());
                reply(200, Value::Object(body))
            }
            ("DELETE", 4) => match cal.get_mut(&segments[3]) {
                Some(e) if e.get("status") != Some(&json!("cancelled")) => {
                    e.insert("status".into(), json!("cancelled"));
                    status(204)
                }
                _ => failure(410, "deleted"),
            },
            _ => status(200),
        };
    }
    failure(404, "notFound")
}

/// A signed-in device: a session cookie and the account it expects.
struct Client {
    http: TestClient,
    account: String,
}

impl Client {
    async fn send(&self, method: &str, path: &str, body: &str) -> (TestResponse, Value) {
        let r =
            self.http.send(method, path, &[("content-type", "application/json")], Body::from(body.to_owned())).await;
        let v = serde_json::from_slice(&r.body).unwrap_or(Value::Null);
        (r, v)
    }

    async fn must(&self, method: &str, path: &str, body: &str, code: u16) -> Value {
        let (r, v) = self.send(method, path, body).await;
        assert_eq!(r.status.as_u16(), code, "{method} {path}: {}", r.text());
        v
    }
}

/// Signs a device in by writing its session, as a completed phone sign-in
/// would: a new account, or another device of `account`.
async fn sign_in(t: &TestServer, account: Option<&str>) -> Client {
    let s = &t.server;
    let now = s.now().timestamp();
    let account = match account {
        Some(a) => a.to_owned(),
        None => {
            let id = new_id();
            s.db.exec(
                "INSERT INTO accounts(id,phone,created_at,seq) VALUES($1,$2,$3,0)",
                &[&id, &format!("+1{id}"), &now],
            )
            .await
            .unwrap();
            id
        }
    };
    let token = new_token();
    s.db.exec(
        "INSERT INTO sessions(id,account,token_hash,name,created_at,last_seen) VALUES($1,$2,$3,$4,$5,$6)",
        &[&new_id(), &account, &hash_token(&token), &"Test", &now, &now],
    )
    .await
    .unwrap();
    let http = t
        .client()
        .with_header("cookie", &format!("{SESSION_COOKIE}={token}"))
        .with_header("x-honorum-account", &account);
    Client { http, account }
}

fn query_of(u: &url::Url) -> HashMap<String, String> {
    u.query_pairs().into_owned().collect()
}

/// Runs the consent round trip for one direction.
async fn connect_google(c: &Client, g: &FakeGoogle, want: &str, scopes: &str) -> Value {
    g.with(|g| g.scopes = format!("openid https://www.googleapis.com/auth/userinfo.email {scopes}"));
    let start = c.must("POST", "/api/me/google/start", &format!(r#"{{"want":"{want}"}}"#), 200).await;
    let consent = url::Url::parse(start["url"].as_str().unwrap()).unwrap();
    let q = query_of(&consent);
    assert!(
        q["code_challenge_method"] == "S256"
            && q["access_type"] == "offline"
            && q["include_granted_scopes"] == "true"
            && q["redirect_uri"].contains("/api/google/callback"),
        "consent url: {consent}"
    );
    let callback = url::form_urlencoded::Serializer::new(String::new())
        .append_pair("state", &q["state"])
        .append_pair("code", "good-code")
        .finish();
    let (rec, _) = c.send("GET", &format!("/api/google/callback?{callback}"), "").await;
    let location = rec.header("location").to_owned();
    let back = url::Url::parse(&location).ok();
    let handoff = back.as_ref().and_then(|b| query_of(b).get("google_handoff").cloned()).unwrap_or_default();
    assert!(
        rec.status == 302 && !handoff.is_empty() && back.as_ref().unwrap().fragment() == Some("settings/schedule"),
        "callback: {} {location}",
        rec.status
    );
    c.must("POST", "/api/me/google/finish", &format!(r#"{{"handoff":"{handoff}"}}"#), 200).await
}

const GOOGLE_PLAN_DOCS: &str = r#"{"since":0,"docs":[
 {"id":"prefs","type":"prefs","updated":1,"body":{"inst":"demo","term":"2026FA","current":"1"}},
 {"id":"s_1","type":"schedule","updated":1,"body":{"id":"1","inst":"demo","term":"2026FA","primary":true,"sections":["10001"],
  "pinned":{"10001":{"id":"10001","course":"CS 101","num":"001","type":"LEC","title":"Programming","meetings":[{"days":"MWF","start":540,"end":590,"location":"Felgar 300"}],"exams":[{"date":"2026-10-08","start":1140,"end":1260,"location":"Engineering 110"}]}},
  "removed":{"10001":["2026-09-07"]},
  "blocks":[{"id":"b1","name":"Work","days":"T","date":"2026-09-01","start":780,"end":840,"location":"Library","note":"Front desk"}]}},
 {"id":"s_2","type":"schedule","updated":1,"body":{"id":"2","inst":"demo","term":"2026FA","primary":false,"sections":["10002"],"pinned":{"10002":{"id":"10002","course":"MATH 1","num":"1","meetings":[{"days":"TR","start":600,"end":650}]}},"blocks":[]}},
 {"id":"t_hw","type":"task","updated":1,"body":{"id":"hw","title":"Homework 3","course":"CS 101","due":"2026-09-10T04:59:00Z","url":"https://canvas.example/hw3"}},
 {"id":"t_old","type":"task","updated":1,"body":{"id":"old","title":"Finished","due":"2026-09-11T04:59:00Z","done":true}}]}"#;

fn field<'a>(e: &'a Event, path: &[&str]) -> &'a Value {
    let mut v = e.get(path[0]).unwrap_or(&Value::Null);
    for k in &path[1..] {
        v = v.get(*k).unwrap_or(&Value::Null);
    }
    v
}

#[tokio::test]
async fn google_publishes_the_planner_to_its_own_calendar() {
    let (t, g) = google_server(None).await;
    let s = &t.server;
    s.db.exec(
        r#"INSERT INTO terms(inst,id,name,start,"end",updated_at) VALUES('demo','2026FA','Fall 2026','2026-08-24','2026-12-11',1)"#,
        &[],
    )
    .await
    .unwrap();
    s.clock.set(Utc.with_ymd_and_hms(2026, 9, 1, 12, 0, 0).unwrap());
    let c = sign_in(&t, None).await;
    let cfg = c.must("GET", "/api/config", "", 200).await;
    assert_eq!(cfg["google_calendar"], json!(true), "config does not offer Google Calendar");
    let uploaded = c.must("POST", "/api/me/sync", GOOGLE_PLAN_DOCS, 200).await;
    let rev: HashMap<String, Value> = uploaded["applied"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| (a["id"].as_str().unwrap().to_owned(), a["rev"].clone()))
        .collect();
    let status = connect_google(&c, &g, "publish", GOOGLE_SCOPE_PUBLISH).await;
    let publish = &status["publish"];
    assert!(
        status["connected"] == json!(true)
            && status["email"] == json!("google-1@example.com")
            && publish["on"] == json!(true)
            && publish["granted"] == json!(true),
        "status after connecting: {status}"
    );
    let stop = CancellationToken::new();
    s.google_tick(&stop).await;
    assert_eq!(g.with(|g| g.calendars.len()), 1);
    let class = g.event(summary_is("CS 101-001")).expect("the class was not published");
    let recurrence = field(&class, &["recurrence"]).to_string();
    // The term starts on a Monday; the removed day is skipped, and the series
    // ends with the term in campus time (UTC here).
    assert!(
        recurrence.contains("RRULE:FREQ=WEEKLY;BYDAY=MO,WE,FR;UNTIL=20261211T235959Z")
            && recurrence.contains("EXDATE;TZID=UTC:20260907T090000"),
        "recurrence: {recurrence}"
    );
    assert!(
        field(&class, &["start", "dateTime"]) == "2026-08-24T09:00:00"
            && field(&class, &["start", "timeZone"]) == "UTC"
            && class["location"] == "Felgar 300"
            && class["description"] == "Programming · LEC",
        "class event: {class:?}"
    );
    let exam = g.event(summary_is("CS 101 Exam"));
    assert!(
        exam.as_ref().is_some_and(
            |e| field(e, &["recurrence"]).is_null() && field(e, &["start", "dateTime"]) == "2026-10-08T19:00:00"
        ),
        "exam: {exam:?}"
    );
    let work = g.event(summary_is("Work"));
    assert!(
        work.as_ref().is_some_and(|e| e["description"] == "Front desk" && field(e, &["recurrence"]).is_null()),
        "personal event: {work:?}"
    );
    let task = g.event(summary_is("Homework 3 (CS 101)"));
    assert!(
        task.as_ref().is_some_and(|e| e["transparency"] == "transparent"
            && field(e, &["end", "dateTime"]) == "2026-09-10T04:59:00Z"
            && field(e, &["start", "dateTime"]) == "2026-09-10T04:29:00Z"
            && e["description"] == "https://canvas.example/hw3"),
        "task: {task:?}"
    );
    assert!(
        g.event(|e| e.get("summary") == Some(&json!("MATH 1-1")) || e.get("summary") == Some(&json!("Finished")))
            .is_none(),
        "a schedule that is not primary, or a done task, was published"
    );
    let written = g.write_count();
    s.google_tick(&stop).await;
    assert_eq!(g.write_count(), written, "an unchanged planner was published again");

    // An edit changes only its own event.
    c.must(
        "POST",
        "/api/me/sync",
        &format!(
            r#"{{"since":0,"docs":[{{"id":"t_hw","type":"task","base":{},"updated":2,"body":{{"id":"hw","title":"Homework 3","course":"CS 101","due":"2026-09-10T04:59:00Z","done":true}}}}]}}"#,
            rev["t_hw"]
        ),
        200,
    )
    .await;
    s.google_tick(&stop).await;
    let writes = g.with(|g| g.writes.clone());
    assert!(
        writes.len() == written + 1 && writes.last().unwrap().starts_with("DELETE "),
        "completing a task: {:?}",
        &writes[written.min(writes.len())..]
    );

    // Once a day, events deleted in Google Calendar are put back.
    g.update(summary_is("CS 101-001"), |e| {
        e.insert("status".into(), json!("cancelled"));
    });
    s.clock.set(Utc.with_ymd_and_hms(2026, 9, 2, 13, 0, 0).unwrap());
    s.google_tick(&stop).await;
    let restored = g.event(summary_is("CS 101-001")).unwrap();
    assert_eq!(restored["status"], "confirmed", "a deleted class was not restored: {restored:?}");

    // Turning due dates off removes them.
    c.must("PATCH", "/api/me/google", r#"{"tasks":false}"#, 200).await;
    s.google_tick(&stop).await;
    assert!(
        g.event(|e| e["summary"].as_str().unwrap_or_default().starts_with("Homework")
            && e.get("status") != Some(&json!("cancelled")))
            .is_none(),
        "tasks stayed after turning them off"
    );

    // Deleting the calendar in Google turns publishing off rather than fighting it.
    g.with(|g| g.calendars.clear());
    c.must(
        "POST",
        "/api/me/sync",
        r#"{"since":0,"docs":[{"id":"t_new","type":"task","updated":3,"body":{"id":"new","title":"Essay","due":"2026-09-20T04:59:00Z"}}]}"#,
        200,
    )
    .await;
    c.must("PATCH", "/api/me/google", r#"{"tasks":true}"#, 200).await;
    s.google_tick(&stop).await;
    let status = c.must("GET", "/api/me/google", "", 200).await;
    assert!(
        status["publish"]["on"] == json!(false) && status["publish"]["error"] == "Deleted in Google Calendar",
        "status after the calendar was deleted: {status}"
    );

    // Turning it on again makes a new calendar.
    c.must("PATCH", "/api/me/google", r#"{"publish":true}"#, 200).await;
    s.google_tick(&stop).await;
    assert!(
        g.with(|g| g.calendars.len()) == 1 && g.event(summary_is("Essay")).is_some(),
        "publishing did not start over"
    );
    // Turning it off deletes the calendar.
    c.must("PATCH", "/api/me/google", r#"{"publish":false}"#, 200).await;
    s.google_tick(&stop).await;
    assert_eq!(g.with(|g| g.calendars.len()), 0, "the calendar outlived publishing");
}

#[tokio::test]
async fn google_shows_chosen_calendars() {
    let (t, g) = google_server(Some("https://honorum.test")).await;
    let s = &t.server;
    let c = sign_in(&t, None).await;
    g.with(|g| {
        g.list = vec![
            json!({"id": "me@example.com", "summary": "Me", "backgroundColor": "#9fe1e7", "primary": true}),
            json!({"id": "team#work@group.calendar.google.com", "summary": "Work", "summaryOverride": "Shifts", "backgroundColor": "#f83a22"}),
        ];
        g.shown.insert(
            "me@example.com".into(),
            vec![
                json!({"id": "a", "status": "confirmed", "summary": "Dentist", "location": "Main St", "start": {"dateTime": "2026-09-02T15:00:00-05:00"}, "end": {"dateTime": "2026-09-02T16:00:00-05:00"}, "htmlLink": "https://calendar.google.com/event?eid=a"}),
                json!({"id": "b", "status": "confirmed", "summary": "Trip", "start": {"date": "2026-09-05"}, "end": {"date": "2026-09-07"}, "transparency": "transparent"}),
                json!({"id": "c", "status": "confirmed", "summary": "Declined", "start": {"dateTime": "2026-09-03T10:00:00Z"}, "end": {"dateTime": "2026-09-03T11:00:00Z"}, "attendees": [{"self": true, "responseStatus": "declined"}]}),
                json!({"id": "d", "status": "cancelled"}),
                json!({"id": "e", "status": "confirmed", "start": {"dateTime": "2026-09-04T10:00:00Z"}, "end": {"dateTime": "2026-09-04T11:00:00Z"}}),
            ],
        );
        g.shown.insert(
            "team#work@group.calendar.google.com".into(),
            vec![
                json!({"id": "w", "status": "confirmed", "summary": "Shift", "start": {"dateTime": "2026-09-02T18:00:00Z"}, "end": {"dateTime": "2026-09-02T22:00:00Z"}}),
            ],
        );
    });
    // Publishing is not permitted by this consent: turning it on is refused.
    let show_scopes = format!("{GOOGLE_SCOPE_LIST} {GOOGLE_SCOPE_EVENTS}");
    let status = connect_google(&c, &g, "show", &show_scopes).await;
    assert!(
        status["granted"] == json!(true)
            && status["show"]["on"] == json!(true)
            && status["publish"]["granted"] == json!(false),
        "status: {status}"
    );
    c.must("PATCH", "/api/me/google", r#"{"publish":true}"#, 403).await;
    // Clearing the permission on Google's consent screen leaves the switch off.
    let denied = connect_google(&c, &g, "publish", &show_scopes).await;
    assert!(
        denied["granted"] == json!(false) && denied["publish"]["on"] == json!(false),
        "a cleared permission: {denied}"
    );
    let stop = CancellationToken::new();
    s.google_tick(&stop).await;
    let status = c.must("GET", "/api/me/google", "", 200).await;
    let calendars = status["show"]["calendars"].as_array().unwrap();
    assert!(
        calendars.len() == 2
            && calendars[0]["on"] == json!(true)
            && calendars[1]["on"] == json!(false)
            && calendars[1]["name"] == "Shifts",
        "calendars: {calendars:?}"
    );
    let (rec, out) = c.send("GET", "/api/me/google/events", "").await;
    let events = out["events"].as_array().cloned().unwrap_or_default();
    assert!(rec.status == 200 && events.len() == 3, "events: {} {out}", rec.status);
    let first = &events[0];
    assert!(
        first["title"] == "Dentist"
            && first["calendar"] == "me@example.com"
            && first["start"] == "2026-09-02T15:00:00-05:00"
            && first["link"] != ""
            && first["location"] == "Main St",
        "timed event: {first}"
    );
    let trip = &events[1];
    assert!(
        trip["allDay"] == json!(true) && trip["start"] == "2026-09-05" && trip["free"] == json!(true),
        "all-day event: {trip}"
    );
    assert_eq!(events[2]["title"], "Busy", "an untitled event: {}", events[2]);
    let etag = rec.header("etag").to_owned();
    let unchanged = c.http.send("GET", "/api/me/google/events", &[("if-none-match", &etag)], Body::empty()).await;
    assert_eq!(unchanged.status, 304, "an unchanged poll");
    // Change notifications run for shown calendars only.
    let watches = g.with(|g| g.watches.clone());
    assert!(
        watches.len() == 1 && watches[0]["address"] == "https://honorum.test/api/google/notify",
        "watches: {watches:?}"
    );
    c.must("PATCH", "/api/me/google", r#"{"calendars":{"team#work@group.calendar.google.com":true}}"#, 200).await;
    s.google_tick(&stop).await;
    let out = c.must("GET", "/api/me/google/events", "", 200).await;
    let watches = g.with(|g| g.watches.len());
    assert!(
        out["events"].as_array().unwrap().len() == 4 && watches == 2,
        "after showing a second calendar: {out} {watches}"
    );
    // A notification with the channel's token has the calendar fetched again.
    let row =
        s.db.fetch_one("SELECT channel,channel_token FROM google_calendars WHERE id='me@example.com'", &[])
            .await
            .unwrap();
    let (channel, token): (String, String) = (row.get(0), row.get(1));
    g.with(|g| g.shown.get_mut("me@example.com").unwrap().truncate(1));
    let notify = async |token: &str| {
        let rec = t
            .client()
            .send(
                "POST",
                "/api/google/notify",
                &[
                    ("x-goog-channel-id", &channel),
                    ("x-goog-channel-token", token),
                    ("x-goog-resource-state", "exists"),
                ],
                Body::empty(),
            )
            .await;
        assert_eq!(rec.status, 200, "notify");
    };
    notify("wrong").await;
    s.db.exec("UPDATE google_calendars SET attempted=0", &[]).await.unwrap();
    s.google_tick(&stop).await;
    let out = c.must("GET", "/api/me/google/events", "", 200).await;
    assert_eq!(out["events"].as_array().unwrap().len(), 4, "a notification with the wrong token was trusted");
    notify(&token).await;
    s.google_tick(&stop).await;
    let out = c.must("GET", "/api/me/google/events", "", 200).await;
    assert_eq!(out["events"].as_array().unwrap().len(), 2, "a notification did not refresh the calendar: {out}");
    // Hiding every calendar stops their notifications and forgets the events.
    c.must("PATCH", "/api/me/google", r#"{"show":false}"#, 200).await;
    s.google_tick(&stop).await;
    let out = c.must("GET", "/api/me/google/events", "", 200).await;
    let stops = g.with(|g| g.stops.clone());
    assert!(out["events"].as_array().unwrap().is_empty() && stops.len() == 2, "after hiding: {out} stops={stops:?}");
}

#[tokio::test]
async fn google_consent_is_finished_only_by_its_own_session() {
    let (t, g) = google_server(None).await;
    let c = sign_in(&t, None).await;
    g.with(|g| g.scopes = GOOGLE_SCOPE_PUBLISH.into());
    let state_of = |start: &Value| query_of(&url::Url::parse(start["url"].as_str().unwrap()).unwrap())["state"].clone();
    let start = c.must("POST", "/api/me/google/start", r#"{"want":"publish"}"#, 200).await;
    let state = state_of(&start);
    // Declining returns to Settings with the reason.
    let (rec, _) = c.send("GET", &format!("/api/google/callback?state={state}&error=access_denied"), "").await;
    assert!(
        rec.status == 302 && rec.header("location").contains("google_error=denied"),
        "declined: {} {}",
        rec.status,
        rec.header("location")
    );
    let (rec, _) = c.send("GET", &format!("/api/google/callback?state={state}&code=good-code"), "").await;
    assert_eq!(rec.status, 400, "a used state was accepted twice");
    let start = c.must("POST", "/api/me/google/start", r#"{"want":"publish","back":"ios"}"#, 200).await;
    let (rec, _) = c.send("GET", &format!("/api/google/callback?state={}&code=good-code", state_of(&start)), "").await;
    let back = url::Url::parse(rec.header("location")).unwrap();
    let handoff = query_of(&back).get("handoff").cloned().unwrap_or_default();
    assert!(back.scheme() == "honorum" && !handoff.is_empty(), "iPhone return: {}", rec.header("location"));
    // Another device of the same account cannot finish it.
    let other = sign_in(&t, Some(&c.account)).await;
    let finish = format!(r#"{{"handoff":"{handoff}"}}"#);
    other.must("POST", "/api/me/google/finish", &finish, 404).await;
    c.must("POST", "/api/me/google/finish", &finish, 200).await;
    c.must("POST", "/api/me/google/finish", &finish, 404).await;
    // Consent expires after ten minutes.
    let start = c.must("POST", "/api/me/google/start", r#"{"want":"publish"}"#, 200).await;
    t.server.clock.shift(TimeDelta::minutes(11));
    let (rec, _) = c.send("GET", &format!("/api/google/callback?state={}&code=good-code", state_of(&start)), "").await;
    assert_eq!(rec.status, 400, "expired consent");
}

#[tokio::test]
async fn google_disconnect_and_revocation() {
    let (t, g) = google_server(None).await;
    let s = &t.server;
    let c = sign_in(&t, None).await;
    c.must("POST", "/api/me/sync", GOOGLE_PLAN_DOCS, 200).await;
    connect_google(&c, &g, "publish", GOOGLE_SCOPE_PUBLISH).await;
    let stop = CancellationToken::new();
    s.google_tick(&stop).await;
    assert_eq!(g.with(|g| g.calendars.len()), 1, "nothing was published");
    // Access removed in the Google account is reported, and work stops.
    g.with(|g| g.revoked = true);
    s.forget_google_tokens(&c.account);
    c.must(
        "POST",
        "/api/me/sync",
        r#"{"since":0,"docs":[{"id":"t_more","type":"task","updated":5,"body":{"id":"more","title":"More","due":"2026-09-30T04:59:00Z"}}]}"#,
        200,
    )
    .await;
    s.google_tick(&stop).await;
    let status = c.must("GET", "/api/me/google", "", 200).await;
    assert_eq!(status["error"], "Access was removed or expired", "revocation was not reported: {status}");
    let refreshes = g.with(|g| g.refreshes);
    s.google_tick(&stop).await;
    assert_eq!(g.with(|g| g.refreshes), refreshes, "a revoked connection kept asking Google");
    // Connecting again asks for every direction that is on and clears the
    // error; disconnecting removes the calendar, revokes the token, and keeps
    // nothing.
    let start = c.must("POST", "/api/me/google/start", r#"{"want":"reconnect"}"#, 200).await;
    let scope = query_of(&url::Url::parse(start["url"].as_str().unwrap()).unwrap())["scope"].clone();
    assert_eq!(scope, format!("openid email {GOOGLE_SCOPE_PUBLISH}"), "reconnect asks for");
    let again = connect_google(&c, &g, "reconnect", GOOGLE_SCOPE_PUBLISH).await;
    assert!(
        again["granted"] == json!(true) && again["error"] == "" && again["publish"]["on"] == json!(true),
        "after connecting again: {again}"
    );
    s.google_tick(&stop).await;
    assert!(g.event(summary_is("More")).is_some(), "publishing did not resume after connecting again");
    c.must("DELETE", "/api/me/google", "", 200).await;
    let (calendars, revokes) = g.with(|g| (g.calendars.len(), g.revokes.clone()));
    assert!(calendars == 0 && revokes == ["refresh-google-1"], "disconnect: calendars={calendars} revokes={revokes:?}");
    for table in ["google_connections", "google_published", "google_calendars", "google_states"] {
        let n: i64 = s.db.fetch_one(&format!("SELECT COUNT(*) FROM {table}"), &[]).await.unwrap().get(0);
        assert_eq!(n, 0, "{table} kept {n} rows");
    }
    // Deleting the account does the same.
    connect_google(&c, &g, "publish", GOOGLE_SCOPE_PUBLISH).await;
    c.must("DELETE", "/api/me", "", 204).await;
    let n: i64 = s.db.fetch_one("SELECT COUNT(*) FROM google_connections", &[]).await.unwrap().get(0);
    let revokes = g.with(|g| g.revokes.len());
    assert!(n == 0 && revokes == 2, "account deletion kept Google: rows={n} revokes={revokes}");
}

#[test]
fn google_occurrences() {
    // A series starts on the first meeting day in its range and skips only
    // its own meeting days; a skipped single date publishes nothing.
    let it = ReminderItem {
        id: "x".into(),
        label: "BIO 1-1".into(),
        days: "TR".into(),
        start: 600,
        end: 675,
        from: "2026-03-02".into(),
        to: "2026-03-20".into(),
        skip: vec!["2026-03-10".into(), "2026-03-11".into(), "2026-03-10".into()],
        ..Default::default()
    };
    let body = google_occurrence(&it, "America/Chicago").expect("a series");
    assert!(
        body.start.date_time == "2026-03-03T10:00:00"
            && body.end.date_time == "2026-03-03T11:15:00"
            && body.start.time_zone == "America/Chicago",
        "series: {body:?}"
    );
    // Chicago is on daylight time by March 20, so the last day ends at 04:59 UTC.
    assert_eq!(
        body.recurrence,
        ["RRULE:FREQ=WEEKLY;BYDAY=TU,TH;UNTIL=20260321T045959Z", "EXDATE;TZID=America/Chicago:20260310T100000"]
    );
    let item = |days: &str, start: i64, end: i64, from: &str, to: &str, skip: &[&str]| ReminderItem {
        days: days.into(),
        start,
        end,
        from: from.into(),
        to: to.into(),
        skip: skip.iter().map(|s| s.to_string()).collect(),
        ..Default::default()
    };
    assert!(
        google_occurrence(&item("S", 1, 2, "2026-03-02", "2026-03-06", &[]), "UTC").is_none(),
        "a range without a meeting day published a series"
    );
    assert!(
        google_occurrence(&item("MTWRFSU", 1, 2, "2026-03-02", "2026-03-02", &["2026-03-02"]), "UTC").is_none(),
        "a hidden sitting was published"
    );
    assert!(
        google_occurrence(&item("M", 60, 60, "2026-03-02", "2026-03-02", &[]), "UTC").is_none(),
        "an empty meeting was published"
    );
    let (a, b) = (google_event_id("c1", "k"), google_event_id("c2", "k"));
    assert!(
        a != b && a.trim_matches(|ch| "0123456789abcdefghijklmnopqrstuv".contains(ch)).is_empty() && a.len() == 32,
        "event ids: {a} {b}"
    );
    assert!(clip(&format!("é{}", "a".repeat(5)), 1).is_empty() && clip(" ab ", 5) == "ab", "clip");
}

#[test]
fn event_encoding_matches_stored_hashes() {
    // Event body hashes and event IDs stored by earlier releases were
    // computed over exactly these bytes; changing them republishes every
    // event.
    let body = super::publish::GoogleEventBody {
        summary: "A & B <c>\u{2028}\u{1}\u{8}\t".into(),
        start: super::publish::GoogleTime { date_time: "2026-01-01T09:00:00".into(), ..Default::default() },
        ..Default::default()
    };
    let raw = crate::api::escaped_json(&body);
    assert_eq!(
        String::from_utf8(raw.clone()).unwrap(),
        r#"{"summary":"A \u0026 B \u003cc\u003e\u2028\u0001\b\t","start":{"dateTime":"2026-01-01T09:00:00"},"end":{}}"#
    );
    use sha2::Digest;
    assert_eq!(
        hex::encode(sha2::Sha256::digest(&raw)),
        "5965047527462c2032f0e8dbc3b92a8fa96d1765372619a1a57751bfb10de160"
    );
    assert_eq!(google_event_id("c1", "k"), "1g53uq6ob7mkp0tto7bdr8l509apfhfq");
}
