//! The reminder index: `reminder_schedule` rows hold each registration's
//! items and next due time, rebuilt through `reminder` work jobs after
//! edits and once a day. A daily rebuild also refreshes catalog metadata
//! and lets a new device's push registration take effect.

use anyhow::anyhow;
use chrono::{DateTime, Datelike, TimeDelta, Utc};
use tokio_util::sync::CancellationToken;

use crate::Server;
use crate::db::{self, Sql};
use crate::ical::{civil_instant, load_location};
use crate::jobs::WorkJob;
use crate::reminders::{REMINDER_LEAD, Reminder, parse_items, weekday_letter};
use crate::session::new_id;
use crate::streaks::parse_instant;

/// The next instant (Unix seconds) a reminder may fire after `now`,
/// looking two days ahead without walking every minute; at most a day
/// away, an hour away for a bad zone.
pub(crate) fn next_reminder_check(rem: &Reminder, now: DateTime<Utc>) -> i64 {
    let Some(zone) = load_location(&rem.tz) else { return (now + TimeDelta::hours(1)).timestamp() };
    let lead = TimeDelta::seconds(REMINDER_LEAD.as_secs() as i64);
    let mut next = now + TimeDelta::hours(24);
    let truncate = |t: DateTime<Utc>| DateTime::from_timestamp(t.timestamp().div_euclid(60) * 60, 0).unwrap_or(t);
    let minute = truncate(now);
    let today = now.with_timezone(&zone).date_naive();
    for it in &rem.items {
        if !it.at.is_empty() {
            if let Some(due) = parse_instant(&it.at) {
                let at = truncate(due - TimeDelta::hours(1));
                if at > minute && at < next {
                    next = at;
                }
            }
            continue;
        }
        for offset in 0..=2 {
            let Some(d) = today.checked_add_signed(TimeDelta::days(offset)) else { continue };
            let date = d.format("%Y-%m-%d").to_string();
            if !it.days.contains(weekday_letter(d.weekday()))
                || (!it.from.is_empty() && date < it.from)
                || (!it.to.is_empty() && date > it.to)
                || it.skip.contains(&date)
            {
                continue;
            }
            let Some(start) = civil_instant(
                zone,
                i64::from(d.year()),
                i64::from(d.month()),
                i64::from(d.day()),
                it.start / 60,
                it.start % 60,
                0,
            ) else {
                continue;
            };
            let at = start - lead;
            if at > minute && at < next {
                next = at;
            }
        }
    }
    next.timestamp()
}

impl Server {
    /// Rebuilds one schedule row from an account's documents (`acct:` keys)
    /// or a guest registration, deleting it when reminders are off or the
    /// registration is gone. Fails when the account or job changed during
    /// derivation, so the job retries.
    pub(crate) async fn rebuild_reminder(&self, job: WorkJob) -> anyhow::Result<()> {
        let mut rem = Reminder { id: job.key.clone(), account: job.account.clone(), ..Reminder::default() };
        let mut seq = 0i64;
        if !job.account.is_empty() {
            let row = self
                .db
                .fetch_opt(
                    "SELECT a.seq,COALESCE((SELECT 1 FROM docs WHERE account=a.id AND id='prefs' AND deleted=0 AND json_text(body,'$.reminders')='true'),0) FROM accounts a WHERE a.id=$1",
                    &[&job.account],
                )
                .await?;
            let enabled = match &row {
                Some(row) => {
                    seq = row.try_get(0)?;
                    row.try_get::<_, i32>(1)? != 0
                }
                None => false,
            };
            if !enabled {
                self.db.exec("DELETE FROM reminder_schedule WHERE id=$1", &[&job.key]).await?;
                return Ok(());
            }
            (rem.tz, rem.items) = self.derive_reminders(&job.account).await?;
            let current: i64 =
                self.db.fetch_one("SELECT seq FROM accounts WHERE id=$1", &[&job.account]).await?.try_get(0)?;
            if seq != current {
                return Err(anyhow!("context canceled"));
            }
        } else {
            let row = self.db.fetch_opt("SELECT target,tz,items FROM reminders WHERE id=$1", &[&job.key]).await?;
            let Some(row) = row else {
                self.db.exec("DELETE FROM reminder_schedule WHERE id=$1", &[&job.key]).await?;
                return Ok(());
            };
            rem.target = row.try_get(0)?;
            rem.tz = row.try_get(1)?;
            let items: String = row.try_get(2)?;
            rem.items = parse_items(&items)?;
        }
        let raw = serde_json::to_string(&rem.items)?;
        // Run immediately once to cover a registration or edit in the existing
        // five-minute catch-up window, then use the indexed next occurrence.
        let mut client = self.db.client().await?;
        let tx = client.transaction().await.map_err(db::Error::from)?;
        let row = tx
            .fetch_one(
                "SELECT generation,owner FROM work_jobs WHERE kind='reminder' AND key=$1 FOR UPDATE",
                &[&job.key],
            )
            .await?;
        let (generation, owner): (i64, String) = (row.try_get(0)?, row.try_get(1)?);
        if generation != job.generation || owner != job.owner {
            return Err(anyhow!("reminder changed during derivation"));
        }
        let now = self.now().timestamp();
        tx.exec(
            "INSERT INTO reminder_schedule(id,account,seq,target,tz,items,next_at,version,built_at) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9) ON CONFLICT(id) DO UPDATE SET account=excluded.account,seq=excluded.seq,target=excluded.target,tz=excluded.tz,items=excluded.items,next_at=excluded.next_at,version=excluded.version,built_at=excluded.built_at",
            &[&rem.id, &rem.account, &seq, &rem.target, &rem.tz, &raw, &now, &new_id(), &now],
        )
        .await?;
        tx.commit().await.map_err(db::Error::from)?;
        Ok(())
    }

    /// Drains up to 100 rebuild jobs, then claims and matches up to 100 due
    /// schedule rows, until none are due or `stop` fires.
    pub(crate) async fn tick_scheduled_reminders(&self, stop: &CancellationToken) {
        self.drain_work(stop, &["reminder"], 100, |job| self.rebuild_reminder(job)).await;
        for _ in 0..100 {
            if stop.is_cancelled() {
                return;
            }
            // The claim keeps other workers off this row for a minute. A
            // failed queue write leaves it due for retry; delivery dedupe
            // records are committed with the push jobs.
            let now = self.now().timestamp();
            let claimed = self
                .db
                .fetch_opt(
                    "UPDATE reminder_schedule SET next_at=$1 WHERE id=(SELECT id FROM reminder_schedule WHERE next_at<=$2 ORDER BY next_at LIMIT 1 FOR UPDATE SKIP LOCKED) RETURNING id,account,seq,target,tz,items,version,built_at",
                    &[&(now + 60), &now],
                )
                .await;
            let row = match claimed {
                Ok(Some(row)) => row,
                Ok(None) => return,
                Err(err) => {
                    tracing::warn!(%err, "reminder schedule unavailable");
                    return;
                }
            };
            let read = || -> Result<(Reminder, i64, String, String, i64), tokio_postgres::Error> {
                let rem = Reminder {
                    id: row.try_get(0)?,
                    account: row.try_get(1)?,
                    target: row.try_get(3)?,
                    tz: row.try_get(4)?,
                    ..Reminder::default()
                };
                Ok((rem, row.try_get(2)?, row.try_get(5)?, row.try_get(6)?, row.try_get(7)?))
            };
            let (mut rem, seq, raw, version, built) = match read() {
                Ok(values) => values,
                Err(err) => {
                    tracing::warn!(%err, "reminder schedule unavailable");
                    return;
                }
            };
            if self.now().timestamp() - built >= 86400 {
                self.exec_logged(
                    "INSERT INTO work_jobs(kind,key,account,run_at) VALUES('reminder',$1,$2,0) ON CONFLICT DO NOTHING",
                    &[&rem.id, &rem.account],
                )
                .await;
                continue;
            }
            if !rem.account.is_empty() {
                let current = self
                    .db
                    .fetch_one("SELECT seq FROM accounts WHERE id=$1", &[&rem.account])
                    .await
                    .and_then(|row| Ok(row.try_get::<_, i64>(0)?));
                if !matches!(current, Ok(current) if current == seq) {
                    continue;
                }
            }
            let Ok(items) = parse_items(&raw) else { continue };
            rem.items = items;
            if self.match_reminders(&rem, self.now()).await.is_err() {
                continue;
            }
            let next = next_reminder_check(&rem, self.now());
            self.exec_logged(
                "UPDATE reminder_schedule SET next_at=$1 WHERE id=$2 AND version=$3",
                &[&next, &rem.id, &version],
            )
            .await;
        }
    }

    /// Queues rebuilds for registrations and accounts that have no schedule
    /// row yet, once at worker startup. Later edits queue rebuilds
    /// transactionally through database triggers.
    pub(crate) async fn seed_reminder_schedules(&self) -> db::Result<()> {
        for q in [
            "INSERT INTO work_jobs(kind,key,account,run_at) SELECT 'reminder','acct:'||account,account,$1 FROM docs WHERE id='prefs' AND deleted=0 AND json_text(body,'$.reminders')='true' AND NOT EXISTS(SELECT 1 FROM reminder_schedule WHERE id='acct:'||docs.account) ON CONFLICT DO NOTHING",
            "INSERT INTO work_jobs(kind,key,run_at) SELECT 'reminder',id,$1 FROM reminders WHERE NOT EXISTS(SELECT 1 FROM reminder_schedule WHERE reminder_schedule.id=reminders.id) ON CONFLICT DO NOTHING",
        ] {
            self.db.exec(q, &[&self.now().timestamp()]).await?;
        }
        Ok(())
    }
}
