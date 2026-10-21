//! The account API for `apps/web/scripts/perf.mjs`: a temporary database,
//! seeding routes, and a simulated Canvas whose every response waits a
//! fixed latency.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use axum::Router;
use axum::body::Body;
use axum::extract::State;
use axum::response::Response;
use axum::routing::{get, post};
use base64::Engine;
use rand::RngExt;
use serde::Deserialize;
use serde_json::json;

use super::canvas::PerfCanvas;
use crate::api::{ApiError, ApiResult, fail, ok, read_json};
use crate::canvas_account::CanvasConnection;
use crate::db::Sql;
use crate::docs::{Doc, push_docs};
use crate::push::Alert;
use crate::session::{hash_token, new_id, new_token};
use crate::testing::TestDatabase;
use crate::{Config, Db, Institution, Server};

#[derive(Clone)]
struct Lab {
    s: Arc<Server>,
    canvas: Arc<PerfCanvas>,
    /// The simulated Canvas origin, such as `http://127.0.0.1:50123`.
    up: String,
}

/// `HONORUM_PERF_CONFIG`: the public configuration the web client is served.
#[derive(Default, Deserialize)]
#[serde(default)]
struct PublicConfig {
    institutions: Vec<PublicInstitution>,
}

#[derive(Default, Deserialize)]
#[serde(default)]
struct PublicInstitution {
    #[serde(alias = "ID")]
    id: String,
    #[serde(alias = "Name")]
    name: String,
    #[serde(alias = "Timezone")]
    timezone: String,
}

/// Serves the lab on `HONORUM_PERF_ADDR` until SIGINT or SIGTERM. The simulated
/// Canvas listens on a free loopback port and delays every response by
/// `HONORUM_PERF_CANVAS_LATENCY` (a duration such as `150ms`, as
/// `parse_duration` reads it; zero when unset or invalid). Institutions come
/// from the public configuration file `HONORUM_PERF_CONFIG`, when set. The
/// database is created from `HONORUM_TEST_POSTGRES` and dropped on return.
///
/// Routes under `/__perf` exist only here:
/// - `GET /__perf/lab`: `{"canvas", "canvas_requests", "canvas_paths"}`.
/// - `POST /__perf/account` (`{"docs", "canvas"}`, 16 MiB): creates an
///   account with one device session and the given documents, optionally
///   connected to the simulated Canvas, and answers
///   `{"id", "phone", "device", "token"}`.
pub async fn run() -> anyhow::Result<()> {
    let addr = std::env::var("HONORUM_PERF_ADDR").unwrap_or_default();
    if addr.is_empty() {
        anyhow::bail!("set HONORUM_PERF_ADDR to serve the performance lab");
    }
    let latency = super::parse_duration(&std::env::var("HONORUM_PERF_CANVAS_LATENCY").unwrap_or_default())
        .unwrap_or(Duration::ZERO);
    let canvas = PerfCanvas::new(latency);
    let upstream = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let up = format!("http://{}", upstream.local_addr()?);
    let fixture = canvas.spawn(upstream);

    let mut public = PublicConfig::default();
    if let Ok(path) = std::env::var("HONORUM_PERF_CONFIG")
        && !path.is_empty()
    {
        let raw = std::fs::read(&path).map_err(|e| anyhow::anyhow!("read {path}: {e}"))?;
        public = serde_json::from_slice(&raw).map_err(|e| anyhow::anyhow!("read {path}: {e}"))?;
    }
    let insts = public
        .institutions
        .into_iter()
        .map(|i| Institution {
            id: i.id,
            name: i.name,
            timezone: i.timezone,
            adapter: None,
            lms: None,
            closures: Vec::new(),
        })
        .collect();

    let dir = std::env::temp_dir().join(format!("honorum-perf-{}", new_id()));
    let database = TestDatabase::new().await;
    let result = serve(&addr, &database.url, insts, dir.clone(), canvas, up, latency).await;
    let _ = std::fs::remove_dir_all(&dir);
    fixture.abort();
    drop(database);
    result
}

async fn serve(
    addr: &str,
    url: &str,
    insts: Vec<Institution>,
    dir: PathBuf,
    canvas: Arc<PerfCanvas>,
    up: String,
    latency: Duration,
) -> anyhow::Result<()> {
    let db = Db::open(url, 16, 8).await?;
    let key: [u8; 32] = rand::rng().random();
    // The lab server forwards a distinct client address per browser context,
    // so quotas behave as they would for separate students.
    let cfg = Config {
        base_url: format!("http://{addr}"),
        trust_proxy: true,
        canvas_key: base64::engine::general_purpose::STANDARD.encode(key),
        ..Config::default()
    };
    let s = Server::new(db.clone(), cfg, insts, dir).await?;
    s.outbound.set_allow_local(true);
    s.set_sender(Some(Arc::new(|_: &str, _: &Alert| (false, Ok(())))));

    let lab = Lab { s: s.clone(), canvas, up: up.clone() };
    let app = Router::new()
        .route("/__perf/lab", get(handle_lab))
        .route("/__perf/account", post(handle_account))
        .with_state(lab)
        .fallback_service(crate::router(s));
    let listener = tokio::net::TcpListener::bind(addr).await?;
    let stop = tokio_util::sync::CancellationToken::new();
    let serving = tokio::spawn(
        axum::serve(listener, app.into_make_service_with_connect_info::<SocketAddr>())
            .with_graceful_shutdown(stop.clone().cancelled_owned())
            .into_future(),
    );
    println!(
        "performance lab API on http://{addr}, simulated Canvas on {up} with {} latency",
        super::format_duration(latency)
    );
    super::shutdown_signal().await;
    stop.cancel();
    let _ = tokio::time::timeout(Duration::from_secs(1), serving).await;
    db.close();
    Ok(())
}

async fn handle_lab(State(lab): State<Lab>) -> ApiResult {
    ok(&json!({
        "canvas": lab.up,
        "canvas_requests": lab.canvas.requests.load(Ordering::Relaxed),
        "canvas_paths": lab.canvas.paths(),
    }))
}

#[derive(Default, Deserialize)]
#[serde(default)]
struct Seed {
    docs: Vec<Doc>,
    canvas: bool,
}

async fn handle_account(State(lab): State<Lab>, body: Body) -> ApiResult<Response> {
    let Some(input) = read_json::<Seed>(body, 16 << 20).await else { return fail(400, "bad seed") };
    let s = &lab.s;
    let internal = |e: &dyn std::fmt::Display| ApiError::new(500, e.to_string());
    let (account, device, token) = (new_id(), new_id(), new_token());
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos();
    let phone = format!("+1555{:07}", nanos % 10_000_000);
    let now = s.now();
    {
        let mut client = s.db.client().await.map_err(|e| internal(&e))?;
        let tx = client.transaction().await.map_err(|e| internal(&e))?;
        tx.exec(
            "INSERT INTO accounts(id,phone,created_at,seq) VALUES($1,$2,$3,0)",
            &[&account, &phone, &now.timestamp()],
        )
        .await
        .map_err(|e| internal(&e))?;
        tx.exec(
            "INSERT INTO sessions(id,account,token_hash,name,created_at,last_seen) VALUES($1,$2,$3,$4,$5,$6)",
            &[&device, &account, &hash_token(&token), &"Performance lab", &now.timestamp(), &now.timestamp()],
        )
        .await
        .map_err(|e| internal(&e))?;
        let mut docs = input.docs;
        for d in &mut docs {
            d.updated = now.timestamp_millis();
        }
        push_docs(&tx, &account, docs, now).await.map_err(|e| internal(&e))?;
        tx.commit().await.map_err(|e| internal(&e))?;
    }
    if input.canvas {
        let base = url::Url::parse(&lab.up).map_err(|e| internal(&e))?;
        let mut c = CanvasConnection { id: new_token(), base: lab.up.clone(), ..CanvasConnection::default() };
        c.credential = s.encrypt_canvas(&account, &c, "perf-lab-token").map_err(|e| internal(&e))?;
        let tasks = s.fetch_canvas_tasks(&base, "perf-lab-token", None, true).await.map_err(|e| internal(&e))?;
        c.connected = true;
        c.synced = s.now().timestamp();
        c.modules_at = s.now().timestamp();
        s.publish_canvas(&account, &c, &tasks, true).await.map_err(|e| internal(&e))?;
    }
    ok(&json!({"id": account, "phone": phone, "device": device, "token": token}))
}
