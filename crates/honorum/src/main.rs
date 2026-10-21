//! The `honorum` binary: the API, catalog collection, notifications, and the
//! optional embedded web client.
//!
//! Flags take a single dash (`-addr :8080`); `--addr`, `-addr=:8080`, and
//! `--addr=:8080` are equivalent. `-addr` is the listen address (default
//! `:8080`, or `HONORUM_ADDR`), `-config` the school configuration file,
//! `-data` the directory for catalog snapshots, and `-role` the runtime role:
//! `api` serves requests, `worker` runs background jobs, and `all` (the
//! default, or `HONORUM_ROLE`) does both. `HONORUM_DATABASE_URL` names the
//! PostgreSQL database. Built with the default `web` feature the
//! binary embeds `apps/web/dist`, which `npm --prefix apps/web run build`
//! produces; release builds fail without it, while debug builds read the
//! folder from disk at run time and tolerate its absence.
//! `--no-default-features` builds the API-only binary.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;
use std::time::Duration;

use honorum_catalog::Http;
use honorum_server::{Config, Db, Institution, Server};
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;

/// Stamped at build time with `HONORUM_VERSION`.
const VERSION: &str = match option_env!("HONORUM_VERSION") {
    Some(v) => v,
    None => "dev",
};

/// How long shutdown waits for requests and background work.
const DRAIN: Duration = Duration::from_secs(40);

/// The command line.
struct Flags {
    addr: String,
    config: String,
    data: String,
    role: String,
}

const USAGE: &str = "Usage of honorum:
  -addr string
    \tlisten address (default \":8080\")
  -config string
    \tconfig file (default \"config/schools.json\")
  -data string
    \tdirectory for catalog snapshots (default \"data\")
  -role string
    \truntime role: api, worker, or all (default \"all\")";

/// Parses the command line: `-name value`, `-name=value`, and the same with
/// two dashes; parsing stops at `--` or the first
/// non-flag argument. `-h`/`-help` prints usage and exits 0; an unknown flag
/// or a missing value prints usage and exits 2.
fn parse_flags(args: impl Iterator<Item = String>) -> Flags {
    let mut flags = Flags {
        addr: env_or("HONORUM_ADDR", ":8080"),
        config: "config/schools.json".into(),
        data: "data".into(),
        role: env_or("HONORUM_ROLE", "all"),
    };
    let mut args = args.peekable();
    while let Some(arg) = args.next() {
        if arg == "--" || arg == "-" || !arg.starts_with('-') {
            break;
        }
        let trimmed = arg.strip_prefix("--").unwrap_or_else(|| &arg[1..]);
        let (name, inline) = match trimmed.split_once('=') {
            Some((n, v)) => (n.to_owned(), Some(v.to_owned())),
            None => (trimmed.to_owned(), None),
        };
        if name == "h" || name == "help" {
            eprintln!("{USAGE}");
            std::process::exit(0);
        }
        let slot = match name.as_str() {
            "addr" => &mut flags.addr,
            "config" => &mut flags.config,
            "data" => &mut flags.data,
            "role" => &mut flags.role,
            _ => {
                eprintln!("flag provided but not defined: -{name}\n{USAGE}");
                std::process::exit(2);
            }
        };
        match inline.or_else(|| args.next()) {
            Some(value) => *slot = value,
            None => {
                eprintln!("flag needs an argument: -{name}\n{USAGE}");
                std::process::exit(2);
            }
        }
    }
    flags
}

fn env_or(key: &str, default: &str) -> String {
    std::env::var(key).ok().filter(|v| !v.is_empty()).unwrap_or_else(|| default.to_owned())
}

/// A positive integer from the environment within `range`, else `message`.
fn env_count(key: &str, default: &str, range: std::ops::RangeInclusive<usize>, message: &str) -> anyhow::Result<usize> {
    env_or(key, default).parse::<usize>().ok().filter(|n| range.contains(n)).ok_or_else(|| anyhow::anyhow!("{message}"))
}

fn main() -> ExitCode {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .with_writer(std::io::stderr)
        .init();
    let flags = parse_flags(std::env::args().skip(1));
    let runtime = tokio::runtime::Builder::new_multi_thread().enable_all().build().expect("a Tokio runtime");
    match runtime.block_on(run(flags)) {
        Ok(code) => code,
        Err(err) => {
            eprintln!("honorum: {err:#}");
            ExitCode::FAILURE
        }
    }
}

async fn run(flags: Flags) -> anyhow::Result<ExitCode> {
    if !matches!(flags.role.as_str(), "api" | "worker" | "all") {
        anyhow::bail!("invalid runtime role");
    }
    let cfg = Config::load(std::path::Path::new(&flags.config))?;
    let url = std::env::var("HONORUM_DATABASE_URL").unwrap_or_default();
    if url.is_empty() {
        anyhow::bail!("HONORUM_DATABASE_URL is required");
    }
    let connections =
        env_count("HONORUM_DB_CONNECTIONS", "16", 2..=100, "HONORUM_DB_CONNECTIONS must be between 2 and 100")?;
    let lock_connections =
        env_count("HONORUM_DB_LOCK_CONNECTIONS", "8", 1..=8, "HONORUM_DB_LOCK_CONNECTIONS must be between 1 and 8")?;
    let db = Db::open(&url, connections, lock_connections).await?;

    let user_agent = format!("Mozilla/5.0 (compatible; Honorum/{VERSION}; +{}; {})", cfg.base_url, cfg.contact);
    let http = Http::new(user_agent, Duration::from_millis(200));
    let mut insts = Vec::with_capacity(cfg.institutions.len());
    for ic in &cfg.institutions {
        let Some(adapter) = honorum_catalog::adapters::build(&ic.adapter, &ic.url, &ic.env, &http) else {
            anyhow::bail!("unknown adapter {:?} for {}", ic.adapter, ic.id);
        };
        insts.push(Institution {
            id: ic.id.clone(),
            name: ic.name.clone(),
            timezone: ic.timezone.clone(),
            adapter: Some(adapter),
            lms: ic.lms.clone(),
            closures: ic.closures.clone().unwrap_or_default(),
        });
    }
    let count = insts.len();
    let mut server = Server::build(db.clone(), cfg, insts, PathBuf::from(&flags.data)).await?;
    server.version = VERSION.to_owned();
    server.web = web::assets();
    let s = Arc::new(server);

    let stop = CancellationToken::new();
    tokio::spawn({
        let stop = stop.clone();
        async move {
            shutdown_signal().await;
            stop.cancel();
        }
    });

    let mut jobs = JoinSet::new();
    let role = flags.role.as_str();
    if role != "api" {
        let (restore, restore_stop) = (s.clone(), stop.clone());
        jobs.spawn(async move {
            let (s, stop) = (restore, restore_stop);
            while !stop.is_cancelled() {
                match s.restore_snapshots(&stop).await {
                    Ok(()) => return,
                    Err(err) => tracing::warn!(err = %err, "snapshot restoration will retry"),
                }
                tokio::select! {
                    () = stop.cancelled() => return,
                    () = tokio::time::sleep(Duration::from_secs(30)) => {}
                }
            }
        });
        macro_rules! run {
            ($method:ident) => {{
                let (s, stop) = (s.clone(), stop.clone());
                jobs.spawn(async move { s.$method(stop).await });
            }};
        }
        run!(run_coordinator);
        run!(run_reminders);
        run!(run_deliveries);
        run!(run_imports);
        run!(run_streaks);
    }
    if role != "worker" {
        let (s, stop) = (s.clone(), stop.clone());
        jobs.spawn(async move { s.run_events(stop).await });
    }
    {
        let (s, stop) = (s.clone(), stop.clone());
        jobs.spawn(async move { s.run_metrics(stop).await });
    }
    {
        let (s, stop) = (s.clone(), stop.clone());
        jobs.spawn(async move { s.run_sessions(stop).await });
    }

    let listener = bind(&flags.addr).await?;
    tracing::info!(addr = %flags.addr, role = %flags.role, institutions = count, "listening");
    let app = honorum_server::router(s.clone()).into_make_service_with_connect_info::<SocketAddr>();
    let mut serving =
        tokio::spawn(axum::serve(listener, app).with_graceful_shutdown(stop.clone().cancelled_owned()).into_future());

    // Wait for a shutdown signal or a server failure.
    let mut failed = false;
    let early = tokio::select! {
        () = stop.cancelled() => None,
        result = &mut serving => Some(result),
    };
    if let Some(result) = early {
        failed = true;
        stop.cancel();
        match result {
            Ok(Ok(())) => tracing::error!("HTTP server stopped"),
            Ok(Err(err)) => tracing::error!(err = %err, "HTTP server stopped"),
            Err(err) => tracing::error!(err = %err, "HTTP server stopped"),
        }
    }

    let deadline = tokio::time::Instant::now() + DRAIN;
    if !failed && tokio::time::timeout_at(deadline, &mut serving).await.is_err() {
        tracing::warn!("HTTP shutdown: drain deadline reached");
        serving.abort();
    }
    let drained = tokio::time::timeout_at(deadline, async { while jobs.join_next().await.is_some() {} }).await;
    match drained {
        Ok(()) => db.close(),
        Err(_) => tracing::warn!("background shutdown deadline reached"),
    }
    Ok(if failed { ExitCode::FAILURE } else { ExitCode::SUCCESS })
}

/// Binds `addr` (`host:port`); a bare `:port` listens on every interface,
/// IPv6 and IPv4 when the host supports both, else IPv4 only.
async fn bind(addr: &str) -> anyhow::Result<tokio::net::TcpListener> {
    if let Some(port) = addr.strip_prefix(':') {
        if let Ok(listener) = tokio::net::TcpListener::bind(format!("[::]:{port}")).await {
            return Ok(listener);
        }
        return Ok(tokio::net::TcpListener::bind(format!("0.0.0.0:{port}")).await?);
    }
    Ok(tokio::net::TcpListener::bind(addr).await?)
}

/// Resolves on SIGINT or SIGTERM.
async fn shutdown_signal() {
    let interrupt = tokio::signal::ctrl_c();
    #[cfg(unix)]
    {
        let mut terminate =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()).expect("a SIGTERM handler");
        tokio::select! {
            _ = interrupt => {}
            _ = terminate.recv() => {}
        }
    }
    #[cfg(not(unix))]
    {
        let _ = interrupt.await;
    }
}

#[cfg(feature = "web")]
mod web {
    use std::borrow::Cow;
    use std::sync::Arc;

    use honorum_server::assets::WebAssets;

    /// The built web client.
    #[derive(rust_embed::Embed)]
    #[folder = "../../apps/web/dist"]
    #[cfg_attr(debug_assertions, allow_missing = true)]
    struct Dist;

    struct Files;

    impl WebAssets for Files {
        fn names(&self) -> Vec<String> {
            Dist::iter().map(Cow::into_owned).collect()
        }

        fn get(&self, path: &str) -> Option<Cow<'static, [u8]>> {
            Dist::get(path).map(|f| f.data)
        }

        fn modified(&self, path: &str) -> Option<i64> {
            Dist::get(path)?.metadata.last_modified().map(|t| t as i64)
        }
    }

    /// The embedded files, or `None` when the build has no web client.
    pub(super) fn assets() -> Option<Arc<dyn WebAssets>> {
        if Dist::get("index.html").is_none() {
            tracing::warn!("apps/web/dist is missing; run `npm --prefix apps/web run build` to serve the web client");
            return None;
        }
        Some(Arc::new(Files))
    }
}

#[cfg(not(feature = "web"))]
mod web {
    /// API-only builds host the web client separately.
    pub(super) fn assets() -> Option<std::sync::Arc<dyn honorum_server::assets::WebAssets>> {
        None
    }
}
