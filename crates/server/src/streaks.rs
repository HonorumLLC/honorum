//! Streaks: a day counts when nothing was overdue as it ended, in the
//! account's time zone, derived from task documents. Finished days stay
//! open to late sync for [`STREAK_GRACE`] days and are then frozen, so an
//! import that later drops an old task cannot rewrite history.

use std::collections::HashMap;
use std::sync::Arc;

use anyhow::{Context, anyhow};
use axum::body::Body;
use axum::extract::State;
use axum::http::HeaderValue;
use axum::http::header::CACHE_CONTROL;
use axum::response::Response;
use chrono::{DateTime, NaiveDate, TimeDelta, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::Server;
use crate::api::{ApiResult, fail, null_default, ok, read_json};
use crate::canvas_account::{string_value, task_body};
use crate::db::{self, Sql};
use crate::ical::{load_location, local_midnight, parse_civil_date};
use crate::session::Auth;

/// Days a finished day stays open to late-arriving sync.
pub(crate) const STREAK_GRACE: i64 = 2;
/// Days the streak display covers, including today.
pub(crate) const STREAK_SHOWN: i64 = 28;

/// One shown day: "clean" or "missed" when finished; today is "clean" or
/// "overdue".
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct StreakDay {
    pub(crate) day: String,
    pub(crate) state: String,
}

/// An account's streak as clients see it (also the cached JSON).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct Streak {
    pub(crate) current: i64,
    pub(crate) longest: i64,
    pub(crate) today: String,
    pub(crate) last_day: String,
    pub(crate) timezone: String,
    pub(crate) overdue: i64,
    pub(crate) days: Vec<StreakDay>,
}

/// A dated task as the streak sees it.
struct StreakTask {
    due: DateTime<Utc>,
    done: bool,
    /// `None` when a completion carries no time; it counts as on time.
    completed: Option<DateTime<Utc>>,
}

impl StreakTask {
    /// Reports whether the task was due and still open at `at`.
    fn overdue_at(&self, at: DateTime<Utc>) -> bool {
        self.due < at && (!self.done || self.completed.is_some_and(|c| c > at))
    }
}

/// Parses an RFC 3339 time (with optional fractional seconds); `None` for
/// empty or invalid text.
pub(crate) fn parse_instant(value: &str) -> Option<DateTime<Utc>> {
    if value.is_empty() {
        return None;
    }
    DateTime::parse_from_rfc3339(value).ok().map(|t| t.with_timezone(&Utc))
}

fn date_text(d: NaiveDate) -> String {
    d.format("%Y-%m-%d").to_string()
}

fn add_days(d: NaiveDate, n: i64) -> anyhow::Result<NaiveDate> {
    d.checked_add_signed(TimeDelta::days(n)).ok_or_else(|| anyhow!("date out of range"))
}

/// The account's live dated tasks.
async fn streak_tasks(db: &dyn Sql, account: &str) -> db::Result<Vec<StreakTask>> {
    let rows =
        db.fetch_all("SELECT body FROM docs WHERE account=$1 AND deleted=0 AND type='task'", &[&account]).await?;
    let mut out = Vec::new();
    for row in rows {
        let raw: Option<String> = row.try_get(0)?;
        let body = task_body(raw.unwrap_or_default().as_bytes());
        let Some(due) = parse_instant(string_value(body.get("due"))) else { continue };
        let done = body.get("done") == Some(&Value::Bool(true));
        let completed = if done { parse_instant(string_value(body.get("completedAt"))) } else { None };
        out.push(StreakTask { due, done, completed });
    }
    Ok(out)
}

/// Works out the streak at `now` and freezes finished days past the grace
/// period. The caller holds the account row lock. The first read records
/// the start day, so days before an account used streaks never count
/// against it.
///
/// Days are civil dates in the account's zone; a day ends at the next
/// local midnight.
pub(crate) async fn read_streak(db: &dyn Sql, account: &str, now: DateTime<Utc>) -> anyhow::Result<Streak> {
    let mut out = Streak { timezone: "UTC".into(), ..Streak::default() };
    if let Some(row) = db.fetch_opt("SELECT timezone FROM account_timezones WHERE account=$1", &[&account]).await? {
        out.timezone = row.try_get(0)?;
    }
    let zone = load_location(&out.timezone).ok_or_else(|| anyhow!("unknown time zone {}", out.timezone))?;
    let today = now.with_timezone(&zone).date_naive();
    out.today = date_text(today);
    db.exec(
        "INSERT INTO account_streaks(account,start) VALUES($1,$2) ON CONFLICT(account) DO NOTHING",
        &[&account, &out.today],
    )
    .await?;
    let first: String =
        db.fetch_one("SELECT start FROM account_streaks WHERE account=$1", &[&account]).await?.try_get(0)?;
    let mut start = parse_civil_date(&first).with_context(|| format!("bad streak start {first}"))?;
    let shown_from = add_days(today, 1 - STREAK_SHOWN)?;

    let (mut through, mut run, mut longest, mut last_clean) = (String::new(), 0i64, 0i64, String::new());
    if let Some(row) = db
        .fetch_opt(
            "SELECT through_day,run,longest,last_clean FROM account_streak_rollups WHERE account=$1",
            &[&account],
        )
        .await?
    {
        (through, run, longest, last_clean) = (row.try_get(0)?, row.try_get(1)?, row.try_get(2)?, row.try_get(3)?);
    }
    if through >= out.today {
        (through, run, longest, last_clean) = (String::new(), 0, 0, String::new());
    }
    out.longest = longest;
    out.last_day = last_clean.clone();

    let mut frozen: HashMap<String, bool> = HashMap::new();
    for row in db
        .fetch_all(
            "SELECT day,clean FROM account_streak_days WHERE account=$1 AND (day>=$2 OR day>$3)",
            &[&account, &date_text(shown_from), &through],
        )
        .await?
    {
        let clean: i64 = row.try_get(1)?;
        frozen.insert(row.try_get(0)?, clean != 0);
    }
    let tasks = streak_tasks(db, account).await?;

    // Frozen history is summarized once. Only the grace period and newly
    // finished days require task evaluation; the display reads at most 28 days.
    if !through.is_empty() {
        let base = parse_civil_date(&through).with_context(|| format!("bad streak rollup {through}"))?;
        let mut d = start.max(shown_from);
        while d <= base {
            let day = date_text(d);
            let state = if frozen.get(&day).copied().unwrap_or(false) { "clean" } else { "missed" };
            out.days.push(StreakDay { day, state: state.into() });
            d = add_days(d, 1)?;
        }
        start = add_days(base, 1)?;
    }

    let (mut rollup_through, mut rollup_run, mut rollup_longest, mut rollup_last) =
        (through.clone(), run, longest, last_clean);
    let mut d = start;
    while d < today {
        let day = date_text(d);
        let settled = add_days(d, STREAK_GRACE)? < today;
        let clean = match frozen.get(&day) {
            Some(&clean) => clean,
            None => {
                let end = local_midnight(zone, add_days(d, 1)?).ok_or_else(|| anyhow!("date out of range"))?;
                let clean = !tasks.iter().any(|t| t.overdue_at(end));
                if settled {
                    db.exec(
                        "INSERT INTO account_streak_days(account,day,clean) VALUES($1,$2,$3) ON CONFLICT DO NOTHING",
                        &[&account, &day, &i64::from(clean)],
                    )
                    .await?;
                }
                clean
            }
        };
        let state = if clean {
            run += 1;
            out.last_day = day.clone();
            "clean"
        } else {
            run = 0;
            "missed"
        };
        out.longest = out.longest.max(run);
        if settled {
            (rollup_through, rollup_run, rollup_longest, rollup_last) =
                (day.clone(), run, out.longest, out.last_day.clone());
        }
        if d >= shown_from {
            out.days.push(StreakDay { day, state: state.into() });
        }
        d = add_days(d, 1)?;
    }
    if !rollup_through.is_empty() && rollup_through != through {
        db.exec(
            "INSERT INTO account_streak_rollups(account,through_day,run,longest,last_clean) VALUES($1,$2,$3,$4,$5) ON CONFLICT(account) DO UPDATE SET through_day=excluded.through_day,run=excluded.run,longest=excluded.longest,last_clean=excluded.last_clean",
            &[&account, &rollup_through, &rollup_run, &rollup_longest, &rollup_last],
        )
        .await?;
    }
    // Today is still in play: it counts while nothing is overdue, and an
    // overdue task can still be finished before midnight.
    out.overdue = tasks.iter().filter(|t| !t.done && t.due < now).count() as i64;
    let mut state = "overdue";
    out.current = run;
    if out.overdue == 0 {
        out.current += 1;
        out.last_day = out.today.clone();
        state = "clean";
    }
    out.longest = out.longest.max(out.current);
    out.days.push(StreakDay { day: out.today.clone(), state: state.into() });
    Ok(out)
}

/// The cached streak, when it was computed at the account's current
/// sequence, in its current zone, and `now` falls in its validity window.
async fn cached_streak(db: &dyn Sql, account: &str, now: DateTime<Utc>) -> anyhow::Result<Option<Streak>> {
    let now = now.timestamp_millis();
    let row = db
        .fetch_opt(
            "SELECT c.result FROM account_streak_cache c JOIN accounts a ON a.id=c.account LEFT JOIN account_timezones z ON z.account=a.id WHERE c.account=$1 AND c.seq=a.seq AND c.valid_from<=$2 AND c.valid_until>$3 AND c.timezone=COALESCE(z.timezone,'UTC')",
            &[&account, &now, &now],
        )
        .await?;
    let Some(row) = row else { return Ok(None) };
    let raw: String = row.try_get(0)?;
    Ok(Some(serde_json::from_str(&raw)?))
}

/// Caches `out` until the next local midnight or the next open task's due
/// time, whichever comes first.
async fn store_streak_cache(db: &dyn Sql, account: &str, out: &Streak, now: DateTime<Utc>) -> anyhow::Result<()> {
    let zone = load_location(&out.timezone).ok_or_else(|| anyhow!("unknown time zone {}", out.timezone))?;
    let tomorrow = add_days(now.with_timezone(&zone).date_naive(), 1)?;
    let mut until = local_midnight(zone, tomorrow).ok_or_else(|| anyhow!("date out of range"))?.timestamp_millis();
    for task in streak_tasks(db, account).await? {
        if !task.done && task.due >= now {
            until = until.min(task.due.timestamp_millis() + 1);
        }
    }
    let raw = serde_json::to_string(out)?;
    db.exec(
        "INSERT INTO account_streak_cache(account,seq,valid_from,valid_until,timezone,result) SELECT id,seq,$1,$2,$3,$4 FROM accounts WHERE id=$5 ON CONFLICT(account) DO UPDATE SET seq=excluded.seq,valid_from=excluded.valid_from,valid_until=excluded.valid_until,timezone=excluded.timezone,result=excluded.result",
        &[&now.timestamp_millis(), &until, &out.timezone, &raw, &account],
    )
    .await?;
    Ok(())
}

impl Server {
    /// The account's streak: the cache when valid, else computed under the
    /// account row lock (so concurrent requests cannot freeze conflicting
    /// results) and cached until the next local midnight or due time. Fails
    /// with `NoRows` for an unknown account.
    pub(crate) async fn account_streak(&self, account: &str) -> anyhow::Result<Streak> {
        let now = self.now();
        if let Some(out) = cached_streak(&self.db, account, now).await? {
            return Ok(out);
        }
        let mut client = self.db.client().await?;
        let tx = client.transaction().await.map_err(db::Error::from)?;
        if tx.exec("UPDATE accounts SET seq=seq WHERE id=$1", &[&account]).await? != 1 {
            return Err(db::Error::NoRows.into());
        }
        if let Some(out) = cached_streak(&tx, account, now).await? {
            return Ok(out);
        }
        let out = read_streak(&tx, account, now).await?;
        store_streak_cache(&tx, account, &out, now).await?;
        tx.commit().await.map_err(db::Error::from)?;
        Ok(out)
    }

    /// Queues a `streak` job for every account with a streak, so days are
    /// frozen while no device asks.
    pub(crate) async fn freeze_streaks(&self) {
        self.exec_logged(
            "INSERT INTO work_jobs(kind,key,account,run_at) SELECT 'streak',account,account,0 FROM account_streaks ON CONFLICT DO NOTHING",
            &[],
        )
        .await;
    }
}

fn no_store(mut response: Response) -> Response {
    response.headers_mut().insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

/// `GET /api/me/streak`: the account's streak.
pub(crate) async fn handle_streak(State(s): State<Arc<Server>>, Auth(se): Auth) -> ApiResult {
    match s.account_streak(&se.account).await {
        Ok(out) => ok(&out).map(no_store),
        Err(_) => fail(500, "could not read streak"),
    }
}

/// A check-in body. Visit times are no longer counted but still accepted
/// from older apps.
#[derive(Default, Deserialize)]
#[serde(default)]
struct Activity {
    inst: String,
    #[serde(deserialize_with = "null_default")]
    times: Vec<i64>,
}

/// `POST /api/me/activity`: a foreground check-in (`inst`, `times`; 8 KiB,
/// at most 200 times, none in the future beyond five minutes). The first
/// fixes the account's time zone from its school; replies with the streak.
pub(crate) async fn handle_activity(State(s): State<Arc<Server>>, Auth(se): Auth, body: Body) -> ApiResult {
    let Some(input) = read_json::<Activity>(body, 8 << 10).await.filter(|a| a.times.len() <= 200) else {
        return fail(400, "invalid activity");
    };
    let latest = (s.now() + TimeDelta::minutes(5)).timestamp();
    if input.times.iter().any(|&at| at > latest || at <= 0) {
        return fail(400, "invalid activity time");
    }
    let zone = match s.inst(&input.inst) {
        Some(inst) if !inst.timezone.is_empty() => inst.timezone.as_str(),
        _ => "UTC",
    };
    let recorded = s
        .db
        .exec(
            "INSERT INTO account_timezones(account,timezone) SELECT id,$1 FROM accounts WHERE id=$2 AND NOT EXISTS(SELECT 1 FROM account_timezones WHERE account=$3) ON CONFLICT(account) DO NOTHING",
            &[&zone, &se.account, &se.account],
        )
        .await;
    if recorded.is_ok()
        && let Ok(out) = s.account_streak(&se.account).await
    {
        return ok(&out).map(no_store);
    }
    fail(500, "could not record activity")
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;
    use serde_json::json;
    use tokio_util::sync::CancellationToken;

    use super::*;
    use crate::test_support::test_server;
    use crate::test_support::{Signed, signed_in_browser};

    fn expect(out: &Value, current: i64, longest: i64, overdue: i64) {
        assert!(
            out["current"] == current && out["longest"] == longest && out["overdue"] == overdue,
            "want current {current}, longest {longest}, overdue {overdue}: {out}"
        );
    }

    #[tokio::test]
    async fn account_streak_counts_days_with_nothing_overdue() {
        let utc = |y, m, d, h| Utc.with_ymd_and_hms(y, m, d, h, 0, 0).unwrap();
        let mut t = test_server().await;
        // Chicago is UTC-5 after the March 8 change, so its midnight is 05:00 UTC.
        let mut now = utc(2026, 3, 10, 18);
        t.server.clock.set(now);
        Arc::get_mut(&mut t.server).expect("an unshared test server").insts[0].timezone = "America/Chicago".into();
        let s = t.server.clone();
        let c = signed_in_browser(&t).await;
        let native = c.native(&t);
        let inst = s.insts[0].id.clone();
        let check_in = |c: Signed, inst: String| async move {
            c.must("POST", "/api/me/activity", &json!({ "inst": inst }).to_string(), 200).await
        };
        let task = |id: &str, due: &str, done: bool, completed: &str| {
            let mut body = json!({ "id": id, "title": id, "due": due, "done": done });
            if !completed.is_empty() {
                body["completedAt"] = json!(completed);
            }
            (format!("t_{id}"), body.to_string())
        };
        let put_task = async |doc: (String, String)| {
            s.db.exec(
                "INSERT INTO docs(account,id,type,seq,updated_at,body) VALUES($1,$2,'task',0,0,$3) ON CONFLICT(account,id) DO UPDATE SET body=excluded.body",
                &[&c.account, &doc.0, &doc.1],
            )
            .await
            .unwrap();
        };

        // Nothing is due, so today already counts. Another device cannot move the zone.
        let out = check_in(c.clone(), inst.clone()).await;
        expect(&out, 1, 1, 0);
        assert!(out["timezone"] == "America/Chicago" && out["today"] == "2026-03-10", "{out}");
        let out = native
            .must("POST", "/api/me/activity", &format!(r#"{{"inst":"unknown","times":[{}]}}"#, now.timestamp()), 200)
            .await;
        assert_eq!(out["timezone"], "America/Chicago", "{out}");

        // A task sent through sync counts once its due time passes; finishing
        // it late but before midnight keeps the day.
        c.must("POST", "/api/me/sync", r#"{"docs":[{"id":"t_a","type":"task","body":{"id":"a","title":"Essay","due":"2026-03-10T20:00:00.000Z","done":false}}]}"#, 200).await;
        expect(&c.must("GET", "/api/me/streak", "", 200).await, 1, 1, 0);
        now = utc(2026, 3, 10, 21);
        s.clock.set(now);
        let out = c.must("GET", "/api/me/streak", "", 200).await;
        expect(&out, 0, 0, 1);
        let days = out["days"].as_array().unwrap();
        assert!(days.len() == 1 && days[0]["state"] == "overdue", "{out}");
        put_task(task("a", "2026-03-10T20:00:00.000Z", true, "2026-03-10T22:00:00.000Z")).await;
        expect(&check_in(c.clone(), inst.clone()).await, 1, 1, 0);
        s.clock.set(utc(2026, 3, 11, 18));
        expect(&check_in(c.clone(), inst.clone()).await, 2, 2, 0);

        // A task still open at midnight loses the day, even when finished the
        // next morning. The day looked clean earlier, so the run it seemed to
        // extend never happened.
        put_task(task("b", "2026-03-11T20:00:00Z", false, "")).await;
        s.clock.set(utc(2026, 3, 12, 18));
        expect(&check_in(c.clone(), inst.clone()).await, 0, 1, 1);
        put_task(task("b", "2026-03-11T20:00:00Z", true, "2026-03-12T17:00:00Z")).await;
        expect(&check_in(c.clone(), inst.clone()).await, 1, 1, 0);

        // A completion made offline counts toward the day it happened once it syncs.
        put_task(task("c", "2026-03-12T23:00:00Z", false, "")).await;
        s.clock.set(utc(2026, 3, 13, 15));
        expect(&check_in(c.clone(), inst.clone()).await, 0, 1, 1);
        put_task(task("c", "2026-03-12T23:00:00Z", true, "2026-03-12T23:30:00Z")).await;
        expect(&check_in(c.clone(), inst.clone()).await, 2, 2, 0);
        // A completion without a time, as imports report, counts as on time.
        put_task(task("d", "2026-03-13T16:00:00Z", true, "")).await;
        expect(&check_in(c.clone(), inst.clone()).await, 2, 2, 0);

        // Finished days freeze after the grace period: a later edit cannot rewrite them.
        now = utc(2026, 3, 16, 18);
        s.clock.set(now);
        let out = check_in(c.clone(), inst.clone()).await;
        expect(&out, 5, 5, 0);
        put_task(task("b", "2026-03-11T20:00:00Z", true, "2026-03-11T21:00:00Z")).await;
        expect(&check_in(c.clone(), inst.clone()).await, 5, 5, 0);
        let frozen = async || -> i64 {
            s.db.fetch_one("SELECT COUNT(*) FROM account_streak_days WHERE account=$1", &[&c.account])
                .await
                .unwrap()
                .get(0)
        };
        assert_eq!(frozen().await, 4);
        let days = out["days"].as_array().unwrap();
        assert!(days.len() == 7 && days[1]["state"] == "missed" && days[6]["day"] == "2026-03-16", "{days:?}");
        // Concurrent check-ins settle the same days once.
        let mut set = tokio::task::JoinSet::new();
        for _ in 0..8 {
            set.spawn(check_in(c.clone(), inst.clone()));
        }
        while let Some(result) = set.join_next().await {
            result.unwrap();
        }
        assert_eq!(frozen().await, 4);
        c.must(
            "POST",
            "/api/me/activity",
            &format!(r#"{{"times":[{}]}}"#, (now + TimeDelta::hours(1)).timestamp()),
            400,
        )
        .await;
        // The hourly sweep settles days while no device asks.
        s.clock.set(utc(2026, 3, 20, 18));
        s.sweep().await;
        s.drain_work(&CancellationToken::new(), &["streak"], 100, |job| {
            let s = s.clone();
            async move { s.account_streak(&job.account).await.map(drop) }
        })
        .await;
        assert_eq!(frozen().await, 8);
    }
}
