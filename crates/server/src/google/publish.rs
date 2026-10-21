//! Publishing the planner to the Honorum calendar in the student's Google
//! account: every term's primary schedule (classes as weekly series, exams,
//! personal events) and, optionally, open tasks at their due time. Weekly
//! meetings become recurring events, so a term is a few dozen events rather
//! than hundreds. Event IDs derive from what each event copies, so a retried
//! write never duplicates one, and `google_published` records what was last
//! written, so a run sends only changed events.

use std::collections::HashMap;
use std::time::Duration;

use anyhow::anyhow;
use chrono::{DateTime, Datelike, FixedOffset, NaiveDate, TimeDelta, TimeZone, Utc};
use chrono_tz::Tz;
use reqwest::Method;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use super::{GOOGLE_SCOPE_PUBLISH, GoogleConnection, GoogleError, calendar_path};
use crate::Server;
use crate::api::escaped_json;
use crate::canvas_account::{DocBody, string_value};
use crate::db::{self, Sql};
use crate::ical::parse_civil_date;
use crate::reminders::{ReminderItem, plan_items, task_items};

/// Event writes per run; the next run continues, so the account is not
/// held long.
pub(crate) const GOOGLE_PUBLISH_BUDGET: usize = 100;
/// The most events the Honorum calendar holds.
pub(crate) const GOOGLE_EVENT_LIMIT: usize = 2000;
/// A deadline is a half-hour ending when the task is due, as on the planner.
pub(crate) const GOOGLE_TASK_LEAD: Duration = Duration::from_secs(30 * 60);

/// A Calendar API time: `dateTime` with `timeZone`, or an all-day `date`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct GoogleTime {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub(crate) date_time: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub(crate) date: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub(crate) time_zone: String,
}

/// The Calendar API event body Honorum writes. Its JSON (without `id` and
/// `status`) is hashed to detect changes.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct GoogleEventBody {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub(crate) id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub(crate) status: String,
    pub(crate) summary: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub(crate) description: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub(crate) location: String,
    pub(crate) start: GoogleTime,
    pub(crate) end: GoogleTime,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) recurrence: Vec<String>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub(crate) transparency: String,
}

/// An event the calendar should hold and the hash of its body.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct GoogleDesired {
    pub(crate) body: GoogleEventBody,
    pub(crate) hash: String,
}

/// What was last written for one key: Google's event ID (empty when Google
/// refused it), the body hash, and the ETag.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct GoogleMirror {
    pub(crate) event: String,
    pub(crate) hash: String,
    pub(crate) etag: String,
}

/// The Google event ID for `key` on a connection: lowercase base32hex
/// (unpadded) of the first 20 bytes of SHA-256(`connection \0 key`), as
/// Google requires, and different for every connection.
pub(crate) fn google_event_id(connection: &str, key: &str) -> String {
    let sum = Sha256::digest(format!("{connection}\0{key}").as_bytes());
    base32hex_lower(&sum[..20])
}

/// Unpadded lowercase base32hex (RFC 4648 section 7).
fn base32hex_lower(data: &[u8]) -> String {
    const ALPHABET: &[u8; 32] = b"0123456789abcdefghijklmnopqrstuv";
    let mut out = String::with_capacity(data.len().div_ceil(5) * 8);
    let (mut buffer, mut bits) = (0u32, 0u32);
    for &byte in data {
        buffer = (buffer << 8) | u32::from(byte);
        bits += 8;
        while bits >= 5 {
            bits -= 5;
            out.push(ALPHABET[((buffer >> bits) & 31) as usize] as char);
        }
    }
    if bits > 0 {
        out.push(ALPHABET[((buffer << (5 - bits)) & 31) as usize] as char);
    }
    out
}

const DAY_LETTERS: &str = "MTWRFSU";

fn google_day(letter: char) -> &'static str {
    match letter {
        'M' => "MO",
        'T' => "TU",
        'W' => "WE",
        'R' => "TH",
        'F' => "FR",
        'S' => "SA",
        _ => "SU",
    }
}

fn day_letter(d: NaiveDate) -> char {
    DAY_LETTERS.as_bytes()[d.weekday().num_days_from_monday() as usize] as char
}

fn clock_of(minute: i64) -> String {
    format!("{:02}:{:02}:00", minute / 60, minute % 60)
}

/// Formats an instant as RFC 3339 with whole seconds and `Z` for a zero
/// offset. Stored event body hashes cover this form.
fn rfc3339(t: DateTime<FixedOffset>) -> String {
    if t.offset().local_minus_utc() == 0 {
        t.format("%Y-%m-%dT%H:%M:%SZ").to_string()
    } else {
        t.format("%Y-%m-%dT%H:%M:%S%:z").to_string()
    }
}

/// Copies a class meeting, exam sitting, or personal event in `zone`: a
/// single event on its date, or a weekly series (`RRULE` with `UNTIL`, and
/// `EXDATE` for skipped days) across its range. `None` when it has no valid
/// time or range, or no meeting day falls in it. An unknown zone is UTC.
pub(crate) fn google_occurrence(it: &ReminderItem, zone: &str) -> Option<GoogleEventBody> {
    if it.end <= it.start || it.start < 0 || it.end > 24 * 60 || it.from.is_empty() || it.to.is_empty() {
        return None;
    }
    let (loc, zone) = if zone.is_empty() {
        (Tz::UTC, String::new())
    } else {
        match zone.parse::<Tz>() {
            Ok(tz) => (tz, zone.to_owned()),
            Err(_) => (Tz::UTC, "UTC".to_owned()),
        }
    };
    let from = parse_civil_date(&it.from)?;
    let to = parse_civil_date(&it.to)?;
    if to < from {
        return None;
    }
    let mut first = from;
    let mut body = GoogleEventBody {
        summary: it.label.clone(),
        location: it.location.clone(),
        description: google_description(it),
        ..Default::default()
    };
    if it.from != it.to {
        let days: Vec<&str> = DAY_LETTERS.chars().filter(|l| it.days.contains(*l)).map(google_day).collect();
        if days.is_empty() {
            return None;
        }
        while !it.days.contains(day_letter(first)) {
            first = first.succ_opt()?;
            if first > to {
                return None;
            }
        }
        let last = to.and_hms_opt(23, 59, 59)?;
        let until = loc.from_local_datetime(&last).earliest().unwrap_or_else(|| loc.from_utc_datetime(&last));
        let until = until.with_timezone(&Utc).format("%Y%m%dT%H%M%SZ");
        body.recurrence = vec![format!("RRULE:FREQ=WEEKLY;BYDAY={};UNTIL={until}", days.join(","))];
        let mut skip = it.skip.clone();
        skip.sort();
        skip.dedup();
        let except: Vec<String> = skip
            .iter()
            .filter_map(|date| parse_civil_date(date))
            .filter(|d| *d >= first && *d <= to && it.days.contains(day_letter(*d)))
            .map(|d| format!("{}T{}", d.format("%Y%m%d"), clock_of(it.start).replace(':', "")))
            .collect();
        if !except.is_empty() {
            body.recurrence.push(format!("EXDATE;TZID={zone}:{}", except.join(",")));
        }
    } else if it.skip.contains(&it.from) {
        return None;
    }
    let date = first.format("%Y-%m-%d");
    body.start = GoogleTime {
        date_time: format!("{date}T{}", clock_of(it.start)),
        time_zone: zone.clone(),
        ..Default::default()
    };
    body.end = GoogleTime { date_time: format!("{date}T{}", clock_of(it.end)), time_zone: zone, ..Default::default() };
    Some(body)
}

fn google_description(it: &ReminderItem) -> String {
    [it.detail.trim(), it.sub.trim()].into_iter().filter(|p| !p.is_empty()).collect::<Vec<_>>().join(" · ")
}

/// Copies an open task due at or after `cutoff`: a transparent half-hour
/// ending at the due time, titled "TITLE (COURSE)", linking an http(s) URL.
/// Shown as free time, so it never blocks anything in Google Calendar.
pub(crate) fn google_deadline(it: &ReminderItem, zone: &str, cutoff: DateTime<Utc>) -> Option<GoogleEventBody> {
    let due = DateTime::parse_from_rfc3339(&it.at).ok()?;
    if due < cutoff {
        return None;
    }
    let mut summary = it.label.trim().to_owned();
    if summary.is_empty() {
        summary = "Task".into();
    }
    if !it.sub.is_empty() {
        summary = format!("{summary} ({})", it.sub);
    }
    let lead = TimeDelta::from_std(GOOGLE_TASK_LEAD).expect("a short lead");
    let mut body = GoogleEventBody {
        summary,
        transparency: "transparent".into(),
        start: GoogleTime { date_time: rfc3339(due - lead), time_zone: zone.to_owned(), ..Default::default() },
        end: GoogleTime { date_time: rfc3339(due), time_zone: zone.to_owned(), ..Default::default() },
        ..Default::default()
    };
    if url::Url::parse(&it.url).is_ok_and(|u| u.scheme() == "https" || u.scheme() == "http") {
        body.description = it.url.clone();
    }
    Some(body)
}

/// Calendar API reply with an ETag.
#[derive(Deserialize, Default)]
#[serde(default)]
struct Saved {
    etag: String,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Created {
    id: String,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct DriftItem {
    #[serde(alias = "ID", deserialize_with = "crate::api::null_default")]
    id: String,
    #[serde(alias = "Status", deserialize_with = "crate::api::null_default")]
    status: String,
    #[serde(alias = "ETag", deserialize_with = "crate::api::null_default")]
    etag: String,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct DriftPage {
    #[serde(deserialize_with = "crate::api::null_default")]
    items: Vec<DriftItem>,
    #[serde(rename = "nextPageToken")]
    next_page_token: String,
}

fn gone(err: &GoogleError) -> bool {
    matches!(err.status(), 404 | 410)
}

impl Server {
    /// Every event the Honorum calendar should hold, by key
    /// (`inst/term/itemID[@date]`, `task/ID`), and the calendar's time zone.
    /// Each term contributes its primary schedule; with none marked, the one
    /// open last, as reminders choose. Keys that collide get `#2`, `#3`, …;
    /// at most [`GOOGLE_EVENT_LIMIT`] events.
    pub(crate) async fn google_plan(
        &self,
        account: &str,
        c: &GoogleConnection,
    ) -> anyhow::Result<(HashMap<String, GoogleDesired>, String)> {
        let (prefs, schedules, tasks) = self.account_plan(account).await?;
        let prefs = prefs.unwrap_or_default();
        let tz = match self.inst(string_value(prefs.get("inst"))) {
            Some(inst) if !inst.timezone.is_empty() => inst.timezone.clone(),
            _ => "UTC".to_owned(),
        };
        let current = string_value(prefs.get("current"));
        let yes = Some(&Value::Bool(true));
        let mut primary: HashMap<String, &DocBody> = HashMap::new();
        for sc in &schedules {
            let key = format!("{}/{}", string_value(sc.get("inst")), string_value(sc.get("term")));
            if let Some(old) = primary.get(&key)
                && (old.get("primary") == yes || (sc.get("primary") != yes && string_value(sc.get("id")) != current))
            {
                continue;
            }
            primary.insert(key, sc);
        }
        let mut terms: Vec<&String> = primary.keys().collect();
        terms.sort();
        let mut out: HashMap<String, GoogleDesired> = HashMap::new();
        let mut add = |key: String, body: GoogleEventBody| {
            if out.len() >= GOOGLE_EVENT_LIMIT {
                return;
            }
            let mut key = key;
            let base = key.clone();
            let mut n = 2;
            while out.contains_key(&key) {
                key = format!("{base}#{n}");
                n += 1;
            }
            let hash = hex::encode(Sha256::digest(escaped_json(&body)));
            out.insert(key, GoogleDesired { body, hash });
        };
        for term in terms {
            let sc = primary[term];
            let Some(inst) = self.inst(string_value(sc.get("inst"))) else { continue };
            let zone = if inst.timezone.is_empty() { "UTC" } else { inst.timezone.as_str() };
            let (start, end) = self.term_dates(&inst.id, string_value(sc.get("term"))).await;
            for it in plan_items(Some(sc), &inst.closures, &start, &end) {
                let mut key = format!("{term}/{}", it.id);
                if it.from == it.to {
                    key = format!("{key}@{}", it.from);
                }
                if let Some(body) = google_occurrence(&it, zone) {
                    add(key, body);
                }
            }
        }
        if c.tasks {
            let cutoff = self.now() - TimeDelta::days(30);
            for it in task_items(&tasks) {
                if let Some(body) = google_deadline(&it, &tz, cutoff) {
                    add(format!("task/{}", it.id), body);
                }
            }
        }
        Ok((out, tz))
    }

    /// What `google_published` records for the account, by key.
    pub(crate) async fn google_mirror(&self, account: &str) -> db::Result<HashMap<String, GoogleMirror>> {
        let rows =
            self.db.fetch_all("SELECT key,event,hash,etag FROM google_published WHERE account=$1", &[&account]).await?;
        let mut out = HashMap::with_capacity(rows.len());
        for row in rows {
            out.insert(
                row.try_get(0)?,
                GoogleMirror { event: row.try_get(1)?, hash: row.try_get(2)?, etag: row.try_get(3)? },
            );
        }
        Ok(out)
    }

    /// Brings the account's Honorum calendar up to date under its Google
    /// lock, creating it first when needed; with publishing off, deletes
    /// it. Writes at most [`GOOGLE_PUBLISH_BUDGET`] events per run and backs
    /// off after failures. Stops if the lock's lease is lost.
    pub(crate) async fn publish_google(&self, account: &str) -> anyhow::Result<()> {
        let lock = self.lock_google(account).await?;
        let lost = lock.lease.lost();
        let result = tokio::select! {
            result = self.publish_locked(account) => result,
            () = lost.cancelled() => Err(anyhow!("the Google Calendar lock was lost")),
        };
        drop(lock);
        result
    }

    async fn publish_locked(&self, account: &str) -> anyhow::Result<()> {
        let Some(mut c) = self.google_connection(account).await? else { return Ok(()) };
        if !c.error.is_empty() {
            return Ok(());
        }
        if !c.publish {
            if !c.calendar.is_empty() {
                return self.unpublish_google(account, &mut c).await;
            }
            return Ok(());
        }
        // Permission removed in the Google account: keep everything and ask to
        // connect again rather than write without it.
        if !c.granted(&[GOOGLE_SCOPE_PUBLISH]) {
            let retry = (self.now() + TimeDelta::hours(1)).timestamp();
            self.db
                .exec(
                    "UPDATE google_connections SET publish_error='Access was removed',publish_retry=$1 WHERE account=$2",
                    &[&retry, &account],
                )
                .await?;
            return Ok(());
        }
        let seq: i64 = self.db.fetch_one("SELECT seq FROM accounts WHERE id=$1", &[&account]).await?.try_get(0)?;
        let (desired, tz) = self.google_plan(account, &c).await?;
        if c.calendar.is_empty() {
            let body = json!({
                "summary": "Honorum",
                "description": "Classes, exams, events and due dates from Honorum. Honorum keeps this calendar up to date; changes made here are replaced.",
                "timeZone": tz,
            });
            match self.google_call::<Created>(account, &mut c, Method::POST, "/calendars", &[], Some(&body)).await {
                Ok(created) if !created.id.is_empty() => c.calendar = created.id,
                Ok(_) => return self.google_publish_failed(account, None).await,
                Err(err) => return self.google_publish_failed(account, Some(err)).await,
            }
            self.db
                .exec("UPDATE google_connections SET calendar=$1 WHERE account=$2", &[&c.calendar, &account])
                .await?;
            self.exec_logged("DELETE FROM google_published WHERE account=$1", &[&account]).await;
        }
        let mut mirror = self.google_mirror(account).await?;
        // Once a day, compare with the calendar itself, so events changed or
        // deleted in Google Calendar are put back.
        if self.now().timestamp() - c.published_at >= 86400
            && !mirror.is_empty()
            && let Err(err) = self.google_drift(account, &mut c, &mut mirror).await
        {
            if gone(&err) {
                return self.google_calendar_gone(account).await;
            }
            return self.google_publish_failed(account, Some(err)).await;
        }
        let mut writes: Vec<&String> = desired
            .iter()
            .filter(|(k, want)| mirror.get(*k).is_none_or(|had| had.hash != want.hash))
            .map(|(k, _)| k)
            .collect();
        let mut deletes: Vec<String> = mirror.keys().filter(|k| !desired.contains_key(*k)).cloned().collect();
        writes.sort();
        deletes.sort();
        let mut budget = GOOGLE_PUBLISH_BUDGET;
        for key in writes {
            if budget == 0 {
                return Ok(());
            }
            budget -= 1;
            let exists = mirror.get(key).is_some_and(|had| !had.event.is_empty());
            if let Err(err) = self.google_write(account, &mut c, key, &desired[key], exists).await {
                if gone(&err) {
                    return self.google_calendar_gone(account).await;
                }
                if err.throttled() || matches!(err.status(), 0 | 401 | 403) {
                    return self.google_publish_failed(account, Some(err)).await;
                }
                // Google refused this one event; keep its hash so it is not
                // retried until it changes, and carry on with the rest.
                tracing::warn!(%err, "Google Calendar refused an event");
                self.exec_logged(
                    "INSERT INTO google_published(account,key,event,hash,etag) VALUES($1,$2,'',$3,'') ON CONFLICT(account,key) DO UPDATE SET hash=excluded.hash",
                    &[&account, key, &desired[key].hash],
                )
                .await;
            }
        }
        for key in deletes {
            if budget == 0 {
                return Ok(());
            }
            budget -= 1;
            let event = mirror[&key].event.clone();
            if !event.is_empty() {
                let path = calendar_path(&c.calendar, &["events", &event]);
                let deleted =
                    self.google_send(account, &mut c, Method::DELETE, &path, &[("sendUpdates", "none")], None).await;
                if let Err(err) = deleted
                    && !gone(&err)
                {
                    return self.google_publish_failed(account, Some(err)).await;
                }
            }
            self.db.exec("DELETE FROM google_published WHERE account=$1 AND key=$2", &[&account, &key]).await?;
        }
        self.db
            .exec(
                "UPDATE google_connections SET published_seq=$1,published_at=$2,publish_error='',publish_retry=0,failures=0 WHERE account=$3 AND id=$4",
                &[&seq, &self.now().timestamp(), &account, &c.id],
            )
            .await?;
        Ok(())
    }

    /// Creates or replaces one event and records what was written. An event
    /// deleted in Google keeps its ID, so a conflicting insert becomes an
    /// update.
    async fn google_write(
        &self,
        account: &str,
        c: &mut GoogleConnection,
        key: &str,
        want: &GoogleDesired,
        exists: bool,
    ) -> Result<(), GoogleError> {
        let id = google_event_id(&c.id, key);
        let mut body = want.body.clone();
        body.status = "confirmed".into();
        let query = [("sendUpdates", "none")];
        let path = calendar_path(&c.calendar, &["events", &id]);
        let value = |b: &GoogleEventBody| serde_json::to_value(b).expect("event bodies serialize");
        let mut result = None;
        if exists {
            result = Some(self.google_call::<Saved>(account, c, Method::PUT, &path, &query, Some(&value(&body))).await);
        }
        let missing = match &result {
            None => true,
            Some(Err(err)) => gone(err),
            Some(Ok(_)) => false,
        };
        if missing {
            body.id = id.clone();
            let events = calendar_path(&c.calendar, &["events"]);
            let mut inserted =
                self.google_call::<Saved>(account, c, Method::POST, &events, &query, Some(&value(&body))).await;
            if matches!(&inserted, Err(e) if e.status() == 409) {
                body.id = String::new();
                inserted = self.google_call::<Saved>(account, c, Method::PUT, &path, &query, Some(&value(&body))).await;
            }
            result = Some(inserted);
        }
        let saved = result.expect("a write was attempted")?;
        self.db
            .exec(
                "INSERT INTO google_published(account,key,event,hash,etag) VALUES($1,$2,$3,$4,$5) ON CONFLICT(account,key) DO UPDATE SET event=excluded.event,hash=excluded.hash,etag=excluded.etag",
                &[&account, &key, &id, &want.hash, &saved.etag],
            )
            .await?;
        Ok(())
    }

    /// Clears the recorded hash of events that no longer match what Honorum
    /// wrote (missing, cancelled, or with another ETag), so the run rewrites
    /// them. Events a student adds to the calendar are left alone. Reads at
    /// most five pages.
    async fn google_drift(
        &self,
        account: &str,
        c: &mut GoogleConnection,
        mirror: &mut HashMap<String, GoogleMirror>,
    ) -> Result<(), GoogleError> {
        let mut found: HashMap<String, DriftItem> = HashMap::new();
        let mut token = String::new();
        let path = calendar_path(&c.calendar, &["events"]);
        for _ in 0..5 {
            let mut q = vec![
                ("showDeleted", "true"),
                ("maxResults", "2500"),
                ("fields", "items(id,status,etag),nextPageToken"),
            ];
            if !token.is_empty() {
                q.push(("pageToken", &token));
            }
            let reply: DriftPage = self.google_call(account, c, Method::GET, &path, &q, None).await?;
            for it in reply.items {
                found.insert(it.id.clone(), it);
            }
            token = reply.next_page_token;
            if token.is_empty() {
                break;
            }
        }
        for m in mirror.values_mut() {
            if m.event.is_empty() {
                continue;
            }
            if found.get(&m.event).is_none_or(|it| it.status == "cancelled" || it.etag != m.etag) {
                m.hash.clear();
            }
        }
        Ok(())
    }

    /// Turns publishing off when the student deleted the Honorum calendar in
    /// Google Calendar: they meant it to go.
    async fn google_calendar_gone(&self, account: &str) -> anyhow::Result<()> {
        let result = self
            .db
            .exec(
                "UPDATE google_connections SET publish=0,calendar='',publish_error='Deleted in Google Calendar' WHERE account=$1",
                &[&account],
            )
            .await;
        self.exec_logged("DELETE FROM google_published WHERE account=$1", &[&account]).await;
        self.changed(account);
        result.map(drop).map_err(Into::into)
    }

    /// Backs off after a failure (2^failures minutes, up to an hour; a day
    /// when Google refused the change) and reports a problem once it
    /// persists for three runs. Returns `cause`; the next run starts where
    /// this one stopped.
    async fn google_publish_failed(&self, account: &str, cause: Option<GoogleError>) -> anyhow::Result<()> {
        let failures: i64 =
            match self.db.fetch_opt("SELECT failures FROM google_connections WHERE account=$1", &[&account]).await {
                Ok(Some(row)) => row.try_get(0).unwrap_or(0),
                _ => 0,
            } + 1;
        let mut wait = TimeDelta::minutes(1 << failures.clamp(0, 6)).min(TimeDelta::hours(1));
        let refused = cause.as_ref().is_some_and(|e| e.status() == 403 && !e.throttled());
        let message = if refused {
            wait = TimeDelta::hours(24);
            "Google did not allow the change"
        } else if failures >= 3 {
            "Could not be updated"
        } else {
            ""
        };
        self.exec_logged(
            "UPDATE google_connections SET failures=$1,publish_retry=$2,publish_error=CASE WHEN $3<>'' THEN $4 ELSE publish_error END WHERE account=$5",
            &[&failures, &(self.now() + wait).timestamp(), &message, &message, &account],
        )
        .await;
        match cause {
            Some(err) => Err(err.into()),
            None => Ok(()),
        }
    }

    /// Deletes the Honorum calendar after publishing was turned off.
    async fn unpublish_google(&self, account: &str, c: &mut GoogleConnection) -> anyhow::Result<()> {
        let path = calendar_path(&c.calendar, &[]);
        if let Err(err) = self.google_send(account, c, Method::DELETE, &path, &[], None).await
            && !gone(&err)
        {
            return self.google_publish_failed(account, Some(err)).await;
        }
        self.db
            .exec(
                "UPDATE google_connections SET calendar='',publish_error='',failures=0,publish_retry=0 WHERE account=$1",
                &[&account],
            )
            .await?;
        self.db.exec("DELETE FROM google_published WHERE account=$1", &[&account]).await?;
        Ok(())
    }
}
