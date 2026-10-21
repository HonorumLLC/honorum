//! Sign-in, sessions, device push registration, and SMS consent records.

use std::sync::{Arc, Mutex};

use crate::auth::SMS_CONSENT_VERSION;
use crate::db::Sql;
use crate::phone::PhoneVerifier;
use crate::push::{Alert, SendOutcome};
use crate::test_support::{Client, TEST_PHONE, TestServer, count, fake_verifier, signed_in_client, test_server};

/// Installs a push sender that records each target and reports `gone`.
fn record_sends(t: &TestServer, gone: bool) -> Arc<Mutex<Vec<String>>> {
    let sent = Arc::new(Mutex::new(Vec::new()));
    let log = sent.clone();
    t.server.set_sender(Some(Arc::new(move |target: &str, _: &Alert| -> SendOutcome {
        log.lock().unwrap().push(target.to_owned());
        (gone, Ok(()))
    })));
    sent
}

async fn tick_test_reminders(t: &TestServer) {
    let stop = tokio_util::sync::CancellationToken::new();
    t.server.tick_scheduled_reminders(&stop).await;
    t.server.tick_scheduled_reminders(&stop).await;
}

#[tokio::test]
async fn accounts_and_sync() {
    let t = test_server().await;
    let s = &t.server;
    let v = fake_verifier(&t);
    let mut a = Client::new(&t);
    a.must("POST", "/api/auth/code", r#"{"consent":"2026-09-08-v2","phone":"+12015550123"}"#, 202).await;
    assert_eq!(v.sent(), vec![TEST_PHONE.to_owned()], "SMS was not requested");
    a.must("POST", "/api/auth/verify", r#"{"phone":"+12015550123","code":"999999"}"#, 400).await;
    let out = a
        .must(
            "POST",
            "/api/auth/verify",
            r#"{"phone":"+12015550123","code":"123456","device":"Firefox on Linux"}"#,
            200,
        )
        .await;
    assert!(
        out["phone"] == TEST_PHONE && out["id"] != "" && !a.cookie.is_empty() && out["token"].is_string(),
        "verify: {out} cookie={}",
        a.cookie
    );
    a.must("POST", "/api/auth/verify", r#"{"phone":"+12015550123","code":"123456"}"#, 400).await;
    let mut b = Client::new(&t);
    b.bearer = out["token"].as_str().unwrap().to_owned();
    let me = b.must("GET", "/api/me", "", 200).await;
    assert_eq!(me["id"], a.account.as_str(), "bearer: {me}");
    let mut c = signed_in_client(&t).await;
    assert_eq!(c.account, a.account, "same number created two accounts");
    let me = a.must("GET", "/api/me", "", 200).await;
    assert_eq!(me["devices"].as_array().unwrap().len(), 2, "devices: {me}");
    // Sync: device A pushes, device C pulls, both edit the same doc.
    let out = a.must("POST", "/api/me/sync", r#"{"since":0,"docs":[{"id":"s_1","type":"schedule","base":0,"updated":10,"body":{"name":"One"}},{"id":"prefs","type":"prefs","base":0,"updated":10,"body":{"inst":"demo"}}]}"#, 200).await;
    assert!(
        out["seq"] == 2
            && out["applied"].as_array().unwrap().len() == 2
            && out["conflicts"].as_array().unwrap().is_empty(),
        "push: {out}"
    );
    let out = c.must("POST", "/api/me/sync", r#"{"since":0,"docs":[]}"#, 200).await;
    let docs = out["docs"].as_array().unwrap();
    assert!(out["seq"] == 2 && docs.len() == 2 && docs[0]["id"] == "s_1" && docs[0]["rev"] == 1, "pull: {out}");
    let out = c.must("POST", "/api/me/sync", r#"{"since":2,"docs":[{"id":"s_1","type":"schedule","base":1,"updated":20,"body":{"name":"One (phone)"}}]}"#, 200).await;
    assert_eq!(out["seq"], 3, "second push: {out}");
    let out = a.must("POST", "/api/me/sync", r#"{"since":2,"docs":[{"id":"s_1","type":"schedule","base":1,"updated":15,"body":{"name":"One (laptop)"}}]}"#, 200).await;
    let conf = out["conflicts"].as_array().unwrap();
    assert!(
        conf.len() == 1 && conf[0]["rev"] == 3 && conf[0]["body"]["name"] == "One (phone)",
        "expected a conflict carrying the phone's copy: {out}"
    );
    a.must(
        "POST",
        "/api/me/sync",
        r#"{"since":3,"docs":[{"id":"s_1","type":"schedule","base":3,"deleted":true,"updated":30}]}"#,
        200,
    )
    .await;
    let out = c.must("POST", "/api/me/sync", r#"{"since":3,"docs":[]}"#, 200).await;
    let docs = out["docs"].as_array().unwrap();
    assert!(docs.len() == 1 && docs[0]["deleted"] == true, "tombstone: {out}");
    a.must(
        "POST",
        "/api/me/sync",
        r#"{"since":0,"docs":[{"id":"bad id","type":"schedule","base":0,"updated":1,"body":{}}]}"#,
        400,
    )
    .await;
    a.must(
        "POST",
        "/api/me/sync",
        r#"{"since":0,"docs":[{"id":"s_9","type":"task","base":0,"updated":1,"body":{}}]}"#,
        400,
    )
    .await;
    a.must(
        "POST",
        "/api/me/sync",
        r#"{"since":0,"docs":[{"id":"s_9","type":"schedule","base":0,"updated":1,"body":{"id":"other"}}]}"#,
        400,
    )
    .await;

    // Alerts fan out to every device with a push subscription.
    s.sync_catalog(&s.insts[0], None, true).await.unwrap();
    a.must(
        "PUT",
        "/api/me/push",
        r#"{"subscription":"{\"endpoint\":\"https://push.example/a\",\"keys\":{\"p256dh\":\"a\",\"auth\":\"b\"}}"}"#,
        204,
    )
    .await;
    let out = a.must("POST", "/api/watches", r#"{"inst":"demo","term":"2026FA","section":"10001"}"#, 201).await;
    let wid = out["id"].as_str().unwrap().to_owned();
    let out2 = c.must("POST", "/api/watches", r#"{"inst":"demo","term":"2026FA","section":"10001"}"#, 200).await;
    assert_eq!(out2["id"], wid.as_str(), "watching twice should reuse the alert: {out2}");
    let (rec, _) = c.send("GET", "/api/me/watches", "").await;
    assert!(rec.text().contains(r#""label":"CS 101-001""#), "watch list: {}", rec.text());
    let sent = record_sends(&t, false);
    t.fake.set_seats("10001", 0);
    s.poll(&s.insts[0]).await.unwrap();
    s.dispatch_pending().await;
    t.fake.set_seats("10001", 3);
    s.poll(&s.insts[0]).await.unwrap();
    s.dispatch_pending().await;
    {
        let sent = sent.lock().unwrap();
        assert!(sent.len() == 1 && sent[0].contains("push.example/a"), "expected one push leg, got {sent:?}");
    }
    // A dead push endpoint is forgotten.
    let sent = record_sends(&t, true);
    t.fake.set_seats("10001", 0);
    s.poll(&s.insts[0]).await.unwrap();
    s.dispatch_pending().await;
    t.fake.set_seats("10001", 1);
    s.db.exec("UPDATE watches SET last_alert=0", &[]).await.unwrap(); // skip the debounce
    s.poll(&s.insts[0]).await.unwrap();
    s.dispatch_pending().await;
    {
        let sent = sent.lock().unwrap();
        assert!(sent.len() == 1 && sent[0].contains("push.example/a"), "expected the push leg once more, got {sent:?}");
    }
    let me = a.must("GET", "/api/me", "", 200).await;
    assert_eq!(me["devices"][0]["push"], false, "dead push subscription should be cleared: {me}");
    // Another account cannot remove the alert; the owner can.
    let (rec, _) = Client::new(&t).send("DELETE", &format!("/api/watches/{wid}"), "").await;
    assert_eq!(rec.status.as_u16(), 403, "stranger deleted the alert");
    c.must("DELETE", &format!("/api/watches/{wid}"), "", 204).await;

    // Reminders derive from the account's documents and reach every device.
    a.must(
        "PUT",
        "/api/me/push",
        r#"{"subscription":"{\"endpoint\":\"https://push.example/a2\",\"keys\":{\"p256dh\":\"a\",\"auth\":\"b\"}}"}"#,
        204,
    )
    .await;
    c.must(
        "PUT",
        "/api/me/push",
        r#"{"subscription":"{\"endpoint\":\"https://push.example/c\",\"keys\":{\"p256dh\":\"a\",\"auth\":\"b\"}}"}"#,
        204,
    )
    .await;
    a.must("POST", "/api/me/sync", r#"{"since":4,"docs":[{"id":"prefs","type":"prefs","base":2,"updated":40,"body":{"inst":"demo","term":"2026FA","current":"2","reminders":true}},{"id":"s_2","type":"schedule","base":0,"updated":40,"body":{"id":"2","inst":"demo","term":"2026FA","primary":true,"sections":["10001"],"pinned":{"10001":{"id":"10001","course":"CS 101","num":"001","type":"LEC","start":"2026-08-01","end":"2026-12-15","meetings":[{"days":"MWF","start":540,"end":590,"location":"Felgar 300"}]}},"blocks":[]}}]}"#, 200).await;
    // The test school's zone is UTC, so 8:45 UTC is a quarter hour before
    // the 9:00 class.
    s.clock.set(chrono::DateTime::parse_from_rfc3339("2026-08-24T08:45:00Z").unwrap().to_utc());
    let sent = record_sends(&t, false);
    tick_test_reminders(&t).await;
    s.dispatch_pending().await;
    {
        let sent = sent.lock().unwrap();
        assert!(
            sent.len() == 2 && sent[0].contains("push.example/a2") && sent[1].contains("push.example/c"),
            "expected a push per device, got {sent:?}"
        );
    }

    // Remove the phone, sign the laptop out, delete the account.
    let phone = c.must("GET", "/api/me", "", 200).await["device"].as_str().unwrap().to_owned();
    a.must("DELETE", &format!("/api/me/devices/{phone}"), "", 204).await;
    let (rec, _) = c.send("GET", "/api/me", "").await;
    assert_eq!(rec.status.as_u16(), 401, "revoked device still signed in");
    a.must("POST", "/api/auth/logout", "", 204).await;
    let (rec, _) = b.send("GET", "/api/me", "").await;
    assert_eq!(rec.status.as_u16(), 401, "signed-out token still works");
    s.clock.reset();
    let mut a = signed_in_client(&t).await;
    a.must("DELETE", "/api/me", "", 204).await;
    let (n, d) =
        (count(&t.server, "SELECT COUNT(*) FROM accounts").await, count(&t.server, "SELECT COUNT(*) FROM docs").await);
    assert!(n == 0 && d == 0, "account not fully deleted: accounts={n} docs={d}");
}

#[tokio::test]
async fn sign_in_limits_and_configuration() {
    let t = test_server().await;
    t.server.set_verifier(None);
    let mut a = Client::new(&t);
    a.must("POST", "/api/auth/code", r#"{"consent":"2026-09-08-v2","phone":"+12015550123"}"#, 503).await;
    a.must("POST", "/api/auth/verify", r#"{"phone":"+12015550123","code":"123456"}"#, 503).await;
    let v = fake_verifier(&t);
    for phone in ["nope", "2015550123", "+02015550123", "+12015550123 ext 1", "+1", "+1234567890123456"] {
        a.must("POST", "/api/auth/code", &format!(r#"{{"consent":"2026-09-08-v2","phone":"{phone}"}}"#), 400).await;
    }
    assert!(v.sent().is_empty(), "invalid numbers reached provider");
    a.must("POST", "/api/auth/code", r#"{"consent":"2026-09-08-v2","phone":"+12015550123"}"#, 202).await;
    let (rec, _) = a.send("POST", "/api/auth/code", r#"{"consent":"2026-09-08-v2","phone":"+12015550123"}"#).await;
    assert!(rec.status == 429 && rec.header("retry-after") == "30" && v.sent().len() == 1, "resend was not throttled");
    // International numbering plans can be shorter than eight digits.
    a.must("POST", "/api/auth/code", r#"{"consent":"2026-09-08-v2","phone":"+6831234"}"#, 202).await;
    // More than ten students can request a code through the same campus IP.
    for n in 124..=134 {
        a.must("POST", "/api/auth/code", &format!(r#"{{"consent":"2026-09-08-v2","phone":"+12015550{n}"}}"#), 202)
            .await;
    }
    a.must("POST", "/api/auth/verify", r#"{"phone":"+12015550123","code":"invalid"}"#, 400).await;
    assert_eq!(v.checks(), 0, "malformed code reached provider");
}

#[tokio::test]
async fn signed_in_writes_count_against_the_account() {
    let t = test_server().await;
    let v = fake_verifier(&t);
    let sign_in = async |phone: &str| {
        v.send(phone).await.unwrap();
        let mut c = Client::new(&t);
        c.must("POST", "/api/auth/verify", &format!(r#"{{"phone":"{phone}","code":"123456"}}"#), 200).await;
        c
    };
    // Two students behind one campus address together exceed the address's budget.
    let mut a = sign_in("+12015550123").await;
    let mut b = sign_in("+12015550124").await;
    for _ in 0..100 {
        a.must("POST", "/api/me/activity", r#"{"inst":"demo"}"#, 200).await;
        b.must("POST", "/api/me/activity", r#"{"inst":"demo"}"#, 200).await;
    }
    for _ in 0..20 {
        a.send("POST", "/api/me/activity", r#"{"inst":"demo"}"#).await;
    }
    a.must("POST", "/api/me/activity", r#"{"inst":"demo"}"#, 429).await;
    // The accounts' traffic left the address's own budget for guests.
    Client::new(&t).must("POST", "/api/watches", "{}", 400).await;
}

#[tokio::test]
async fn sms_requires_recorded_consent_before_sending() {
    let t = test_server().await;
    let calls = Arc::new(Mutex::new(Vec::<String>::new()));
    let db = t.server.db.clone();
    let server = t.server.clone();
    let log = calls.clone();
    let fake = crate::phone_tests::fake_twilio(move |_req| {
        let (db, server, log) = (db.clone(), server.clone(), log.clone());
        async move {
            let row = db.fetch_opt("SELECT phone,consented_at,version FROM sms_consents", &[]).await.unwrap();
            let problem = match row {
                None => "provider contacted before consent was saved".to_owned(),
                Some(row) => {
                    let (phone, at, version): (String, i64, String) = (row.get(0), row.get(1), row.get(2));
                    if phone != TEST_PHONE || version != SMS_CONSENT_VERSION || at != server.now().timestamp() {
                        format!("incorrect consent record: {phone:?} {version:?} {at}")
                    } else {
                        String::new()
                    }
                }
            };
            log.lock().unwrap().push(problem);
            (201, r#"{"status":"pending","to":"+12015550123"}"#.to_owned())
        }
    })
    .await;
    let cfg = crate::config::Twilio { service_sid: "VAtest".into(), ..Default::default() };
    t.server.set_verifier(Some(Arc::new(crate::phone::TwilioVerify::new(cfg, &fake))));
    let mut c = Client::new(&t);
    for body in [
        r#"{"phone":"+12015550123"}"#,
        r#"{"phone":"+12015550123","consent":"old"}"#,
        r#"{"phone":"+12015550123","consent":true}"#,
    ] {
        c.must("POST", "/api/auth/code", body, 400).await;
    }
    let stored = count(&t.server, "SELECT COUNT(*) FROM sms_consents").await;
    assert!(stored == 0 && calls.lock().unwrap().is_empty(), "unconfirmed request reached storage or provider");
    c.must("POST", "/api/auth/code", r#"{"phone":"+12015550123","consent":"2026-09-08-v2"}"#, 202).await;
    let calls = calls.lock().unwrap();
    assert_eq!(calls.len(), 1, "confirmed request was not sent");
    assert_eq!(calls[0], "", "{}", calls[0]);
}

#[tokio::test]
async fn sms_consent_write_failure_prevents_send() {
    let t = test_server().await;
    let v = fake_verifier(&t);
    t.server.db.client().await.unwrap().batch_execute("CREATE FUNCTION reject_consent() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RAISE EXCEPTION 'disk unavailable'; END $$; CREATE TRIGGER reject_consent BEFORE INSERT ON sms_consents FOR EACH ROW EXECUTE FUNCTION reject_consent()").await.unwrap();
    let mut c = Client::new(&t);
    c.must("POST", "/api/auth/code", r#"{"phone":"+12015550123","consent":"2026-09-08-v2"}"#, 503).await;
    assert!(v.sent().is_empty(), "SMS sent without durable consent");
}

#[tokio::test]
async fn sms_consent_retention() {
    let t = test_server().await;
    let s = &t.server;
    let now = s.now().timestamp();
    s.db.exec(
        "INSERT INTO sms_consents(id,phone,consented_at,version) VALUES('expired',$1,$2,$3),('recent',$4,$5,$6)",
        &[&TEST_PHONE, &(now - 366 * 86400), &SMS_CONSENT_VERSION, &TEST_PHONE, &now, &SMS_CONSENT_VERSION],
    )
    .await
    .unwrap();
    s.sweep().await;
    let rows = s.db.fetch_all("SELECT id FROM sms_consents", &[]).await.unwrap();
    assert!(rows.len() == 1 && rows[0].get::<_, String>(0) == "recent", "incorrect consent retention");
}
