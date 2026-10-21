//! Database layer checks against a real PostgreSQL.

use std::time::Duration;

use crate::db::{Db, Sql};
use crate::testing::TestDatabase;

#[tokio::test]
async fn schema_initializes_twice_and_binds_parameters() {
    let test = TestDatabase::new().await;
    let db = Db::open(&test.url, 4, 2).await.unwrap();
    Db::open(&test.url, 4, 2).await.unwrap();
    db.exec("INSERT INTO kv(k,v) VALUES($1,$2)", &[&"a", &"it's ?"]).await.unwrap();
    let row = db.fetch_one("SELECT v FROM kv WHERE k=$1 AND v<>'?'", &[&"a"]).await.unwrap();
    assert_eq!(row.get::<_, String>(0), "it's ?");
    assert!(db.fetch_opt("SELECT v FROM kv WHERE k=$1", &[&"missing"]).await.unwrap().is_none());
}

#[tokio::test]
async fn advisory_locks_exclude_and_release() {
    let test = TestDatabase::new().await;
    let db = Db::open(&test.url, 4, 2).await.unwrap();
    let held = db.lock("scope", "key").await.unwrap();
    let waiting = tokio::time::timeout(Duration::from_millis(200), db.lock("scope", "key")).await;
    assert!(waiting.is_err(), "a second holder acquired the lock");
    drop(held);
    tokio::time::timeout(Duration::from_secs(5), db.lock("scope", "key"))
        .await
        .expect("the lock was released")
        .unwrap();
}

#[tokio::test]
async fn transactions_roll_back_when_dropped() {
    let test = TestDatabase::new().await;
    let db = Db::open(&test.url, 4, 2).await.unwrap();
    {
        let mut client = db.client().await.unwrap();
        let tx = client.transaction().await.unwrap();
        tx.exec("INSERT INTO kv(k,v) VALUES('t','1')", &[]).await.unwrap();
    }
    assert!(db.fetch_opt("SELECT v FROM kv WHERE k='t'", &[]).await.unwrap().is_none());
}

#[tokio::test]
async fn notifications_reach_listeners() {
    let test = TestDatabase::new().await;
    let db = Db::open(&test.url, 4, 2).await.unwrap();
    let (_client, mut notes) = db.listen("honorum_accounts").await.unwrap();
    db.exec("SELECT pg_notify('honorum_accounts', $1)", &[&"acct"]).await.unwrap();
    let note = tokio::time::timeout(Duration::from_secs(5), notes.recv()).await.unwrap().unwrap();
    assert_eq!(note.payload(), "acct");
}
