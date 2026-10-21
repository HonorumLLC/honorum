//! Document sync, paging, history, owner guards, and document validation.

use std::collections::HashSet;

use serde_json::Value;

use crate::accounts_tests::batch;
use crate::api::{compact_json, final_json, is_compact_json};
use crate::db::Sql;
use crate::docs::{Doc, PULL_BODY_MAX, valid_docs};
use crate::session::hash_token;
use crate::test_support::test_server;
use crate::test_support::{Client, count, signed_in_client};

#[tokio::test]
async fn sync_exchange_keeps_unseen_changes_and_omits_upload_echo() {
    let t = test_server().await;
    let mut c = signed_in_client(&t).await;
    c.must(
        "POST",
        "/api/me/sync",
        r#"{"since":0,"docs":[{"id":"t_remote","type":"task","body":{"title":"Remote"}}]}"#,
        200,
    )
    .await;
    let out = c
        .must(
            "POST",
            "/api/me/sync",
            r#"{"since":0,"docs":[{"id":"t_local","type":"task","body":{"title":"Local"}}]}"#,
            200,
        )
        .await;
    let docs = out["docs"].as_array().unwrap();
    assert!(
        out["seq"] == 2
            && docs.len() == 1
            && docs[0]["id"] == "t_remote"
            && out["applied"].as_array().unwrap().len() == 1,
        "{out}"
    );
    let out = c
        .must(
            "POST",
            "/api/me/sync",
            r#"{"since":0,"docs":[{"id":"t_remote","type":"task","base":0,"body":{"title":"Stale"}}]}"#,
            200,
        )
        .await;
    assert!(out["conflicts"].as_array().unwrap().len() == 1 && out["applied"].as_array().unwrap().is_empty(), "{out}");
    c.must(
        "POST",
        "/api/me/sync",
        r#"{"since":2,"docs":[{"id":"t_remote","type":"task","base":1,"deleted":true}]}"#,
        200,
    )
    .await;
    let out = c.must("POST", "/api/me/sync", r#"{"since":2,"docs":[]}"#, 200).await;
    assert_eq!(out["docs"][0]["deleted"], true, "{out}");
}

#[tokio::test]
async fn sync_pages_bound_bytes_and_do_not_skip_remote_rows() {
    let t = test_server().await;
    let mut c = signed_in_client(&t).await;
    {
        let mut client = t.server.db.client().await.unwrap();
        let tx = client.transaction().await.unwrap();
        let body = format!(r#"{{"title":"{}"}}"#, "<".repeat(10000));
        for i in 1..=505i64 {
            let payload = if i <= 100 { body.as_str() } else { "{}" };
            tx.exec(
                "INSERT INTO docs(account,id,type,seq,updated_at,deleted,body) VALUES($1,$2,'task',$3,0,0,$4)",
                &[&c.account, &format!("t_{i}"), &i, &payload],
            )
            .await
            .unwrap();
        }
        tx.exec("UPDATE accounts SET seq=505 WHERE id=$1", &[&c.account]).await.unwrap();
        tx.commit().await.unwrap();
    }
    let mut out =
        c.must("POST", "/api/me/sync", r#"{"since":0,"docs":[{"id":"t_mine","type":"task","body":{}}]}"#, 200).await;
    let mut seen = HashSet::new();
    let mut pages = 0;
    loop {
        pages += 1;
        // Measured as responses encode it, escaping included.
        let data = compact_json(&serde_json::to_string(&out["docs"]).unwrap());
        assert!(data.len() <= PULL_BODY_MAX, "unbounded page: {}", data.len());
        for d in out["docs"].as_array().unwrap() {
            seen.insert(d["id"].as_str().unwrap().to_owned());
        }
        if out["more"] != true {
            break;
        }
        let since = out["seq"].as_i64().unwrap();
        out = c.must("POST", "/api/me/sync", &format!(r#"{{"since":{since},"docs":[]}}"#), 200).await;
    }
    assert!(
        seen.len() == 506 && pages >= 2 && out["seq"] == 506,
        "lost changes: seen={} pages={pages} cursor={}",
        seen.len(),
        out["seq"]
    );
}

#[tokio::test]
async fn watch_version_changes_without_document_edits() {
    let t = test_server().await;
    let s = &t.server;
    let mut c = signed_in_client(&t).await;
    s.sync_catalog(&s.insts[0], None, true).await.unwrap();
    let first = c.must("POST", "/api/me/sync", r#"{"since":0,"docs":[]}"#, 200).await;
    let version = first["watches_version"].as_str().unwrap().to_owned();
    let sync = |version: &str| format!(r#"{{"since":0,"docs":[],"watches_version":"{version}"}}"#);
    let out = c.must("POST", "/api/me/sync", &sync(&version), 200).await;
    assert!(out.get("watches").is_none(), "unchanged watches repeated");
    s.db.exec(
        "INSERT INTO watches(id,inst,term,section,account,created_at) VALUES('w','demo','2026FA','10001',$1,0)",
        &[&c.account],
    )
    .await
    .unwrap();
    let out = c.must("POST", "/api/me/sync", &sync(&version), 200).await;
    assert!(out["seq"] == first["seq"] && out["watches"].as_array().unwrap().len() == 1, "{out}");
    let version = out["watches_version"].as_str().unwrap().to_owned();
    let raw: String =
        s.db.fetch_one("SELECT data FROM sections WHERE inst='demo' AND term='2026FA' AND id='10001'", &[])
            .await
            .unwrap()
            .get(0);
    let mut section: serde_json::Map<String, Value> = serde_json::from_str(&raw).unwrap();
    section.insert("title".into(), "Renamed course".into());
    let renamed = serde_json::to_string(&section).unwrap();
    s.db.exec("UPDATE sections SET data=$1 WHERE inst='demo' AND term='2026FA' AND id='10001'", &[&renamed])
        .await
        .unwrap();
    let out = c.must("POST", "/api/me/sync", &sync(&version), 200).await;
    assert!(out["watches_version"] != version.as_str() && out["watches"][0]["title"] == "Renamed course", "{out}");
    batch(s, "CREATE FUNCTION reject_doc_write() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RAISE EXCEPTION 'must stay readonly'; END $$; CREATE TRIGGER reject_doc_write BEFORE UPDATE ON accounts FOR EACH ROW EXECUTE FUNCTION reject_doc_write()").await;
    c.must("POST", "/api/me/sync", r#"{"since":0,"docs":[]}"#, 200).await;
}

#[tokio::test]
async fn history_keeps_replaced_versions_for_restoration() {
    let t = test_server().await;
    let mut c = signed_in_client(&t).await;
    c.must(
        "POST",
        "/api/me/sync",
        r#"{"since":0,"docs":[{"id":"t_x","type":"task","updated":1,"body":{"id":"x","title":"First"}}]}"#,
        200,
    )
    .await;
    c.must(
        "POST",
        "/api/me/sync",
        r#"{"since":1,"docs":[{"id":"t_x","type":"task","base":1,"updated":2,"body":{"id":"x","title":"Second"}}]}"#,
        200,
    )
    .await;
    let out = c.must("GET", "/api/me/history/t_x", "", 200).await;
    let versions = out["versions"].as_array().unwrap();
    assert!(
        versions.len() == 1 && versions[0]["rev"] == 1 && versions[0]["body"]["title"] == "First",
        "history: {out}"
    );
    // Restoring is an ordinary upload against the current revision.
    let restored = c
        .must(
            "POST",
            "/api/me/sync",
            r#"{"since":2,"docs":[{"id":"t_x","type":"task","base":2,"updated":3,"body":{"id":"x","title":"First"}}]}"#,
            200,
        )
        .await;
    assert_eq!(restored["applied"].as_array().unwrap().len(), 1, "{restored}");
    let out = c.must("GET", "/api/me/history/t_x", "", 200).await;
    assert_eq!(out["versions"].as_array().unwrap().len(), 2, "{out}");
    c.must("GET", "/api/me/history/bad%20id", "", 400).await;
    t.server.clock.shift(chrono::TimeDelta::days(40));
    t.server.sweep().await;
    let out = c.must("GET", "/api/me/history/t_x", "", 200).await;
    assert!(out["versions"].as_array().unwrap().is_empty(), "history retained past thirty days");
}

#[tokio::test]
async fn doc_owner_guards_shared_cookies() {
    let t = test_server().await;
    let s = &t.server;
    s.db.exec("INSERT INTO accounts(id,phone,created_at) VALUES('a','+12015550123',0)", &[]).await.unwrap();
    let now = s.now().timestamp();
    s.db.exec(
        "INSERT INTO sessions(id,account,token_hash,name,created_at,last_seen) VALUES('s','a',$1,'Test',$2,$3)",
        &[&hash_token("test-token"), &now, &now],
    )
    .await
    .unwrap();
    let mut c = Client::new(&t);
    c.cookie = "test-token".into();
    c.account = "b".into();
    c.must(
        "POST",
        "/api/me/sync",
        r#"{"since":0,"docs":[{"id":"t_x","type":"task","body":{"title":"Wrong account"}}]}"#,
        409,
    )
    .await;
    c.must("POST", "/api/me/sync", r#"{"since":0,"docs":[]}"#, 409).await;
    c.account = "a".into();
    c.must("POST", "/api/me/sync", r#"{"since":0,"docs":[]}"#, 200).await;
    assert_eq!(count(&t.server, "SELECT COUNT(*) FROM docs").await, 0, "mismatched account wrote documents");
}

fn doc(json: &str) -> Doc {
    serde_json::from_str(json).unwrap()
}

#[test]
fn document_validation() {
    assert!(valid_docs(&[doc(r#"{"id":"t_1","type":"task","body":{"id":"1"}}"#)]));
    // A null body is valid JSON.
    assert!(valid_docs(&[doc(r#"{"id":"t_1","type":"task","body":null}"#)]));
    // The body's id matches case-insensitively, the last one winning.
    assert!(!valid_docs(&[doc(r#"{"id":"t_1","type":"task","body":{"ID":"2"}}"#)]));
    assert!(valid_docs(&[doc(r#"{"id":"t_1","type":"task","body":{"id":"2","Id":"1"}}"#)]));
    assert!(valid_docs(&[doc(r#"{"id":"prefs","type":"prefs","body":{"id":"anything"}}"#)]));
    assert!(!valid_docs(&[doc(r#"{"id":"t_1","type":"task"}"#)]));
    assert!(valid_docs(&[doc(r#"{"id":"t_1","type":"task","deleted":true}"#)]));
    assert!(!valid_docs(&[doc(r#"{"id":"t_1","type":"task","body":[1]}"#)]));
    assert!(!valid_docs(&[doc(r#"{"id":"t_1","type":"task","body":{"id":1}}"#)]));
    assert!(!valid_docs(&[doc(r#"{"id":"x_1","type":"task","body":{}}"#)]));
}

#[test]
fn raw_bodies_encode_compact_and_escaped() {
    let raw = "{ \"a\" : [1, 2],\n \"b\": \"x <&> \\\" y\u{2028}\" }";
    let encoded = r#"{"a":[1,2],"b":"x \u003c\u0026\u003e \" y\u2028"}"#;
    assert_eq!(compact_json(raw), encoded);
    assert!(!is_compact_json(raw) && is_compact_json(encoded), "compact check");
    assert!(!is_compact_json("{\"a\":\"\u{2029}\"}"), "U+2029");
    assert!(!is_compact_json(r#"{"a":"\" <"}"#) && is_compact_json(r#"{"a":"\\"}"#), "escapes");
    assert_eq!(final_json(encoded.to_owned()), encoded);
    assert_eq!(final_json(raw.to_owned()), encoded);
}

#[tokio::test]
async fn bodies_are_stored_as_responses_encode_them() {
    let t = test_server().await;
    let mut c = signed_in_client(&t).await;
    let (_, _) = c
        .send(
            "POST",
            "/api/me/sync",
            r#"{"since":0,"docs":[{"id":"t_1","type":"task","body":{ "id" : "1", "title" : "<b> & \u2028" }}]}"#,
        )
        .await;
    let stored: String = t.server.db.fetch_one("SELECT body FROM docs", &[]).await.unwrap().get(0);
    assert_eq!(stored, r#"{"id":"1","title":"\u003cb\u003e \u0026 \u2028"}"#, "stored body");
    // A body an earlier release stored verbatim is converted as it is read.
    t.server.db.exec(r#"UPDATE docs SET body='{ "id": "1", "title": "<i>" }'"#, &[]).await.unwrap();
    let (res, _) = c.send("POST", "/api/me/sync", r#"{"since":0,"docs":[]}"#).await;
    assert!(
        res.text().contains(r#""body":{"id":"1","title":"\u003ci\u003e"}"#),
        "verbatim body not converted: {}",
        res.text()
    );
}
