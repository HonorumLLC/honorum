//! Worker coordination: one elected coordinator runs collection, the Google
//! worker, and the hourly sweep; every worker consumes the durable queues.
//! Losing the database session releases leadership.

use std::sync::Arc;
use std::time::Duration;

use tokio_util::sync::CancellationToken;

use crate::Server;

impl Server {
    /// Repeatedly takes the `coordinator` advisory lock and, while holding
    /// it, runs the monitor, the Google worker, and the hourly sweep; the
    /// three stop when the lease is lost or `stop` fires. Retries a second
    /// after losing it.
    pub async fn run_coordinator(self: &Arc<Self>, stop: CancellationToken) {
        while !stop.is_cancelled() {
            self.coordinate(&stop).await;
            tokio::select! {
                () = stop.cancelled() => return,
                () = tokio::time::sleep(Duration::from_secs(1)) => {}
            }
        }
    }

    /// Holds the coordinator lock once, running the coordinator's work
    /// until the lease is lost or `stop` fires.
    async fn coordinate(self: &Arc<Self>, stop: &CancellationToken) {
        let lease = tokio::select! {
            () = stop.cancelled() => return,
            lease = self.db.lock("coordinator", "catalog-google-maintenance") => lease,
        };
        let lease = match lease {
            Ok(lease) => lease,
            Err(err) => {
                tracing::error!(%err, "worker coordination unavailable");
                return;
            }
        };
        let work = stop.child_token();
        let lost = lease.lost();
        let watch = async {
            tokio::select! {
                () = lost.cancelled() => work.cancel(),
                () = work.cancelled() => {}
            }
        };
        let sweep = async {
            let mut tick = tokio::time::interval(Duration::from_secs(3600));
            tick.tick().await;
            loop {
                self.sweep().await;
                tokio::select! {
                    () = work.cancelled() => return,
                    _ = tick.tick() => {}
                }
            }
        };
        let monitor = async {
            self.run_monitor(work.clone()).await;
        };
        let google = async {
            self.run_google(work.clone()).await;
        };
        tokio::join!(
            async {
                tokio::join!(monitor, google, sweep);
                // Every worker has stopped; end the lease watch too.
                work.cancel();
            },
            watch
        );
        drop(lease);
    }

    /// Runs two workers draining `streak` jobs (1000 per pass, every second)
    /// until `stop` fires.
    pub async fn run_streaks(self: &Arc<Self>, stop: CancellationToken) {
        let workers = (0..2).map(|_| async {
            let mut ticker = tokio::time::interval(Duration::from_secs(1));
            ticker.tick().await;
            loop {
                self.drain_work(&stop, &["streak"], 1000, |job| async move {
                    self.account_streak(&job.account).await.map(drop)
                })
                .await;
                tokio::select! {
                    () = stop.cancelled() => return,
                    _ = ticker.tick() => {}
                }
            }
        });
        futures::future::join_all(workers).await;
    }
}
