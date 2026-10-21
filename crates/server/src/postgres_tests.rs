//! PostgreSQL behavior: reopening, initialization, claims, locks, events,
//! snapshots, replicas, and batched revisions.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};
use std::time::Duration;

use serde_json::value::RawValue;
use tokio_util::sync::CancellationToken;

use crate::Server;
use crate::db::{self, Db, Sql};
use crate::docs::{Doc, pull_docs, push_docs};
use crate::session::MAX_DEVICES;
use crate::test_support::test_server;
use crate::test_support::{TEST_PHONE, count};
use crate::testing::TestDatabase;

fn task(id: &str, base: i64, body: &str) -> Doc {
    Doc {
        id: id.into(),
        kind: "task".into(),
        base,
        body: Some(RawValue::from_string(body.into()).unwrap()),
        ..Doc::default()
    }
}

#[tokio::test]
async fn database_reopens_phone_accounts() {
    let test = TestDatabase::new().await;
    let db = Db::open(&test.url, 4, 2).await.unwrap();
    db.exec("INSERT INTO accounts(id,phone) VALUES('a','+12015550123')", &[]).await.unwrap();
    db.close();
    let db = Db::open(&test.url, 4, 2).await.unwrap();
    let phone: String = db.fetch_one("SELECT phone FROM accounts WHERE id='a'", &[]).await.unwrap().get(0);
    assert_eq!(phone, TEST_PHONE, "lost account");
    assert!(
        db.exec("INSERT INTO accounts(id,phone) VALUES('b','+12015550123')", &[]).await.is_err(),
        "duplicate phone accepted"
    );
}

#[tokio::test]
async fn invalid_database_initialization_rolls_back() {
    let test = TestDatabase::new().await;
    let db = Db::open(&test.url, 4, 2).await.unwrap();
    // An incompatible existing schema must fail without committing new tables.
    db.client()
        .await
        .unwrap()
        .batch_execute("DROP SCHEMA public CASCADE; DROP SCHEMA catalog CASCADE; CREATE SCHEMA public; CREATE TABLE watches(id TEXT PRIMARY KEY)")
        .await
        .unwrap();
    assert!(crate::schema::initialize(&db).await.is_err(), "invalid initialization succeeded");
    let n = db
        .fetch_one(
            "SELECT COUNT(*) FROM information_schema.tables WHERE table_schema='public' AND table_name='accounts'",
            &[],
        )
        .await
        .unwrap()
        .get::<_, i64>(0);
    assert_eq!(n, 0, "partial schema");
}

#[tokio::test]
async fn postgres_claims_and_new_generations() {
    let t = test_server().await;
    let s = &t.server;
    s.db.exec("INSERT INTO work_jobs(kind,key,run_at) VALUES('lab','one',0)", &[]).await.unwrap();
    assert!(s.claim_work(&[]).await.unwrap().is_none());
    assert!(s.claim_work(&["other"]).await.unwrap().is_none());
    let mut tasks = tokio::task::JoinSet::new();
    for _ in 0..8 {
        let s = s.clone();
        tasks.spawn(async move { s.claim_work(&["other", "lab"]).await.unwrap() });
    }
    let jobs: Vec<_> = tasks.join_all().await.into_iter().flatten().collect();
    assert_eq!(jobs.len(), 1, "claimed {} times", jobs.len());
    let job = jobs.into_iter().next().unwrap();
    s.run_work(job, |_| async {
        s.db.exec("UPDATE work_jobs SET generation=generation+1,run_at=0 WHERE kind='lab'", &[]).await?;
        Ok(())
    })
    .await;
    let job = s.claim_work(&["lab"]).await.unwrap();
    assert!(matches!(&job, Some(j) if j.generation == 2), "new generation lost: {job:?}");
    // A live lock blocks the same key in another holder, while unrelated
    // keys remain available.
    let held = s.db.lock("lab", "a").await.unwrap();
    let duplicate = tokio::time::timeout(Duration::from_millis(50), s.db.lock("lab", "a")).await;
    assert!(duplicate.is_err(), "duplicate distributed lock");
    let other = s.db.lock("lab", "b").await.unwrap();
    drop(other);
    drop(held);
}

#[tokio::test]
async fn postgres_remote_events_and_scheduled_reminders() {
    let t = test_server().await;
    let s = &t.server;
    s.db.exec("INSERT INTO accounts(id,phone,seq) VALUES('a','1',0)", &[]).await.unwrap();
    let wake = s.events.subscribe("a", "");
    let stop = CancellationToken::new();
    let listener = {
        let (s, stop) = (s.clone(), stop.clone());
        tokio::spawn(async move { s.run_events(stop).await })
    };
    tokio::time::timeout(Duration::from_secs(3), wake.woken()).await.expect("listener did not start");
    s.db.exec("UPDATE accounts SET seq=1 WHERE id='a'", &[]).await.unwrap();
    tokio::time::timeout(Duration::from_secs(3), wake.woken())
        .await
        .expect("commit in another connection did not wake listener");
    stop.cancel();
    tokio::time::timeout(Duration::from_secs(3), listener).await.expect("listener did not stop").unwrap();
    drop(wake);
    // A due guest reminder is derived once, claimed from the due index, and
    // enqueued exactly once even when two workers tick at the same instant.
    let current = chrono::DateTime::parse_from_rfc3339("2026-10-05T13:45:00Z").unwrap().to_utc();
    s.clock.set(current);
    s.db.exec(
        "INSERT INTO reminders(id,target,tz,items,updated_at) VALUES('guest','destination','UTC',$1,$2)",
        &[&r#"[{"ID":"class","Label":"CS","Days":"M","Start":840}]"#, &current.timestamp()],
    )
    .await
    .unwrap();
    let ticking = CancellationToken::new();
    s.tick_scheduled_reminders(&ticking).await;
    s.tick_scheduled_reminders(&ticking).await;
    assert_eq!(count(&t.server, "SELECT COUNT(*) FROM deliveries").await, 1, "scheduled deliveries");
    let next = count(&t.server, "SELECT next_at FROM reminder_schedule WHERE id='guest'").await;
    assert!(next > current.timestamp(), "schedule not advanced: {next}");
}

#[tokio::test]
async fn postgres_account_serialization_and_snapshot() {
    let test = TestDatabase::new().await;
    let db = Db::open(&test.url, 16, 2).await.unwrap();
    db.exec("INSERT INTO accounts(id,phone,seq) VALUES('a','1',0),('b','2',0)", &[]).await.unwrap();
    let mut reader = db.client().await.unwrap();
    let read = reader
        .build_transaction()
        .isolation_level(tokio_postgres::IsolationLevel::RepeatableRead)
        .read_only(true)
        .start()
        .await
        .unwrap();
    read.fetch_one("SELECT seq FROM accounts WHERE id='a'", &[]).await.unwrap();
    let mut writer_client = db.client().await.unwrap();
    let writer = writer_client.transaction().await.unwrap();
    push_docs(&writer, "a", vec![task("t_1", 0, r#"{"title":"one"}"#)], chrono::Utc::now()).await.unwrap();
    {
        let mut other_client = db.client().await.unwrap();
        let other = other_client.transaction().await.unwrap();
        other.exec("UPDATE accounts SET seq=seq WHERE id='b'", &[]).await.unwrap();
        other.rollback().await.unwrap();
    }
    writer.commit().await.unwrap();
    let (docs, cursor, _) = pull_docs(&read, "a", 0, &HashMap::new()).await.unwrap();
    assert!(cursor == 0 && docs.is_empty(), "snapshot skipped changes: {docs:?} {cursor}");
    read.commit().await.unwrap();
    let mut write_client = db.client().await.unwrap();
    let write = write_client.transaction().await.unwrap();
    let (_, conflicts, _) =
        push_docs(&write, "a", vec![task("t_1", 0, r#"{"title":"stale"}"#)], chrono::Utc::now()).await.unwrap();
    assert_eq!(conflicts.len(), 1, "conflict recovery: {conflicts:?}");
}

#[tokio::test]
async fn postgres_limits_across_replicas_and_concurrent_sign_in() {
    let t = test_server().await;
    let s = t.server.clone();
    let dir = std::env::temp_dir().join(format!("honorum-test-{}", crate::session::new_id()));
    let replica = Server::new(s.db.clone(), s.cfg.clone(), Vec::new(), dir).await.unwrap();
    let accepted = Arc::new(AtomicI64::new(0));
    let mut tasks = tokio::task::JoinSet::new();
    for i in 0..40 {
        let instance = if i % 2 == 0 { replica.clone() } else { s.clone() };
        let accepted = accepted.clone();
        tasks.spawn(async move {
            if instance.rate_allow("sms-number:+12015550123", 5, 600).await.unwrap() {
                accepted.fetch_add(1, Ordering::SeqCst);
            }
        });
    }
    tasks.join_all().await;
    assert_eq!(accepted.load(Ordering::SeqCst), 5, "replicas accepted the wrong number of requests");
    let mut tasks = tokio::task::JoinSet::new();
    for _ in 0..MAX_DEVICES + 5 {
        let s = s.clone();
        tasks.spawn(async move { s.start_session("+12015550123", "Test").await.unwrap() });
    }
    tasks.join_all().await;
    assert_eq!(count(&t.server, "SELECT count(*) FROM sessions").await, MAX_DEVICES, "device count");
}

#[tokio::test]
async fn postgres_local_limits_stay_within_budget_across_replicas() {
    let t = test_server().await;
    let s = t.server.clone();
    let dir = std::env::temp_dir().join(format!("honorum-test-{}", crate::session::new_id()));
    let replica = Server::new(s.db.clone(), s.cfg.clone(), Vec::new(), dir).await.unwrap();
    let start = chrono::Utc::now();
    s.clock.set(start);
    replica.clock.set(start);
    // Each replica admits part of the budget in process; the database
    // records every admitted request with the next read.
    let mut accepted = 0;
    for i in 0..60 {
        let instance = if i % 2 == 0 { &replica } else { &s };
        if instance.request_allow("account:a", 20, 60).await.unwrap() {
            accepted += 1;
        }
    }
    assert_eq!(accepted, 20, "replicas accepted the wrong number of requests");
    let hits: String = s.db.fetch_one("SELECT hits FROM rate_limits", &[]).await.unwrap().get(0);
    assert_eq!(serde_json::from_str::<Vec<i64>>(&hits).unwrap().len(), 20, "recorded requests");
    // A new period starts over.
    s.clock.set(start + chrono::TimeDelta::seconds(61));
    assert!(s.request_allow("account:a", 20, 60).await.unwrap(), "next period refused");
}

#[tokio::test]
async fn postgres_session_activity_is_written_in_batches() {
    let t = test_server().await;
    let s = &t.server;
    let c = crate::test_support::signed_in_browser(&t).await;
    s.clock.set(chrono::Utc::now());
    let earlier = s.now().timestamp() - 3600;
    s.db.exec("UPDATE sessions SET last_seen=$1", &[&earlier]).await.unwrap();
    c.must("GET", "/api/me", "", 200).await;
    assert_eq!(count(s, "SELECT last_seen FROM sessions").await, earlier, "activity written on the request path");
    s.flush_seen().await;
    assert_eq!(count(s, "SELECT last_seen FROM sessions").await, s.now().timestamp(), "activity not written");
}

#[tokio::test]
async fn postgres_heartbeats_read_every_stream_at_once() {
    use crate::hub::Wakeup;
    let t = test_server().await;
    let s = &t.server;
    s.db.exec("INSERT INTO accounts(id,phone,seq) VALUES('a','1',7)", &[]).await.unwrap();
    s.db.exec("INSERT INTO sessions(id,account,token_hash,created_at,last_seen) VALUES('live','a','h',0,0)", &[])
        .await
        .unwrap();
    let live = s.events.subscribe("a", "live");
    let gone = s.events.subscribe("a", "gone");
    s.heartbeat(&s.events.slots(0, 1)).await.unwrap();
    assert!(matches!(live.woken().await, Wakeup::Beat(Some((7, 0)))), "live stream heartbeat");
    assert!(matches!(gone.woken().await, Wakeup::Beat(None)), "deleted session kept streaming");
    // A change outranks a heartbeat read before it.
    s.heartbeat(&s.events.slots(0, 1)).await.unwrap();
    s.events.notify("a");
    assert!(matches!(live.woken().await, Wakeup::Changed), "heartbeat hid a change");
}

#[tokio::test]
async fn postgres_batched_revisions_and_deleted_account() {
    let t = test_server().await;
    let s = &t.server;
    s.db.exec("INSERT INTO accounts(id,phone,seq) VALUES('a','1',0)", &[]).await.unwrap();
    {
        let mut client = s.db.client().await.unwrap();
        let tx = client.transaction().await.unwrap();
        let (applied, conflicts, seq) = push_docs(
            &tx,
            "a",
            vec![
                task("t_same", 0, r#"{"title":"first"}"#),
                task("t_same", 1, r#"{"title":"second"}"#),
                task("t_same", 1, r#"{"title":"stale"}"#),
            ],
            s.now(),
        )
        .await
        .unwrap();
        assert!(
            seq == 2
                && applied.len() == 2
                && conflicts.len() == 1
                && conflicts[0].body.as_ref().unwrap().get() == r#"{"title":"second"}"#,
            "batched revisions: {applied:?} {conflicts:?} {seq}"
        );
        tx.commit().await.unwrap();
    }
    let body: String =
        s.db.fetch_one("SELECT body FROM doc_history WHERE account='a' AND id='t_same' AND rev=1", &[])
            .await
            .unwrap()
            .get(0);
    assert_eq!(body, r#"{"title":"first"}"#, "intermediate history");
    s.db.exec("DELETE FROM accounts WHERE id='a'", &[]).await.unwrap();
    let err = s.account_streak("a").await.expect_err("deleted account streak");
    assert!(matches!(err.downcast_ref::<db::Error>(), Some(db::Error::NoRows)), "deleted account streak: {err}");
    assert_eq!(
        count(&t.server, "SELECT count(*) FROM account_streaks WHERE account='a'").await,
        0,
        "streak resurrected after deletion"
    );
}
