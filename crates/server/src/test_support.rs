//! Shared fixtures for the crate's unit tests: an in-process HTTP client,
//! a server over a fresh database, and signed-in account clients.
//!
//! [`test_server`] builds a server for the demo school "demo" ("Demo U"),
//! served by a [`FakeAdapter`], with the test configuration: base URL
//! `http://x`, contact `mailto:t@x`, 60-second polls, one-hour catalogs, and
//! a 30-minute debounce. [`test_server_with`] adjusts that configuration
//! before the server reads it; tests that store credentials set
//! `cfg.canvas_key` to [`TEST_CANVAS_KEY`].
//!
//! Sign-in goes through a [`FakeVerifier`] for [`TEST_PHONE`]. [`Client`]
//! tracks the session cookie and account as responses set them;
//! [`Signed`] is an immutable signed-in browser for tests that share one
//! client across closures.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use axum::Router;
use axum::body::Body;
use axum::http::{HeaderMap, HeaderName, HeaderValue, Method, Request, StatusCode};
use honorum_catalog::adapters::demo::Demo;
use honorum_catalog::{Adapter, AdapterError, AdapterState, Seats, Section, Term};
use serde_json::Value;
use tower::ServiceExt;

use crate::db::Sql;
use crate::phone::{PhoneError, PhoneVerifier};
use crate::session::SESSION_COOKIE;
use crate::testing::TestDatabase;
use crate::{Config, Db, Institution, Server};

/// Base64 of 32 zero bytes: an all-zero credential key.
pub(crate) const TEST_CANVAS_KEY: &str = "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=";

/// The phone number test accounts sign in with.
pub(crate) const TEST_PHONE: &str = "+12015550123";

/// Sends requests to a router without a network.
#[derive(Clone)]
pub(crate) struct TestClient {
    router: Router,
    headers: HeaderMap,
}

/// A buffered response.
#[derive(Debug)]
pub(crate) struct TestResponse {
    pub(crate) status: StatusCode,
    pub(crate) headers: HeaderMap,
    pub(crate) body: bytes::Bytes,
}

impl TestResponse {
    pub(crate) fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }

    pub(crate) fn json(&self) -> serde_json::Value {
        serde_json::from_slice(&self.body).unwrap_or_else(|e| panic!("{e}: {}", self.text()))
    }

    pub(crate) fn header(&self, name: &str) -> &str {
        self.headers.get(name).and_then(|v| v.to_str().ok()).unwrap_or_default()
    }
}

impl TestClient {
    pub(crate) fn new(router: Router) -> Self {
        Self { router, headers: HeaderMap::new() }
    }

    /// A client that adds `name: value` to every request.
    pub(crate) fn with_header(&self, name: &str, value: &str) -> Self {
        let mut next = self.clone();
        next.headers.insert(HeaderName::try_from(name).unwrap(), HeaderValue::from_str(value).unwrap());
        next
    }

    /// A client that sends `Authorization: Bearer <token>`.
    pub(crate) fn bearer(&self, token: &str) -> Self {
        self.with_header("authorization", &format!("Bearer {token}"))
    }

    pub(crate) async fn send(
        &self,
        method: &str,
        path: &str,
        headers: &[(&str, &str)],
        body: impl Into<Body>,
    ) -> TestResponse {
        let mut request = Request::builder().method(Method::from_bytes(method.as_bytes()).unwrap()).uri(path);
        for (k, v) in &self.headers {
            request = request.header(k, v);
        }
        for (k, v) in headers {
            request = request.header(*k, *v);
        }
        let request = request.body(body.into()).unwrap();
        let response = self.router.clone().oneshot(request).await.unwrap();
        let (parts, body) = response.into_parts();
        let body = http_body_util::BodyExt::collect(body).await.unwrap().to_bytes();
        TestResponse { status: parts.status, headers: parts.headers, body }
    }

    pub(crate) async fn get(&self, path: &str) -> TestResponse {
        self.send("GET", path, &[], Body::empty()).await
    }

    pub(crate) async fn delete(&self, path: &str) -> TestResponse {
        self.send("DELETE", path, &[], Body::empty()).await
    }

    /// Sends `value` as a JSON body.
    pub(crate) async fn json(&self, method: &str, path: &str, value: &serde_json::Value) -> TestResponse {
        self.send(method, path, &[("content-type", "application/json")], serde_json::to_vec(value).unwrap()).await
    }

    pub(crate) async fn post(&self, path: &str, value: &serde_json::Value) -> TestResponse {
        self.json("POST", path, value).await
    }

    pub(crate) async fn put(&self, path: &str, value: &serde_json::Value) -> TestResponse {
        self.json("PUT", path, value).await
    }
}

/// The demo adapter with controllable seat counts: availability reports
/// capacity 10 and `seats[id]` available (0 when unset) for each requested
/// section. Terms and the catalog come from the static demo fixture.
pub(crate) struct FakeAdapter {
    pub(crate) demo: Demo,
    pub(crate) seats: Mutex<HashMap<String, i64>>,
}

impl FakeAdapter {
    /// A fake over a demo adapter whose seat counts never change.
    pub(crate) fn new() -> Self {
        let mut demo = Demo::new();
        demo.static_seats = true;
        Self { demo, seats: Mutex::new(HashMap::new()) }
    }

    /// Sets the available seats reported for a section.
    pub(crate) fn set_seats(&self, id: &str, available: i64) {
        self.seats.lock().expect("fake seats lock").insert(id.to_owned(), available);
    }
}

impl Default for FakeAdapter {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Adapter for FakeAdapter {
    async fn terms(&self) -> Result<Vec<Term>, AdapterError> {
        self.demo.terms().await
    }

    async fn catalog(&self, term: &str) -> Result<Vec<Section>, AdapterError> {
        self.demo.catalog(term).await
    }

    async fn availability(&self, _term: &str, ids: &[String]) -> Result<Vec<Seats>, AdapterError> {
        let seats = self.seats.lock().expect("fake seats lock");
        Ok(ids
            .iter()
            .map(|id| Seats {
                id: id.clone(),
                capacity: 10,
                available: seats.get(id).copied().unwrap_or(0),
                waitlist: 0,
            })
            .collect())
    }

    fn version(&self) -> &str {
        self.demo.version()
    }

    fn minimum_poll_interval(&self) -> Option<std::time::Duration> {
        self.demo.minimum_poll_interval()
    }

    fn state(&self) -> Option<&dyn AdapterState> {
        self.demo.state()
    }
}

/// A test server with its fake adapter and database guard (keep the guard
/// alive for the test; dropping it drops the database).
pub(crate) struct TestServer {
    pub(crate) server: Arc<Server>,
    pub(crate) fake: Arc<FakeAdapter>,
    pub(crate) db: TestDatabase,
}

impl TestServer {
    /// An in-process client for the server's router.
    pub(crate) fn client(&self) -> TestClient {
        TestClient::new(crate::router(self.server.clone()))
    }
}

/// [`test_server_with`] with the configuration unchanged.
pub(crate) async fn test_server() -> TestServer {
    test_server_with(|_| {}).await
}

/// Builds a test server, letting `configure` adjust the test configuration
/// before [`Server::new`] reads it.
pub(crate) async fn test_server_with(configure: impl FnOnce(&mut Config)) -> TestServer {
    let db = TestDatabase::new().await;
    let pool = Db::open(&db.url, 8, 4).await.expect("open the test database");
    let mut cfg = Config {
        base_url: "http://x".into(),
        contact: "mailto:t@x".into(),
        poll_seconds: 60,
        catalog_hours: 1,
        debounce_minutes: 30,
        ..Config::default()
    };
    configure(&mut cfg);
    let fake = Arc::new(FakeAdapter::new());
    let insts = vec![Institution {
        id: "demo".into(),
        name: "Demo U".into(),
        timezone: String::new(),
        adapter: Some(fake.clone()),
        lms: None,
        closures: Vec::new(),
    }];
    let dir = std::env::temp_dir().join(format!("honorum-test-{}", crate::session::new_id()));
    let server = Server::new(pool, cfg, insts, dir).await.expect("build the test server");
    TestServer { server, fake, db }
}

/// An account client: remembers the session cookie a response sets and the
/// account a response names, and sends both (cookie plus
/// `X-Honorum-Account`) or a bearer token with every request.
pub(crate) struct Client {
    pub(crate) http: TestClient,
    pub(crate) cookie: String,
    pub(crate) bearer: String,
    pub(crate) account: String,
}

impl Client {
    pub(crate) fn new(t: &TestServer) -> Self {
        Self { http: t.client(), cookie: String::new(), bearer: String::new(), account: String::new() }
    }

    /// Sends a JSON request and returns the response with its body parsed
    /// (`Null` when it is not JSON).
    pub(crate) async fn send(&mut self, method: &str, path: &str, body: &str) -> (TestResponse, Value) {
        let mut headers = vec![("content-type", "application/json".to_owned())];
        if !self.cookie.is_empty() {
            headers.push(("cookie", format!("{SESSION_COOKIE}={}", self.cookie)));
            headers.push(("x-honorum-account", self.account.clone()));
        }
        if !self.bearer.is_empty() {
            headers.push(("authorization", format!("Bearer {}", self.bearer)));
        }
        let headers: Vec<(&str, &str)> = headers.iter().map(|(k, v)| (*k, v.as_str())).collect();
        let res = self.http.send(method, path, &headers, Body::from(body.to_owned())).await;
        for value in res.headers.get_all("set-cookie") {
            let value = value.to_str().unwrap_or_default();
            if let Some(rest) = value.strip_prefix(&format!("{SESSION_COOKIE}=")) {
                self.cookie = rest.split(';').next().unwrap_or_default().to_owned();
            }
        }
        let out: Value = serde_json::from_slice(&res.body).unwrap_or(Value::Null);
        if let (Some(id), false) = (out["id"].as_str(), out["phone"].is_null()) {
            self.account = id.to_owned();
        }
        (res, out)
    }

    /// [`Client::send`], failing the test unless the status is `code`.
    pub(crate) async fn must(&mut self, method: &str, path: &str, body: &str, code: u16) -> Value {
        let (res, out) = self.send(method, path, body).await;
        assert_eq!(res.status.as_u16(), code, "{method} {path}: {}", res.text());
        out
    }
}

/// A signed-in browser: the session cookie and `X-Honorum-Account`.
#[derive(Clone)]
pub(crate) struct Signed {
    pub(crate) client: TestClient,
    pub(crate) account: String,
    /// The session token from the cookie.
    pub(crate) token: String,
}

impl Signed {
    /// A native client of the same session: the cookie's token as a bearer
    /// credential, no cookie, no account header.
    pub(crate) fn native(&self, t: &TestServer) -> Signed {
        Signed { client: t.client().bearer(&self.token), account: self.account.clone(), token: self.token.clone() }
    }

    /// Sends a JSON body (empty for none) and requires `code`; returns the
    /// decoded reply, `null` when it is not JSON.
    pub(crate) async fn must(&self, method: &str, path: &str, body: &str, code: u16) -> serde_json::Value {
        let response =
            self.client.send(method, path, &[("content-type", "application/json")], Body::from(body.to_owned())).await;
        assert_eq!(response.status.as_u16(), code, "{method} {path}: {}", response.text());
        serde_json::from_slice(&response.body).unwrap_or(serde_json::Value::Null)
    }
}

/// [`signed_in_client`] as a [`Signed`] browser: the session cookie and
/// `X-Honorum-Account` on every request.
pub(crate) async fn signed_in_browser(t: &TestServer) -> Signed {
    let c = signed_in_client(t).await;
    let client = t
        .client()
        .with_header("cookie", &format!("{SESSION_COOKIE}={}", c.cookie))
        .with_header("x-honorum-account", &c.account);
    Signed { client, account: c.account, token: c.cookie }
}

/// A phone verifier whose every sent code is 123456, and a code works once.
#[derive(Default)]
pub(crate) struct FakeVerifier {
    pub(crate) state: Mutex<FakeState>,
}

#[derive(Default)]
pub(crate) struct FakeState {
    pub(crate) codes: HashMap<String, String>,
    pub(crate) sent: Vec<String>,
    pub(crate) checks: usize,
}

impl FakeVerifier {
    pub(crate) fn sent(&self) -> Vec<String> {
        self.state.lock().unwrap().sent.clone()
    }

    pub(crate) fn checks(&self) -> usize {
        self.state.lock().unwrap().checks
    }
}

#[async_trait]
impl PhoneVerifier for FakeVerifier {
    async fn send(&self, phone: &str) -> anyhow::Result<()> {
        let mut state = self.state.lock().unwrap();
        state.codes.insert(phone.to_owned(), "123456".into());
        state.sent.push(phone.to_owned());
        Ok(())
    }

    async fn check(&self, phone: &str, code: &str) -> anyhow::Result<()> {
        let mut state = self.state.lock().unwrap();
        state.checks += 1;
        let expected = state.codes.get(phone).cloned().unwrap_or_default();
        if expected.is_empty() {
            return Err(PhoneError {
                status: 400,
                message: "This code has expired or was already used. Request another code.".into(),
                ..Default::default()
            }
            .into());
        }
        if expected != code {
            return Err(PhoneError {
                status: 400,
                message: "That code is incorrect. Try again.".into(),
                ..Default::default()
            }
            .into());
        }
        state.codes.remove(phone);
        Ok(())
    }
}

/// Installs a [`FakeVerifier`] and returns it.
pub(crate) fn fake_verifier(t: &TestServer) -> Arc<FakeVerifier> {
    let v = Arc::new(FakeVerifier::default());
    t.server.set_verifier(Some(v.clone()));
    v
}

/// Signs [`TEST_PHONE`] in as device "Test",
/// installing a [`FakeVerifier`] when the server has no verifier.
pub(crate) async fn signed_in_client(t: &TestServer) -> Client {
    if t.server.verifier().is_none() {
        fake_verifier(t);
    }
    t.server.verifier().unwrap().send(TEST_PHONE).await.unwrap();
    let mut c = Client::new(t);
    c.must("POST", "/api/auth/verify", r#"{"phone":"+12015550123","code":"123456","device":"Test"}"#, 200).await;
    c
}

/// The first column of `query`'s first row.
pub(crate) async fn count(s: &Server, query: &str) -> i64 {
    s.db.fetch_one(query, &[]).await.unwrap().get(0)
}
