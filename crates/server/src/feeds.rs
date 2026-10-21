//! Account calendar feeds. A feed URL is a secret sealed like a Canvas token
//! (scope `feed`); every device receives the feed's tasks through document
//! sync.

use std::sync::Arc;
use std::time::Duration;

use anyhow::anyhow;
use axum::body::Body;
use axum::extract::{Path, State};
use axum::http::HeaderValue;
use axum::http::header::CACHE_CONTROL;
use serde::{Deserialize, Serialize};

use crate::Server;
use crate::api::{ApiError, ApiResult, fail, null_default, ok, read_json};
use crate::canvas_account::{IMPORT_FRESHNESS, ImportError};
use crate::db::{self, Param, Sql};
use crate::ical::{load_location, parse_ics, recent_tasks};
use crate::lms::Task;
use crate::session::{Auth, new_id};

/// The most feeds one account may connect.
pub(crate) const MAX_ACCOUNT_FEEDS: usize = 20;

/// One account feed as clients see it; the sealed URL is never serialized.
#[derive(Clone, Debug, Default, Serialize)]
pub(crate) struct FeedConnection {
    pub(crate) id: String,
    pub(crate) label: String,
    pub(crate) tz: String,
    pub(crate) synced: i64,
    pub(crate) error: String,
    #[serde(skip)]
    pub(crate) credential: Vec<u8>,
}

impl FeedConnection {
    fn from_row(row: &db::Row) -> db::Result<Self> {
        Ok(Self {
            id: row.try_get(0)?,
            label: row.try_get(1)?,
            tz: row.try_get(2)?,
            synced: row.try_get(3)?,
            error: row.try_get(4)?,
            credential: row.try_get(5)?,
        })
    }
}

/// The error a lost import lock leaves.
fn lease_lost() -> ImportError {
    ImportError::Other(anyhow!("context canceled"))
}

impl Server {
    /// The account's feeds, oldest first.
    pub(crate) async fn account_feeds(&self, account: &str) -> db::Result<Vec<FeedConnection>> {
        self.db
            .fetch_all(
                "SELECT id,label,tz,synced,error,credential FROM feed_connections WHERE account=$1 ORDER BY created_at,id",
                &[&account],
            )
            .await?
            .iter()
            .map(FeedConnection::from_row)
            .collect()
    }

    /// One feed of the account; `Err(NoRows)` when it does not exist.
    pub(crate) async fn feed_connection(&self, account: &str, id: &str) -> db::Result<FeedConnection> {
        let row = self
            .db
            .fetch_one(
                "SELECT id,label,tz,synced,error,credential FROM feed_connections WHERE account=$1 AND id=$2",
                &[&account, &id],
            )
            .await?;
        FeedConnection::from_row(&row)
    }

    /// The campus time zone of the school in the account's preferences, or
    /// "UTC"; floating feed times are read in it.
    pub(crate) async fn account_timezone(&self, account: &str) -> String {
        let inst = self
            .db
            .fetch_opt(
                "SELECT COALESCE(json_text(body,'$.inst'),'') FROM docs WHERE account=$1 AND id='prefs' AND deleted=0",
                &[&account],
            )
            .await
            .ok()
            .flatten()
            .and_then(|row| row.try_get::<_, String>(0).ok())
            .unwrap_or_default();
        match self.inst(&inst) {
            Some(i) if !i.timezone.is_empty() => i.timezone.clone(),
            _ => "UTC".into(),
        }
    }

    /// Relays one feed (holding a provider slot, 30-second budget) and reads
    /// it into tasks due no more than two weeks ago. Fails with 400 for a
    /// bad URL and 502 for an unreachable feed or one that is not a
    /// calendar ("that url is not a calendar feed"). An unknown `tz` reads
    /// floating times in UTC.
    pub(crate) async fn fetch_feed_tasks(&self, raw: &str, label: &str, tz: &str) -> Result<Vec<Task>, ImportError> {
        let url = self.outbound.safe_url(raw).map_err(|e| ImportError::new(400, e))?;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
        let host = match url.host() {
            Some(url::Host::Ipv6(address)) => address.to_string(),
            Some(host) => host.to_string().to_lowercase(),
            None => String::new(),
        };
        let _slot = tokio::time::timeout_at(deadline, self.provider_slot(&host))
            .await
            .map_err(|_| ImportError::Other(anyhow!("context deadline exceeded")))??;
        let (body, _) = tokio::time::timeout_at(deadline, self.relay(&url, ""))
            .await
            .map_err(|_| ImportError::new(502, "context deadline exceeded"))?
            .map_err(|e| ImportError::new(502, e.to_string()))?;
        let head = &body[..body.len().min(200)];
        if !head.windows(15).any(|w| w == b"BEGIN:VCALENDAR") {
            return Err(ImportError::new(502, "that url is not a calendar feed"));
        }
        let zone = load_location(tz).unwrap_or(chrono_tz::Tz::UTC);
        Ok(recent_tasks(parse_ics(&String::from_utf8_lossy(&body), "feed", label, zone), self.now()))
    }

    /// Imports one feed unless a recent import covers it (or `force`). The
    /// refresh endpoint and the background queue share it.
    pub(crate) async fn refresh_feed_connection(
        &self,
        account: &str,
        id: &str,
        force: bool,
    ) -> Result<FeedConnection, ImportError> {
        let started = self.now().timestamp();
        let lock = self.lock_canvas(account).await.map_err(|_| ImportError::new(503, "the refresh was cancelled"))?;
        let lost = lock.lease.lost();
        let mut f =
            self.feed_connection(account, id).await.map_err(|_| ImportError::new(404, "calendar feed not found"))?;
        if f.synced >= started || (!force && started - f.synced < IMPORT_FRESHNESS.as_secs() as i64) {
            return Ok(f);
        }
        self.db
            .exec("UPDATE feed_connections SET attempted=$1 WHERE account=$2 AND id=$3", &[&started, &account, &id])
            .await
            .map_err(|_| ImportError::new(500, "could not record the refresh"))?;
        let raw = self
            .open(account, id, "feed", &f.credential)
            .map_err(|_| ImportError::new(503, "the feed address is unavailable; add the calendar again"))?;
        let fetched = tokio::select! {
            fetched = self.fetch_feed_tasks(&raw, &f.label, &f.tz) => fetched,
            () = lost.cancelled() => Err(lease_lost()),
        };
        let tasks = match fetched {
            Ok(tasks) => tasks,
            Err(err) => {
                self.exec_logged(
                    "UPDATE feed_connections SET error='The calendar could not be refreshed.' WHERE account=$1 AND id=$2",
                    &[&account, &id],
                )
                .await;
                self.count("feed_failures", 1);
                return Err(err);
            }
        };
        f.synced = self.now().timestamp();
        f.error.clear();
        let persist: &[Param] = &[&f.synced, &account, &id];
        let published = tokio::select! {
            published = self.publish_import(
                account,
                "feed",
                id,
                &tasks,
                Some(("UPDATE feed_connections SET synced=$1,error='' WHERE account=$2 AND id=$3", persist)),
            ) => published,
            () = lost.cancelled() => Err(anyhow!("context canceled")),
        };
        if published.is_err() {
            return Err(ImportError::new(500, "could not save the calendar's tasks"));
        }
        self.count("feed_imports", 1);
        drop(lock);
        Ok(f)
    }

    /// The account's feeds as `GET /api/me/feeds` reports them.
    async fn write_feeds(&self, account: &str) -> ApiResult {
        let Ok(feeds) = self.account_feeds(account).await else {
            return fail(500, "could not read calendar feeds");
        };
        let mut response = ok(&serde_json::json!({ "feeds": feeds, "available": self.integrations_available() }))?;
        response.headers_mut().insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
        Ok(response)
    }
}

/// `GET /api/me/feeds`: `{"feeds": [...], "available": bool}`.
pub(crate) async fn handle_feeds(State(s): State<Arc<Server>>, Auth(se): Auth) -> ApiResult {
    s.write_feeds(&se.account).await
}

#[derive(Default, Deserialize)]
#[serde(default)]
struct NewFeed {
    #[serde(alias = "URL", alias = "Url", deserialize_with = "null_default")]
    url: String,
    #[serde(alias = "Label", deserialize_with = "null_default")]
    label: String,
}

/// `PUT /api/me/feeds`: adds a feed (`url`, `label`; 4 KiB) and imports it.
/// Labels are cut to 120 bytes.
pub(crate) async fn handle_add_feed(State(s): State<Arc<Server>>, Auth(se): Auth, body: Body) -> ApiResult {
    let Some(input) = read_json::<NewFeed>(body, 4 << 10).await else { return fail(400, "bad json") };
    let url = input.url.trim().to_owned();
    let mut label = input.label.trim().to_owned();
    if label.len() > 120 {
        let mut cut = 120;
        while !label.is_char_boundary(cut) {
            cut -= 1;
        }
        label.truncate(cut);
    }
    if !s.integrations_available() {
        return fail(503, "Account calendar feeds are not configured on this server");
    }
    if let Err(err) = s.outbound.safe_url(&url) {
        return fail(400, err);
    }
    let Ok(lock) = s.lock_canvas(&se.account).await else { return fail(503, "the request was cancelled") };
    let lost = lock.lease.lost();
    let Ok(feeds) = s.account_feeds(&se.account).await else { return fail(500, "could not read calendar feeds") };
    if feeds.len() >= MAX_ACCOUNT_FEEDS {
        return fail(400, "An account can connect up to 20 calendar feeds");
    }
    if feeds.iter().any(|f| s.open(&se.account, &f.id, "feed", &f.credential).is_ok_and(|existing| existing == url)) {
        return fail(409, "That calendar is already connected");
    }
    let tz = s.account_timezone(&se.account).await;
    let fetched = tokio::select! {
        fetched = s.fetch_feed_tasks(&url, &label, &tz) => fetched,
        () = lost.cancelled() => Err(lease_lost()),
    };
    let tasks = fetched.map_err(ApiError::from)?;
    let id = new_id();
    let Ok(credential) = s.seal(&se.account, &id, "feed", &url) else {
        return fail(500, "could not protect the feed address");
    };
    let now = s.now().timestamp();
    let persist: &[Param] = &[&se.account, &id, &credential, &label, &tz, &now, &now, &now];
    let published = tokio::select! {
        published = s.publish_import(
            &se.account,
            "feed",
            &id,
            &tasks,
            Some((
                "INSERT INTO feed_connections(account,id,credential,label,tz,synced,attempted,error,created_at) VALUES($1,$2,$3,$4,$5,$6,$7,'',$8)",
                persist,
            )),
        ) => published,
        () = lost.cancelled() => Err(anyhow!("context canceled")),
    };
    if published.is_err() {
        return fail(500, "could not save the calendar feed");
    }
    let response = s.write_feeds(&se.account).await;
    drop(lock);
    response
}

/// `DELETE /api/me/feeds/{id}`: removes the feed and the tasks it imported.
pub(crate) async fn handle_delete_feed(
    State(s): State<Arc<Server>>,
    Auth(se): Auth,
    Path(id): Path<String>,
) -> ApiResult {
    let Ok(lock) = s.lock_canvas(&se.account).await else { return fail(503, "the request was cancelled") };
    let Ok(removed) = s.db.exec("DELETE FROM feed_connections WHERE account=$1 AND id=$2", &[&se.account, &id]).await
    else {
        return fail(500, "could not remove the calendar feed");
    };
    if removed > 0 && s.delete_import_tasks(&se.account, "feed", &id).await.is_err() {
        return fail(500, "could not remove the feed's tasks");
    }
    let response = s.write_feeds(&se.account).await;
    drop(lock);
    response
}

#[derive(Default, Deserialize)]
#[serde(default)]
struct Refresh {
    #[serde(alias = "Force", deserialize_with = "null_default")]
    force: bool,
}

/// `POST /api/me/feeds/refresh`: imports every feed (`force`; 1 KiB). The
/// first failure is reported after every feed has been tried.
pub(crate) async fn handle_refresh_feeds(State(s): State<Arc<Server>>, Auth(se): Auth, body: Body) -> ApiResult {
    let Some(input) = read_json::<Refresh>(body, 1024).await else { return fail(400, "invalid refresh") };
    let Ok(feeds) = s.account_feeds(&se.account).await else { return fail(500, "could not read calendar feeds") };
    let mut failure = None;
    for f in &feeds {
        if let Err(err) = s.refresh_feed_connection(&se.account, &f.id, input.force).await {
            failure.get_or_insert(err);
        }
    }
    if let Some(err) = failure {
        return Err(err.into());
    }
    s.write_feeds(&se.account).await
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use axum::Router;
    use axum::routing::get;
    use chrono::TimeDelta;
    use serde_json::Value;
    use tokio_util::sync::CancellationToken;

    use super::*;
    use crate::ical::tests::FIXTURE_ICS;
    use crate::test_support::{Signed, signed_in_browser};
    use crate::test_support::{TEST_CANVAS_KEY, TestServer, test_server_with};

    /// A test server with the credential key, a signed-in client, and an
    /// upstream serving the fixture at `/feed.ics` that counts reads.
    async fn feed_fixture() -> (TestServer, Signed, String, Arc<AtomicUsize>) {
        let reads = Arc::new(AtomicUsize::new(0));
        let counter = reads.clone();
        let upstream = Router::new().route(
            "/feed.ics",
            get(move || {
                let counter = counter.clone();
                async move {
                    counter.fetch_add(1, Ordering::SeqCst);
                    ([("content-type", "text/calendar")], FIXTURE_ICS)
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, upstream).await });
        let t = test_server_with(|cfg| cfg.canvas_key = TEST_CANVAS_KEY.into()).await;
        t.server.outbound.set_allow_local(true);
        let c = signed_in_browser(&t).await;
        (t, c, url, reads)
    }

    async fn refresh_queued_imports(s: &Arc<Server>) {
        s.queue_stale_imports().await.unwrap();
        s.drain_work(&CancellationToken::new(), &["canvas", "feed"], 300, |job| s.import_work(job)).await;
    }

    #[tokio::test]
    async fn account_feeds_import_refresh_and_remove() {
        let (t, c, up, reads) = feed_fixture().await;
        let s = t.server.clone();
        c.must("PUT", "/api/me/feeds", &format!(r#"{{"url":"{up}/nope","label":"x"}}"#), 502).await;
        let out =
            c.must("PUT", "/api/me/feeds", &format!(r#"{{"url":"{up}/feed.ics","label":"ENGL 1213"}}"#), 200).await;
        let feeds = out["feeds"].as_array().unwrap();
        assert!(feeds.len() == 1 && feeds[0]["label"] == "ENGL 1213", "feeds: {out}");
        assert!(!out.to_string().contains("feed.ics"), "feed address returned to the client");
        c.must("PUT", "/api/me/feeds", &format!(r#"{{"url":"{up}/feed.ics","label":"Again"}}"#), 409).await;
        let id = feeds[0]["id"].as_str().unwrap().to_owned();
        let pulled = c.must("POST", "/api/me/sync", r#"{"since":0,"docs":[]}"#, 200).await;
        let docs = pulled["docs"].as_array().unwrap();
        assert_eq!(docs.len(), 2, "expected two imported tasks: {pulled}");
        let body = &docs[0]["body"];
        assert!(
            body["source"] == "feed"
                && body["canvasConnection"] == id.as_str()
                && body["uid"].as_str().unwrap().starts_with("feed:"),
            "task: {body}"
        );
        // Within the freshness window a refresh is free; forced, it reads the feed again.
        c.must("POST", "/api/me/feeds/refresh", r#"{"force":false}"#, 200).await;
        assert_eq!(reads.load(Ordering::SeqCst), 1, "fresh feed re-read");
        // A forced refresh in the same second still shares the import just made.
        let later = s.now() + TimeDelta::seconds(2);
        s.clock.set(later);
        c.must("POST", "/api/me/feeds/refresh", r#"{"force":true}"#, 200).await;
        assert_eq!(reads.load(Ordering::SeqCst), 2, "forced refresh skipped");
        // The background loop refreshes a stale feed while a device is active.
        s.db.exec("UPDATE feed_connections SET synced=0,attempted=0", &[]).await.unwrap();
        refresh_queued_imports(&s).await;
        assert_eq!(reads.load(Ordering::SeqCst), 3, "background refresh skipped");
        s.db.exec("UPDATE feed_connections SET synced=0,attempted=0", &[]).await.unwrap();
        s.db.exec("UPDATE sessions SET last_seen=$1", &[&(later - TimeDelta::days(40)).timestamp()]).await.unwrap();
        refresh_queued_imports(&s).await;
        assert_eq!(reads.load(Ordering::SeqCst), 3, "refreshed an account with no active device");
        s.db.exec("UPDATE sessions SET last_seen=$1", &[&later.timestamp()]).await.unwrap();
        let out = c.must("DELETE", &format!("/api/me/feeds/{id}"), "", 200).await;
        assert!(out["feeds"].as_array().unwrap().is_empty(), "{out}");
        let since = pulled["seq"].as_f64().unwrap();
        let pulled = c.must("POST", "/api/me/sync", &format!(r#"{{"since":{since:.0},"docs":[]}}"#), 200).await;
        let docs = pulled["docs"].as_array().unwrap();
        for d in docs {
            assert_eq!(d["deleted"], Value::Bool(true), "feed task survived removal: {d}");
        }
        assert_eq!(docs.len(), 2, "expected two tombstones");
    }
}
