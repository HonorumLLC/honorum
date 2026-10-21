//! Class and due-date reminders: guests register their meeting patterns and
//! due tasks; accounts' reminders are derived from their documents. Each
//! occurrence is queued once as a push delivery fifteen minutes ahead (an
//! hour for due times).

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use axum::body::Body;
use axum::extract::{Path, State};
use chrono::{DateTime, Datelike, TimeDelta, Timelike, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio_util::sync::CancellationToken;

use crate::Server;
use crate::api::{ApiResult, empty, fail, null_default, read_json};
use crate::canvas_account::{DocBody, string_value, task_body};
use crate::closures::Closure;
use crate::db::{self, Sql};
use crate::ical::{load_location, parse_civil_date};
use crate::push::{Alert, push_endpoint};
use crate::shares::SYNC_ID;
use crate::streaks::parse_instant;
use crate::watches::Watch;

/// How long before a class starts its reminder is sent.
pub(crate) const REMINDER_LEAD: Duration = Duration::from_secs(15 * 60);

/// One reminder pattern or one-off item. Stored JSON (`reminders.items`,
/// `reminder_schedule.items`) uses capitalized field names (`ID`, `Label`, …);
/// existing rows were written with them, so renaming a field breaks stored
/// reminders. Guests send lowercase names, accepted as aliases. The derived
/// fields after `skip` are never serialized.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ReminderItem {
    #[serde(rename = "ID", alias = "id", default)]
    pub(crate) id: String,
    #[serde(rename = "Label", alias = "label", default)]
    pub(crate) label: String,
    #[serde(rename = "Sub", alias = "sub", default)]
    pub(crate) sub: String,
    #[serde(rename = "Location", alias = "location", default)]
    pub(crate) location: String,
    /// Meeting days as letters `MTWRFSU`.
    #[serde(rename = "Days", alias = "days", default)]
    pub(crate) days: String,
    /// Minutes since midnight, campus time.
    #[serde(rename = "Start", alias = "start", default)]
    pub(crate) start: i64,
    /// First date, `YYYY-MM-DD`; empty for no bound.
    #[serde(rename = "From", alias = "from", default)]
    pub(crate) from: String,
    /// Last date, `YYYY-MM-DD`; empty for no bound.
    #[serde(rename = "To", alias = "to", default)]
    pub(crate) to: String,
    /// RFC 3339 instant for one-off items such as task due times.
    #[serde(rename = "At", alias = "at", default)]
    pub(crate) at: String,
    /// Dates the class does not meet.
    #[serde(rename = "Skip", alias = "skip", default, deserialize_with = "null_default")]
    pub(crate) skip: Vec<String>,
    /// Minutes since midnight; derived items only.
    #[serde(skip)]
    pub(crate) end: i64,
    /// `class`, `exam`, `event`, or `task`; derived items only.
    #[serde(skip)]
    pub(crate) kind: String,
    /// The course title or an event's notes; derived items only.
    #[serde(skip)]
    pub(crate) detail: String,
    /// Where a task lives upstream; derived items only.
    #[serde(skip)]
    pub(crate) url: String,
}

/// A reminder registration: a guest's (`target` is its push subscription)
/// or an account's (`account` set, items derived).
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct Reminder {
    #[serde(alias = "ID")]
    pub(crate) id: String,
    #[serde(alias = "Target")]
    pub(crate) target: String,
    #[serde(alias = "TZ")]
    pub(crate) tz: String,
    #[serde(alias = "Account")]
    pub(crate) account: String,
    #[serde(alias = "Items", deserialize_with = "null_default")]
    pub(crate) items: Vec<ReminderItem>,
}

/// The weekday letters reminders use, Monday first.
pub(crate) const WEEKDAYS: &[u8; 7] = b"MTWRFSU";

/// The letter for a weekday in [`WEEKDAYS`].
pub(crate) fn weekday_letter(day: chrono::Weekday) -> char {
    char::from(WEEKDAYS[day.num_days_from_monday() as usize])
}

/// Parses stored reminder items. Rows written by earlier releases hold
/// `null` for an empty list.
pub(crate) fn parse_items(raw: &str) -> serde_json::Result<Vec<ReminderItem>> {
    Ok(serde_json::from_str::<Option<Vec<ReminderItem>>>(raw)?.unwrap_or_default())
}

fn if_non_empty(prefix: &str, v: &str) -> String {
    if v.is_empty() { String::new() } else { format!("{prefix}{v}") }
}

/// A JSON number truncated toward zero to an integer; zero for anything
/// else.
fn number_value(v: Option<&Value>) -> i64 {
    v.and_then(Value::as_f64).unwrap_or_default() as i64
}

fn object(v: Option<&Value>) -> Option<&DocBody> {
    v.and_then(Value::as_object)
}

fn list(v: Option<&Value>) -> &[Value] {
    v.and_then(Value::as_array).map(Vec::as_slice).unwrap_or_default()
}

/// Expands closure ranges to one entry per civil date (at most 5000), in
/// date order; a later closure replaces an earlier one on the same date.
fn closure_index(closures: &[Closure]) -> BTreeMap<String, Closure> {
    let mut out = BTreeMap::new();
    for c in closures {
        let Some(mut d) = parse_civil_date(&c.from) else { continue };
        loop {
            let date = d.format("%Y-%m-%d").to_string();
            if date.as_str() > c.to.as_str() || out.len() >= 5000 {
                break;
            }
            out.insert(date, c.clone());
            match d.succ_opt() {
                Some(next) => d = next,
                None => break,
            }
        }
    }
    out
}

/// One schedule's occurrences: each class meeting as a weekly pattern
/// skipping removed days and closures (a partial-day closure spares classes
/// before its `after`), substitute weekdays that run it, its exam sittings,
/// and the schedule's personal events. Empty without a schedule.
pub(crate) fn plan_items(
    schedule: Option<&DocBody>,
    closures: &[Closure],
    term_start: &str,
    term_end: &str,
) -> Vec<ReminderItem> {
    let Some(schedule) = schedule else { return Vec::new() };
    let closed = closure_index(closures);
    let removed_of = |id: &str| -> Vec<String> {
        object(schedule.get("removed"))
            .map(|removed| list(removed.get(id)).iter().map(|v| string_value(Some(v)).to_owned()).collect())
            .unwrap_or_default()
    };
    let text = |m: &DocBody, k: &str| string_value(m.get(k)).to_owned();
    let mut items = Vec::new();
    let pinned = object(schedule.get("pinned"));
    for v in list(schedule.get("sections")) {
        let id = string_value(Some(v));
        let Some(sec) = pinned.and_then(|p| object(p.get(id))) else { continue };
        let label = format!("{}-{}", text(sec, "course"), text(sec, "num"));
        let (sub, title) = (text(sec, "type"), text(sec, "title"));
        let mut from = text(sec, "start");
        let mut to = text(sec, "end");
        if from.is_empty() {
            from = term_start.to_owned();
        }
        if to.is_empty() {
            to = term_end.to_owned();
        }
        let removed = removed_of(id);
        for mv in list(sec.get("meetings")) {
            let Some(m) = mv.as_object() else { continue };
            let days = text(m, "days");
            let (start, end) = (number_value(m.get("start")), number_value(m.get("end")));
            // A closure that starts partway through the day spares earlier classes.
            let mut skip = removed.clone();
            skip.extend(closed.iter().filter(|(_, c)| c.cancels(start)).map(|(date, _)| date.clone()));
            let item_id = format!("{id}:{days}:{start}");
            items.push(ReminderItem {
                id: item_id.clone(),
                label: label.clone(),
                sub: sub.clone(),
                location: text(m, "location"),
                days: days.clone(),
                start,
                end,
                from: from.clone(),
                to: to.clone(),
                skip,
                kind: "class".into(),
                detail: title.clone(),
                ..ReminderItem::default()
            });
            // A substitute weekday runs another day's classes.
            for (date, closure) in &closed {
                if closure.follows.is_empty()
                    || removed.contains(date)
                    || (!from.is_empty() && *date < from)
                    || (!to.is_empty() && *date > to)
                    || !days.contains(closure.follows.as_str())
                {
                    continue;
                }
                items.push(ReminderItem {
                    id: item_id.clone(),
                    label: label.clone(),
                    sub: sub.clone(),
                    location: text(m, "location"),
                    days: "MTWRFSU".into(),
                    start,
                    end,
                    from: date.clone(),
                    to: date.clone(),
                    kind: "class".into(),
                    detail: title.clone(),
                    ..ReminderItem::default()
                });
            }
        }
        // Exam sittings are dated, so each is its own one-day item.
        for (i, ev) in list(sec.get("exams")).iter().enumerate() {
            let Some(e) = ev.as_object() else { continue };
            let date = text(e, "date");
            if date.is_empty() {
                continue;
            }
            let exam_id = format!("{id}:exam:{i}");
            items.push(ReminderItem {
                skip: removed_of(&exam_id),
                id: exam_id,
                label: format!("{} Exam", text(sec, "course")),
                sub: label.clone(),
                location: text(e, "location"),
                days: "MTWRFSU".into(),
                start: number_value(e.get("start")),
                end: number_value(e.get("end")),
                from: date.clone(),
                to: date,
                kind: "exam".into(),
                detail: title.clone(),
                ..ReminderItem::default()
            });
        }
    }
    for bv in list(schedule.get("blocks")) {
        let Some(b) = bv.as_object() else { continue };
        let id = text(b, "id");
        let (mut from, mut to) = (text(b, "date"), text(b, "date"));
        if from.is_empty() {
            (from, to) = (term_start.to_owned(), term_end.to_owned());
        }
        let (days, start) = (text(b, "days"), number_value(b.get("start")));
        items.push(ReminderItem {
            id: format!("{id}:{days}:{start}"),
            label: text(b, "name"),
            location: text(b, "location"),
            days,
            start,
            end: number_value(b.get("end")),
            from,
            to,
            skip: removed_of(&id),
            kind: "event".into(),
            detail: text(b, "note"),
            ..ReminderItem::default()
        });
    }
    items
}

/// The open tasks with a due time, as one-off items.
pub(crate) fn task_items(tasks: &[DocBody]) -> Vec<ReminderItem> {
    tasks
        .iter()
        .filter(|t| t.get("done") != Some(&Value::Bool(true)) && !string_value(t.get("due")).is_empty())
        .map(|t| ReminderItem {
            id: string_value(t.get("id")).to_owned(),
            label: string_value(t.get("title")).to_owned(),
            sub: string_value(t.get("course")).to_owned(),
            at: string_value(t.get("due")).to_owned(),
            kind: "task".into(),
            url: string_value(t.get("url")).to_owned(),
            ..ReminderItem::default()
        })
        .collect()
}

impl Server {
    /// Polls indexed schedules after seeding missing ones (retrying each
    /// second), with four workers ticking every five seconds, until `stop`.
    pub async fn run_reminders(self: &Arc<Self>, stop: CancellationToken) {
        while !stop.is_cancelled() {
            match self.seed_reminder_schedules().await {
                Ok(()) => break,
                Err(err) => {
                    tracing::warn!(%err, "reminder initialization will retry");
                    tokio::select! {
                        () = stop.cancelled() => return,
                        () = tokio::time::sleep(Duration::from_secs(1)) => {}
                    }
                }
            }
        }
        let mut workers = tokio::task::JoinSet::new();
        for _ in 0..4 {
            let (s, stop) = (self.clone(), stop.clone());
            workers.spawn(async move {
                let mut ticker = tokio::time::interval(Duration::from_secs(5));
                ticker.tick().await;
                loop {
                    s.tick_scheduled_reminders(&stop).await;
                    tokio::select! {
                        () = stop.cancelled() => return,
                        _ = ticker.tick() => {}
                    }
                }
            });
        }
        while workers.join_next().await.is_some() {}
    }

    /// The documents reminders and calendar copies come from: preferences
    /// (`None` without them), schedules, and tasks.
    pub(crate) async fn account_plan(
        &self,
        account: &str,
    ) -> db::Result<(Option<DocBody>, Vec<DocBody>, Vec<DocBody>)> {
        let rows = self
            .db
            .fetch_all(
                "SELECT id,body FROM docs WHERE account=$1 AND deleted=0 AND (type='prefs' OR type='schedule' OR type='task')",
                &[&account],
            )
            .await?;
        let (mut prefs, mut schedules, mut tasks) = (None, Vec::new(), Vec::new());
        for row in rows {
            let id: String = row.try_get(0)?;
            let raw: String = row.try_get(1)?;
            let body = task_body(raw.as_bytes());
            if id == "prefs" {
                prefs = Some(body);
            } else if id.starts_with("s_") {
                schedules.push(body);
            } else if id.starts_with("t_") {
                tasks.push(body);
            }
        }
        Ok((prefs, schedules, tasks))
    }

    /// A term's first and last days; empty when unknown.
    pub(crate) async fn term_dates(&self, inst: &str, term: &str) -> (String, String) {
        let row = self
            .db
            .fetch_opt(
                r#"SELECT COALESCE(start,''),COALESCE("end",'') FROM terms WHERE inst=$1 AND id=$2"#,
                &[&inst, &term],
            )
            .await;
        match row {
            Ok(Some(row)) => (row.try_get(0).unwrap_or_default(), row.try_get(1).unwrap_or_default()),
            _ => (String::new(), String::new()),
        }
    }

    /// The account's time zone and reminder items: its primary schedule for
    /// the term it plans in, and its open tasks (the same items the guest
    /// client registers for itself); at most 1000 items.
    pub(crate) async fn derive_reminders(&self, account: &str) -> db::Result<(String, Vec<ReminderItem>)> {
        let (prefs, schedules, tasks) = self.account_plan(account).await?;
        let pref = |k: &str| prefs.as_ref().map(|p| string_value(p.get(k)).to_owned()).unwrap_or_default();
        let (inst, term, current) = (pref("inst"), pref("term"), pref("current"));
        let mut tz = "UTC".to_owned();
        let mut closures: &[Closure] = &[];
        if let Some(i) = self.inst(&inst) {
            if !i.timezone.is_empty() {
                tz = i.timezone.clone();
            }
            closures = &i.closures;
        }
        let mut primary: Option<&DocBody> = None;
        for sc in &schedules {
            if string_value(sc.get("inst")) != inst || string_value(sc.get("term")) != term {
                continue;
            }
            if sc.get("primary") == Some(&Value::Bool(true)) {
                primary = Some(sc);
                break;
            }
            if primary.is_none() || string_value(sc.get("id")) == current {
                primary = Some(sc);
            }
        }
        let (term_start, term_end) = self.term_dates(&inst, &term).await;
        let mut items = plan_items(primary, closures, &term_start, &term_end);
        items.extend(task_items(&tasks));
        items.truncate(1000);
        Ok((tz, items))
    }

    /// Queues every occurrence due within the five minutes up to `now`
    /// (a catch-up window), each at most once. Class reminders go out
    /// [`REMINDER_LEAD`] before the start; task reminders an hour before
    /// the due time.
    pub(crate) async fn match_reminders(&self, rem: &Reminder, now: DateTime<Utc>) -> anyhow::Result<()> {
        let zone = load_location(&rem.tz).ok_or_else(|| anyhow::anyhow!("unknown time zone {}", rem.tz))?;
        for offset in (0..=4).rev() {
            let observed = now - TimeDelta::minutes(offset);
            let at = (observed + TimeDelta::from_std(REMINDER_LEAD)?).with_timezone(&zone);
            let day = weekday_letter(at.weekday());
            let minute = i64::from(at.hour() * 60 + at.minute());
            let date = at.format("%Y-%m-%d").to_string();
            for it in &rem.items {
                let (title, body, expires);
                if !it.at.is_empty() {
                    let Some(due) = parse_instant(&it.at) else { continue };
                    if (due - TimeDelta::hours(1)).timestamp() / 60 != observed.timestamp() / 60 {
                        continue;
                    }
                    expires = due.timestamp();
                    title = format!("{} is due in an hour", it.label);
                    body = format!(
                        "Due {}{}",
                        due.with_timezone(&zone).format("%a %-I:%M%P"),
                        if_non_empty(" · ", &it.sub)
                    );
                } else {
                    if it.start != minute
                        || !it.days.contains(day)
                        || (!it.from.is_empty() && date < it.from)
                        || (!it.to.is_empty() && date > it.to)
                        || it.skip.contains(&date)
                    {
                        continue;
                    }
                    expires = at.timestamp();
                    title = format!("{} starts soon", format!("{} {}", it.label, it.sub).trim());
                    body = format!("Starts at {}{}", at.format("%-I:%M%P"), if_non_empty(" in ", &it.location));
                }
                let identity = if it.id.is_empty() {
                    format!("{}|{}|{}|{}|{}", it.label, it.sub, it.days, it.start, it.at)
                } else {
                    it.id.clone()
                };
                let key = format!("{}|{}|{}", rem.id, identity, at.format("%Y-%m-%dT%H:%M"));
                let alert = Alert { title, body, url: self.cfg.base_url.clone() };
                if let Err(err) = self.queue_reminder(rem, &key, &alert, expires).await {
                    tracing::warn!(id = %rem.id, %err, "reminder could not be queued");
                    return Err(err);
                }
            }
        }
        Ok(())
    }

    /// Records `key` in `reminder_log` and, if new, queues the alert to the
    /// reminder's destinations in the same transaction.
    pub(crate) async fn queue_reminder(
        &self,
        rem: &Reminder,
        key: &str,
        alert: &Alert,
        expires: i64,
    ) -> anyhow::Result<()> {
        let mut client = self.db.client().await?;
        let tx = client.transaction().await.map_err(db::Error::from)?;
        let now = self.now().timestamp();
        let count = tx
            .exec("INSERT INTO reminder_log(key,sent_at) VALUES($1,$2) ON CONFLICT DO NOTHING", &[&key, &now])
            .await?;
        if count == 0 {
            return Ok(());
        }
        let watch =
            Watch { id: rem.id.clone(), target: rem.target.clone(), account: rem.account.clone(), last_alert: 0 };
        self.enqueue(&tx, "reminder", &watch, alert, key, expires).await?;
        tx.commit().await.map_err(db::Error::from)?;
        Ok(())
    }
}

/// `PUT /api/reminders/{id}`: registers a guest's reminders ([`Reminder`];
/// 64 KiB, at most 100 items; 204). The server cannot see a guest planner,
/// so guests register their patterns.
pub(crate) async fn handle_put_reminder(State(s): State<Arc<Server>>, Path(id): Path<String>, body: Body) -> ApiResult {
    if !SYNC_ID.is_match(&id) {
        return fail(400, "bad id");
    }
    let Some(input) = read_json::<Reminder>(body, 64 << 10).await.filter(|r| r.items.len() <= 100) else {
        return fail(400, "bad reminder");
    };
    let endpoint = push_endpoint(&input.target);
    if endpoint.is_empty() {
        return fail(400, "bad push subscription");
    }
    if load_location(&input.tz).is_none() {
        return fail(400, "bad time zone");
    }
    let items = serde_json::to_string(&input.items).unwrap_or_else(|_| "[]".into());
    let saved = s
        .db
        .exec(
            "INSERT INTO reminders(id,target,endpoint,tz,items,updated_at) VALUES($1,$2,$3,$4,$5,$6) ON CONFLICT(id) DO UPDATE SET target=excluded.target,endpoint=excluded.endpoint,tz=excluded.tz,items=excluded.items,updated_at=excluded.updated_at",
            &[&id, &input.target, &endpoint, &input.tz, &items, &s.now().timestamp()],
        )
        .await;
    if saved.is_err() {
        return fail(500, "reminder could not be saved");
    }
    Ok(empty(204))
}

/// `DELETE /api/reminders/{id}`: removes a guest's reminders (204).
pub(crate) async fn handle_delete_reminder(State(s): State<Arc<Server>>, Path(id): Path<String>) -> ApiResult {
    if s.db.exec("DELETE FROM reminders WHERE id=$1", &[&id]).await.is_err() {
        return fail(500, "reminder could not be removed");
    }
    Ok(empty(204))
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use axum::response::IntoResponse;
    use chrono::TimeZone;
    use chrono_tz::Tz;
    use serde_json::json;

    use super::*;
    use crate::closures::load_closures;
    use crate::test_support::signed_in_browser;
    use crate::test_support::{TestServer, test_server};

    /// A daily-expired schedule queues its rebuild on one poll and runs it
    /// on the next.
    async fn tick_test_reminders(s: &Server) {
        let stop = CancellationToken::new();
        s.tick_scheduled_reminders(&stop).await;
        s.tick_scheduled_reminders(&stop).await;
    }

    async fn put(t: &TestServer, id: &str, body: &str) -> u16 {
        let response = t
            .client()
            .send("PUT", &format!("/api/reminders/{id}"), &[("content-type", "application/json")], body.to_owned())
            .await;
        response.status.as_u16()
    }

    #[tokio::test]
    async fn reminder_writes_report_database_failures() {
        let t = test_server().await;
        t.server.db.close();
        let body = r#"{"target":"{\"endpoint\":\"https://push.example/x\"}","tz":"UTC","items":[]}"#;
        let id = "ab".repeat(16);
        let put = handle_put_reminder(State(t.server.clone()), Path(id.clone()), Body::from(body)).await;
        let status = put.map_or_else(|e| e.into_response().status(), |r| r.status());
        assert_eq!(status.as_u16(), 500, "PUT reported success after database failure");
        let delete = handle_delete_reminder(State(t.server.clone()), Path(id)).await;
        let status = delete.map_or_else(|e| e.into_response().status(), |r| r.status());
        assert_eq!(status.as_u16(), 500, "DELETE reported success after database failure");
    }

    #[tokio::test]
    async fn reminders() {
        let t = test_server().await;
        let s = t.server.clone();
        let sent: Arc<Mutex<Vec<Alert>>> = Arc::default();
        let log = sent.clone();
        s.set_sender(Some(Arc::new(move |_: &str, a: &Alert| {
            log.lock().unwrap().push(a.clone());
            (false, Ok(()))
        })));
        let chicago: Tz = "America/Chicago".parse().unwrap();
        let at = |y, m, d, h, mi| chicago.with_ymd_and_hms(y, m, d, h, mi, 0).unwrap().with_timezone(&Utc);
        // Monday 2026-08-24 08:45 Chicago: a 9:00 class should be announced.
        s.clock.set(at(2026, 8, 24, 8, 45));
        let sent_titles = || sent.lock().unwrap().clone();

        let id = "cd".repeat(16);
        let body = r#"{"target":"{\"endpoint\":\"https://push.example/x\",\"keys\":{\"p256dh\":\"a\",\"auth\":\"b\"}}","tz":"America/Chicago","items":[{"label":"CS 101-001","location":"Felgar Hall 304","days":"MWF","start":540,"from":"2026-08-24","to":"2026-12-11"},{"label":"MATH 151","days":"TR","start":540}]}"#;
        assert_eq!(put(&t, &id, body).await, 204);
        tick_test_reminders(&s).await;
        s.dispatch_pending().await;
        tick_test_reminders(&s).await; // same minute again: deduplicated
        s.dispatch_pending().await;
        let got = sent_titles();
        assert!(
            got.len() == 1 && got[0].title.contains("CS 101-001") && got[0].body.contains("Felgar Hall 304"),
            "expected one reminder, got {got:?}"
        );
        s.clock.set(at(2026, 8, 25, 8, 45)); // Tuesday
        tick_test_reminders(&s).await;
        s.dispatch_pending().await;
        let got = sent_titles();
        assert!(got.len() == 2 && got[1].title.contains("MATH 151"), "expected the Tuesday class, got {got:?}");

        // A task due at 10:00 local is announced at 09:00.
        let body = r#"{"target":"{\"endpoint\":\"https://push.example/x\",\"keys\":{\"p256dh\":\"a\",\"auth\":\"b\"}}","tz":"America/Chicago","items":[{"label":"Project 1","sub":"CS 1213-010","at":"2026-08-26T15:00:00Z"}]}"#;
        put(&t, &id, body).await;
        s.clock.set(at(2026, 8, 26, 9, 0));
        tick_test_reminders(&s).await;
        s.dispatch_pending().await;
        let got = sent_titles();
        assert!(got.len() == 3 && got[2].title.contains("Project 1 is due"), "expected the task reminder, got {got:?}");

        // A class does not meet on a holiday.
        let body = r#"{"target":"{\"endpoint\":\"https://push.example/x\",\"keys\":{\"p256dh\":\"a\",\"auth\":\"b\"}}","tz":"America/Chicago","items":[{"label":"CS 101-001","days":"MWF","start":540,"skip":["2026-09-07"]}]}"#;
        put(&t, &id, body).await;
        s.clock.set(at(2026, 9, 7, 8, 45)); // Labor Day
        tick_test_reminders(&s).await;
        s.dispatch_pending().await;
        s.clock.set(at(2026, 9, 9, 8, 45));
        tick_test_reminders(&s).await;
        s.dispatch_pending().await;
        let got = sent_titles();
        assert!(
            got.len() == 4 && got[3].title.contains("CS 101-001"),
            "expected the holiday skipped and Wednesday sent, got {got:?}"
        );

        t.client().delete(&format!("/api/reminders/{id}")).await;
        s.clock.set(at(2026, 8, 26, 8, 45));
        tick_test_reminders(&s).await;
        s.dispatch_pending().await;
        let got = sent_titles();
        assert_eq!(got.len(), 4, "deleted reminder still fired: {got:?}");
    }

    // Exam sittings saved with a class become one-day reminders of their own.
    #[tokio::test]
    async fn account_reminders_include_exams() {
        let t = test_server().await;
        let a = signed_in_browser(&t).await;
        a.must("POST", "/api/me/sync", r#"{"since":0,"docs":[{"id":"prefs","type":"prefs","base":0,"updated":40,"body":{"inst":"demo","term":"2026FA","current":"2","reminders":true}},{"id":"s_2","type":"schedule","base":0,"updated":40,"body":{"id":"2","inst":"demo","term":"2026FA","primary":true,"sections":["10001"],"pinned":{"10001":{"id":"10001","course":"CS 101","num":"001","type":"LEC","meetings":[{"days":"MWF","start":540,"end":590,"location":"Felgar 300"}],"exams":[{"date":"2026-10-08","start":1140,"end":1260,"location":"Engineering 110"},{"date":"2026-12-09","start":480,"end":600}]}},"removed":{"10001:exam:1":["2026-12-09"]},"blocks":[]}}]}"#, 200).await;
        let (_, items) = t.server.derive_reminders(&a.account).await.unwrap();
        let exams: Vec<_> = items.iter().filter(|it| it.id.contains(":exam:")).collect();
        assert!(
            exams.len() == 2
                && exams[0].label == "CS 101 Exam"
                && exams[0].sub == "CS 101-001"
                && exams[0].from == "2026-10-08"
                && exams[0].to == "2026-10-08"
                && exams[0].days == "MTWRFSU"
                && exams[0].start == 1140
                && exams[0].location == "Engineering 110",
            "expected two exam items, got {exams:?}"
        );
        assert_eq!(exams[1].skip, vec!["2026-12-09".to_owned()], "a hidden exam should carry its skipped date");
    }

    #[test]
    fn partial_closure_spares_earlier_classes() {
        let schedule = json!({
            "sections": ["a"],
            "pinned": {"a": {"course": "CS 101", "num": "001", "meetings": [
                {"days": "TR", "start": 540.0, "end": 615.0},
                {"days": "TR", "start": 900.0, "end": 975.0},
            ]}},
        });
        let closures = vec![Closure {
            from: "2026-10-29".into(),
            to: "2026-10-29".into(),
            name: "Concert".into(),
            after: 900,
            ..Closure::default()
        }];
        let items = plan_items(schedule.as_object(), &closures, "2026-08-24", "2026-12-11");
        assert_eq!(items.len(), 2, "expected two meetings, got {items:?}");
        for it in &items {
            let skipped = it.skip.iter().any(|d| d == "2026-10-29");
            assert_eq!(skipped, it.start >= 900, "meeting at {}: skipped {skipped}", it.start);
        }
        let all = load_closures().unwrap();
        assert!(
            all["ou"].iter().any(|c| c.from == "2026-10-29" && c.after == 900),
            "OU lacks the Oct. 29 afternoon closure"
        );
    }

    // Stored items keep their capitalized field names; guests' lowercase
    // names and stored `null` lists are accepted.
    #[test]
    fn reminder_items_keep_stored_names() {
        let it = ReminderItem { id: "x".into(), start: 540, kind: "class".into(), ..ReminderItem::default() };
        let raw = serde_json::to_value(&it).unwrap();
        assert_eq!(raw["ID"], "x");
        assert_eq!(raw["Start"], 540);
        assert!(raw.get("kind").is_none() && raw.get("Kind").is_none());
        let back = parse_items(r#"[{"ID":"x","Start":540,"Skip":null},{"id":"y","days":"MWF"}]"#).unwrap();
        assert_eq!(back[0].id, "x");
        assert!(back[0].skip.is_empty());
        assert_eq!(back[1].days, "MWF");
        assert!(parse_items("null").unwrap().is_empty());
    }

    // The next check lands fifteen minutes before the next class, across a
    // DST change, and an hour before a due time.
    #[test]
    fn next_check_finds_the_next_occurrence() {
        use crate::reminder_schedule::next_reminder_check;
        let chicago: Tz = "America/Chicago".parse().unwrap();
        // Saturday before the fall-back change; class Monday 9:00 is out of
        // range, Sunday's 9:00 (after the change) is in range.
        let now = chicago.with_ymd_and_hms(2026, 10, 31, 12, 0, 0).unwrap().with_timezone(&Utc);
        let rem = Reminder {
            tz: "America/Chicago".into(),
            items: vec![ReminderItem { days: "U".into(), start: 540, ..ReminderItem::default() }],
            ..Reminder::default()
        };
        let want = chicago.with_ymd_and_hms(2026, 11, 1, 8, 45, 0).unwrap().timestamp();
        assert_eq!(next_reminder_check(&rem, now), want);
        let rem = Reminder {
            tz: "America/Chicago".into(),
            items: vec![ReminderItem { at: "2026-10-31T20:00:30Z".into(), ..ReminderItem::default() }],
            ..Reminder::default()
        };
        assert_eq!(next_reminder_check(&rem, now), Utc.with_ymd_and_hms(2026, 10, 31, 19, 0, 0).unwrap().timestamp());
        let bad = Reminder { tz: "Nowhere/Else".into(), ..Reminder::default() };
        assert_eq!(next_reminder_check(&bad, now), now.timestamp() + 3600);
    }
}
