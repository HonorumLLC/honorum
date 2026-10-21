//! The capacity lab. `scripts/load-test.py` runs separate seeding,
//! resource-limited server, and traffic processes, each with
//! `HONORUM_LOAD_CONFIG` naming a `LoadConfig` file and `HONORUM_LOAD_MODE`
//! choosing `seed`, `serve`, or `traffic`. Servers also read
//! `HONORUM_LOAD_ROLE` (`api`, `worker`, or `all`).
//!
//! Configuration and result files form the interface with
//! `scripts/load-test.py`. Update both sides when these fields change.

use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::time::Duration;

use anyhow::{Context, bail};
use async_trait::async_trait;
use axum::Router;
use axum::extract::State;
use axum::response::Response;
use axum::routing::{get, post};
use base64::Engine;
use chrono::{DateTime, SecondsFormat, TimeDelta, Utc};
use serde::{Deserialize, Serialize};
use serde_json::value::RawValue;
use serde_json::{Map, Value, json};
use tokio::sync::{Mutex as AsyncMutex, OnceCell, Semaphore};
use tokio::task::JoinSet;
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

use super::canvas::PerfCanvas;
use crate::api::ok;
use crate::canvas_account::CanvasConnection;
use crate::db::{Param, Sql};
use crate::docs::{Doc, insert_rows, push_docs};
use crate::push::{Alert, SendOutcome, Sender};
use crate::session::hash_token;
use crate::{Config, Db, Server};

/// The lab configuration written by `scripts/load-test.py`, which uses
/// these capitalized field names.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct LoadConfig {
    #[serde(rename = "Dir")]
    pub(crate) dir: PathBuf,
    /// This process's listen address (servers) or the first API (traffic).
    #[serde(rename = "Addr")]
    pub(crate) addr: String,
    /// Where the simulated Canvas listens.
    #[serde(rename = "CanvasAddr")]
    pub(crate) canvas_addr: String,
    #[serde(rename = "DatabaseURL")]
    pub(crate) database_url: String,
    /// Every API replica; traffic is spread across them.
    #[serde(rename = "APIs", deserialize_with = "nullable")]
    pub(crate) apis: Vec<String>,
    /// A separate worker process, when there is one.
    #[serde(rename = "WorkerAddr")]
    pub(crate) worker_addr: String,
    #[serde(rename = "Users")]
    pub(crate) users: i64,
    #[serde(rename = "Tasks")]
    pub(crate) tasks: i64,
    #[serde(rename = "Days")]
    pub(crate) days: i64,
    #[serde(rename = "Warmup")]
    pub(crate) warmup: i64,
    #[serde(rename = "Seconds")]
    pub(crate) seconds: i64,
    #[serde(rename = "Streams")]
    pub(crate) streams: bool,
    #[serde(rename = "QueryConnections")]
    pub(crate) query_connections: usize,
    #[serde(rename = "APILockConnections")]
    pub(crate) api_lock_connections: usize,
    #[serde(rename = "WorkerLockConnections")]
    pub(crate) worker_lock_connections: usize,
}

fn nullable<'de, D: serde::Deserializer<'de>, T: Default + Deserialize<'de>>(d: D) -> Result<T, D::Error> {
    Ok(Option::<T>::deserialize(d)?.unwrap_or_default())
}

/// One seeded account in `users.json`.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
struct LoadUser {
    #[serde(rename = "ID")]
    id: String,
    #[serde(rename = "Token")]
    token: String,
    #[serde(rename = "Seq")]
    seq: i64,
    #[serde(rename = "Rev")]
    rev: i64,
}

fn load_json<T: for<'de> Deserialize<'de>>(path: &Path) -> anyhow::Result<T> {
    let raw = std::fs::read(path).with_context(|| format!("read {}", path.display()))?;
    serde_json::from_slice(&raw).with_context(|| format!("parse {}", path.display()))
}

/// Writes `value` as two-space indented JSON, readable only by the owner.
fn save_json(path: &Path, value: &impl Serialize) -> anyhow::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let body = serde_json::to_vec_pretty(value)?;
    let mut file = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(path)?;
    file.write_all(&body)?;
    Ok(())
}

/// Runs the mode named by `HONORUM_LOAD_MODE` with the configuration
/// `HONORUM_LOAD_CONFIG`, after checking that the database is a disposable
/// `honorum_load…` database on IPv4 loopback, that every address is on
/// IPv4 loopback, and that the sizes are in range.
pub async fn run() -> anyhow::Result<()> {
    let path = std::env::var("HONORUM_LOAD_CONFIG").unwrap_or_default();
    if path.is_empty() {
        bail!("run scripts/load-test.py to opt into the capacity lab");
    }
    let c: LoadConfig = load_json(Path::new(&path))?;
    let safe = url::Url::parse(&c.database_url)
        .is_ok_and(|u| u.host_str() == Some("127.0.0.1") && u.path().starts_with("/honorum_load"));
    if !safe {
        bail!("load database must be a disposable honorum_load database on IPv4 loopback");
    }
    if c.users < 1 || c.users > 100_000 || c.tasks < 1 || c.days < 3 || c.seconds < 1 || c.warmup < 0 {
        bail!("invalid load configuration");
    }
    let mut addresses = vec![&c.addr, &c.canvas_addr];
    addresses.extend(&c.apis);
    if !c.worker_addr.is_empty() {
        addresses.push(&c.worker_addr);
    }
    for addr in addresses {
        if addr.rsplit_once(':').map(|(host, _)| host) != Some("127.0.0.1") {
            bail!("the load lab only accepts IPv4 loopback addresses");
        }
    }
    match std::env::var("HONORUM_LOAD_MODE").unwrap_or_default().as_str() {
        "seed" => seed(&c).await,
        "serve" => serve(&c).await,
        "traffic" => traffic(c).await,
        _ => bail!("choose seed, serve, or traffic"),
    }
}

/// Push delivery for the lab: a deterministic, local fixture that takes
/// 20 ms and never reaches an actual notification destination.
struct SlowSender;

#[async_trait]
impl Sender for SlowSender {
    async fn send(&self, _target: &str, _alert: &Alert) -> SendOutcome {
        tokio::time::sleep(Duration::from_millis(20)).await;
        (false, Ok(()))
    }
}

async fn load_server(c: &LoadConfig) -> anyhow::Result<Arc<Server>> {
    let queries = if c.query_connections == 0 { 16 } else { c.query_connections };
    let mut locks = if c.api_lock_connections == 0 { 8 } else { c.api_lock_connections };
    if std::env::var("HONORUM_LOAD_ROLE").unwrap_or_default() == "worker" && c.worker_lock_connections > 0 {
        locks = c.worker_lock_connections;
    }
    let db = Db::open(&c.database_url, queries, locks).await?;
    let cfg = Config {
        base_url: format!("http://{}", c.addr),
        canvas_key: base64::engine::general_purpose::STANDARD.encode([42u8; 32]),
        ..Config::default()
    };
    let s = Server::new(db, cfg, Vec::new(), c.dir.clone()).await?;
    s.set_sender(Some(Arc::new(SlowSender)));
    Ok(s)
}

fn date(t: DateTime<Utc>) -> String {
    t.format("%Y-%m-%d").to_string()
}

fn rfc3339(t: DateTime<Utc>) -> String {
    t.to_rfc3339_opts(SecondsFormat::Secs, true)
}

/// RFC 3339 in UTC with fractional seconds, trailing zeros removed.
fn rfc3339_nano(t: DateTime<Utc>) -> String {
    let full = t.to_rfc3339_opts(SecondsFormat::Nanos, true);
    let Some(stripped) = full.strip_suffix('Z') else { return full };
    let trimmed = stripped.trim_end_matches('0').trim_end_matches('.');
    format!("{trimmed}Z")
}

fn raw(value: &Value) -> Option<Box<RawValue>> {
    RawValue::from_string(value.to_string()).ok()
}

/// Seeds `Users` accounts through the real reconciliation code, against the
/// simulated Canvas with no latency. Accounts already present (by count)
/// are kept, so stages grow one database. Writes `Dir/users.json`.
async fn seed(c: &LoadConfig) -> anyhow::Result<()> {
    let s = load_server(c).await?;
    s.outbound.set_allow_local(true);
    let listener = tokio::net::TcpListener::bind(&c.canvas_addr).await?;
    let fixture = PerfCanvas::new(Duration::ZERO).spawn(listener);
    let result = seed_accounts(&s, c).await;
    fixture.abort();
    s.db.close();
    result
}

async fn seed_accounts(s: &Arc<Server>, c: &LoadConfig) -> anyhow::Result<()> {
    let base = format!("http://{}", c.canvas_addr);
    let tasks = s.fetch_canvas_tasks(&url::Url::parse(&base)?, "synthetic", None, true).await?;
    let existing: i64 = s.db.fetch_one("SELECT count(*) FROM accounts", &[]).await?.try_get(0)?;
    let now = s.now();
    let days_ago = |n: i64| now - TimeDelta::days(n);
    let mut users = Vec::with_capacity(c.users as usize);
    for i in 0..c.users {
        let id = format!("load_{i:06}");
        let token = format!("synthetic-load-token-{id}");
        if i >= existing {
            let mut client = s.db.client().await?;
            let tx = client.transaction().await?;
            let created = days_ago(c.days).timestamp();
            tx.exec(
                "INSERT INTO accounts(id,phone,created_at,seq) VALUES($1,$2,$3,0)",
                &[&id, &format!("+1555{i:07}"), &created],
            )
            .await?;
            tx.exec(
                "INSERT INTO sessions(id,account,token_hash,name,created_at,last_seen,push) VALUES($1,$2,$3,$4,$5,$6,$7)",
                &[
                    &id,
                    &id,
                    &hash_token(&token),
                    &"Load fixture",
                    &now.timestamp(),
                    &(now.timestamp() - i % 300),
                    &format!("synthetic-push:{id}"),
                ],
            )
            .await?;
            tx.exec("INSERT INTO account_timezones VALUES($1,'UTC')", &[&id]).await?;
            tx.exec("INSERT INTO account_streaks VALUES($1,$2)", &[&id, &date(days_ago(c.days))]).await?;
            let day_names: Vec<String> = (3..=c.days).rev().map(|day| date(days_ago(day))).collect();
            let clean = 1i64;
            let rows: Vec<Vec<Param>> = day_names.iter().map(|day| vec![&id as Param, day, &clean]).collect();
            insert_rows(&tx, "INSERT INTO account_streak_days(account,day,clean)", "", 3, &rows).await?;
            let mut docs = vec![Doc {
                id: "prefs".into(),
                kind: "prefs".into(),
                body: raw(&json!({"reminders": true})),
                ..Doc::default()
            }];
            for j in 0..c.tasks {
                let done = j < c.tasks / 2;
                let mut due = if done { now - TimeDelta::days(j + 1) } else { now + TimeDelta::days(j + 1) };
                // Ten percent of accounts have a task reminder due during the run.
                if j == c.tasks - 1 && i % 10 == 0 {
                    due = now + TimeDelta::hours(1) + TimeDelta::seconds(30 + i % 90);
                }
                let body = json!({
                    "id": j.to_string(),
                    "title": format!("Assignment {j}"),
                    "source": "manual",
                    "course": format!("Course {}", j % 6),
                    "due": rfc3339(due),
                    "done": done,
                    "note": "Synthetic task for repeatable capacity testing.",
                });
                docs.push(Doc {
                    id: format!("t_{j}"),
                    kind: "task".into(),
                    updated: now.timestamp_millis(),
                    body: raw(&body),
                    ..Doc::default()
                });
            }
            push_docs(&tx, &id, docs, s.now()).await?;
            tx.commit().await?;
            if i % 2 == 0 {
                let mut conn = CanvasConnection {
                    id: format!("canvas_{id}"),
                    base: base.clone(),
                    synced: now.timestamp() - i % 3600,
                    modules_at: now.timestamp(),
                    ..CanvasConnection::default()
                };
                conn.credential = s.encrypt_canvas(&id, &conn, "synthetic")?;
                s.publish_canvas(&id, &conn, &tasks, true).await?;
            }
        }
        let seq: i64 = s.db.fetch_one("SELECT seq FROM accounts WHERE id=$1", &[&id]).await?.try_get(0)?;
        let rev: i64 =
            s.db.fetch_one("SELECT seq FROM docs WHERE account=$1 AND id='t_0'", &[&id]).await?.try_get(0)?;
        users.push(LoadUser { id, token, seq, rev });
        if (i + 1) % 1000 == 0 {
            println!("seeded {} accounts", i + 1);
        }
    }
    // Stagger freshness across an hour, regardless of time spent seeding or in a
    // previous stage. Otherwise setup duration silently creates an import burst.
    let at = s.now().timestamp();
    s.db.exec(
        "UPDATE canvas_connections SET synced=CAST($1 AS BIGINT)-(CAST(substr(account,6) AS INTEGER)*7919)%3600,attempted=0,modules=$2",
        &[&at, &at],
    )
    .await?;
    load_due(s, s.now() + TimeDelta::hours(1) + TimeDelta::seconds(90), c.tasks).await?;
    save_json(&c.dir.join("users.json"), &users)
}

/// Moves the last task of every tenth account to `due`, so its reminder
/// falls during the measurement.
async fn load_due(s: &Server, due: DateTime<Utc>, tasks: i64) -> anyhow::Result<()> {
    s.db.exec(
        "UPDATE docs SET body=(body::jsonb || jsonb_build_object('due',$1::text))::text WHERE id=$2 AND CAST(substr(account,6) AS INTEGER)%10=0",
        &[&rfc3339(due), &format!("t_{}", tasks - 1)],
    )
    .await?;
    Ok(())
}

/// State shared by the lab server's routes.
struct Serving {
    s: Arc<Server>,
    c: LoadConfig,
    role: String,
    canvas: Arc<PerfCanvas>,
    stop: CancellationToken,
    started: OnceCell<()>,
    jobs: std::sync::Mutex<JoinSet<()>>,
    sweep_ms: AtomicI64,
    sweep_done: AtomicBool,
    sweep_started: AtomicBool,
    /// Set when starting the workload failed; the process then exits
    /// unsuccessfully.
    failed: AtomicBool,
}

impl Serving {
    fn run<F>(&self, job: F)
    where
        F: Future<Output = ()> + Send + 'static,
    {
        self.jobs.lock().expect("jobs lock").spawn(job);
    }
}

/// Serves the API on `Addr` until SIGINT or SIGTERM, plus:
/// - `POST /__load/start`: starts background work once. The `api` role
///   runs only event fan-out; other roles move the reminder burst to an
///   hour and 30 seconds from now and run reminders, imports, deliveries,
///   and streaks (and events unless `worker`), and one forced sweep after
///   `Warmup + 30` seconds.
/// - `GET /__load/stats`: process, pool, job, delivery, and database
///   statistics.
///
/// Roles other than `api` also serve the simulated Canvas on `CanvasAddr`
/// with 300 ms latency.
async fn serve(c: &LoadConfig) -> anyhow::Result<()> {
    let s = load_server(c).await?;
    let role = std::env::var("HONORUM_LOAD_ROLE").unwrap_or_default();
    s.outbound.set_allow_local(true);
    let canvas = PerfCanvas::new(Duration::from_millis(300));
    let fixture =
        if role != "api" { Some(canvas.spawn(tokio::net::TcpListener::bind(&c.canvas_addr).await?)) } else { None };
    let stop = CancellationToken::new();
    tokio::spawn({
        let stop = stop.clone();
        async move {
            super::shutdown_signal().await;
            stop.cancel();
        }
    });
    let state = Arc::new(Serving {
        s: s.clone(),
        c: c.clone(),
        role,
        canvas,
        stop: stop.clone(),
        started: OnceCell::new(),
        jobs: std::sync::Mutex::new(JoinSet::new()),
        sweep_ms: AtomicI64::new(0),
        sweep_done: AtomicBool::new(false),
        sweep_started: AtomicBool::new(false),
        failed: AtomicBool::new(false),
    });
    let app = Router::new()
        .route("/__load/start", post(handle_start))
        .route("/__load/stats", get(handle_stats))
        .with_state(state.clone())
        .fallback_service(crate::router(s.clone()));
    let listener = tokio::net::TcpListener::bind(&c.addr).await?;
    println!("load server pid={} address={}", std::process::id(), c.addr);
    // Stopping drops the listener at once rather than draining open
    // requests and event streams.
    tokio::select! {
        result = axum::serve(listener, app.into_make_service_with_connect_info::<SocketAddr>()).into_future() => result?,
        () = stop.cancelled() => {}
    }
    let mut jobs = std::mem::take(&mut *state.jobs.lock().expect("jobs lock"));
    while jobs.join_next().await.is_some() {}
    if let Some(fixture) = fixture {
        fixture.abort();
    }
    s.db.close();
    if state.failed.load(Ordering::Relaxed) {
        bail!("the workload could not start");
    }
    Ok(())
}

async fn handle_start(State(st): State<Arc<Serving>>) -> Response {
    st.started
        .get_or_init(|| async {
            let s = &st.s;
            macro_rules! run {
                ($method:ident) => {{
                    let (s, stop) = (s.clone(), st.stop.clone());
                    st.run(async move { s.$method(stop).await });
                }};
            }
            run!(run_sessions);
            if st.role == "api" {
                run!(run_events);
                return;
            }
            // Schedule the synthetic reminder burst relative to the start of the
            // workload, not seeding or connection setup, which can take minutes.
            if let Err(err) = load_due(s, s.now() + TimeDelta::hours(1) + TimeDelta::seconds(30), st.c.tasks).await {
                eprintln!("load_due: {err:#}");
                st.failed.store(true, Ordering::Relaxed);
                return;
            }
            run!(run_reminders);
            run!(run_imports);
            run!(run_deliveries);
            run!(run_streaks);
            if st.role != "worker" {
                run!(run_events);
            }
            // Guarantee one hourly maintenance pass is represented even when the
            // short measurement does not happen to cross an hour boundary.
            let sweeper = st.clone();
            st.run(async move {
                let delay = Duration::from_secs((sweeper.c.warmup + 30) as u64);
                tokio::select! {
                    () = sweeper.stop.cancelled() => return,
                    () = tokio::time::sleep(delay) => {}
                }
                sweeper.sweep_started.store(true, Ordering::Relaxed);
                let began = std::time::Instant::now();
                sweeper.s.sweep().await;
                sweeper.sweep_ms.store(began.elapsed().as_millis() as i64, Ordering::Relaxed);
                sweeper.sweep_done.store(true, Ordering::Relaxed);
            });
        })
        .await;
    ok(&json!({"started": true})).unwrap_or_else(axum::response::IntoResponse::into_response)
}

/// Process CPU time (user and system), in seconds.
fn cpu_seconds() -> f64 {
    use nix::sys::resource::{UsageWho, getrusage};
    use nix::sys::time::TimeValLike;
    getrusage(UsageWho::RUSAGE_SELF)
        .map(|u| (u.user_time().num_microseconds() + u.system_time().num_microseconds()) as f64 / 1e6)
        .unwrap_or(0.0)
}

fn pool_json((max, open, idle, waiting): (usize, usize, usize, usize)) -> Value {
    json!({
        "MaxOpenConnections": max,
        "OpenConnections": open,
        "InUse": open.saturating_sub(idle),
        "Idle": idle,
        "WaitCount": waiting,
    })
}

async fn handle_stats(State(st): State<Arc<Serving>>) -> Response {
    let s = &st.s;
    let [queries, locks] = s.db.pool_status();
    let mut db = pool_json(queries);
    db["Locks"] = pool_json(locks);
    let mut stats = Map::new();
    let mut put = |k: &str, v: Value| {
        stats.insert(k.to_owned(), v);
    };
    put("db", db);
    put("resident_bytes", json!(memory_stats::memory_stats().map_or(0, |m| m.physical_mem)));
    put("tokio_tasks", json!(tokio::runtime::Handle::current().metrics().num_alive_tasks()));
    put("cpu_seconds", json!(cpu_seconds()));
    put("canvas_requests", json!(st.canvas.requests.load(Ordering::Relaxed)));
    put("sweep_ms", json!(st.sweep_ms.load(Ordering::Relaxed)));
    put("sweep_done", json!(st.sweep_done.load(Ordering::Relaxed)));
    put("sweep_started", json!(st.sweep_started.load(Ordering::Relaxed)));
    put("errors", json!(s.counters.errors.load(Ordering::Relaxed)));
    if let Ok(row) = s
        .db
        .fetch_one(
            "SELECT count(*) FILTER(WHERE delivered_at>0),count(*) FILTER(WHERE delivered_at=0),count(*) FILTER(WHERE delivered_at<0) FROM deliveries",
            &[],
        )
        .await
        && let (Ok(delivered), Ok(pending), Ok(expired)) =
            (row.try_get::<_, i64>(0), row.try_get::<_, i64>(1), row.try_get::<_, i64>(2))
    {
        put("delivered", json!(delivered));
        put("delivery_pending", json!(pending));
        put("delivery_expired", json!(expired));
    }
    if let Ok(row) = s
        .db
        .fetch_one("SELECT count(*),count(*) FILTER(WHERE kind='streak'),COALESCE(min(run_at),0) FROM work_jobs", &[])
        .await
        && let (Ok(jobs), Ok(streaks), Ok(oldest)) =
            (row.try_get::<_, i64>(0), row.try_get::<_, i64>(1), row.try_get::<_, i64>(2))
    {
        put("job_pending", json!(jobs));
        put("streak_pending", json!(streaks));
        put("oldest_job_at", json!(oldest));
        put("sweep_done", json!(st.sweep_done.load(Ordering::Relaxed) && streaks == 0));
    }
    if let Ok(row) = s.db.fetch_one("SELECT pg_database_size(current_database())", &[]).await
        && let Ok(size) = row.try_get::<_, i64>(0)
    {
        put("database_bytes", json!(size));
    }
    ok(&stats).unwrap_or_else(axum::response::IntoResponse::into_response)
}

/// One measured request.
#[derive(Clone, Debug, Default)]
struct Observation {
    kind: &'static str,
    /// Milliseconds from the scheduled time to completion.
    ms: f64,
    /// Milliseconds from the scheduled time to the request starting.
    lag: f64,
    status: u16,
    failure: String,
}

fn summary(samples: &[Observation]) -> Value {
    let mut ms: Vec<f64> = samples.iter().map(|s| s.ms).collect();
    let mut lag: Vec<f64> = samples.iter().map(|s| s.lag).collect();
    let (mut codes, mut failures) = (BTreeMap::<String, i64>::new(), BTreeMap::<String, i64>::new());
    let mut bad = 0;
    for s in samples {
        *codes.entry(s.status.to_string()).or_default() += 1;
        if !s.failure.is_empty() {
            bad += 1;
            *failures.entry(s.failure.clone()).or_default() += 1;
        }
    }
    ms.sort_by(f64::total_cmp);
    lag.sort_by(f64::total_cmp);
    let percentile = |v: &[f64], p: f64| {
        if v.is_empty() { 0.0 } else { v[(v.len() - 1).min((v.len() as f64 * p) as usize)] }
    };
    json!({
        "requests": samples.len(),
        "errors": bad,
        "error_fraction": bad as f64 / samples.len().max(1) as f64,
        "p50_ms": percentile(&ms, 0.50),
        "p95_ms": percentile(&ms, 0.95),
        "p99_ms": percentile(&ms, 0.99),
        "generator_lag_p95_ms": percentile(&lag, 0.95),
        "status_codes": codes,
        "failures": failures,
    })
}

/// A 128-bit PCG generator with DXSM output. The user order it produces for
/// a seed matches capacity results recorded by earlier releases, so runs
/// with the same seed stay comparable.
struct Pcg {
    hi: u64,
    lo: u64,
}

impl Pcg {
    fn uint64(&mut self) -> u64 {
        const MUL: u128 = (2549297995355413924u128 << 64) | 4865540595714422341;
        const INC: u128 = (6364136223846793005u128 << 64) | 1442695040888963407;
        let state = ((u128::from(self.hi) << 64) | u128::from(self.lo)).wrapping_mul(MUL).wrapping_add(INC);
        (self.hi, self.lo) = ((state >> 64) as u64, state as u64);
        const CHEAP_MUL: u64 = 0xda942042e4dd58b5;
        let mut hi = self.hi;
        hi ^= hi >> 32;
        hi = hi.wrapping_mul(CHEAP_MUL);
        hi ^= hi >> 48;
        hi.wrapping_mul(self.lo | 1)
    }

    /// A uniform value in `[0, n)`, by multiply-and-reject (Lemire).
    fn uint64n(&mut self, n: u64) -> u64 {
        if n & (n - 1) == 0 {
            return self.uint64() & (n - 1);
        }
        let product = u128::from(self.uint64()) * u128::from(n);
        let (mut hi, mut lo) = ((product >> 64) as u64, product as u64);
        if lo < n {
            let thresh = n.wrapping_neg() % n;
            while lo < thresh {
                let product = u128::from(self.uint64()) * u128::from(n);
                (hi, lo) = ((product >> 64) as u64, product as u64);
            }
        }
        hi
    }

    /// Fisher–Yates from the end.
    fn shuffle<T>(&mut self, items: &mut [T]) {
        for i in (1..items.len()).rev() {
            let j = self.uint64n(i as u64 + 1) as usize;
            items.swap(i, j);
        }
    }
}

/// The traffic generator's view of one account.
struct TrafficUser {
    token: String,
    /// Sequence and revision; locked for the duration of a request.
    state: Arc<AsyncMutex<(i64, i64)>>,
}

#[derive(Default, Deserialize)]
#[serde(default)]
struct Reply {
    seq: i64,
    applied: Vec<AppliedReply>,
    conflicts: Vec<Value>,
    current: Option<i64>,
    error: String,
}

#[derive(Default, Deserialize)]
#[serde(default)]
struct AppliedReply {
    id: String,
    rev: i64,
}

/// Opens the event streams (when `Streams`), starts every server, then
/// sends open-loop traffic for `Warmup + Seconds`: per account one sync and
/// one streak read per minute and one task edit per five minutes, at most
/// 2048 in flight. Records the measured requests and server statistics in
/// `Dir/result.json`.
async fn traffic(mut c: LoadConfig) -> anyhow::Result<()> {
    let seeded: Vec<LoadUser> = load_json(&c.dir.join("users.json"))?;
    if seeded.len() as i64 != c.users {
        bail!("seed count does not match");
    }
    let mut seeded = seeded;
    Pcg { hi: 42, lo: 17 }.shuffle(&mut seeded);
    let users: Arc<Vec<TrafficUser>> = Arc::new(
        seeded
            .into_iter()
            .map(|u| TrafficUser { token: u.token, state: Arc::new(AsyncMutex::new((u.seq, u.rev))) })
            .collect(),
    );
    let base = format!("http://{}", c.addr);
    if c.apis.is_empty() {
        c.apis = vec![c.addr.clone()];
    }
    let worker_base = if c.worker_addr.is_empty() { base } else { format!("http://{}", c.worker_addr) };
    let client = reqwest::Client::builder().pool_max_idle_per_host(2048).timeout(Duration::from_secs(10)).build()?;
    let stats = {
        let (client, url) = (client.clone(), format!("{worker_base}/__load/stats"));
        move || {
            let (client, url) = (client.clone(), url.clone());
            async move {
                match client.get(&url).send().await {
                    Ok(res) => res.json::<Value>().await.unwrap_or(Value::Null),
                    Err(err) => json!({"error": err.to_string()}),
                }
            }
        }
    };

    let close_streams = CancellationToken::new();
    let opened = Arc::new(AtomicI64::new(0));
    let dropped = Arc::new(AtomicI64::new(0));
    let mut readers = JoinSet::new();
    if c.streams {
        // Establish connections before measuring steady traffic. Initial frames
        // must arrive; an HTTP 200 with a stalled body does not count as connected.
        let stream_client = reqwest::Client::builder().pool_max_idle_per_host(1).build()?;
        let gate = Arc::new(Semaphore::new(32));
        let mut connecting = JoinSet::new();
        for (index, u) in users.iter().enumerate() {
            let url = format!("http://{}/api/me/events", c.apis[index % c.apis.len()]);
            let permit = gate.clone().acquire_owned().await?;
            let (client, token) = (stream_client.clone(), u.token.clone());
            connecting.spawn(async move {
                let _permit = permit;
                let connect = async {
                    let mut res = client.get(&url).bearer_auth(&token).send().await.ok()?;
                    let mut line = Vec::new();
                    while !line.contains(&b'\n') {
                        line.extend_from_slice(&res.chunk().await.ok()??);
                    }
                    let first = line.split(|&b| b == b'\n').next().unwrap_or_default();
                    (res.status() == 200 && first == b"event: seq").then_some(res)
                };
                tokio::time::timeout(Duration::from_secs(10), connect).await.ok().flatten()
            });
        }
        while let Some(joined) = connecting.join_next().await {
            let Ok(Some(mut res)) = joined else { continue };
            opened.fetch_add(1, Ordering::Relaxed);
            let (stop, dropped) = (close_streams.clone(), dropped.clone());
            readers.spawn(async move {
                tokio::select! {
                    () = stop.cancelled() => {}
                    () = async { while let Ok(Some(_)) = res.chunk().await {} } => {
                        if !stop.is_cancelled() {
                            dropped.fetch_add(1, Ordering::Relaxed);
                        }
                    }
                }
            });
        }
    }

    let mut start_addresses = c.apis.clone();
    if !c.worker_addr.is_empty() {
        start_addresses.push(c.worker_addr.clone());
    }
    for addr in &start_addresses {
        client
            .post(format!("http://{addr}/__load/start"))
            .header("Content-Type", "application/json")
            .send()
            .await
            .with_context(|| format!("start {addr}"))?;
    }
    let before = stats().await;
    let start = Instant::now();
    let measure = start + Duration::from_secs(c.warmup as u64);
    let end = measure + Duration::from_secs(c.seconds as u64);
    // One sync and one streak read per minute, plus one task edit per five
    // minutes. Open-loop arrivals continue when the API slows down.
    let rate = c.users as f64 * 2.2 / 60.0;
    let step_nanos = (1e9 / rate) as u64;
    let collected = Arc::new(std::sync::Mutex::new(Vec::<Observation>::new()));
    const INFLIGHT: u32 = 2048;
    let inflight = Arc::new(Semaphore::new(INFLIGHT as usize));
    let since = |due: Instant| Instant::now().saturating_duration_since(due).as_secs_f64() * 1e3;
    let mut n: u64 = 0;
    loop {
        let due = start + Duration::from_nanos(step_nanos.saturating_mul(n));
        if due >= end {
            break;
        }
        tokio::time::sleep_until(due).await;
        let count = users.len() as u64;
        let index = (n % count) as usize;
        let slot = (n / count + n % count) % 11;
        let kind = match slot {
            5..=9 => "activity",
            10 => "edit",
            _ => "poll",
        };
        let record = due >= measure;
        let api = format!("http://{}", c.apis[(n % c.apis.len() as u64) as usize]);
        n += 1;
        let Ok(permit) = inflight.clone().try_acquire_owned() else {
            if record {
                let o = Observation { kind, lag: since(due), failure: "inflight limit".into(), ..Default::default() };
                collected.lock().expect("observations lock").push(o);
            }
            continue;
        };
        let (users, client, collected) = (users.clone(), client.clone(), collected.clone());
        tokio::spawn(async move {
            let _permit = permit;
            let mut o = Observation { kind, lag: since(due), ..Default::default() };
            let user = &users[index];
            let Ok(mut state) = user.state.clone().try_lock_owned() else {
                if record {
                    o.failure = "user still busy".into();
                    collected.lock().expect("observations lock").push(o);
                }
                return;
            };
            let (path, payload) = match kind {
                "activity" => ("/api/me/activity", json!({"inst": ""})),
                "edit" => {
                    let now = Utc::now();
                    let body = json!({
                        "id": "0",
                        "title": "Load edited assignment",
                        "source": "manual",
                        "done": true,
                        "completedAt": rfc3339_nano(now),
                        "due": rfc3339(now - TimeDelta::hours(24)),
                    });
                    let doc = Doc {
                        id: "t_0".into(),
                        kind: "task".into(),
                        base: state.1,
                        updated: now.timestamp_millis(),
                        body: raw(&body),
                        ..Doc::default()
                    };
                    ("/api/me/sync", json!({"since": state.0, "docs": [doc]}))
                }
                _ => ("/api/me/sync", json!({"since": state.0, "docs": []})),
            };
            let sent = client
                .post(format!("{api}{path}"))
                .bearer_auth(&user.token)
                .header("Content-Type", "application/json")
                .body(payload.to_string())
                .send()
                .await;
            match sent {
                Err(_) => o.failure = "transport".into(),
                Ok(res) => {
                    o.status = res.status().as_u16();
                    let body = res.bytes().await.unwrap_or_default();
                    let reply = serde_json::from_slice::<Reply>(&body);
                    o.failure = match &reply {
                        _ if o.status != 200 => {
                            format!(
                                "HTTP {}: {}",
                                o.status,
                                reply.as_ref().map(|r| r.error.as_str()).unwrap_or_default()
                            )
                        }
                        Err(_) => "invalid JSON".into(),
                        Ok(r) if kind == "activity" && r.current.is_none() => "missing streak".into(),
                        Ok(r) if !r.conflicts.is_empty() => "revision conflict".into(),
                        Ok(r) if kind == "edit" && r.applied.len() != 1 => "missing acknowledgement".into(),
                        Ok(r) => {
                            if kind != "activity" {
                                state.0 = r.seq;
                                if let Some(d) = r.applied.iter().find(|d| d.id == "t_0") {
                                    state.1 = d.rev;
                                }
                            }
                            String::new()
                        }
                    };
                }
            }
            drop(state);
            o.ms = since(due);
            if record {
                collected.lock().expect("observations lock").push(o);
            }
        });
    }
    // Every request holds a permit until it finishes.
    drop(inflight.acquire_many(INFLIGHT).await?);
    let after = stats().await;
    let stream_drops = dropped.load(Ordering::Relaxed);
    close_streams.cancel();
    while readers.join_next().await.is_some() {}

    let collected = std::mem::take(&mut *collected.lock().expect("observations lock"));
    let total = summary(&collected);
    let number = |v: &Value, key: &str| v[key].as_f64().unwrap_or(f64::NAN);
    let mut pass = number(&total, "error_fraction") <= 0.01
        && number(&total, "p95_ms") <= 1000.0
        && number(&total, "generator_lag_p95_ms") < 100.0;
    let mut by_kind = Map::new();
    for kind in ["poll", "activity", "edit"] {
        let subset: Vec<Observation> = collected.iter().filter(|o| o.kind == kind).cloned().collect();
        let v = summary(&subset);
        pass = pass && !subset.is_empty() && number(&v, "p95_ms") <= 1000.0 && number(&v, "error_fraction") <= 0.01;
        by_kind.insert(kind.into(), v);
    }
    if c.seconds >= 45 {
        pass = pass && after.get("sweep_done") == Some(&Value::Bool(true));
    }
    let streams_opened = opened.load(Ordering::Relaxed);
    if c.streams {
        pass = pass && streams_opened as f64 >= 0.99 * c.users as f64 && stream_drops == 0;
    }
    let result = json!({
        "users": c.users,
        "target_rps": rate,
        "seconds": c.seconds,
        "warmup_seconds": c.warmup,
        "summary": total,
        "by_endpoint": by_kind,
        "streams_opened": streams_opened,
        "streams_dropped": stream_drops,
        "before": before,
        "after": after,
        "pass": pass,
    });
    save_json(&c.dir.join("result.json"), &result)?;
    println!(
        "users={} rps={rate:.1} p95={:.1}ms errors={:.2}% pass={pass}",
        c.users,
        number(&total, "p95_ms"),
        100.0 * number(&total, "error_fraction"),
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn reminder_seed_changes_only_the_selected_tasks() {
        let t = crate::test_support::test_server().await;
        let s = &t.server;
        let original = s.now();
        for account in ["load_000000", "load_000001"] {
            s.db.exec("INSERT INTO accounts(id,phone) VALUES($1,$1)", &[&account]).await.unwrap();
            let mut client = s.db.client().await.unwrap();
            let tx = client.transaction().await.unwrap();
            let docs = (0..2)
                .map(|id| Doc {
                    id: format!("t_{id}"),
                    kind: "task".into(),
                    body: raw(&json!({"title": "Keep this title", "due": rfc3339(original)})),
                    ..Doc::default()
                })
                .collect();
            push_docs(&tx, account, docs, original).await.unwrap();
            tx.commit().await.unwrap();
        }
        let due = original + TimeDelta::hours(2);
        load_due(s, due, 2).await.unwrap();
        for row in s.db.fetch_all("SELECT account,id,body FROM docs", &[]).await.unwrap() {
            let account: String = row.get(0);
            let id: String = row.get(1);
            let body: Value = serde_json::from_str(row.get(2)).unwrap();
            let expected = if account == "load_000000" && id == "t_1" { due } else { original };
            assert_eq!(body["due"], rfc3339(expected));
            assert_eq!(body["title"], "Keep this title");
        }
    }

    #[test]
    fn shuffle_is_reproducible() {
        for (n, head, tail) in [
            (10, [6, 9, 7, 2, 3, 5, 8, 4, 1, 0], [4, 1, 0]),
            (100, [58, 90, 40, 97, 10, 82, 62, 76, 23, 67], [79, 15, 4]),
            (1000, [813, 385, 232, 88, 70, 656, 345, 938, 766, 705], [811, 153, 44]),
        ] {
            let mut v: Vec<usize> = (0..n).collect();
            Pcg { hi: 42, lo: 17 }.shuffle(&mut v);
            assert_eq!(v[..10], head);
            assert_eq!(v[n - 3..], tail);
        }
    }

    #[test]
    fn timestamps_trim_trailing_zeros() {
        let t = DateTime::parse_from_rfc3339("2026-10-15T12:00:00.120000000Z").unwrap().with_timezone(&Utc);
        assert_eq!(rfc3339_nano(t), "2026-10-15T12:00:00.12Z");
        let whole = DateTime::parse_from_rfc3339("2026-10-15T12:00:00Z").unwrap().with_timezone(&Utc);
        assert_eq!(rfc3339_nano(whole), "2026-10-15T12:00:00Z");
        assert_eq!(rfc3339(t), "2026-10-15T12:00:00Z");
    }
}
