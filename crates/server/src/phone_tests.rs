//! Twilio Verify requests and sign-in sessions.

use std::collections::HashMap;
use std::future::Future;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::Router;
use axum::extract::Request;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use base64::Engine;
use http_body_util::BodyExt;

use crate::config::Twilio;
use crate::db::Sql;
use crate::phone::{PhoneError, PhoneVerifier, TwilioVerify, new_phone_verifier};
use crate::session::{MAX_DEVICES, hash_token};
use crate::test_support::{Client, TEST_PHONE, count, fake_verifier, signed_in_client};
use crate::test_support::{test_server, test_server_with};

/// A request the fake provider received.
pub(crate) struct FakeRequest {
    pub(crate) method: String,
    pub(crate) path: String,
    /// The decoded `user:password` of HTTP basic authentication.
    pub(crate) credentials: String,
    pub(crate) form: HashMap<String, String>,
}

/// Serves `router` on a loopback port and returns its base URL.
pub(crate) async fn serve(router: Router) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    format!("http://{address}")
}

/// A fake Twilio answering every request with `handler`'s status and body.
pub(crate) async fn fake_twilio<F, Fut>(handler: F) -> String
where
    F: Fn(FakeRequest) -> Fut + Clone + Send + Sync + 'static,
    Fut: Future<Output = (u16, String)> + Send + 'static,
{
    serve(Router::new().fallback(move |request: Request| {
        let handler = handler.clone();
        async move {
            let (parts, body) = request.into_parts();
            let credentials = parts
                .headers
                .get("authorization")
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.strip_prefix("Basic "))
                .and_then(|v| base64::engine::general_purpose::STANDARD.decode(v).ok())
                .map(|v| String::from_utf8_lossy(&v).into_owned())
                .unwrap_or_default();
            let body = body.collect().await.unwrap().to_bytes();
            let form = url::form_urlencoded::parse(&body).into_owned().collect();
            let (status, body) = handler(FakeRequest {
                method: parts.method.to_string(),
                path: parts.uri.path().to_owned(),
                credentials,
                form,
            })
            .await;
            (StatusCode::from_u16(status).unwrap(), body).into_response()
        }
    }))
    .await
}

fn test_twilio() -> Twilio {
    Twilio { account_sid: "ACtest".into(), auth_token: "secret".into(), service_sid: "VAtest".into() }
}

#[tokio::test]
async fn twilio_requests_and_verification_results() {
    let cases = [
        ("send", "Verifications", r#"{"status":"pending","to":"+12015550123"}"#, 201, 0),
        ("approved", "VerificationCheck", r#"{"status":"approved","to":"+12015550123"}"#, 200, 0),
        ("wrong code", "VerificationCheck", r#"{"status":"pending","to":"+12015550123"}"#, 200, 400),
        ("wrong recipient", "VerificationCheck", r#"{"status":"approved","to":"+12015550124"}"#, 200, 503),
        ("expired or used", "VerificationCheck", r#"{"code":20404}"#, 404, 400),
        ("exhausted", "VerificationCheck", r#"{"code":60202}"#, 429, 429),
        ("rate limited", "Verifications", r#"{"code":60203}"#, 429, 429),
        ("landline", "Verifications", r#"{"code":60205}"#, 400, 400),
        ("fraud blocked", "Verifications", r#"{"code":60410}"#, 403, 400),
        ("credentials rejected", "Verifications", r#"{"code":20003,"message":"sensitive provider detail"}"#, 401, 503),
        ("invalid response", "VerificationCheck", "<html>outage</html>", 502, 503),
        ("missing status", "VerificationCheck", r#"{"to":"+12015550123"}"#, 200, 503),
    ];
    for (name, resource, body, http_status, want_status) in cases {
        let calls = Arc::new(AtomicUsize::new(0));
        let problems = Arc::new(Mutex::new(Vec::<String>::new()));
        let (seen, log) = (calls.clone(), problems.clone());
        let url = fake_twilio(move |r: FakeRequest| {
            let (seen, log) = (seen.clone(), log.clone());
            async move {
                seen.fetch_add(1, Ordering::SeqCst);
                let mut log = log.lock().unwrap();
                if r.path != format!("/v2/Services/VAtest/{resource}") || r.method != "POST" {
                    log.push(format!("{} {}", r.path, r.method));
                }
                if r.credentials != "ACtest:secret" {
                    log.push("missing provider credentials".into());
                }
                if r.form.get("To").map(String::as_str) != Some(TEST_PHONE) {
                    log.push("phone did not survive form encoding".into());
                }
                if resource == "Verifications" && r.form.get("Channel").map(String::as_str) != Some("sms") {
                    log.push("wrong channel".into());
                }
                if resource == "VerificationCheck" && r.form.get("Code").map(String::as_str) != Some("012345") {
                    log.push("leading zero was lost".into());
                }
                (http_status, body.to_owned())
            }
        })
        .await;
        let v = TwilioVerify::new(test_twilio(), &url);
        let result =
            if resource == "Verifications" { v.send(TEST_PHONE).await } else { v.check(TEST_PHONE, "012345").await };
        assert_eq!(calls.load(Ordering::SeqCst), 1, "{name}: provider request retried");
        assert!(problems.lock().unwrap().is_empty(), "{name}: {:?}", problems.lock().unwrap());
        if want_status == 0 {
            result.unwrap_or_else(|e| panic!("{name}: {e}"));
            continue;
        }
        let err = result.expect_err(name);
        let message = err.to_string();
        let failure = err.downcast::<PhoneError>().unwrap_or_else(|e| panic!("{name}: unexpected error: {e}"));
        assert!(
            failure.status == want_status && !message.contains("sensitive"),
            "{name}: unexpected error: {failure:?}"
        );
        if want_status == 429 {
            assert!(failure.retry_after > 0, "{name}: rate limit has no retry delay");
        }
    }
}

#[tokio::test]
async fn twilio_cancellation_and_redirects() {
    // Dropping the call cancels the request; it is never retried.
    let calls = Arc::new(AtomicUsize::new(0));
    let seen = calls.clone();
    let url = fake_twilio(move |_| {
        let seen = seen.clone();
        async move {
            seen.fetch_add(1, Ordering::SeqCst);
            std::future::pending::<()>().await;
            (200, String::new())
        }
    })
    .await;
    let v = TwilioVerify::new(test_twilio(), &url);
    let result = tokio::time::timeout(Duration::from_millis(500), v.check(TEST_PHONE, "123456")).await;
    assert!(result.is_err() && calls.load(Ordering::SeqCst) == 1, "cancellation did not stop verification");

    // A redirect is not followed, so credentials never leave the provider.
    let followed = Arc::new(AtomicUsize::new(0));
    let hits = followed.clone();
    let other = serve(Router::new().fallback(move || {
        hits.fetch_add(1, Ordering::SeqCst);
        async { "{}" }
    }))
    .await;
    let redirect = serve(Router::new().fallback(move || {
        let other = other.clone();
        async move {
            let mut response: Response = (StatusCode::FOUND, "{}").into_response();
            response.headers_mut().insert("location", other.parse().unwrap());
            response
        }
    }))
    .await;
    let v = TwilioVerify::new(test_twilio(), &redirect);
    assert!(v.send(TEST_PHONE).await.is_err(), "redirect was accepted");
    assert_eq!(followed.load(Ordering::SeqCst), 0, "followed redirect with credentials");
}

#[tokio::test]
async fn concurrent_code_verification_creates_one_session() {
    let t = test_server().await;
    let v = fake_verifier(&t);
    v.send(TEST_PHONE).await.unwrap();
    let client = t.client();
    let mut tasks = tokio::task::JoinSet::new();
    for _ in 0..8 {
        let client = client.clone();
        tasks.spawn(async move {
            client.post("/api/auth/verify", &serde_json::json!({"phone": TEST_PHONE, "code": "123456"})).await.status
        });
    }
    while let Some(status) = tasks.join_next().await {
        let status = status.unwrap().as_u16();
        assert!(status == 200 || status == 400, "unexpected verification status: {status}");
    }
    let accounts = count(&t.server, "SELECT COUNT(*) FROM accounts").await;
    let sessions = count(&t.server, "SELECT COUNT(*) FROM sessions").await;
    assert!(accounts == 1 && sessions == 1, "accounts={accounts} sessions={sessions}");
}

#[tokio::test]
async fn failed_session_write_rolls_back_and_requires_new_code() {
    let t = test_server().await;
    let v = fake_verifier(&t);
    v.send(TEST_PHONE).await.unwrap();
    let admin = t.server.db.client().await.unwrap();
    admin.batch_execute("CREATE FUNCTION reject_session() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RAISE EXCEPTION 'disk unavailable'; END $$; CREATE TRIGGER reject_session BEFORE INSERT ON sessions FOR EACH ROW EXECUTE FUNCTION reject_session()").await.unwrap();
    let mut c = Client::new(&t);
    let out = c.must("POST", "/api/auth/verify", r#"{"phone":"+12015550123","code":"123456"}"#, 503).await;
    assert!(
        c.cookie.is_empty() && out["error"].as_str().unwrap().contains("another code"),
        "failed sign-in was reported incorrectly"
    );
    assert_eq!(count(&t.server, "SELECT COUNT(*) FROM accounts").await, 0, "partial account committed");
    admin.batch_execute("DROP TRIGGER reject_session ON sessions").await.unwrap();
    c.must("POST", "/api/auth/verify", r#"{"phone":"+12015550123","code":"123456"}"#, 400).await;
    c.must("POST", "/api/auth/code", r#"{"consent":"2026-09-08-v2","phone":"+12015550123"}"#, 202).await;
    c.must("POST", "/api/auth/verify", r#"{"phone":"+12015550123","code":"123456"}"#, 200).await;
}

#[tokio::test]
async fn device_limit_retires_oldest_session() {
    let t = test_server().await;
    let mut c = signed_in_client(&t).await;
    let now = t.server.now().timestamp();
    for i in 0..MAX_DEVICES - 1 {
        let id = i.to_string();
        t.server
            .db
            .exec(
                "INSERT INTO sessions(id,account,token_hash,name,created_at,last_seen) VALUES($1,$2,$3,'Test',$4,$5)",
                &[&id, &c.account, &hash_token(&id), &(now - 100 - i), &(now - 100 - i)],
            )
            .await
            .unwrap();
    }
    signed_in_client(&t).await;
    let total = count(&t.server, "SELECT COUNT(*) FROM sessions").await;
    let oldest = count(&t.server, &format!("SELECT COUNT(*) FROM sessions WHERE id='{}'", MAX_DEVICES - 2)).await;
    assert!(total == MAX_DEVICES && oldest == 0, "count={total} oldest retained={oldest}");
    c.must("GET", "/api/me", "", 200).await;
}

#[tokio::test]
async fn twilio_configuration_and_public_capabilities() {
    // Partial credentials are refused by configuration loading (see the
    // config tests); without credentials there is no verifier.
    assert!(new_phone_verifier(&Twilio::default()).is_none(), "unconfigured SMS enabled");
    let private = Twilio {
        account_sid: "ACprivate".into(),
        auth_token: "private-secret".into(),
        service_sid: "VAprivate".into(),
    };
    assert!(new_phone_verifier(&private).is_some());
    let t = test_server_with(|cfg| cfg.twilio = private.clone()).await;
    let mut c = Client::new(&t);
    for path in ["/api/config", "/api/status"] {
        let (rec, out) = c.send("GET", path, "").await;
        assert!(
            rec.status == 200 && out["phone_auth"] == true && !rec.text().contains("private"),
            "bad public config: {}",
            rec.text()
        );
    }
}
