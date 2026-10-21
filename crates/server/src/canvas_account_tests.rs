//! Account Canvas connections end to end: publication into account
//! documents, credential protection, completion write-back, import
//! reconciliation against the shared task contract fixture, account locks,
//! and module refresh rules, with the Canvas fixtures they share. The
//! streak test lives with [`crate::streaks`].

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};

use axum::extract::Request;
use chrono::{DateTime, TimeDelta, Utc};
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

use super::*;
use crate::lms::PLANNER_WINDOWS;
use crate::lms::tests::{Upstream, error, reply, upstream};
use crate::seal::decode_canvas_key;
use crate::test_support::{Client, signed_in_client};
use crate::test_support::{TEST_CANVAS_KEY, TestServer, test_server, test_server_with};

/// A Canvas whose planner lists one assignment and
/// whose course list counts reads, a keyed test server, and a signed-in
/// client.
pub(crate) struct CanvasFixture {
    pub(crate) t: TestServer,
    pub(crate) c: Client,
    pub(crate) reads: Arc<AtomicI32>,
    pub(crate) broken: Arc<AtomicBool>,
    /// Set when a request arrives without the fixture token.
    pub(crate) unauthorized: Arc<AtomicBool>,
    pub(crate) up: Upstream,
}

pub(crate) async fn canvas_fixture() -> CanvasFixture {
    let reads = Arc::new(AtomicI32::new(0));
    let broken = Arc::new(AtomicBool::new(false));
    let unauthorized = Arc::new(AtomicBool::new(false));
    let up = upstream({
        let (reads, broken, unauthorized) = (reads.clone(), broken.clone(), unauthorized.clone());
        move |req: Request| {
            let (reads, broken, unauthorized) = (reads.clone(), broken.clone(), unauthorized.clone());
            async move {
                if req.headers().get("authorization").and_then(|v| v.to_str().ok()) != Some("Bearer fixture-token") {
                    unauthorized.store(true, Ordering::SeqCst);
                }
                if broken.load(Ordering::SeqCst) {
                    return error(503, "unavailable");
                }
                match req.uri().path() {
                    "/api/v1/courses" => {
                        reads.fetch_add(1, Ordering::SeqCst);
                        reply(200, "[]")
                    }
                    "/api/v1/planner/items" => reply(
                        200,
                        r#"[{"plannable_type":"assignment","plannable":{"id":42,"title":"Essay","due_at":"2026-09-12T12:00:00Z"}}]"#,
                    ),
                    "/api/v1/planner/overrides" => reply(200, r#"{"id":7,"marked_complete":true}"#),
                    _ => error(404, "404 page not found"),
                }
            }
        }
    })
    .await;
    let t = keyed_server().await;
    let c = signed_in_client(&t).await;
    CanvasFixture { t, c, reads, broken, unauthorized, up }
}

/// A test server with the all-zero credential key that may reach loopback
/// hosts.
pub(crate) async fn keyed_server() -> TestServer {
    let t = test_server_with(|cfg| cfg.canvas_key = TEST_CANVAS_KEY.into()).await;
    t.server.outbound.set_allow_local(true);
    t
}

pub(crate) async fn connect_fixture(c: &mut Client, base: &str) -> Value {
    c.must("PUT", "/api/me/canvas", &json!({"base": base, "token": "fixture-token"}).to_string(), 200).await
}

/// Another handle on the same signed-in device.
pub(crate) fn same_device(c: &Client) -> Client {
    Client { http: c.http.clone(), cookie: c.cookie.clone(), bearer: c.bearer.clone(), account: c.account.clone() }
}

async fn exec(t: &TestServer, query: &str, params: &[Param<'_>]) {
    t.server.db.exec(query, params).await.unwrap();
}

async fn count(t: &TestServer, query: &str, params: &[Param<'_>]) -> i64 {
    t.server.db.fetch_one(query, params).await.unwrap().get(0)
}

async fn body_of(t: &TestServer, account: &str, id: &str) -> DocBody {
    let row =
        t.server.db.fetch_one("SELECT body FROM docs WHERE account=$1 AND id=$2", &[&account, &id]).await.unwrap();
    task_body(row.get::<_, String>(0).as_bytes())
}

#[tokio::test]
async fn account_canvas_publishes_once_and_protects_credential() {
    let mut f = canvas_fixture().await;
    let (t, account) = (&f.t, f.c.account.clone());
    let status = connect_fixture(&mut f.c, &f.up.url).await;
    assert_eq!(status["connected"], true, "{status}");
    assert!(!status.to_string().contains("fixture-token"), "credential returned");
    let connection = t.server.canvas_connection(&account).await.unwrap();
    assert!(!String::from_utf8_lossy(&connection.credential).contains("fixture-token"), "plaintext credential stored");
    assert!(
        t.server.decrypt_canvas("another-account", &connection).is_err(),
        "ciphertext was transferable between accounts"
    );
    exec(t, "UPDATE canvas_connections SET synced=0 WHERE account=$1", &[&account]).await;
    let refreshes = (0..6).map(|_| {
        let mut c = same_device(&f.c);
        async move { c.must("POST", "/api/me/canvas/refresh", r#"{"force":false}"#, 200).await }
    });
    futures::future::join_all(refreshes).await;
    assert_eq!(f.reads.load(Ordering::SeqCst), 2, "imports were not coalesced");
    let out = f.c.must("POST", "/api/me/sync", r#"{"docs":[]}"#, 200).await;
    let docs = out["docs"].as_array().unwrap();
    assert_eq!(docs.len(), 1, "{out}");
    let body = &docs[0]["body"];
    assert!(body["source"] == "canvas" && body["title"] == "Essay", "{body}");
    // A failed refresh keeps every published document and shared freshness.
    f.broken.store(true, Ordering::SeqCst);
    exec(t, "UPDATE canvas_connections SET synced=1 WHERE account=$1", &[&account]).await;
    f.c.must("POST", "/api/me/canvas/refresh", r#"{"force":true}"#, 502).await;
    assert_eq!(count(t, "SELECT COUNT(*) FROM docs WHERE account=$1 AND deleted=0", &[&account]).await, 1);
    f.broken.store(false, Ordering::SeqCst);
    f.c.must("DELETE", "/api/me/canvas", "", 200).await;
    let status = f.c.must("GET", "/api/me/canvas", "", 200).await;
    assert_eq!(status["connected"], false, "{status}");
    f.c.must("DELETE", "/api/me", "", 204).await;
    for table in [
        "canvas_connections",
        "canvas_items",
        "account_activity",
        "account_timezones",
        "account_streaks",
        "account_streak_days",
    ] {
        assert_eq!(count(t, &format!("SELECT COUNT(*) FROM {table}"), &[]).await, 0, "{table} retained account data");
    }
    assert!(!f.unauthorized.load(Ordering::SeqCst), "missing upstream auth");
}

#[tokio::test]
async fn account_canvas_migrates_edits_and_retries_completion() {
    let mut f = canvas_fixture().await;
    let account = f.c.account.clone();
    f.c.must(
        "POST",
        "/api/me/sync",
        r#"{"docs":[{"id":"t_old","type":"task","body":{"id":"old","uid":"canvas:assignment:42","source":"canvas","note":"Keep my note","course":"BIO 101","courseEdited":true,"done":true,"pendingCanvasDone":"intent-1"}},{"id":"t_duplicate","type":"task","body":{"id":"duplicate","uid":"canvas:assignment:42","source":"canvas","note":"Second note"}}]}"#,
        200,
    )
    .await;
    connect_fixture(&mut f.c, &f.up.url).await;
    let body = body_of(&f.t, &account, "t_old").await;
    assert!(body["note"] == "Keep my note" && body["done"] == true && body["course"] == "BIO 101", "{body:?}");
    let duplicate = body_of(&f.t, &account, "t_duplicate").await;
    assert!(duplicate["source"] == "manual" && duplicate["note"] == "Second note", "{duplicate:?}");
    let out = f.c.must("POST", "/api/me/canvas/complete", r#"{"document":"t_old","intent":"stale"}"#, 200).await;
    assert_eq!(out["applied"], false, "{out}");
    let out = f.c.must("POST", "/api/me/canvas/complete", r#"{"document":"t_old","intent":"intent-1"}"#, 200).await;
    assert_eq!(out["applied"], true, "{out}");
    let body = body_of(&f.t, &account, "t_old").await;
    assert!(!body.contains_key("pendingCanvasDone") && body["note"] == "Keep my note", "{body:?}");
    let out = f.c.must("POST", "/api/me/canvas/complete", r#"{"document":"t_old","intent":"intent-1"}"#, 200).await;
    assert_eq!(out["applied"], false, "{out}");
}

#[tokio::test]
async fn account_features_require_account_ownership_and_encryption_key() {
    let t = test_server().await;
    let mut c = signed_in_client(&t).await;
    c.must("PUT", "/api/me/canvas", r#"{"base":"https://canvas.example.edu","token":"token"}"#, 503).await;
    c.account = "wrong-account".into();
    for path in ["/api/me/canvas", "/api/me/streak"] {
        c.must("GET", path, "", 409).await;
    }
    c.must("POST", "/api/me/activity", "{}", 409).await;
    c.must("DELETE", "/api/me/canvas", "", 409).await;
    assert!(decode_canvas_key("not-a-key").is_err(), "invalid encryption key accepted");
}

#[tokio::test]
async fn account_canvas_keeps_established_identity_and_deleted_tasks() {
    let mut f = canvas_fixture().await;
    let (t, account) = (&f.t, f.c.account.clone());
    connect_fixture(&mut f.c, &f.up.url).await;
    async fn document(t: &TestServer, account: &str) -> String {
        let row =
            t.server.db.fetch_one("SELECT document FROM canvas_items WHERE account=$1", &[&account]).await.unwrap();
        row.get::<_, String>(0)
    }
    let id = document(t, &account).await;
    // Another device uploads an import it made before it discovered the connection.
    f.c.must(
        "POST",
        "/api/me/sync",
        r#"{"docs":[{"id":"t_late","type":"task","body":{"id":"late","uid":"canvas:assignment:42","source":"canvas","note":"Offline note"}}]}"#,
        200,
    )
    .await;
    exec(t, "UPDATE canvas_connections SET synced=0 WHERE account=$1", &[&account]).await;
    f.c.must("POST", "/api/me/canvas/refresh", "{}", 200).await;
    let current = document(t, &account).await;
    assert_eq!(current, id, "identity changed");
    let copy = body_of(t, &account, "t_late").await;
    assert!(copy["source"] == "manual" && copy["note"] == "Offline note", "{copy:?}");
    let rev = count(t, "SELECT seq FROM docs WHERE account=$1 AND id=$2", &[&account, &id]).await;
    f.c.must(
        "POST",
        "/api/me/sync",
        &json!({"docs": [{"id": id, "type": "task", "base": rev, "deleted": true}]}).to_string(),
        200,
    )
    .await;
    exec(t, "UPDATE canvas_connections SET synced=0 WHERE account=$1", &[&account]).await;
    f.c.must("POST", "/api/me/canvas/refresh", "{}", 200).await;
    let deleted = count(t, "SELECT deleted FROM docs WHERE account=$1 AND id=$2", &[&account, &id]).await;
    assert_eq!(deleted, 1, "refresh resurrected a deleted task");
}

/// The task an import keeps for `uid`: its document ID, revision, and body.
async fn imported(t: &TestServer, account: &str, uid: &str) -> (String, i64, DocBody) {
    let row = t
        .server
        .db
        .fetch_one(
            "SELECT d.id,d.seq,d.body FROM docs d JOIN canvas_items i ON i.account=d.account AND i.document=d.id WHERE d.account=$1 AND i.uid=$2",
            &[&account, &uid],
        )
        .await
        .unwrap();
    (row.get(0), row.get(1), task_body(row.get::<_, String>(2).as_bytes()))
}

fn task(uid: &str, title: &str, due: &str) -> Task {
    Task { uid: uid.into(), title: title.into(), due: due.into(), ..Task::default() }
}

#[tokio::test]
async fn account_import_keeps_the_students_due_date_until_the_source_moves_it() {
    let t = test_server().await;
    let mut c = signed_in_client(&t).await;
    let account = c.account.clone();
    let publish = |essay: &'static str, reading: &'static str| {
        let (s, account) = (t.server.clone(), account.clone());
        async move {
            let tasks = [
                task("canvas:assignment:1", "Essay", essay),
                task("canvas:assignment:2", "Reading", reading),
                task("canvas:assignment:3", "Quiz", "2026-10-03T05:59:59Z"),
            ];
            s.publish_import(&account, "canvas", "connection", &tasks, None).await.unwrap();
        }
    };
    let due = |uid: &'static str| {
        let (t, account) = (&t, account.clone());
        async move { string_value(imported(t, &account, uid).await.2.get("due")).to_owned() }
    };
    async fn edit(t: &TestServer, c: &mut Client, uid: &str, due: &str) {
        let (id, rev, mut body) = imported(t, &c.account.clone(), uid).await;
        body.insert("due".into(), due.into());
        let docs = json!({"docs": [{"id": id, "type": "task", "base": rev, "body": body}]});
        c.must("POST", "/api/me/sync", &docs.to_string(), 200).await;
    }
    // A guest import from before importedDue was recorded counts as unchanged.
    c.must(
        "POST",
        "/api/me/sync",
        r#"{"docs":[{"id":"t_guest","type":"task","body":{"id":"guest","uid":"canvas:assignment:3","source":"canvas","title":"Quiz","due":"2026-10-07T18:00:00.000Z"}}]}"#,
        200,
    )
    .await;
    publish("2026-10-02T05:59:59Z", "").await;
    assert_eq!(due("canvas:assignment:3").await, "2026-10-03T05:59:59Z", "guest import kept its due date");
    edit(&t, &mut c, "canvas:assignment:1", "2026-10-05T18:00:00.000Z").await;
    edit(&t, &mut c, "canvas:assignment:2", "2026-10-06T18:00:00.000Z").await;
    publish("2026-10-02T05:59:59Z", "").await;
    assert!(
        due("canvas:assignment:1").await == "2026-10-05T18:00:00.000Z"
            && due("canvas:assignment:2").await == "2026-10-06T18:00:00.000Z",
        "a refresh replaced the student's due dates"
    );
    // The instructor moves the essay; the reading stays undated in Canvas.
    publish("2026-10-09T05:59:59Z", "").await;
    let (essay, reading) = (due("canvas:assignment:1").await, due("canvas:assignment:2").await);
    assert!(essay == "2026-10-09T05:59:59Z" && reading == "2026-10-06T18:00:00.000Z", "{essay} {reading}");
}

#[tokio::test]
async fn account_import_keeps_finished_tasks_it_no_longer_lists() {
    let t = test_server().await;
    let c = signed_in_client(&t).await;
    let publish = |tasks: Vec<Task>| {
        let (s, account) = (t.server.clone(), c.account.clone());
        async move { s.publish_import(&account, "canvas", "connection", &tasks, None).await.unwrap() }
    };
    let finished = Task {
        done: true,
        reason: "submitted".into(),
        ..task("canvas:assignment:1", "Reflection", "2026-09-01T04:59:59Z")
    };
    publish(vec![finished, task("canvas:assignment:2", "Skipped reading", "2026-09-01T04:59:59Z")]).await;
    // A week later both have left Canvas's planner window.
    publish(vec![]).await;
    let rows = t
        .server
        .db
        .fetch_all("SELECT body FROM docs WHERE account=$1 AND type='task' AND deleted=0", &[&c.account])
        .await
        .unwrap();
    let titles: Vec<String> = rows
        .iter()
        .map(|r| string_value(task_body(r.get::<_, String>(0).as_bytes()).get("title")).to_owned())
        .collect();
    assert_eq!(titles, ["Reflection"], "kept {titles:?}");
    // The instructor extends the reading, so Canvas lists it again.
    publish(vec![task("canvas:assignment:2", "Skipped reading", "2026-09-20T04:59:59Z")]).await;
    let row = t
        .server
        .db
        .fetch_one(
            "SELECT d.deleted,d.body FROM docs d JOIN canvas_items i ON i.account=d.account AND i.document=d.id WHERE d.account=$1 AND i.uid='canvas:assignment:2'",
            &[&c.account],
        )
        .await
        .expect("a task the import removed did not come back");
    let (deleted, raw): (i64, String) = (row.get(0), row.get(1));
    assert!(
        deleted == 0 && task_body(raw.as_bytes())["due"] == "2026-09-20T04:59:59Z",
        "a task the import removed did not come back: {deleted} {raw}"
    );
}

#[tokio::test]
async fn account_canvas_publication_failure_rolls_back_credential_and_tasks() {
    let mut f = canvas_fixture().await;
    f.t.server
        .db
        .client()
        .await
        .unwrap()
        .batch_execute("CREATE FUNCTION reject_import() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RAISE EXCEPTION 'disk unavailable'; END $$; CREATE TRIGGER reject_import BEFORE INSERT ON docs FOR EACH ROW EXECUTE FUNCTION reject_import()")
        .await
        .unwrap();
    f.c.must("PUT", "/api/me/canvas", &json!({"base": f.up.url, "token": "fixture-token"}).to_string(), 500).await;
    for table in ["canvas_connections", "canvas_items", "docs"] {
        assert_eq!(
            count(&f.t, &format!("SELECT COUNT(*) FROM {table}"), &[]).await,
            0,
            "partial import left {table} rows"
        );
    }
}

#[tokio::test]
async fn account_canvas_completion_does_not_acknowledge_newer_intent() {
    let started = CancellationToken::new();
    let release = CancellationToken::new();
    let up = upstream({
        let (started, release) = (started.clone(), release.clone());
        move |_req: Request| {
            let (started, release) = (started.clone(), release.clone());
            async move {
                started.cancel();
                release.cancelled().await;
                reply(200, r#"{"id":7,"marked_complete":true}"#)
            }
        }
    })
    .await;
    let t = keyed_server().await;
    let mut c = signed_in_client(&t).await;
    let connection =
        CanvasConnection { id: "connection".into(), base: up.url.clone(), connected: true, ..Default::default() };
    let credential = t.server.encrypt_canvas(&c.account, &connection, "fixture-token").unwrap();
    exec(
        &t,
        "INSERT INTO canvas_connections(account,id,base,credential) VALUES($1,$2,$3,$4)",
        &[&c.account, &connection.id, &connection.base, &credential],
    )
    .await;
    c.must(
        "POST",
        "/api/me/sync",
        r#"{"docs":[{"id":"t_item","type":"task","body":{"id":"item","source":"canvas","uid":"canvas:assignment:42","done":true,"pendingCanvasDone":"first"}}]}"#,
        200,
    )
    .await;
    let mut other = same_device(&c);
    let done = tokio::spawn(async move {
        other.must("POST", "/api/me/canvas/complete", r#"{"document":"t_item","intent":"first"}"#, 200).await
    });
    started.cancelled().await;
    let rev = count(&t, "SELECT seq FROM docs WHERE account=$1 AND id='t_item'", &[&c.account]).await;
    let edit = json!({"docs": [{"id": "t_item", "type": "task", "base": rev, "body": {"id": "item", "source": "canvas", "uid": "canvas:assignment:42", "done": false, "pendingCanvasDone": "second", "note": "new edit"}}]});
    c.must("POST", "/api/me/sync", &edit.to_string(), 200).await;
    release.cancel();
    let out = done.await.unwrap();
    assert_eq!(out["applied"], false, "{out}");
    let body = body_of(&t, &c.account, "t_item").await;
    assert!(body["pendingCanvasDone"] == "second" && body["done"] == false && body["note"] == "new edit", "{body:?}");
}

#[tokio::test]
async fn account_locks_separate_accounts_and_honor_cancellation() {
    let locks = AccountLocks::default();
    let unlock = locks.lock("a").await;
    // Another account is not held up by the first.
    let other = locks.lock("b").await;
    drop(other);
    // A waiter on the same account gives up when its request ends.
    let waited = tokio::time::timeout(std::time::Duration::from_millis(10), locks.lock("a")).await;
    assert!(waited.is_err(), "a cancelled waiter took a held lock");
    drop(unlock);
    let again = locks.lock("a").await;
    drop(again);
    let left = locks.held.lock().unwrap().len();
    assert_eq!(left, 0, "{left} locks left behind");
}

/// A decoded JSON value as comparable text: `<none>` for null or absent,
/// strings unquoted, anything else as JSON.
fn display_value(v: Option<&Value>) -> String {
    match v {
        None | Some(Value::Null) => "<none>".into(),
        Some(Value::String(s)) => s.clone(),
        Some(other) => other.to_string(),
    }
}

/// A task's source identity, or its title when it has none.
fn contract_key(task: &DocBody) -> String {
    match string_value(task.get("uid")) {
        "" => format!("manual:{}", string_value(task.get("title"))),
        uid => uid.to_owned(),
    }
}

/// The import reconciliation fixture is generated by the web client's guest
/// merge; an account import on the server must reach the same tasks.
#[tokio::test]
async fn task_import_contract_fixture() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../contracts/fixtures/tasks-merge.json");
    let fixture: Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    for case in fixture["cases"].as_array().unwrap() {
        // Account imports are always complete; previews happen only on devices.
        if case["complete"] != true {
            continue;
        }
        let name = case["name"].as_str().unwrap();
        let t = test_server().await;
        let mut client = signed_in_client(&t).await;
        for (i, task) in case["existing"].as_array().unwrap().iter().enumerate() {
            let mut task = task.clone();
            task["id"] = i.to_string().into();
            let docs = json!({"docs": [{"id": format!("t_{i}"), "type": "task", "body": task}]});
            client.must("POST", "/api/me/sync", &docs.to_string(), 200).await;
        }
        let incoming: Vec<Task> = serde_json::from_value(case["incoming"].clone()).unwrap();
        let source = case["source"].as_str().unwrap();
        t.server.publish_import(&client.account, source, "connection", &incoming, None).await.unwrap();
        let rows = t
            .server
            .db
            .fetch_all("SELECT body FROM docs WHERE account=$1 AND type='task' AND deleted=0", &[&client.account])
            .await
            .unwrap();
        let got: BTreeMap<String, DocBody> = rows
            .iter()
            .map(|r| task_body(r.get::<_, String>(0).as_bytes()))
            .map(|body| (contract_key(&body), body))
            .collect();
        let want: BTreeMap<String, DocBody> = case["expected"]
            .as_array()
            .unwrap()
            .iter()
            .map(|task| task.as_object().unwrap().clone())
            .map(|body| (contract_key(&body), body))
            .collect();
        assert_eq!(got.keys().collect::<Vec<_>>(), want.keys().collect::<Vec<_>>(), "{name}: tasks");
        for (key, w) in &want {
            let g = &got[key];
            for field in ["title", "course", "done", "reason", "importedDue"] {
                let (gv, wv) = (g.get(field), w.get(field));
                let blank = matches!(gv, None | Some(Value::Null)) && wv == Some(&Value::String(String::new()));
                assert!(
                    display_value(gv) == display_value(wv) || blank,
                    "{name}: {key} {field} = {gv:?}, expected {wv:?}"
                );
            }
            assert!(
                same_instant(string_value(g.get("due")), string_value(w.get("due"))),
                "{name}: {key} due = {:?}, expected {:?}",
                g.get("due"),
                w.get("due")
            );
        }
    }
}

/// One course whose planner lists a reading a
/// module requires marking done, counting planner and module reads.
async fn canvas_module_fixture(t: TestServer) -> (TestServer, Client, Arc<AtomicI32>, Arc<AtomicI32>, Upstream) {
    let planner = Arc::new(AtomicI32::new(0));
    let modules = Arc::new(AtomicI32::new(0));
    let up = upstream({
        let (planner, modules) = (planner.clone(), modules.clone());
        move |req: Request| {
            let (planner, modules) = (planner.clone(), modules.clone());
            async move {
                match req.uri().path() {
                    "/api/v1/courses" => reply(200, r#"[{"id":1,"name":"Programming","course_code":"CS 101"}]"#),
                    "/api/v1/planner/items" => {
                        planner.fetch_add(1, Ordering::SeqCst);
                        reply(
                            200,
                            r#"[{"plannable_type":"wiki_page","course_id":1,"context_name":"CS 101","plannable":{"id":40,"title":"Week 1 reading","todo_date":"2026-09-20T04:59:00Z","url":"week-1"},"submissions":false}]"#,
                        )
                    }
                    "/api/v1/courses/1/modules" => {
                        modules.fetch_add(1, Ordering::SeqCst);
                        reply(
                            200,
                            r#"[{"id":5,"items":[{"id":11,"type":"Page","page_url":"week-1","completion_requirement":{"type":"must_mark_done","completed":false}}]}]"#,
                        )
                    }
                    _ => error(404, "404 page not found"),
                }
            }
        }
    })
    .await;
    let c = signed_in_client(&t).await;
    (t, c, planner, modules, up)
}

#[tokio::test]
async fn canvas_manual_refresh_keeps_modules_until_rescan_or_daily() {
    let t = keyed_server().await;
    let start = DateTime::<Utc>::from_timestamp(Utc::now().timestamp(), 0).unwrap();
    t.server.clock.set(start);
    let (t, mut c, planner, modules, up) = canvas_module_fixture(t).await;
    connect_fixture(&mut c, &up.url).await;
    let windows = PLANNER_WINDOWS as i32;
    // The planner is fetched once per date window; connect resolves module
    // routing once so completion works immediately.
    assert!(
        planner.load(Ordering::SeqCst) == windows && modules.load(Ordering::SeqCst) == 1,
        "connect: planner={} modules={}",
        planner.load(Ordering::SeqCst),
        modules.load(Ordering::SeqCst)
    );
    let account = c.account.clone();
    let module_item = || {
        let (t, account) = (&t, account.clone());
        async move {
            let row = t
                .server
                .db
                .fetch_one("SELECT body FROM docs WHERE account=$1 AND type='task' AND deleted=0", &[&account])
                .await
                .unwrap();
            string_value(task_body(row.get::<_, String>(0).as_bytes()).get("moduleItem")).to_owned()
        }
    };
    assert_eq!(module_item().await, "5:11", "connect did not record module routing");
    let mut now = start;
    let mut modules_at = now.timestamp();
    let minute = TimeDelta::minutes(1);
    for (name, advance, body, imports, scans) in [
        ("fresh automatic refresh", minute, r#"{"force":false}"#, 1, 1),
        ("manual refresh bypasses freshness", TimeDelta::zero(), r#"{"force":true}"#, 2, 1),
        ("repeated manual refresh stays fast", minute, r#"{"force":true}"#, 3, 1),
        ("stale automatic refresh stays fast", TimeDelta::minutes(5), r#"{"force":false}"#, 4, 1),
        // A planner-only import just completed; it cannot satisfy a module rescan.
        ("explicit rescan after planner refresh", TimeDelta::zero(), r#"{"rescan_modules":true}"#, 5, 2),
        ("overlapping rescans share the full import", TimeDelta::zero(), r#"{"rescan_modules":true}"#, 5, 2),
        ("rescan bypasses freshness on its own", minute, r#"{"rescan_modules":true}"#, 6, 3),
        ("daily refresh scans modules", TimeDelta::hours(24), r#"{"force":false}"#, 7, 4),
    ] {
        now += advance;
        t.server.clock.set(now);
        let before = modules.load(Ordering::SeqCst);
        c.must("POST", "/api/me/canvas/refresh", body, 200).await;
        let (p, m) = (planner.load(Ordering::SeqCst), modules.load(Ordering::SeqCst));
        assert!(
            p == imports * windows && m == scans,
            "{name}: planner={p} modules={m}, want {} and {scans}",
            imports * windows
        );
        assert_eq!(module_item().await, "5:11", "{name}: refresh lost module routing");
        if m != before {
            modules_at = now.timestamp();
        }
        let connection = t.server.canvas_connection(&account).await.unwrap();
        assert_eq!(connection.modules_at, modules_at, "{name}: module scan timestamp");
    }
}
