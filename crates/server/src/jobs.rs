//! The durable work queue in `work_jobs`.
//!
//! Imports, reminder rebuilds, and streak maintenance are rows keyed by kind
//! and key. A worker claims one due row with `FOR UPDATE SKIP LOCKED`, renews
//! its two-minute lease every thirty seconds, and acknowledges only the owner
//! and generation it claimed. A crash leaves the row to be claimed again
//! when its lease lapses, and a newer edit, which bumps the generation,
//! cannot be erased by an older acknowledgement.

use std::future::Future;
use std::time::Duration;

use tokio_util::sync::CancellationToken;

use crate::Server;
use crate::db::{self, Sql};
use crate::session::new_id;

/// The longest one job may run.
const JOB_TIMEOUT: Duration = Duration::from_secs(180);

/// A claimed job.
#[derive(Clone, Debug)]
pub(crate) struct WorkJob {
    pub(crate) kind: String,
    pub(crate) key: String,
    pub(crate) account: String,
    pub(crate) payload: String,
    pub(crate) owner: String,
    pub(crate) generation: i64,
    pub(crate) attempts: i64,
}

impl Server {
    /// Claims the earliest due job of the given kinds, if any.
    pub(crate) async fn claim_work(&self, kinds: &[&str]) -> db::Result<Option<WorkJob>> {
        let now = self.now().timestamp();
        let owner = new_id();
        let lease = now + 120;
        let row = self
            .db
            .fetch_opt(
                "UPDATE work_jobs SET lease_until=$1,owner=$2 WHERE (kind,key)=(SELECT kind,key FROM work_jobs WHERE run_at<=$3 AND lease_until<=$3 AND kind=ANY($4) ORDER BY run_at LIMIT 1 FOR UPDATE SKIP LOCKED) RETURNING kind,key,account,payload,generation,attempts",
                &[&lease, &owner, &now, &kinds],
            )
            .await?;
        let Some(row) = row else { return Ok(None) };
        Ok(Some(WorkJob {
            kind: row.try_get(0)?,
            key: row.try_get(1)?,
            account: row.try_get(2)?,
            payload: row.try_get(3)?,
            generation: row.try_get(4)?,
            attempts: row.try_get(5)?,
            owner,
        }))
    }

    /// Runs one claimed job, renewing its lease, and records the result. A
    /// failure is retried after 15 seconds, doubling per attempt up to an
    /// hour. The job stops if its lease cannot be renewed, since another
    /// worker may then claim it.
    pub(crate) async fn run_work<F, Fut>(&self, job: WorkJob, run: F)
    where
        F: FnOnce(WorkJob) -> Fut,
        Fut: Future<Output = anyhow::Result<()>>,
    {
        let renew = async {
            let mut ticker = tokio::time::interval(Duration::from_secs(30));
            ticker.tick().await;
            loop {
                ticker.tick().await;
                let renewed = self
                    .db
                    .exec(
                        "UPDATE work_jobs SET lease_until=$1 WHERE kind=$2 AND key=$3 AND owner=$4",
                        &[&(self.now().timestamp() + 120), &job.kind, &job.key, &job.owner],
                    )
                    .await;
                if !matches!(renewed, Ok(1)) {
                    return;
                }
            }
        };
        let result = tokio::select! {
            result = tokio::time::timeout(JOB_TIMEOUT, run(job.clone())) => {
                result.unwrap_or_else(|_| Err(anyhow::anyhow!("job timed out")))
            }
            () = renew => Err(anyhow::anyhow!("job lease lost")),
        };
        let release = "UPDATE work_jobs SET owner='',lease_until=0 WHERE kind=$1 AND key=$2 AND owner=$3";
        let acknowledged = tokio::time::timeout(Duration::from_secs(5), async {
            let saved = match &result {
                Ok(()) => {
                    let deleted = self
                        .db
                        .exec(
                            "DELETE FROM work_jobs WHERE kind=$1 AND key=$2 AND owner=$3 AND generation=$4",
                            &[&job.kind, &job.key, &job.owner, &job.generation],
                        )
                        .await;
                    // A newer generation stays queued; this runner cannot erase it.
                    self.exec_logged(release, &[&job.kind, &job.key, &job.owner]).await;
                    deleted.map(drop)
                }
                Err(err) => {
                    tracing::warn!(kind = %job.kind, %err, "background job will retry");
                    let delay = (15_i64 << job.attempts.min(8)).min(3600);
                    let saved = self
                        .db
                        .exec(
                            "UPDATE work_jobs SET attempts=attempts+1,run_at=$1,lease_until=0,owner='',error='job failed; see server logs' WHERE kind=$2 AND key=$3 AND owner=$4 AND generation=$5",
                            &[&(self.now().timestamp() + delay), &job.kind, &job.key, &job.owner, &job.generation],
                        )
                        .await;
                    self.exec_logged(release, &[&job.kind, &job.key, &job.owner]).await;
                    saved.map(drop)
                }
            };
            if let Err(err) = saved {
                tracing::error!(kind = %job.kind, %err, "background job result could not be saved");
            }
        })
        .await;
        if acknowledged.is_err() {
            tracing::error!(kind = %job.kind, "background job result could not be saved in time");
        }
    }

    /// Claims and runs up to `limit` jobs of the given kinds, one at a time,
    /// until none are due or `stop` fires.
    pub(crate) async fn drain_work<F, Fut>(&self, stop: &CancellationToken, kinds: &[&str], limit: usize, run: F)
    where
        F: Fn(WorkJob) -> Fut,
        Fut: Future<Output = anyhow::Result<()>>,
    {
        for _ in 0..limit {
            if stop.is_cancelled() {
                return;
            }
            let job = match self.claim_work(kinds).await {
                Ok(Some(job)) => job,
                Ok(None) => return,
                Err(err) => {
                    tracing::warn!(%err, "background queue unavailable");
                    return;
                }
            };
            self.run_work(job, &run).await;
        }
    }
}
