//! Background account imports: Canvas connections and calendar feeds are
//! refreshed through the `work_jobs` queue, so every device sees new
//! assignments and reminders fire without a client opening the app.

use std::sync::Arc;
use std::time::Duration;

use tokio_util::sync::CancellationToken;

use crate::Server;
use crate::canvas_account::{CanvasRefreshOptions, IMPORT_DEVICE, IMPORT_INTERVAL, IMPORT_RETRY};
use crate::db::{self, Param, Sql};
use crate::jobs::WorkJob;

/// Background imports running at once. Each spends most of its time
/// waiting on Canvas or a calendar host; publication uses short
/// transactions.
pub(crate) const IMPORT_WORKERS: usize = 8;

impl Server {
    /// Runs `IMPORT_WORKERS` (8) workers draining `canvas` and `feed` jobs
    /// (20 per pass, every second) and queues stale imports every ten
    /// seconds, until `stop` fires; then waits for the workers. A running
    /// import stops with `stop` and is retried later.
    pub async fn run_imports(self: &Arc<Self>, stop: CancellationToken) {
        let mut workers = tokio::task::JoinSet::new();
        for _ in 0..IMPORT_WORKERS {
            let (s, stop) = (self.clone(), stop.clone());
            workers.spawn(async move {
                let mut ticker = tokio::time::interval(Duration::from_secs(1));
                ticker.tick().await;
                loop {
                    s.drain_work(&stop, &["canvas", "feed"], 20, |job| {
                        let (s, stop) = (&s, &stop);
                        async move {
                            tokio::select! {
                                result = s.import_work(job) => result,
                                () = stop.cancelled() => Err(anyhow::anyhow!("context canceled")),
                            }
                        }
                    })
                    .await;
                    tokio::select! {
                        () = stop.cancelled() => return,
                        _ = ticker.tick() => {}
                    }
                }
            });
        }
        let mut ticker = tokio::time::interval(Duration::from_secs(10));
        ticker.tick().await;
        loop {
            let queued = tokio::select! {
                queued = self.queue_stale_imports() => Some(queued),
                () = stop.cancelled() => None,
            };
            if let Some(Err(err)) = queued {
                tracing::warn!(%err, "import scheduling failed");
            }
            tokio::select! {
                () = stop.cancelled() => {
                    while workers.join_next().await.is_some() {}
                    return;
                }
                _ = ticker.tick() => {}
            }
        }
    }

    /// Queues an import for every Canvas connection and feed that has not
    /// synced within [`IMPORT_INTERVAL`] nor been attempted within
    /// [`IMPORT_RETRY`], for accounts with a device seen within
    /// [`IMPORT_DEVICE`].
    pub(crate) async fn queue_stale_imports(&self) -> db::Result<()> {
        let now = self.now().timestamp();
        let stale = now - IMPORT_INTERVAL.as_secs() as i64;
        let retry = now - IMPORT_RETRY.as_secs() as i64;
        let active = now - IMPORT_DEVICE.as_secs() as i64;
        for query in [
            "INSERT INTO work_jobs(kind,key,account,run_at) SELECT 'canvas',account,account,$1 FROM canvas_connections WHERE length(credential)>0 AND synced<$2 AND attempted<$3 AND EXISTS(SELECT 1 FROM sessions WHERE sessions.account=canvas_connections.account AND last_seen>$4) ON CONFLICT DO NOTHING",
            "INSERT INTO work_jobs(kind,key,account,payload,run_at) SELECT 'feed',account||':'||id,account,id,$1 FROM feed_connections WHERE synced<$2 AND attempted<$3 AND EXISTS(SELECT 1 FROM sessions WHERE sessions.account=feed_connections.account AND last_seen>$4) ON CONFLICT DO NOTHING",
        ] {
            self.db.exec(query, &[&now, &stale, &retry, &active]).await?;
        }
        Ok(())
    }

    /// Runs one import job: `canvas` (key = account) or `feed` (payload =
    /// feed id). Accounts with no recent device are skipped; other kinds
    /// fail with "unknown import job kind".
    pub(crate) async fn import_work(&self, job: WorkJob) -> anyhow::Result<()> {
        let since = self.now().timestamp() - IMPORT_DEVICE.as_secs() as i64;
        let active: i64 = self
            .db
            .fetch_one("SELECT COUNT(*) FROM sessions WHERE account=$1 AND last_seen>$2", &[&job.account, &since])
            .await?
            .try_get(0)?;
        if active == 0 {
            return Ok(());
        }
        match job.kind.as_str() {
            "canvas" => {
                self.refresh_canvas_connection(&job.account, CanvasRefreshOptions::default()).await?;
                Ok(())
            }
            "feed" => {
                self.refresh_feed_connection(&job.account, &job.payload, false).await?;
                Ok(())
            }
            _ => anyhow::bail!("unknown import job kind"),
        }
    }

    /// Runs a query returning one text column.
    pub(crate) async fn query_strings(&self, query: &str, params: &[Param<'_>]) -> db::Result<Vec<String>> {
        let rows = self.db.fetch_all(query, params).await?;
        rows.iter().map(|row| Ok(row.try_get(0)?)).collect()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::Ordering;

    use tokio_util::sync::CancellationToken;

    use crate::canvas_account::tests::{canvas_fixture, connect_fixture};
    use crate::db::Sql;
    use crate::test_support::TestServer;

    /// Queues stale imports and runs them.
    async fn refresh_queued_imports(t: &TestServer) {
        let s = &t.server;
        s.queue_stale_imports().await.unwrap();
        s.drain_work(&CancellationToken::new(), &["canvas", "feed"], 300, |job| s.import_work(job)).await;
    }

    #[tokio::test]
    async fn background_canvas_refresh_uses_active_devices() {
        let mut f = canvas_fixture().await;
        connect_fixture(&mut f.c, &f.up.url).await;
        let before = f.reads.load(Ordering::SeqCst);
        refresh_queued_imports(&f.t).await;
        assert_eq!(f.reads.load(Ordering::SeqCst), before, "fresh connection refreshed");
        f.t.server.db.exec("UPDATE canvas_connections SET synced=0,attempted=0", &[]).await.unwrap();
        refresh_queued_imports(&f.t).await;
        assert_eq!(f.reads.load(Ordering::SeqCst), before + 1, "stale connection not refreshed");
        let attempted: i64 =
            f.t.server.db.fetch_one("SELECT attempted FROM canvas_connections", &[]).await.unwrap().get(0);
        assert_ne!(attempted, 0, "attempt not recorded");
    }
}
