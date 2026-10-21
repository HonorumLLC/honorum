//! Showing Google events: the chosen calendars' events from two weeks back
//! to four months ahead are copied into the server, where every device of
//! the account reads them. Google expands repeating events itself. A
//! calendar is fetched again when Google reports a change (push channels to
//! `/api/google/notify`) or, behind that, when a poll comes due. Also the
//! Google worker loop.

use std::sync::Arc;
use std::time::Duration;

use axum::extract::State;
use axum::http::{HeaderMap, HeaderValue, header};
use chrono::TimeDelta;
use reqwest::Method;
use serde::{Deserialize, Serialize};
use serde_json::json;
use subtle::ConstantTimeEq;
use tokio_util::sync::CancellationToken;

use super::{
    GOOGLE_SCOPE_EVENTS, GOOGLE_SCOPE_LIST, GOOGLE_STATE_TTL, GoogleConnection, GoogleError, calendar_path,
    random_token,
};
use crate::Server;
use crate::api::{ApiResult, empty, escaped_json, fail, ok};
use crate::canvas_account::IMPORT_DEVICE;
use crate::db::{Param, Sql};
use crate::session::{Auth, new_id};

/// How often the calendar list is read again.
pub(crate) const GOOGLE_LIST_EVERY: Duration = Duration::from_secs(12 * 3600);
/// The poll interval for calendars with a change channel.
pub(crate) const GOOGLE_POLL_WATCHED: Duration = Duration::from_secs(6 * 3600);
/// The poll interval without one, as on a local server.
pub(crate) const GOOGLE_POLL_PLAIN: Duration = Duration::from_secs(15 * 60);
/// The lifetime requested for a change channel.
pub(crate) const GOOGLE_CHANNEL_TTL: Duration = Duration::from_secs(7 * 24 * 3600);
/// The most events kept per calendar.
pub(crate) const GOOGLE_SHOWN_LIMIT: usize = 2000;

/// One event as devices draw it. `start` and `end` are RFC 3339, or dates
/// when the event lasts all day. `calendar` is filled in when served.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct GoogleShown {
    pub(crate) id: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub(crate) calendar: String,
    pub(crate) title: String,
    pub(crate) start: String,
    pub(crate) end: String,
    #[serde(rename = "allDay", skip_serializing_if = "std::ops::Not::not")]
    pub(crate) all_day: bool,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub(crate) location: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub(crate) link: String,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub(crate) free: bool,
}

/// Trims `s` and cuts it to at most `n` bytes on a character boundary.
pub(crate) fn clip(s: &str, n: usize) -> &str {
    let s = s.trim();
    if s.len() <= n {
        return s;
    }
    let mut cut = n;
    while cut > 0 && !s.is_char_boundary(cut) {
        cut -= 1;
    }
    &s[..cut]
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct CalendarEntry {
    id: String,
    summary: String,
    #[serde(rename = "summaryOverride")]
    summary_override: String,
    #[serde(rename = "backgroundColor")]
    background_color: String,
    primary: bool,
    deleted: bool,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct CalendarPage {
    #[serde(deserialize_with = "crate::api::null_default")]
    items: Vec<CalendarEntry>,
    #[serde(rename = "nextPageToken")]
    next_page_token: String,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct When {
    #[serde(rename = "dateTime")]
    date_time: String,
    date: String,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Attendee {
    #[serde(rename = "self")]
    is_self: bool,
    #[serde(rename = "responseStatus")]
    response_status: String,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct EventItem {
    id: String,
    status: String,
    summary: String,
    location: String,
    transparency: String,
    #[serde(rename = "htmlLink")]
    html_link: String,
    start: When,
    end: When,
    #[serde(deserialize_with = "crate::api::null_default")]
    attendees: Vec<Attendee>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct EventPage {
    #[serde(deserialize_with = "crate::api::null_default")]
    items: Vec<EventItem>,
    #[serde(rename = "nextPageToken")]
    next_page_token: String,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct WatchReply {
    #[serde(rename = "resourceId")]
    resource_id: String,
    expiration: String,
}

/// One calendar in `GET /api/me/google/events`.
#[derive(Serialize)]
struct CalendarOut {
    id: String,
    name: String,
    color: String,
    synced: i64,
}

impl Server {
    /// Where Google sends change notifications: `BASE/api/google/notify`,
    /// or empty when the base URL is not HTTPS. Google calls only HTTPS
    /// addresses, so a local server polls instead.
    pub(crate) fn google_webhook(&self) -> String {
        if !self.cfg.base_url.starts_with("https://") {
            return String::new();
        }
        format!("{}/api/google/notify", self.cfg.base_url.trim_end_matches('/'))
    }

    /// Runs [`Server::google_tick`] when Google is available, then waits for
    /// a wakeup or 30 seconds, until `stop` fires. Keeps connected calendars
    /// current: publishing after the account's documents change and once a
    /// day, and fetching shown calendars when Google reports a change or a
    /// poll comes due.
    pub(crate) async fn run_google(self: &Arc<Self>, stop: CancellationToken) {
        loop {
            if self.google_available() {
                self.google_tick(&stop).await;
            }
            tokio::select! {
                () = stop.cancelled() => return,
                () = self.google.wake.notified() => {}
                () = tokio::time::sleep(Duration::from_secs(30)) => {}
            }
        }
    }

    /// One pass: publishes up to 20 accounts that changed or are due,
    /// lists, hides, fetches, and watches shown calendars, and deletes
    /// expired consent states. Returns early when `stop` fires.
    pub(crate) async fn google_tick(&self, stop: &CancellationToken) {
        let now = self.now().timestamp();
        let active = now - IMPORT_DEVICE.as_secs() as i64;
        let accounts = self
            .query_strings(
                "SELECT g.account FROM google_connections g JOIN accounts a ON a.id=g.account WHERE g.error='' AND g.publish_retry<=$1 AND ((g.publish=1 AND (g.published_seq<>a.seq OR (g.published_at<$2 AND EXISTS(SELECT 1 FROM sessions s WHERE s.account=g.account AND s.last_seen>$3)))) OR (g.publish=0 AND g.calendar<>'')) LIMIT 20",
                &[&now, &(now - 86400), &active],
            )
            .await;
        let accounts = match accounts {
            Ok(accounts) => accounts,
            Err(err) => {
                tracing::warn!(%err, "Google Calendar queue");
                return;
            }
        };
        for account in accounts {
            tokio::select! {
                () = stop.cancelled() => return,
                result = self.publish_google(&account) => {
                    if let Err(err) = result {
                        tracing::warn!(%err, "Google Calendar publish failed");
                    }
                }
            }
        }
        tokio::select! {
            () = stop.cancelled() => return,
            () = self.show_google(stop) => {}
        }
        self.exec_logged(
            "DELETE FROM google_states WHERE created_at<$1",
            &[&(now - GOOGLE_STATE_TTL.as_secs() as i64)],
        )
        .await;
    }

    /// Runs a query returning `(account, calendar)` pairs; empty on failure.
    async fn google_pairs(&self, query: &str, params: &[Param<'_>]) -> Vec<(String, String)> {
        match self.db.fetch_all(query, params).await {
            Ok(rows) => rows.iter().filter_map(|r| Some((r.try_get(0).ok()?, r.try_get(1).ok()?))).collect(),
            Err(err) => {
                tracing::warn!(%err, "Google Calendar queue");
                Vec::new()
            }
        }
    }

    /// Lists calendars, clears ones no longer shown, fetches the chosen
    /// ones that changed or came due, and renews change notifications a day
    /// before they lapse.
    async fn show_google(&self, stop: &CancellationToken) {
        let now = self.now().timestamp();
        let accounts = self
            .query_strings(
                "SELECT account FROM google_connections WHERE show=1 AND error='' AND listed_at<$1 LIMIT 20",
                &[&(now - GOOGLE_LIST_EVERY.as_secs() as i64)],
            )
            .await
            .unwrap_or_default();
        for account in accounts {
            if stop.is_cancelled() {
                return;
            }
            self.list_google_calendars(&account).await;
        }
        for (account, calendar) in self
            .google_pairs(
                "SELECT c.account,c.id FROM google_calendars c JOIN google_connections g ON g.account=c.account WHERE (g.show=0 OR c.selected=0) AND (c.channel<>'' OR c.events<>'[]') LIMIT 50",
                &[],
            )
            .await
        {
            self.hide_google_calendar(&account, &calendar).await;
        }
        let active = now - IMPORT_DEVICE.as_secs() as i64;
        for (account, calendar) in self
            .google_pairs(
                "SELECT c.account,c.id FROM google_calendars c JOIN google_connections g ON g.account=c.account WHERE g.show=1 AND g.error='' AND c.selected=1 AND c.attempted<$1 AND (c.dirty=1 OR (c.synced<CASE WHEN c.channel<>'' THEN CAST($2 AS BIGINT) ELSE CAST($3 AS BIGINT) END AND EXISTS(SELECT 1 FROM sessions s WHERE s.account=c.account AND s.last_seen>$4))) ORDER BY c.dirty DESC,c.synced LIMIT 25",
                &[
                    &(now - 60),
                    &(now - GOOGLE_POLL_WATCHED.as_secs() as i64),
                    &(now - GOOGLE_POLL_PLAIN.as_secs() as i64),
                    &active,
                ],
            )
            .await
        {
            if stop.is_cancelled() {
                return;
            }
            self.pull_google_calendar(&account, &calendar).await;
        }
        if self.google_webhook().is_empty() {
            return;
        }
        for (account, calendar) in self
            .google_pairs(
                "SELECT c.account,c.id FROM google_calendars c JOIN google_connections g ON g.account=c.account WHERE g.show=1 AND g.error='' AND c.selected=1 AND c.error='' AND c.channel_expires<$1 LIMIT 25",
                &[&(now + 86400)],
            )
            .await
        {
            if stop.is_cancelled() {
                return;
            }
            self.watch_google_calendar(&account, &calendar).await;
        }
    }

    /// The account's connection under its Google lock, when showing is on,
    /// working, and (with `granted`) permitted. Logs a lock failure.
    async fn shown_connection(&self, account: &str) -> Option<(super::GoogleLock<'_>, GoogleConnection)> {
        let lock = match self.lock_google(account).await {
            Ok(lock) => lock,
            Err(err) => {
                tracing::warn!(%err, "Google Calendar lock unavailable");
                return None;
            }
        };
        let c = self.google_connection(account).await.ok().flatten()?;
        (c.show && c.error.is_empty()).then_some((lock, c))
    }

    /// Records the calendars the student can show. The first listing shows
    /// their main calendar; others start hidden.
    async fn list_google_calendars(&self, account: &str) {
        let Some((lock, mut c)) = self.shown_connection(account).await else { return };
        if !c.granted(&[GOOGLE_SCOPE_LIST, GOOGLE_SCOPE_EVENTS]) {
            return;
        }
        let lost = lock.lease.lost();
        tokio::select! {
            () = self.list_locked(account, &mut c) => {}
            () = lost.cancelled() => {}
        }
    }

    async fn list_locked(&self, account: &str, c: &mut GoogleConnection) {
        let mut all = Vec::new();
        let mut token = String::new();
        for _ in 0..5 {
            let mut q = vec![
                ("maxResults", "250"),
                ("fields", "items(id,summary,summaryOverride,backgroundColor,primary,deleted),nextPageToken"),
            ];
            if !token.is_empty() {
                q.push(("pageToken", &token));
            }
            let reply: CalendarPage =
                match self.google_call(account, c, Method::GET, "/users/me/calendarList", &q, None).await {
                    Ok(reply) => reply,
                    Err(_) => {
                        // Try again in a quarter of an hour.
                        let retry = (self.now() + TimeDelta::minutes(15)
                            - TimeDelta::from_std(GOOGLE_LIST_EVERY).expect("a short interval"))
                        .timestamp();
                        self.exec_logged(
                            "UPDATE google_connections SET listed_at=$1 WHERE account=$2",
                            &[&retry, &account],
                        )
                        .await;
                        return;
                    }
                };
            all.extend(reply.items);
            token = reply.next_page_token;
            if token.is_empty() {
                break;
            }
        }
        let known: i64 =
            match self.db.fetch_opt("SELECT COUNT(*) FROM google_calendars WHERE account=$1", &[&account]).await {
                Ok(Some(row)) => row.try_get(0).unwrap_or(0),
                _ => 0,
            };
        let saved: crate::db::Result<()> = async {
            let mut client = self.db.client().await?;
            let tx = client.transaction().await?;
            let mut keep: Vec<&str> = Vec::new();
            for e in &all {
                if e.deleted || e.id.is_empty() || e.id == c.calendar {
                    continue;
                }
                let name = if e.summary_override.is_empty() { &e.summary } else { &e.summary_override };
                tx.exec(
                    "INSERT INTO google_calendars(account,id,name,color,main,selected) VALUES($1,$2,$3,$4,$5,$6) ON CONFLICT(account,id) DO UPDATE SET name=excluded.name,color=excluded.color,main=excluded.main",
                    &[&account, &e.id, name, &e.background_color, &i64::from(e.primary), &i64::from(known == 0 && e.primary)],
                )
                .await?;
                keep.push(&e.id);
            }
            tx.exec("DELETE FROM google_calendars WHERE account=$1 AND NOT (id=ANY($2))", &[&account, &keep]).await?;
            tx.exec(
                "UPDATE google_connections SET listed_at=$1,version=version+1 WHERE account=$2",
                &[&self.now().timestamp(), &account],
            )
            .await?;
            tx.commit().await?;
            Ok(())
        }
        .await;
        if saved.is_ok() {
            self.changed(account);
        }
    }

    /// Fetches one chosen calendar's events.
    async fn pull_google_calendar(&self, account: &str, calendar: &str) {
        let Some((lock, mut c)) = self.shown_connection(account).await else { return };
        let lost = lock.lease.lost();
        tokio::select! {
            () = self.pull_locked(account, &mut c, calendar) => {}
            () = lost.cancelled() => {}
        }
    }

    async fn pull_locked(&self, account: &str, c: &mut GoogleConnection, calendar: &str) {
        let now = self.now().timestamp();
        if !c.granted(&[GOOGLE_SCOPE_LIST, GOOGLE_SCOPE_EVENTS]) {
            self.exec_logged(
                "UPDATE google_calendars SET attempted=$1,dirty=0,error='Access was removed' WHERE account=$2 AND id=$3",
                &[&now, &account, &calendar],
            )
            .await;
            return;
        }
        // Clear the change mark first, so a report arriving during the fetch marks it again.
        self.exec_logged(
            "UPDATE google_calendars SET attempted=$1,dirty=0 WHERE account=$2 AND id=$3",
            &[&now, &account, &calendar],
        )
        .await;
        let events = match self.fetch_google_events(account, c, calendar).await {
            Ok(events) => events,
            Err(err) => {
                let message =
                    if matches!(err.status(), 404 | 403) { "No longer available" } else { "Could not be refreshed" };
                self.exec_logged(
                    "UPDATE google_calendars SET error=$1 WHERE account=$2 AND id=$3",
                    &[&message, &account, &calendar],
                )
                .await;
                return;
            }
        };
        let raw = String::from_utf8(escaped_json(&events)).expect("JSON is UTF-8");
        let synced = self.now().timestamp();
        let Ok(changed) = self
            .db
            .exec(
                "UPDATE google_calendars SET events=$1,synced=$2,error='' WHERE account=$3 AND id=$4 AND events<>$5",
                &[&raw, &synced, &account, &calendar, &raw],
            )
            .await
        else {
            return;
        };
        if changed > 0 {
            self.exec_logged("UPDATE google_connections SET version=version+1 WHERE account=$1", &[&account]).await;
            self.changed(account);
            return;
        }
        self.exec_logged(
            "UPDATE google_calendars SET synced=$1,error='' WHERE account=$2 AND id=$3",
            &[&synced, &account, &calendar],
        )
        .await;
    }

    /// Reads up to four pages of a calendar's events, skipping cancelled
    /// and declined ones; at most [`GOOGLE_SHOWN_LIMIT`].
    async fn fetch_google_events(
        &self,
        account: &str,
        c: &mut GoogleConnection,
        calendar: &str,
    ) -> Result<Vec<GoogleShown>, GoogleError> {
        let now = self.now();
        let time_min = (now - TimeDelta::days(14)).format("%Y-%m-%dT%H:%M:%SZ").to_string();
        let time_max = (now + TimeDelta::days(120)).format("%Y-%m-%dT%H:%M:%SZ").to_string();
        let path = calendar_path(calendar, &["events"]);
        let mut out = Vec::new();
        let mut token = String::new();
        for _ in 0..4 {
            let mut q = vec![
                ("singleEvents", "true"),
                ("orderBy", "startTime"),
                ("maxResults", "2500"),
                ("timeMin", time_min.as_str()),
                ("timeMax", time_max.as_str()),
                (
                    "fields",
                    "items(id,status,summary,location,start,end,transparency,htmlLink,attendees(self,responseStatus)),nextPageToken",
                ),
            ];
            if !token.is_empty() {
                q.push(("pageToken", &token));
            }
            let reply: EventPage = self.google_call(account, c, Method::GET, &path, &q, None).await?;
            for it in reply.items {
                if it.status == "cancelled" || out.len() >= GOOGLE_SHOWN_LIMIT {
                    continue;
                }
                if it.attendees.iter().any(|a| a.is_self && a.response_status == "declined") {
                    continue;
                }
                let mut e = GoogleShown {
                    id: it.id,
                    title: clip(&it.summary, 200).to_owned(),
                    location: clip(&it.location, 200).to_owned(),
                    link: it.html_link,
                    free: it.transparency == "transparent",
                    ..Default::default()
                };
                if e.title.is_empty() {
                    e.title = "Busy".into();
                }
                if !it.start.date_time.is_empty() {
                    (e.start, e.end) = (it.start.date_time, it.end.date_time);
                } else {
                    (e.start, e.end, e.all_day) = (it.start.date, it.end.date, true);
                }
                if e.start.is_empty() || e.end.is_empty() {
                    continue;
                }
                out.push(e);
            }
            token = reply.next_page_token;
            if token.is_empty() {
                break;
            }
        }
        Ok(out)
    }

    /// Asks Google to report changes to one calendar, replacing a channel
    /// that is about to lapse. A failure is retried in an hour; polling
    /// covers the gap.
    async fn watch_google_calendar(&self, account: &str, calendar: &str) {
        let Some((lock, mut c)) = self.shown_connection(account).await else { return };
        if !c.granted(&[GOOGLE_SCOPE_LIST, GOOGLE_SCOPE_EVENTS]) {
            return;
        }
        let lost = lock.lease.lost();
        tokio::select! {
            () = self.watch_locked(account, &mut c, calendar) => {}
            () = lost.cancelled() => {}
        }
    }

    async fn watch_locked(&self, account: &str, c: &mut GoogleConnection, calendar: &str) {
        let (old_channel, old_resource) = self.calendar_channel(account, calendar).await;
        let (channel, token) = (new_id(), random_token(24));
        let body = json!({
            "id": channel,
            "type": "web_hook",
            "address": self.google_webhook(),
            "token": token,
            "params": {"ttl": GOOGLE_CHANNEL_TTL.as_secs().to_string()},
        });
        let path = calendar_path(calendar, &["events", "watch"]);
        let Ok(reply) = self.google_call::<WatchReply>(account, c, Method::POST, &path, &[], Some(&body)).await else {
            // Renewal starts a day before expiry, so this retries in an hour.
            self.exec_logged(
                "UPDATE google_calendars SET channel_expires=$1 WHERE account=$2 AND id=$3",
                &[&(self.now() + TimeDelta::hours(25)).timestamp(), &account, &calendar],
            )
            .await;
            return;
        };
        let mut expires = (self.now() + TimeDelta::from_std(GOOGLE_CHANNEL_TTL).expect("a week")).timestamp();
        if let Ok(ms) = reply.expiration.parse::<i64>()
            && ms > 0
        {
            expires = ms / 1000;
        }
        self.exec_logged(
            "UPDATE google_calendars SET channel=$1,resource=$2,channel_token=$3,channel_expires=$4 WHERE account=$5 AND id=$6",
            &[&channel, &reply.resource_id, &token, &expires, &account, &calendar],
        )
        .await;
        if !old_channel.is_empty() {
            let stop = json!({"id": old_channel, "resourceId": old_resource});
            let _ = self.google_send(account, c, Method::POST, "/channels/stop", &[], Some(&stop)).await;
        }
    }

    /// A calendar's change channel and resource; empty when none.
    async fn calendar_channel(&self, account: &str, calendar: &str) -> (String, String) {
        match self
            .db
            .fetch_opt(
                "SELECT channel,resource FROM google_calendars WHERE account=$1 AND id=$2",
                &[&account, &calendar],
            )
            .await
        {
            Ok(Some(row)) => (row.try_get(0).unwrap_or_default(), row.try_get(1).unwrap_or_default()),
            _ => (String::new(), String::new()),
        }
    }

    /// Stops notifications for a calendar no longer shown and forgets its
    /// events.
    async fn hide_google_calendar(&self, account: &str, calendar: &str) {
        let _lock = match self.lock_google(account).await {
            Ok(lock) => lock,
            Err(err) => {
                tracing::warn!(%err, "Google Calendar lock unavailable");
                return;
            }
        };
        let (channel, resource) = self.calendar_channel(account, calendar).await;
        if let Ok(Some(mut c)) = self.google_connection(account).await
            && c.error.is_empty()
            && !channel.is_empty()
        {
            let stop = json!({"id": channel, "resourceId": resource});
            let _ = self.google_send(account, &mut c, Method::POST, "/channels/stop", &[], Some(&stop)).await;
        }
        self.exec_logged(
            "UPDATE google_calendars SET channel='',resource='',channel_token='',channel_expires=0,events='[]',synced=0 WHERE account=$1 AND id=$2",
            &[&account, &calendar],
        )
        .await;
        self.exec_logged("UPDATE google_connections SET version=version+1 WHERE account=$1", &[&account]).await;
        self.changed(account);
    }
}

/// `POST /api/google/notify`: marks a calendar for fetching when the
/// channel ID and secret token match and the state is not `sync`. Always 200.
pub(crate) async fn handle_google_notify(State(s): State<Arc<Server>>, headers: HeaderMap) -> ApiResult {
    let get = |name: &str| headers.get(name).and_then(|v| v.to_str().ok()).unwrap_or_default();
    let (channel, token) = (get("x-goog-channel-id"), get("x-goog-channel-token"));
    if !channel.is_empty()
        && let Ok(Some(row)) =
            s.db.fetch_opt("SELECT account,id,channel_token FROM google_calendars WHERE channel=$1", &[&channel]).await
        && let (Ok(account), Ok(calendar), Ok(want)) =
            (row.try_get::<_, String>(0), row.try_get::<_, String>(1), row.try_get::<_, String>(2))
        && !want.is_empty()
        && bool::from(token.as_bytes().ct_eq(want.as_bytes()))
        && get("x-goog-resource-state") != "sync"
    {
        s.exec_logged("UPDATE google_calendars SET dirty=1 WHERE account=$1 AND id=$2", &[&account, &calendar]).await;
        s.kick_google();
    }
    Ok(empty(200))
}

/// `GET /api/me/google/events`: the shown calendars' events, with an ETag
/// `"ID-VERSION"` that follows the connection's version (304 when it
/// matches). A device looking at unwatched calendars synced more than five
/// minutes ago has them fetched.
pub(crate) async fn handle_google_events(
    State(s): State<Arc<Server>>,
    Auth(se): Auth,
    headers: HeaderMap,
) -> ApiResult {
    const FAILED: &str = "could not read Google Calendar events";
    let Ok(c) = s.google_connection(&se.account).await else { return fail(500, FAILED) };
    let Some(c) = c.filter(|c| c.show) else {
        return ok(&json!({"version": 0, "calendars": [], "events": []}));
    };
    let etag = format!("\"{}-{}\"", c.id, c.version);
    let if_none_match = headers.get(header::IF_NONE_MATCH).and_then(|v| v.to_str().ok()).unwrap_or_default();
    if crate::assets::matches_etag(if_none_match, &etag) {
        return Ok(empty(304));
    }
    let Ok(rows) = s
        .db
        .fetch_all(
            "SELECT id,name,color,synced,channel,events FROM google_calendars WHERE account=$1 AND selected=1 ORDER BY main DESC,name,id",
            &[&se.account],
        )
        .await
    else {
        return fail(500, FAILED);
    };
    let (mut calendars, mut events) = (Vec::new(), Vec::new());
    let mut stale = false;
    let now = s.now().timestamp();
    for row in &rows {
        let read = || -> Result<(CalendarOut, String, String), tokio_postgres::Error> {
            Ok((
                CalendarOut {
                    id: row.try_get(0)?,
                    name: row.try_get(1)?,
                    color: row.try_get(2)?,
                    synced: row.try_get(3)?,
                },
                row.try_get(4)?,
                row.try_get(5)?,
            ))
        };
        let Ok((cal, channel, raw)) = read() else { return fail(500, FAILED) };
        let list: Vec<GoogleShown> = serde_json::from_str(&raw).unwrap_or_default();
        for mut e in list {
            e.calendar = cal.id.clone();
            events.push(e);
        }
        stale = stale || (channel.is_empty() && now - cal.synced > 5 * 60);
        calendars.push(cal);
    }
    if stale {
        s.exec_logged(
            "UPDATE google_calendars SET dirty=1 WHERE account=$1 AND selected=1 AND channel=''",
            &[&se.account],
        )
        .await;
        s.kick_google();
    }
    let mut response = crate::api::json(
        axum::http::StatusCode::OK,
        &json!({"version": c.version, "etag": etag, "calendars": calendars, "events": events}),
    );
    if let Ok(value) = HeaderValue::from_str(&etag) {
        response.headers_mut().insert(header::ETAG, value);
    }
    Ok(response)
}
