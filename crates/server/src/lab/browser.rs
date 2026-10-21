//! The server behind the web browser suite: the release server binary
//! against a disposable database.

use std::time::Duration;

use anyhow::Context;
use nix::sys::signal::{Signal, kill};
use nix::unistd::Pid;

use crate::testing::TestDatabase;

/// How long the server may take to stop after it is interrupted.
const WAIT_DELAY: Duration = Duration::from_secs(45);

/// Creates a database from `HONORUM_TEST_POSTGRES` and runs the binary
/// named by `HONORUM_E2E_BINARY` on `127.0.0.1:${HONORUM_TEST_PORT:-18080}`
/// with `apps/web/e2e/config.json` and a temporary data directory, relative
/// to the working directory. SIGINT or SIGTERM is forwarded to the server
/// as SIGINT; a server that has not exited 45 seconds later is killed. The
/// database and data directory are removed on return. Fails when the
/// server exits unsuccessfully without being interrupted.
pub async fn run() -> anyhow::Result<()> {
    let binary = std::env::var("HONORUM_E2E_BINARY").unwrap_or_default();
    if binary.is_empty() {
        anyhow::bail!("set HONORUM_E2E_BINARY (run the web browser suite to start this server)");
    }
    let db = TestDatabase::new().await;
    let port = std::env::var("HONORUM_TEST_PORT").ok().filter(|p| !p.is_empty()).unwrap_or_else(|| "18080".into());
    let addr = format!("127.0.0.1:{port}");
    let data = std::env::temp_dir().join(format!("honorum-browser-{}", crate::session::new_id()));
    std::fs::create_dir_all(&data)?;
    let result = serve(&binary, &addr, &data, &db.url).await;
    let _ = std::fs::remove_dir_all(&data);
    drop(db);
    result
}

async fn serve(binary: &str, addr: &str, data: &std::path::Path, url: &str) -> anyhow::Result<()> {
    let signal = super::shutdown_signal();
    tokio::pin!(signal);
    let mut child = tokio::process::Command::new(binary)
        .args(["-addr", addr, "-config", "apps/web/e2e/config.json", "-data"])
        .arg(data)
        .env("HONORUM_DATABASE_URL", url)
        .env("HONORUM_BASE_URL", format!("http://{addr}"))
        .kill_on_drop(true)
        .spawn()
        .with_context(|| format!("start {binary}"))?;
    tokio::select! {
        status = child.wait() => {
            let status = status?;
            if !status.success() {
                anyhow::bail!("{binary}: {status}");
            }
            Ok(())
        }
        () = &mut signal => {
            if let Some(id) = child.id().and_then(|id| i32::try_from(id).ok()) {
                let _ = kill(Pid::from_raw(id), Signal::SIGINT);
            }
            if tokio::time::timeout(WAIT_DELAY, child.wait()).await.is_err() {
                let _ = child.kill().await;
            }
            Ok(())
        }
    }
}
