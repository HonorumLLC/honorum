//! Account details, devices, deletion, and account guards.

use std::time::Duration;

use axum::body::Body;
use serde_json::Value;

use crate::db::Sql;
use crate::session::SESSION_COOKIE;
use crate::test_support::TestClient;
use crate::test_support::test_server;
use crate::test_support::{Client, TEST_PHONE, count, signed_in_client};
use crate::{Db, Server};

/// Runs SQL that may hold several statements, such as trigger definitions.
pub(crate) async fn batch(s: &Server, sql: &str) {
    s.db.client().await.unwrap().batch_execute(sql).await.unwrap();
}

fn device_name(me: &Value) -> String {
    me["devices"]
        .as_array()
        .unwrap()
        .iter()
        .find(|d| d["current"] == true)
        .unwrap_or_else(|| panic!("no current device: {me}"))["name"]
        .as_str()
        .unwrap()
        .to_owned()
}

#[tokio::test]
async fn account_writes_report_failures() {
    let cases = [
        ("POST", "/api/auth/logout", "", "DELETE ON sessions"),
        ("DELETE", "/api/me/devices/other", "", "DELETE ON sessions"),
        ("PUT", "/api/me/push", r#"{"subscription":""}"#, "UPDATE ON sessions"),
        ("DELETE", "/api/me/feeds/f", "", "DELETE ON feed_connections"),
    ];
    for (method, path, body, operation) in cases {
        let t = test_server().await;
        let mut c = signed_in_client(&t).await;
        batch(&t.server, "INSERT INTO feed_connections(account,id,credential) SELECT account,'f',''::bytea FROM sessions LIMIT 1;
INSERT INTO sessions(id,account,token_hash,name,created_at,last_seen) SELECT 'other',account,'other','Other',0,last_seen FROM sessions LIMIT 1;").await;
        batch(&t.server, &format!("CREATE FUNCTION reject_write() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RAISE EXCEPTION 'disk unavailable'; END $$; CREATE TRIGGER reject_write BEFORE {operation} FOR EACH ROW EXECUTE FUNCTION reject_write()")).await;
        let cookie = c.cookie.clone();
        c.must(method, path, body, 500).await;
        assert_eq!(c.cookie, cookie, "{method} {path}: failed write cleared cookie");
    }
}

#[tokio::test]
async fn account_patch_renames_device() {
    let t = test_server().await;
    let mut c = signed_in_client(&t).await;
    let out = c.must("PATCH", "/api/me", r#"{"device":"New name"}"#, 200).await;
    assert_eq!(device_name(&out), "New name", "rename: {out}");
    batch(&t.server, "CREATE FUNCTION reject_name() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RAISE EXCEPTION 'disk unavailable'; END $$; CREATE TRIGGER reject_name BEFORE UPDATE OF name ON sessions FOR EACH ROW EXECUTE FUNCTION reject_name()").await;
    c.must("PATCH", "/api/me", r#"{"device":"Other"}"#, 500).await;
    let out = c.must("GET", "/api/me", "", 200).await;
    assert_eq!(device_name(&out), "New name", "failed rename changed the device: {out}");
}

#[tokio::test]
async fn session_database_failure_does_not_sign_out() {
    let t = test_server().await;
    let mut c = signed_in_client(&t).await;
    t.server.db.close();
    c.must("GET", "/api/me", "", 500).await;
    // Sign-in just read the address's limit, so this process admits the
    // request without the database, and the session read fails.
    c.must("POST", "/api/me/sync", r#"{"since":0,"docs":[]}"#, 500).await;
}

#[tokio::test]
async fn account_deletion_removes_sessions_and_data() {
    let t = test_server().await;
    let mut c = signed_in_client(&t).await;
    c.must(
        "POST",
        "/api/me/sync",
        r#"{"since":0,"docs":[{"id":"t_one","type":"task","body":{"title":"Private"}}]}"#,
        200,
    )
    .await;
    c.must(
        "POST",
        "/api/me/sync",
        r#"{"since":1,"docs":[{"id":"t_one","type":"task","base":1,"body":{"title":"Private, edited"}}]}"#,
        200,
    )
    .await;
    c.must("DELETE", "/api/me", "", 204).await;
    for table in ["accounts", "sessions", "docs", "doc_history"] {
        let n = count(&t.server, &format!("SELECT COUNT(*) FROM {table}")).await;
        assert_eq!(n, 0, "retained {table}");
    }
}

#[tokio::test]
async fn account_watch_requires_session_and_destination() {
    let t = test_server().await;
    Client::new(&t).must("POST", "/api/watches", r#"{"channel":"account"}"#, 401).await;
    let mut c = signed_in_client(&t).await;
    // No device has notifications on.
    c.must("POST", "/api/watches", r#"{"channel":"account"}"#, 400).await;
}

#[tokio::test]
async fn account_header_guards_all_account_actions() {
    let t = test_server().await;
    let mut c = signed_in_client(&t).await;
    let name = device_name(&c.must("GET", "/api/me", "", 200).await);
    let cookie = format!("{SESSION_COOKIE}={}", c.cookie);
    let mut failures = Vec::new();
    for owner in ["another-account", TEST_PHONE, ""] {
        for (method, path, body) in [
            ("GET", "/api/me", ""),
            ("PATCH", "/api/me", r#"{"device":"Stale tab"}"#),
            ("DELETE", "/api/me", ""),
            ("POST", "/api/auth/logout", ""),
            ("PUT", "/api/me/feeds", r#"{"url":"https://example.edu/feed.ics"}"#),
            ("GET", "/api/me/history/t_x", ""),
            ("POST", "/api/me/sync", r#"{"since":0,"docs":[]}"#),
            ("POST", "/api/watches", r#"{"channel":"push"}"#),
        ] {
            let headers = [("cookie", cookie.as_str()), ("x-honorum-account", owner)];
            let res = c.http.send(method, path, &headers, Body::from(body)).await;
            if res.status != 409 {
                failures.push(format!("{method} {path} with owner {owner:?}: {} {}", res.status, res.text()));
            }
        }
    }
    assert!(failures.is_empty(), "{failures:#?}");
    let out = c.must("GET", "/api/me", "", 200).await;
    assert_eq!(device_name(&out), name, "stale tab modified account");
}

#[tokio::test]
async fn account_owner_survives_contact_changes() {
    let t = test_server().await;
    let mut c = signed_in_client(&t).await;
    t.server.db.exec("UPDATE accounts SET phone='+442079460123'", &[]).await.unwrap();
    let out = c.must("GET", "/api/me", "", 200).await;
    assert!(out["id"] == c.account.as_str() && out["phone"] == "+442079460123", "{out}");
    let cookie = format!("{SESSION_COOKIE}={}", c.cookie);
    let res = c.http.send("GET", "/api/me", &[("cookie", cookie.as_str())], Body::empty()).await;
    assert_eq!(res.status, 409, "cookie request without ownership was accepted");
}

#[tokio::test]
async fn concurrent_account_watches_reuse_one_registration() {
    let t = test_server().await;
    let mut c = signed_in_client(&t).await;
    t.server.sync_catalog(&t.server.insts[0], None, true).await.unwrap();
    c.must(
        "PUT",
        "/api/me/push",
        r#"{"subscription":"{\"endpoint\":\"https://push.example/a\",\"keys\":{\"p256dh\":\"a\",\"auth\":\"b\"}}"}"#,
        204,
    )
    .await;
    let http: TestClient = c
        .http
        .with_header("cookie", &format!("{SESSION_COOKIE}={}", c.cookie))
        .with_header("x-honorum-account", &c.account);
    let mut tasks = tokio::task::JoinSet::new();
    for _ in 0..8 {
        let http = http.clone();
        tasks.spawn(async move {
            let res = http
                .send("POST", "/api/watches", &[], Body::from(r#"{"inst":"demo","term":"2026FA","section":"10001"}"#))
                .await;
            assert!(res.status == 200 || res.status == 201, "create watch: {} {}", res.status, res.text());
            res.json()["id"].as_str().unwrap().to_owned()
        });
    }
    let ids: Vec<String> = tasks.join_all().await;
    assert!(!ids[0].is_empty() && ids.iter().all(|id| *id == ids[0]), "duplicate account watches created");
    assert_eq!(count(&t.server, "SELECT COUNT(*) FROM watches").await, 1);
}

#[tokio::test]
async fn watch_deletion_reports_database_failure() {
    let t = test_server().await;
    batch(&t.server, "INSERT INTO watches(id) VALUES('guest'); CREATE FUNCTION reject_delete() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RAISE EXCEPTION 'disk unavailable'; END $$; CREATE TRIGGER reject_delete BEFORE DELETE ON watches FOR EACH ROW EXECUTE FUNCTION reject_delete()").await;
    let mut c = Client::new(&t);
    c.must("DELETE", "/api/watches/guest", "", 500).await;
    t.server.db.close();
    // The first deletion just read the address's limit, so this process
    // admits the request without the database, and the deletion fails.
    c.must("DELETE", "/api/watches/guest", "", 500).await;
}

#[tokio::test]
async fn config_needs_only_one_database_connection() {
    let t = test_server().await;
    t.server.sync_catalog(&t.server.insts[0], None, true).await.unwrap();
    // A second server shares the database through a one-connection pool.
    let single = Db::open(&t.db.url, 1, 1).await.unwrap();
    let s = Server::new(single, t.server.cfg.clone(), t.server.insts.clone(), t.server.data_dir.clone()).await.unwrap();
    let client = TestClient::new(crate::router(s));
    let res = tokio::time::timeout(Duration::from_secs(2), client.get("/api/config"))
        .await
        .expect("configuration deadlocked waiting for another database connection");
    assert!(
        res.status == 200 && res.text().contains(r#""structure_snapshot":"#),
        "configuration: {} {}",
        res.status,
        res.text()
    );
}
