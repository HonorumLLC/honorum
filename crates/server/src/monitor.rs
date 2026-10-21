//! Catalog collection and seat polling. Each school runs a catalog job and
//! a seat job with independent due times and failure backoff; global slots
//! (two catalog, four seat) reserve capacity for seat checks while large
//! catalog jobs run.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use chrono::{DateTime, NaiveDate, TimeDelta, Utc};
use honorum_catalog::{Exam, Meeting, Seats, Section, Term};
use serde::Serialize;
use tokio::sync::Semaphore;
use tokio_util::sync::CancellationToken;

use crate::Server;
use crate::db::Sql;
use crate::snapshot::{compact_float, parse_section};
use crate::state::Institution;

/// Global catalog collection slots.
pub(crate) const CATALOG_SLOTS: usize = 2;
/// Global seat polling slots.
pub(crate) const SEAT_SLOTS: usize = 4;

/// The latest term end date (`YYYY-MM-DD`) that counts as over: a day
/// before `now`, so no campus time zone ends a term early.
pub(crate) fn term_ended(now: DateTime<Utc>) -> String {
    (now - TimeDelta::days(1)).format("%Y-%m-%d").to_string()
}

/// Reports a catalog of at least 200 sections that publishes seat counts yet
/// has no full section while registration has started, which suggests an
/// adapter filtering by availability. Adapters must return every section;
/// "open seats only" is a client checkbox, never a server-side query flag.
pub(crate) fn suspiciously_open(secs: &[Section]) -> bool {
    if secs.len() < 200 {
        return false;
    }
    let (mut known, mut full, mut started) = (0, 0, 0);
    for s in secs {
        if s.available >= 0 && s.capacity > 0 {
            known += 1;
            if s.available == 0 {
                full += 1;
            }
            if s.available < s.capacity {
                // Someone has enrolled, so registration is open.
                started += 1;
            }
        }
    }
    // A term nobody has enrolled in yet is legitimately all open.
    known >= 200 && full == 0 && started > known / 10
}

/// Puts the term containing `now` first, then upcoming terms by start date,
/// then past ones in their given (newest-first) order. Undated lists are
/// reversed, since adapters without dates list newest first and the current
/// term should be reached soonest.
pub(crate) fn order_terms(terms: &[Term], now: DateTime<Utc>) -> Vec<Term> {
    let mut out = terms.to_vec();
    if out.first().is_none_or(|t| t.start.is_empty()) {
        out.reverse();
        return out;
    }
    let today = now.format("%Y-%m-%d").to_string();
    let rank = |t: &Term| -> u8 {
        if t.start <= today && (t.end.is_empty() || today <= t.end) {
            0
        } else if t.start > today {
            1
        } else {
            2
        }
    };
    out.sort_by(|a, b| {
        let (ra, rb) = (rank(a), rank(b));
        if ra != rb {
            ra.cmp(&rb)
        } else if ra == 1 {
            a.start.cmp(&b.start)
        } else {
            std::cmp::Ordering::Equal
        }
    });
    out
}

/// The most frequent section date range, for adapters that report no term
/// dates; empty strings when no section has dates.
pub(crate) fn common_dates(secs: &[Section]) -> (String, String) {
    let mut count: HashMap<(&str, &str), i64> = HashMap::new();
    let mut best: (&str, &str) = ("", "");
    for s in secs {
        if s.start.is_empty() {
            continue;
        }
        let k = (s.start.as_str(), s.end.as_str());
        let n = {
            let n = count.entry(k).or_default();
            *n += 1;
            *n
        };
        if n > count.get(&best).copied().unwrap_or(0) {
            best = k;
        }
    }
    (best.0.to_owned(), best.1.to_owned())
}

/// The stored form of a section, without seat counts. Stored rows are
/// compared as text to detect changes and were written in this field order,
/// so reordering fields rewrites every stored section.
#[derive(Serialize)]
struct StoredStructure<'a> {
    id: &'a str,
    course: &'a str,
    subject: &'a str,
    title: &'a str,
    num: &'a str,
    #[serde(rename = "type")]
    kind: &'a str,
    #[serde(skip_serializing_if = "str::is_empty")]
    link: &'a str,
    instructors: &'a [String],
    meetings: &'a [Meeting],
    #[serde(skip_serializing_if = "<[Exam]>::is_empty")]
    exams: &'a [Exam],
    #[serde(skip_serializing_if = "is_zero", serialize_with = "compact_float")]
    credits: &'a f64,
    #[serde(skip_serializing_if = "str::is_empty")]
    start: &'a str,
    #[serde(skip_serializing_if = "str::is_empty")]
    end: &'a str,
    #[serde(skip_serializing_if = "str::is_empty")]
    mode: &'a str,
    #[serde(skip_serializing_if = "<[String]>::is_empty")]
    attributes: &'a [String],
    #[serde(skip_serializing_if = "str::is_empty")]
    description: &'a str,
    #[serde(skip_serializing_if = "str::is_empty")]
    prereqs: &'a str,
    #[serde(skip_serializing_if = "str::is_empty")]
    url: &'a str,
}

fn is_zero(v: &&f64) -> bool {
    **v == 0.0
}

/// The stored section JSON: the section without seat counts, which live in
/// `section_seats`. Fails on values JSON cannot hold, such as NaN credits.
pub(crate) fn section_structure(sec: &Section) -> serde_json::Result<String> {
    serde_json::to_string(&StoredStructure {
        id: &sec.id,
        course: &sec.course,
        subject: &sec.subject,
        title: &sec.title,
        num: &sec.number,
        kind: &sec.kind,
        link: &sec.link,
        instructors: &sec.instructors,
        meetings: &sec.meetings,
        exams: &sec.exams,
        credits: &sec.credits,
        start: &sec.start,
        end: &sec.end,
        mode: &sec.mode,
        attributes: &sec.attributes,
        description: &sec.description,
        prereqs: &sec.prereqs,
        url: &sec.source_url,
    })
}

/// A stored section row and its seat counts.
struct Stored {
    data: String,
    capacity: i64,
    available: i64,
    waitlist: i64,
}

/// The `kv` key of an adapter's saved routing state.
fn state_key(inst: &Institution, version: &str) -> String {
    format!("adapter:{}:{version}", inst.id)
}

impl Server {
    /// Runs every school's catalog and seat jobs until `stop` fires. Adapter
    /// state saved in `kv` warms a school's catalog job.
    ///
    /// Catalog and seat jobs have independent due times and failure backoff.
    /// The global slots reserve capacity for seat checks while large catalog
    /// jobs run.
    pub(crate) async fn run_monitor(self: &Arc<Self>, stop: CancellationToken) {
        let catalogs = Arc::new(Semaphore::new(CATALOG_SLOTS));
        let seats = Arc::new(Semaphore::new(SEAT_SLOTS));
        let mut workers = tokio::task::JoinSet::new();
        for inst in &self.insts {
            let Some(adapter) = &inst.adapter else { continue };
            let mut warm = true;
            if let Some(state) = adapter.state() {
                let data = self.kv_get(&state_key(inst, adapter.version())).await;
                warm = !data.is_empty() && state.load_state(data.as_bytes()).is_ok();
            }
            for kind in [JobKind::Catalog, JobKind::Seats] {
                let (s, inst, stop) = (self.clone(), inst.clone(), stop.clone());
                let (catalogs, seats) = (catalogs.clone(), seats.clone());
                workers.spawn(async move { s.job_loop(&inst, kind, warm, &catalogs, &seats, &stop).await });
            }
        }
        while workers.join_next().await.is_some() {}
    }

    async fn job_loop(
        &self,
        inst: &Institution,
        kind: JobKind,
        mut warm: bool,
        catalogs: &Semaphore,
        seats: &Semaphore,
        stop: &CancellationToken,
    ) {
        let key = format!("next:{}:{}", kind.name(), inst.id);
        let mut next: i64 = self.kv_get(&key).await.parse().unwrap_or(0);
        if kind == JobKind::Catalog && !warm {
            next = 0;
        }
        let mut failures: u32 = 0;
        let interval = match kind {
            JobKind::Catalog => Duration::from_secs(self.cfg.catalog_hours.max(0) as u64 * 3600),
            JobKind::Seats => self.poll_interval(inst),
        };
        loop {
            let wait = Duration::from_secs((next - Utc::now().timestamp()).max(0) as u64);
            tokio::select! {
                () = stop.cancelled() => return,
                () = tokio::time::sleep(wait) => {}
            }
            let job = async {
                match kind {
                    JobKind::Catalog => self.sync_catalog(inst, Some(catalogs), !warm).await,
                    JobKind::Seats => self.with_collection_slot(Some(seats), self.poll(inst)).await,
                }
            };
            let result = tokio::select! {
                () = stop.cancelled() => return,
                result = job => result,
            };
            if kind == JobKind::Catalog && result.is_ok() {
                warm = true;
            }
            if stop.is_cancelled() {
                return;
            }
            let mut delay = interval;
            let message = result.as_ref().err().map(|err| format!("{err:#}"));
            if let Some(err) = &message {
                failures += 1;
                delay = (self.poll_interval(inst) * (1 << failures.min(6))).min(Duration::from_secs(3600));
                tracing::warn!(inst = %inst.id, kind = kind.name(), %err, "background job failed");
            } else {
                failures = 0;
            }
            // Availability health is independent of catalog failures.
            if kind == JobKind::Seats || message.is_some() {
                self.record(&inst.id, message.as_deref()).await;
            }
            next = Utc::now().timestamp() + delay.as_secs() as i64;
            self.kv_set(&key, &next.to_string()).await;
        }
    }

    /// Notes what the monitor is doing without touching health flags.
    pub(crate) async fn progress(&self, inst: &str, message: &str) {
        self.exec_logged(
            "INSERT INTO adapter_status(inst,ok,message) VALUES($1,0,$2) ON CONFLICT(inst) DO UPDATE SET message=excluded.message",
            &[&inst, &message],
        )
        .await;
    }

    /// Records a job result in `adapter_status`: healthy on `None`; on an
    /// error, its message, the failure count, and a pause of the poll
    /// interval doubled per failure up to an hour. Invalidates the status
    /// cache.
    pub(crate) async fn record(&self, inst: &str, err: Option<&str>) {
        self.invalidate_status();
        let now = Utc::now().timestamp();
        let Some(err) = err else {
            self.exec_logged(
                "INSERT INTO adapter_status(inst,ok,message,last_ok,failures,paused_until) VALUES($1,1,'ok',$2,0,0)
			ON CONFLICT(inst) DO UPDATE SET ok=1,message='ok',last_ok=$3,failures=0,paused_until=0",
                &[&inst, &now, &now],
            )
            .await;
            return;
        };
        let prior = match self.db.fetch_opt("SELECT failures FROM adapter_status WHERE inst=$1", &[&inst]).await {
            Ok(Some(row)) => row.try_get::<_, Option<i64>>(0).ok().flatten().unwrap_or(0),
            _ => 0,
        };
        let failures = prior + 1;
        let poll = Duration::from_secs(self.cfg.poll_seconds.max(0) as u64);
        let backoff = (poll * (1 << failures.min(6))).min(Duration::from_secs(3600));
        tracing::warn!(%inst, %err, failures, backoff = ?backoff, "adapter error");
        let paused_until = now + backoff.as_secs() as i64;
        self.exec_logged(
            "INSERT INTO adapter_status(inst,ok,message,failures,paused_until) VALUES($1,0,$2,$3,$4)
		ON CONFLICT(inst) DO UPDATE SET ok=0,message=excluded.message,failures=excluded.failures,paused_until=excluded.paused_until",
            &[&inst, &err, &failures, &paused_until],
        )
        .await;
    }

    /// The seat poll interval: the school's `poll_seconds`, else the
    /// server's, but never below the adapter's minimum.
    pub(crate) fn poll_interval(&self, inst: &Institution) -> Duration {
        let mut interval = Duration::from_secs(self.cfg.poll_seconds.max(0) as u64);
        if let Some(cfg) = self.cfg.institutions.iter().find(|c| c.id == inst.id && c.poll_seconds > 0) {
            interval = Duration::from_secs(cfg.poll_seconds as u64);
        }
        if let Some(minimum) = inst.adapter.as_ref().and_then(|a| a.minimum_poll_interval()) {
            interval = interval.max(minimum);
        }
        interval
    }

    /// Runs `job` holding one of `slots`, when given, and counts the wait.
    async fn with_collection_slot<T>(&self, slots: Option<&Semaphore>, job: impl std::future::Future<Output = T>) -> T {
        let Some(slots) = slots else { return job.await };
        let started = Instant::now();
        let _permit = slots.acquire().await.expect("collection slots are never closed");
        self.counters.collection_queue_nanos.fetch_add(started.elapsed().as_nanos() as i64, Ordering::Relaxed);
        job.await
    }

    /// Reports whether an archived term (ended over two weeks ago) was
    /// collected within `archive_catalog_hours` (default 168). Terms with
    /// unknown end dates keep ordinary refresh timing; their age is never
    /// guessed from provider ids.
    async fn archived_term_fresh(&self, inst: &Institution, t: &Term) -> bool {
        let row = self
            .db
            .fetch_opt(r#"SELECT updated_at,COALESCE("end",'') FROM terms WHERE inst=$1 AND id=$2"#, &[&inst.id, &t.id])
            .await;
        let Ok(Some(row)) = row else { return false };
        let (Ok(Some(checked)), Ok(stored_end)) = (row.try_get::<_, Option<i64>>(0), row.try_get::<_, String>(1))
        else {
            return false;
        };
        if checked <= 0 {
            return false;
        }
        let end = if t.end.is_empty() { stored_end } else { t.end.clone() };
        let Ok(end) = NaiveDate::parse_from_str(&end, "%Y-%m-%d") else { return false };
        let now = self.now();
        let end = end.and_hms_opt(0, 0, 0).expect("midnight exists").and_utc();
        if end + TimeDelta::days(14) >= now {
            return false;
        }
        let hours = if self.cfg.archive_catalog_hours == 0 { 168 } else { self.cfg.archive_catalog_hours };
        now.timestamp() - checked < hours * 3600
    }

    /// Collects every term of one school, holding a slot of `slots` (when
    /// given) per adapter call. Unless `force`, recently checked archived
    /// terms are skipped. Republishes every published term afterwards and
    /// returns the first failure.
    ///
    /// The slot is released between terms, letting current terms at other
    /// schools make progress.
    pub(crate) async fn sync_catalog(
        &self,
        inst: &Institution,
        slots: Option<&Semaphore>,
        force: bool,
    ) -> anyhow::Result<()> {
        let Some(adapter) = inst.adapter.as_ref() else { return Ok(()) };
        let terms =
            self.with_collection_slot(slots, adapter.terms()).await.map_err(|err| anyhow::anyhow!("terms: {err}"))?;
        let terms = order_terms(&terms, self.now());
        for t in &terms {
            self.db
                .exec(
                    r#"INSERT INTO terms(inst,id,name,start,"end",updated_at) VALUES($1,$2,$3,$4,$5,0) ON CONFLICT(inst,id) DO UPDATE SET name=excluded.name,start=CASE WHEN excluded.start<>'' THEN excluded.start ELSE terms.start END,"end"=CASE WHEN excluded."end"<>'' THEN excluded."end" ELSE terms."end" END"#,
                    &[&inst.id, &t.id, &t.name, &t.start, &t.end],
                )
                .await?;
        }
        self.invalidate_public_config();
        let mut first_error: Option<anyhow::Error> = None;
        for t in terms {
            if !force && self.archived_term_fresh(inst, &t).await {
                continue;
            }
            let result = self
                .with_collection_slot(slots, async {
                    let mut t = t;
                    self.progress(&inst.id, &format!("fetching {}", t.name)).await;
                    let secs =
                        adapter.catalog(&t.id).await.map_err(|err| anyhow::anyhow!("catalog {}: {err}", t.id))?;
                    if t.start.is_empty() {
                        (t.start, t.end) = common_dates(&secs);
                    }
                    if suspiciously_open(&secs) {
                        tracing::warn!(inst = %inst.id, term = %t.id, sections = secs.len(), "catalog has no full sections; is the adapter filtering by availability?");
                    }
                    self.store_catalog(inst, &t, &secs).await?;
                    if let Some(state) = adapter.state()
                        && let Ok(data) = state.save_state()
                    {
                        self.kv_set(&state_key(inst, adapter.version()), &String::from_utf8_lossy(&data)).await;
                    }
                    self.write_snapshot(&inst.id, &t.id).await
                })
                .await;
            if let Err(err) = result
                && first_error.is_none()
            {
                first_error = Some(err);
            }
        }
        // The last collected term can change another term's offered-course
        // metadata. Recheck all already-published terms after the batch;
        // unchanged revisions skip.
        let rows = self.db.fetch_all("SELECT id FROM terms WHERE inst=$1 AND updated_at>0", &[&inst.id]).await?;
        let mut published = Vec::with_capacity(rows.len());
        for row in rows {
            published.push(row.try_get::<_, String>(0)?);
        }
        for term in published {
            if let Err(err) = self.write_snapshot(&inst.id, &term).await
                && first_error.is_none()
            {
                first_error = Some(err);
            }
        }
        first_error.map_or(Ok(()), Err)
    }

    /// Stores one term's sections: changed structure JSON, changed seat
    /// counts, removed sections, and seat-open alerts for sections that
    /// went from zero to available, in one transaction under the write lock.
    /// Section JSON is prepared before the transaction; seat changes never
    /// rewrite it.
    pub(crate) async fn store_catalog(&self, inst: &Institution, term: &Term, secs: &[Section]) -> anyhow::Result<()> {
        let _write = self.write_lock.lock().await;
        let now = self.now().timestamp();
        let rows = self
            .db
            .fetch_all(
                "SELECT s.id,s.data,COALESCE(a.capacity,0),COALESCE(a.available,-1),COALESCE(a.waitlist,0) FROM sections s LEFT JOIN section_seats a ON a.inst=s.inst AND a.term=s.term AND a.id=s.id WHERE s.inst=$1 AND s.term=$2",
                &[&inst.id, &term.id],
            )
            .await?;
        let mut existing: HashMap<String, Stored> = HashMap::with_capacity(rows.len());
        for row in rows {
            existing.insert(
                row.try_get(0)?,
                Stored {
                    data: row.try_get::<_, Option<String>>(1)?.unwrap_or_default(),
                    capacity: row.try_get(2)?,
                    available: row.try_get(3)?,
                    waitlist: row.try_get(4)?,
                },
            );
        }
        let mut changed: Vec<(&Section, String)> = Vec::new();
        let mut seats: Vec<&Section> = Vec::new();
        let mut opened: Vec<&Section> = Vec::new();
        let mut present = std::collections::HashSet::with_capacity(secs.len());
        for sec in secs {
            present.insert(sec.id.as_str());
            let body = section_structure(sec)?;
            let old = existing.get(&sec.id);
            if old.is_none_or(|o| o.data != body) {
                changed.push((sec, body));
            }
            if old.is_none_or(|o| {
                o.capacity != sec.capacity || o.available != sec.available || o.waitlist != sec.waitlist
            }) {
                seats.push(sec);
            }
            if old.is_some_and(|o| o.available == 0) && sec.available > 0 {
                opened.push(sec);
            }
        }
        let mut client = self.db.client().await?;
        let tx = client.transaction().await?;
        tx.exec(
            r#"INSERT INTO terms(inst,id,name,start,"end",updated_at) VALUES($1,$2,$3,$4,$5,$6) ON CONFLICT(inst,id) DO UPDATE SET name=excluded.name,start=excluded.start,"end"=excluded."end",updated_at=excluded.updated_at"#,
            &[&inst.id, &term.id, &term.name, &term.start, &term.end, &now],
        )
        .await?;
        for (sec, data) in &changed {
            tx.exec(
                "INSERT INTO sections(inst,term,id,data) VALUES($1,$2,$3,$4) ON CONFLICT(inst,term,id) DO UPDATE SET data=excluded.data",
                &[&inst.id, &term.id, &sec.id, data],
            )
            .await?;
        }
        for sec in &seats {
            tx.exec(
                "INSERT INTO section_seats(inst,term,id,capacity,available,waitlist,checked_at,changed_at) VALUES($1,$2,$3,$4,$5,$6,0,$7) ON CONFLICT(inst,term,id) DO UPDATE SET capacity=excluded.capacity,available=excluded.available,waitlist=excluded.waitlist,checked_at=0,changed_at=excluded.changed_at",
                &[&inst.id, &term.id, &sec.id, &sec.capacity, &sec.available, &sec.waitlist, &now],
            )
            .await?;
        }
        for id in existing.keys() {
            if !present.contains(id.as_str()) {
                tx.exec("DELETE FROM sections WHERE inst=$1 AND term=$2 AND id=$3", &[&inst.id, &term.id, id]).await?;
            }
        }
        for sec in opened {
            self.queue_open(&tx, inst, &term.id, &sec.id, sec.available, &sec.course, &sec.number, &sec.title, now)
                .await?;
        }
        tx.commit().await?;
        self.invalidate_public_config();
        Ok(())
    }

    /// Fetches availability for watched sections of terms that have not
    /// ended, once per section; one term failing does not stop the others.
    /// Watches of ended terms are left for the sweep to remove.
    pub(crate) async fn poll(&self, inst: &Institution) -> anyhow::Result<()> {
        let Some(adapter) = inst.adapter.as_ref() else { return Ok(()) };
        let rows = self
            .db
            .fetch_all(
                r#"SELECT DISTINCT w.term, w.section FROM watches w LEFT JOIN terms t ON t.inst=w.inst AND t.id=w.term WHERE w.inst=$1 AND (COALESCE(t."end",'')='' OR t."end">=$2)"#,
                &[&inst.id, &term_ended(self.now())],
            )
            .await?;
        let mut by_term: HashMap<String, Vec<String>> = HashMap::new();
        for row in rows {
            let term: String = row.try_get::<_, Option<String>>(0)?.unwrap_or_default();
            let id: String = row.try_get::<_, Option<String>>(1)?.unwrap_or_default();
            by_term.entry(term).or_default().push(id);
        }
        let mut failed = Vec::new();
        for (term, ids) in by_term {
            let seats = match adapter.availability(&term, &ids).await {
                Ok(seats) => seats,
                Err(err) => {
                    failed.push(format!("availability {term}: {err}"));
                    continue;
                }
            };
            self.store_seats(inst, &term, &seats).await?;
            if let Some(state) = adapter.state()
                && let Ok(data) = state.save_polling_state()
                && !data.is_empty()
            {
                self.kv_set(&state_key(inst, adapter.version()), &String::from_utf8_lossy(&data)).await;
            }
        }
        if !failed.is_empty() {
            anyhow::bail!(failed.join("\n"));
        }
        Ok(())
    }

    /// Stores polled seat counts and queues seat-open alerts in one
    /// transaction, then republishes the seat snapshot. Only the small seat
    /// object is republished; structure waits for the catalog job.
    pub(crate) async fn store_seats(&self, inst: &Institution, term: &str, seats: &[Seats]) -> anyhow::Result<()> {
        {
            let _write = self.write_lock.lock().await;
            let mut client = self.db.client().await?;
            let tx = client.transaction().await?;
            tx.exec(
                "UPDATE section_seats SET checked_at=checked_at WHERE inst=$1 AND term=$2 AND id=''",
                &[&inst.id, &term],
            )
            .await?;
            let now = self.now().timestamp();
            for st in seats {
                let Some(row) = tx
                    .fetch_opt(
                        "SELECT available FROM section_seats WHERE inst=$1 AND term=$2 AND id=$3",
                        &[&inst.id, &term, &st.id],
                    )
                    .await?
                else {
                    continue;
                };
                let prior: i64 = row.try_get(0)?;
                tx.exec(
                    "UPDATE section_seats SET capacity=$1,available=$2,waitlist=$3,checked_at=$4,changed_at=CASE WHEN capacity<>$5 OR available<>$6 OR waitlist<>$7 THEN $8 ELSE changed_at END WHERE inst=$9 AND term=$10 AND id=$11",
                    &[
                        &st.capacity, &st.available, &st.waitlist, &now, &st.capacity, &st.available, &st.waitlist,
                        &now, &inst.id, &term, &st.id,
                    ],
                )
                .await?;
                if prior == 0 && st.available > 0 {
                    let data: String = tx
                        .fetch_one(
                            "SELECT data FROM sections WHERE inst=$1 AND term=$2 AND id=$3",
                            &[&inst.id, &term, &st.id],
                        )
                        .await?
                        .try_get(0)?;
                    let sec = parse_section(&data)?;
                    self.queue_open(&tx, inst, term, &sec.id, st.available, &sec.course, &sec.number, &sec.title, now)
                        .await?;
                }
            }
            tx.commit().await?;
        }
        self.write_seat_snapshot(&inst.id, term).await
    }
}

/// The two jobs each school runs.
#[derive(Clone, Copy, PartialEq, Eq)]
enum JobKind {
    Catalog,
    Seats,
}

impl JobKind {
    fn name(self) -> &'static str {
        match self {
            JobKind::Catalog => "catalog",
            JobKind::Seats => "seats",
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use std::sync::Mutex;

    use async_trait::async_trait;
    use chrono::TimeZone;
    use honorum_catalog::{Adapter, AdapterError, AdapterState};

    use super::*;
    use crate::config::InstitutionConfig;
    use crate::push::{Alert, SendOutcome};
    use crate::snapshot::{complete_snapshot, snapshot_file, snapshot_key};
    use crate::test_support::{FakeAdapter, TestClient, count, test_server, test_server_with};
    use crate::testing::TestDatabase;
    use crate::{Config, Db};

    /// A test server like [`test_server`] whose demo school is served by
    /// `adapter`.
    pub(crate) async fn server_with_adapter(adapter: Arc<dyn Adapter>) -> (Arc<Server>, TestDatabase) {
        let db = TestDatabase::new().await;
        let pool = Db::open(&db.url, 8, 4).await.expect("open the test database");
        let cfg = Config {
            base_url: "http://x".into(),
            contact: "mailto:t@x".into(),
            poll_seconds: 60,
            catalog_hours: 1,
            debounce_minutes: 30,
            ..Config::default()
        };
        let insts = vec![Institution {
            id: "demo".into(),
            name: "Demo U".into(),
            timezone: String::new(),
            adapter: Some(adapter),
            lms: None,
            closures: Vec::new(),
        }];
        let dir = std::env::temp_dir().join(format!("honorum-test-{}", crate::session::new_id()));
        (Server::new(pool, cfg, insts, dir).await.expect("build the test server"), db)
    }

    /// The file path and URL of a term's current published object.
    pub(crate) async fn published(s: &Server, kind: &str, inst: &str, term: &str) -> (std::path::PathBuf, String) {
        let hash = s.kv_get(&snapshot_key(kind, inst, term)).await;
        (
            snapshot_file(&s.data_dir.join("catalog").join(inst), term, kind, &hash),
            format!("/catalog/{inst}/{term}.{kind}.{hash}.json"),
        )
    }

    /// Records alerts sent through the server.
    fn recorder(s: &Server, gone: bool) -> Arc<Mutex<Vec<Alert>>> {
        let sent = Arc::new(Mutex::new(Vec::new()));
        let log = sent.clone();
        s.set_sender(Some(Arc::new(move |_: &str, a: &Alert| -> SendOutcome {
            log.lock().unwrap().push(a.clone());
            (gone, Ok(()))
        })));
        sent
    }

    #[tokio::test]
    async fn monitor_alerts_once_and_debounces() {
        let t = test_server().await;
        let s = &t.server;
        let inst = &s.insts[0];
        s.sync_catalog(inst, None, true).await.unwrap();
        let (path, _) = published(s, "structure", "demo", "2026FA").await;
        assert!(complete_snapshot(&path), "snapshot not written");
        let sent = recorder(s, false);

        // Two subscribers on one closed section (10001 has 0 seats in the fixture).
        for id in ["w1", "w2"] {
            let target = format!("sub-{id}");
            s.db.exec(
                "INSERT INTO watches(id,inst,term,section,channel,target,created_at) VALUES($1,$2,$3,$4,'push',$5,0)",
                &[&id, &"demo", &"2026FA", &"10001", &target],
            )
            .await
            .unwrap();
        }
        let step = |available: i64| {
            t.fake.set_seats("10001", available);
            async {
                let _ = s.poll(inst).await;
                s.dispatch_pending().await;
            }
        };
        step(0).await;
        assert_eq!(sent.lock().unwrap().len(), 0, "no alert expected while closed");
        step(2).await;
        {
            let sent = sent.lock().unwrap();
            assert!(
                sent.len() == 2 && sent[0].title.contains("CS 101 001"),
                "expected one alert per subscriber, got {sent:?}"
            );
        }
        step(2).await; // still open: no new transition
        step(0).await;
        step(1).await; // flapped back open inside the debounce window
        assert_eq!(sent.lock().unwrap().len(), 2, "debounce failed");
        assert_eq!(count(s, "SELECT COUNT(*) FROM alerts WHERE delivered_at>0").await, 2);

        // A dead push endpoint is pruned on first failure.
        recorder(s, true);
        s.db.exec("UPDATE watches SET last_alert=0", &[]).await.unwrap();
        step(0).await;
        step(3).await;
        assert_eq!(count(s, "SELECT COUNT(*) FROM watches").await, 0, "expected pruned watches");
    }

    #[tokio::test]
    async fn backoff() {
        let t = test_server().await;
        let s = &t.server;
        s.record("demo", Some("context deadline exceeded")).await;
        s.record("demo", Some("context deadline exceeded")).await;
        let st = s.status().await.institutions.unwrap()[0].clone();
        assert!(!st.ok && st.failures == 2 && st.paused_until != 0, "bad status {st:?}");
        s.record("demo", None).await;
        let st = s.status().await.institutions.unwrap()[0].clone();
        assert!(st.ok && st.paused_until == 0, "bad status after recovery {st:?}");
    }

    #[tokio::test]
    async fn catalog_rejects_invalid_section_before_changing_stored_data() {
        let t = test_server().await;
        let s = &t.server;
        let mut term = Term { id: "test".into(), name: "Original term".into(), ..Term::default() };
        let mut section =
            Section { id: "one".into(), course: "CS 101".into(), title: "Original title".into(), ..Section::default() };
        s.store_catalog(&s.insts[0], &term, std::slice::from_ref(&section)).await.unwrap();
        term.name = "Changed term".into();
        section.title = "Changed title".into();
        section.credits = f64::NAN;
        assert!(s.store_catalog(&s.insts[0], &term, &[section]).await.is_err(), "invalid JSON value was accepted");
        let name: String =
            s.db.fetch_one("SELECT name FROM terms WHERE inst='demo' AND id='test'", &[]).await.unwrap().get(0);
        let title: String =
            s.db.fetch_one("SELECT json_text(data,'$.title') FROM sections WHERE inst='demo' AND term='test'", &[])
                .await
                .unwrap()
                .get(0);
        assert_eq!((name.as_str(), title.as_str()), ("Original term", "Original title"));
    }

    #[tokio::test]
    async fn api() {
        let t = test_server().await;
        let s = &t.server;
        let _ = s.sync_catalog(&s.insts[0], None, true).await;
        let c = t.client().with_header("accept-encoding", "gzip");
        let rec = c.get("/api/config").await;
        assert!(rec.status == 200 && rec.text().contains("2026FA"), "config: {} {}", rec.status, rec.text());
        let (_, url) = published(s, "structure", "demo", "2026FA").await;
        assert_eq!(c.get(&url).await.header("content-encoding"), "gzip", "catalog was not served compressed");
        let send = |method: &'static str, path: String, body: &'static str| {
            let c = c.clone();
            async move { c.send(method, &path, &[], body).await }
        };
        let rec = send(
            "POST",
            "/api/watches".into(),
            r#"{"inst":"demo","term":"2026FA","section":"10001","channel":"push","target":"{\"endpoint\":\"https://push.example/x\",\"keys\":{\"p256dh\":\"a\",\"auth\":\"b\"}}"}"#,
        )
        .await;
        assert_eq!(rec.status, 201, "create watch: {}", rec.text());
        let id = rec.json()["id"].as_str().unwrap_or_default().to_owned();
        let rec = send(
            "POST",
            "/api/watches".into(),
            r#"{"inst":"demo","term":"2026FA","section":"nope","channel":"push","target":"{\"endpoint\":\"https://p/x\"}"}"#,
        )
        .await;
        assert_eq!(rec.status, 404, "unknown section");
        let rec = send(
            "POST",
            "/api/watches".into(),
            r#"{"inst":"demo","term":"2026FA","section":"10001","channel":"email","target":"a@b.c"}"#,
        )
        .await;
        assert_eq!(rec.status, 400, "email channel accepted: {}", rec.text());
        let rec = send("DELETE", format!("/api/watches/{id}"), "").await;
        assert_eq!(rec.status, 204, "remove watch: {}", rec.text());
        let share = "ab".repeat(16);
        assert_eq!(send("PUT", format!("/api/sync/{share}"), "secret").await.status, 204, "sync put");
        assert_eq!(send("PUT", format!("/api/sync/{share}"), "replaced").await.status, 409, "share overwritten");
        assert_eq!(send("GET", format!("/api/sync/{share}"), "").await.text(), "secret", "sync get");
        assert_eq!(send("PUT", "/api/sync/zzz".into(), "x").await.status, 400, "bad sync id");
        let rec = send("GET", "/status".into(), "").await;
        assert!(rec.status == 200 && rec.text().contains("Demo U"), "status page: {}", rec.status);
    }

    #[tokio::test]
    async fn progress_shows_as_syncing() {
        let t = test_server().await;
        let s = &t.server;
        s.progress("demo", "fetching Fall 2026").await;
        let st = s.status().await.institutions.unwrap()[0].clone();
        assert!(!st.ok && st.message == "fetching Fall 2026" && st.paused_until == 0, "bad status {st:?}");
        let page = t.client().get("/status").await.text();
        assert!(page.contains("syncing") && !page.contains(">down<"), "status page should say syncing: {page}");
    }

    #[test]
    fn suspiciously_open_classifies_catalogs() {
        let (mut open, mut mixed, mut unknown, mut future) = (vec![], vec![], vec![], vec![]);
        for i in 0..300 {
            open.push(Section { available: 1 + i % 5, capacity: 30, ..Section::default() });
            mixed.push(Section { available: i % 3, capacity: 30, ..Section::default() });
            unknown.push(Section { available: -1, ..Section::default() });
            // Registration not open yet.
            future.push(Section { available: 30, capacity: 30, ..Section::default() });
        }
        assert!(
            suspiciously_open(&open)
                && !suspiciously_open(&mixed)
                && !suspiciously_open(&unknown)
                && !suspiciously_open(&open[..50])
                && !suspiciously_open(&future),
            "guard misclassified a catalog"
        );
    }

    #[test]
    fn order_terms_puts_current_first() {
        let now = Utc.with_ymd_and_hms(2026, 9, 3, 0, 0, 0).unwrap();
        let term = |id: &str, start: &str, end: &str| Term {
            id: id.into(),
            name: String::new(),
            start: start.into(),
            end: end.into(),
        };
        let dated = [
            term("sp27", "2027-01-19", "2027-05-07"),
            term("su26", "2026-05-26", "2026-08-06"),
            term("fa26", "2026-08-24", "2026-12-10"),
        ];
        let got: Vec<String> = order_terms(&dated, now).into_iter().map(|t| t.id).collect();
        assert_eq!(got, ["fa26", "sp27", "su26"], "dated order");
        let named = |id: &str, name: &str| Term { id: id.into(), name: name.into(), ..Term::default() };
        let undated = [named("202630", "Summer 2027"), named("202620", "Spring 2027"), named("202610", "Fall 2026")];
        let got = order_terms(&undated, now);
        assert!(got[0].id == "202610" && got[2].id == "202630", "undated order: {got:?}");
    }

    #[tokio::test]
    async fn retarget_moves_watches_and_reminders() {
        let t = test_server().await;
        let s = &t.server;
        let old = r#"{"endpoint":"https://push.example/a","keys":{"p256dh":"x","auth":"y"}}"#;
        let neu = r#"{"endpoint":"https://push.example/b","keys":{"p256dh":"x2","auth":"y2"}}"#;
        let other = r#"{"endpoint":"https://push.example/other","keys":{"p256dh":"x","auth":"y"}}"#;
        let endpoint = |t: &str| crate::push::push_endpoint(t);
        s.db.exec(
            "INSERT INTO watches(id,inst,term,section,channel,target,endpoint,created_at) VALUES('w1','demo','t','s1','push',$1,$2,0)",
            &[&old, &endpoint(old)],
        )
        .await
        .unwrap();
        s.db.exec(
            "INSERT INTO watches(id,inst,term,section,channel,target,endpoint,created_at) VALUES('w2','demo','t','s2','push',$1,$2,0)",
            &[&other, &endpoint(other)],
        )
        .await
        .unwrap();
        s.db.exec(
            "INSERT INTO reminders(id,target,endpoint,tz,items,updated_at) VALUES('r1',$1,$2,'UTC','[]',0)",
            &[&old, &endpoint(old)],
        )
        .await
        .unwrap();
        let c = t.client();
        let rr = c.post("/api/push/retarget", &serde_json::json!({"from": old, "to": neu})).await;
        assert!(rr.status == 200 && rr.text().contains(r#""moved":2"#), "retarget: {} {}", rr.status, rr.text());
        let target = |q: &'static str| async move { s.db.fetch_one(q, &[]).await.unwrap().get::<_, String>(0) };
        assert_eq!(target("SELECT target FROM watches WHERE id='w1'").await, neu, "watch target");
        assert_eq!(target("SELECT target FROM reminders WHERE id='r1'").await, neu, "reminder target");
        assert_eq!(target("SELECT target FROM watches WHERE id='w2'").await, other, "another browser's watch changed");
        let rr = c.send("POST", "/api/push/retarget", &[], r#"{"from":"","to":"nope"}"#).await;
        assert_eq!(rr.status, 400, "bad retarget accepted");
    }

    #[tokio::test]
    async fn catalog_check_touches_only_term_freshness() {
        let t = test_server().await;
        let s = &t.server;
        let at = Utc.timestamp_opt(1000, 0).unwrap();
        s.clock.set(at);
        let term = Term { id: "t".into(), name: "Fall".into(), ..Term::default() };
        let secs =
            [Section { id: "s".into(), course: "CS 101".into(), available: 0, capacity: 10, ..Section::default() }];
        s.store_catalog(&s.insts[0], &term, &secs).await.unwrap();
        let revisions = || async {
            let row = s.db.fetch_one("SELECT structure,seats FROM catalog_revisions", &[]).await.unwrap();
            (row.get::<_, i64>(0), row.get::<_, i64>(1))
        };
        let before = revisions().await;
        let at = at + TimeDelta::hours(1);
        s.clock.set(at);
        s.store_catalog(&s.insts[0], &term, &secs).await.unwrap();
        let after = revisions().await;
        let checked = count(s, "SELECT checked_at FROM section_seats").await;
        let term_checked = count(s, "SELECT updated_at FROM terms").await;
        assert!(
            before == after && checked == 0 && term_checked == at.timestamp(),
            "unchanged rows updated: {after:?} {checked} {term_checked}"
        );
    }

    /// Delegates to the fake, except where a test overrides a call.
    struct Wrapped<F> {
        fake: Arc<FakeAdapter>,
        hooks: F,
    }

    /// Records which terms were polled and fails for one the school stopped
    /// serving.
    struct Retiring {
        polled: Mutex<Vec<String>>,
    }

    #[async_trait]
    impl Adapter for Wrapped<Retiring> {
        async fn terms(&self) -> Result<Vec<Term>, AdapterError> {
            self.fake.terms().await
        }
        async fn catalog(&self, term: &str) -> Result<Vec<Section>, AdapterError> {
            self.fake.catalog(term).await
        }
        async fn availability(&self, term: &str, ids: &[String]) -> Result<Vec<Seats>, AdapterError> {
            self.hooks.polled.lock().unwrap().push(term.to_owned());
            if term == "retired" {
                return Err(AdapterError::new("context deadline exceeded"));
            }
            self.fake.availability(term, ids).await
        }
        fn version(&self) -> &str {
            self.fake.version()
        }
    }

    #[tokio::test]
    async fn ended_term_watches_are_skipped_and_expire() {
        let t = test_server().await;
        let s = &t.server;
        let now = s.now();
        let day = |offset: i64| (now + TimeDelta::days(offset)).format("%Y-%m-%d").to_string();
        for (id, start, end) in
            [("ended", day(-120), day(-2)), ("retired", String::new(), String::new()), ("current", day(-30), day(60))]
        {
            s.db.exec(
                r#"INSERT INTO terms(inst,id,name,start,"end",updated_at) VALUES('demo',$1,$2,$3,$4,0)"#,
                &[&id, &id, &start, &end],
            )
            .await
            .unwrap();
        }
        let year_ago = (now.checked_sub_months(chrono::Months::new(12)).unwrap() - TimeDelta::days(1)).timestamp();
        for (id, term, created) in [
            ("w1", "ended", now.timestamp()),
            ("w2", "retired", now.timestamp()),
            ("w3", "current", now.timestamp()),
            ("w4", "current", year_ago),
        ] {
            s.db.exec(
                "INSERT INTO watches(id,inst,term,section,channel,target,created_at) VALUES($1,'demo',$2,'10001','push','sub',$3)",
                &[&id, &term, &created],
            )
            .await
            .unwrap();
        }
        let adapter = Arc::new(Wrapped { fake: t.fake.clone(), hooks: Retiring { polled: Mutex::new(Vec::new()) } });
        let mut inst = s.insts[0].clone();
        inst.adapter = Some(adapter.clone());
        assert!(s.poll(&inst).await.is_err(), "a failing term was not reported");
        let polled = adapter.hooks.polled.lock().unwrap().join(",");
        assert!(!polled.contains("ended") && polled.contains("current"), "polled {polled}");
        s.sweep().await;
        let kept: Vec<String> =
            s.db.fetch_all("SELECT id FROM watches ORDER BY id", &[]).await.unwrap().iter().map(|r| r.get(0)).collect();
        assert_eq!(kept.join(","), "w2,w3");
    }

    /// A catalog call that never finishes, and a poll that reports itself.
    struct Stalled {
        started: tokio::sync::mpsc::Sender<()>,
        polled: tokio::sync::mpsc::Sender<()>,
    }

    #[async_trait]
    impl Adapter for Wrapped<Stalled> {
        async fn terms(&self) -> Result<Vec<Term>, AdapterError> {
            self.fake.terms().await
        }
        async fn catalog(&self, _term: &str) -> Result<Vec<Section>, AdapterError> {
            let _ = self.hooks.started.try_send(());
            std::future::pending().await
        }
        async fn availability(&self, term: &str, ids: &[String]) -> Result<Vec<Seats>, AdapterError> {
            let _ = self.hooks.polled.try_send(());
            self.fake.availability(term, ids).await
        }
        fn version(&self) -> &str {
            self.fake.version()
        }
    }

    #[tokio::test]
    async fn seat_job_runs_during_blocked_catalog_and_stops() {
        let (started_tx, mut started) = tokio::sync::mpsc::channel(1);
        let (polled_tx, mut polled) = tokio::sync::mpsc::channel(1);
        let adapter = Arc::new(Wrapped {
            fake: Arc::new(FakeAdapter::new()),
            hooks: Stalled { started: started_tx, polled: polled_tx },
        });
        let (s, _db) = server_with_adapter(adapter).await;
        s.db.exec(
            "INSERT INTO watches(id,inst,term,section,channel,target) VALUES('w','demo','2026FA','10001','push','sub')",
            &[],
        )
        .await
        .unwrap();
        let stop = CancellationToken::new();
        let monitor = {
            let (s, stop) = (s.clone(), stop.clone());
            tokio::spawn(async move { s.run_monitor(stop).await })
        };
        for ch in [&mut started, &mut polled] {
            tokio::time::timeout(Duration::from_secs(3), ch.recv()).await.expect("job blocked behind another job");
        }
        stop.cancel();
        tokio::time::timeout(Duration::from_secs(3), monitor).await.expect("jobs did not stop").unwrap();
    }

    /// Fixed terms; records catalog calls.
    struct Recording {
        terms: Vec<Term>,
        calls: Mutex<Vec<String>>,
    }

    #[async_trait]
    impl Adapter for Wrapped<Recording> {
        async fn terms(&self) -> Result<Vec<Term>, AdapterError> {
            Ok(self.hooks.terms.clone())
        }
        async fn catalog(&self, term: &str) -> Result<Vec<Section>, AdapterError> {
            self.hooks.calls.lock().unwrap().push(term.to_owned());
            Ok(vec![Section {
                id: "s".into(),
                course: "CS 101".into(),
                title: "Intro".into(),
                available: -1,
                ..Section::default()
            }])
        }
        async fn availability(&self, term: &str, ids: &[String]) -> Result<Vec<Seats>, AdapterError> {
            self.fake.availability(term, ids).await
        }
        fn version(&self) -> &str {
            self.fake.version()
        }
    }

    #[tokio::test]
    async fn archived_terms_skip_refresh_and_unknown_dates_stay_active() {
        let t = test_server().await;
        let s = &t.server;
        let now = Utc.with_ymd_and_hms(2026, 9, 8, 12, 0, 0).unwrap();
        s.clock.set(now);
        let term = |id: &str, name: &str, start: &str, end: &str| Term {
            id: id.into(),
            name: name.into(),
            start: start.into(),
            end: end.into(),
        };
        let adapter = Arc::new(Wrapped {
            fake: t.fake.clone(),
            hooks: Recording {
                terms: vec![
                    term("past", "Past", "2026-01-01", "2026-05-01"),
                    term("current", "Current", "2026-08-01", "2026-12-01"),
                    term("unknown", "Unknown", "", ""),
                ],
                calls: Mutex::new(Vec::new()),
            },
        });
        let mut inst = s.insts[0].clone();
        inst.adapter = Some(adapter.clone());
        s.sync_catalog(&inst, None, true).await.unwrap();
        adapter.hooks.calls.lock().unwrap().clear();
        let slots = Semaphore::new(1);
        s.sync_catalog(&inst, Some(&slots), false).await.unwrap();
        {
            let calls = adapter.hooks.calls.lock().unwrap();
            assert_eq!(calls.len(), 2, "expected active/unknown refreshes only: {calls:?}");
            assert!(!calls.iter().any(|t| t == "past"), "recently checked past term refetched");
        }
        adapter.hooks.calls.lock().unwrap().clear();
        s.clock.set(now + TimeDelta::days(8));
        s.sync_catalog(&inst, None, false).await.unwrap();
        let calls = adapter.hooks.calls.lock().unwrap();
        assert_eq!(calls.len(), 3, "expired archive was not refreshed: {calls:?}");
    }

    /// A provider that tolerates polls no more often than every five minutes.
    struct SlowPoll(Arc<FakeAdapter>);

    #[async_trait]
    impl Adapter for SlowPoll {
        async fn terms(&self) -> Result<Vec<Term>, AdapterError> {
            self.0.terms().await
        }
        async fn catalog(&self, term: &str) -> Result<Vec<Section>, AdapterError> {
            self.0.catalog(term).await
        }
        async fn availability(&self, term: &str, ids: &[String]) -> Result<Vec<Seats>, AdapterError> {
            self.0.availability(term, ids).await
        }
        fn version(&self) -> &str {
            self.0.version()
        }
        fn minimum_poll_interval(&self) -> Option<Duration> {
            Some(Duration::from_secs(300))
        }
        fn state(&self) -> Option<&dyn AdapterState> {
            None
        }
    }

    #[tokio::test]
    async fn provider_and_institution_polling_intervals() {
        let school = |poll_seconds: i64| {
            move |cfg: &mut Config| {
                if poll_seconds > 0 {
                    cfg.institutions =
                        vec![InstitutionConfig { id: "demo".into(), poll_seconds, ..InstitutionConfig::default() }];
                }
            }
        };
        for (poll_seconds, want, message) in
            [(0, 300, "provider interval"), (600, 600, "school interval"), (10, 300, "provider floor ignored")]
        {
            let t = test_server_with(school(poll_seconds)).await;
            let mut inst = t.server.insts[0].clone();
            inst.adapter = Some(Arc::new(SlowPoll(t.fake.clone())));
            assert_eq!(t.server.poll_interval(&inst), Duration::from_secs(want), "{message}");
        }
    }

    #[tokio::test]
    async fn catalog_tables_live_in_their_own_schema() {
        let t = test_server().await;
        let s = &t.server;
        s.sync_catalog(&s.insts[0], None, true).await.unwrap();
        let in_main = count(
            s,
            "SELECT COUNT(*) FROM information_schema.tables WHERE table_schema='public' AND table_name IN ('sections','terms','section_seats')",
        )
        .await;
        let in_catalog = count(
            s,
            "SELECT COUNT(*) FROM information_schema.tables WHERE table_schema='catalog' AND table_name IN ('sections','terms','section_seats')",
        )
        .await;
        assert_eq!((in_main, in_catalog), (0, 3), "catalog tables");
        assert!(count(s, "SELECT COUNT(*) FROM sections").await > 0, "unqualified catalog reads");
    }

    #[tokio::test]
    async fn seat_counts_carry_published_exam_sittings() {
        let t = test_server().await;
        let s = &t.server;
        for q in [
            r#"INSERT INTO terms(inst,id,name,start,"end",updated_at) VALUES('demo','t','Term','2026-08-24','2026-12-18',100)"#,
            r#"INSERT INTO sections(inst,term,id,data) VALUES('demo','t','a','{"id":"a","exams":[{"date":"2026-12-15","start":480,"end":600,"location":"Hall 1"}]}'),('demo','t','b','{"id":"b"}')"#,
            "INSERT INTO section_seats(inst,term,id,capacity,available,waitlist,checked_at,changed_at) VALUES('demo','t','a',30,5,0,200,200),('demo','t','b',20,0,3,200,200),('demo','t','c',10,1,0,200,200)",
        ] {
            s.db.exec(q, &[]).await.unwrap();
        }
        let res = t.client().get("/api/seats?inst=demo&term=t&ids=a,b,c").await;
        assert_eq!(res.status, 200, "seats: {}", res.text());
        let out = res.json();
        let out = out.as_array().expect("an array");
        assert_eq!(out.len(), 3, "seats: {}", res.text());
        let by_id = |id: &str| out.iter().find(|x| x["id"] == id).cloned().unwrap_or_default();
        // A section with sittings lists them, one without lists none, and
        // seats without a structure row leave saved sittings alone by
        // omitting the field.
        let a = by_id("a");
        assert!(
            a["available"] == 5 && a["exams"].as_array().is_some_and(|e| e.len() == 1 && e[0]["location"] == "Hall 1"),
            "a: {}",
            res.text()
        );
        assert!(by_id("b")["exams"].as_array().is_some_and(|e| e.is_empty()), "b: {}", res.text());
        let c = by_id("c");
        assert!(c["available"] == 1 && c.get("exams").is_none(), "c: {}", res.text());
    }

    #[tokio::test]
    async fn readiness_and_catalog_rebuild_on_request() {
        let t = test_server().await;
        let s = &t.server;
        let c: TestClient = t.client();
        assert_eq!(c.get("/healthz").await.status, 200);
        assert_eq!(c.get("/readyz").await.status, 200);
        s.sync_catalog(&s.insts[0], None, true).await.unwrap();
        std::fs::remove_dir_all(s.data_dir.join("catalog")).unwrap();
        let (_, url) = published(s, "structure", "demo", "2026FA").await;
        let rec = c.get(&url).await;
        assert_eq!(rec.status, 200, "catalog was not rebuilt on request: {}", rec.text());
        let rec = c.get(&url.replacen("2026FA", "nope", 1)).await;
        assert_eq!(rec.status, 404, "unknown term rebuilt");
    }

    #[tokio::test]
    async fn closures_load_and_serve() {
        let all = crate::closures::load_closures().unwrap();
        for id in ["ou", "tamu", "rutgers", "uci", "demo"] {
            assert!(all.get(id).is_some_and(|c| !c.is_empty()), "no closures for {id}");
        }
        let t = test_server().await;
        let body = t.client().get("/api/config").await.text();
        assert!(
            body.contains(r#""closures":[{"from":"2026-09-07","to":"2026-09-07","name":"Labor Day"}"#),
            "config lacks the demo closures: {}",
            &body[..body.len().min(300)]
        );
    }
}
