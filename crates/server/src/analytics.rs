//! Daily analytics: counters gathered by [`crate::metrics::Meter`] are
//! flushed every minute, and gauges (totals from the database) recorded
//! every ten minutes, into `daily_metrics`. Rows hold counts only.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio_util::sync::CancellationToken;

use crate::Server;
use crate::db::{self, Sql};
use crate::metrics::metric_day;

/// How often counters are flushed.
pub(crate) const METRICS_FLUSH: Duration = Duration::from_secs(60);
/// How often gauges are recorded.
pub(crate) const GAUGE_EVERY: Duration = Duration::from_secs(10 * 60);
/// Days of metrics kept.
pub(crate) const METRICS_KEPT: i64 = 2 * 365;

/// The totals recorded each day, by metric name. Active accounts are those
/// with a device seen in the last day, week, or 30 days.
/// `$1` is the current Unix time. The cast prevents PostgreSQL from
/// inferring `INTEGER` from the subtraction operand.
pub(crate) const GAUGE_QUERIES: &[(&str, &str)] = &[
    ("accounts", "SELECT COUNT(*) FROM accounts"),
    ("active_day", "SELECT COUNT(DISTINCT account) FROM sessions WHERE last_seen>$1::BIGINT-86400"),
    ("active_week", "SELECT COUNT(DISTINCT account) FROM sessions WHERE last_seen>$1::BIGINT-7*86400"),
    ("active_month", "SELECT COUNT(DISTINCT account) FROM sessions WHERE last_seen>$1::BIGINT-30*86400"),
    ("devices", "SELECT COUNT(*) FROM sessions"),
    ("push_devices", "SELECT COUNT(*) FROM sessions WHERE push<>''"),
    ("canvas", "SELECT COUNT(*) FROM canvas_connections WHERE length(credential)>0"),
    ("feeds", "SELECT COUNT(*) FROM feed_connections"),
    ("google", "SELECT COUNT(*) FROM google_connections"),
    ("watches", "SELECT COUNT(*) FROM watches"),
    ("schedules", "SELECT COUNT(*) FROM docs WHERE type='schedule' AND deleted=0"),
    ("tasks", "SELECT COUNT(*) FROM docs WHERE type='task' AND deleted=0"),
];

impl Server {
    /// Adds the counters gathered since the last flush to their days in one
    /// transaction; a failed write puts them back for the next flush.
    pub(crate) async fn flush_counts(&self) {
        let pending = self.meter.take();
        if pending.is_empty() {
            return;
        }
        if let Err(err) = self.write_counts(&pending).await {
            tracing::warn!(%err, "analytics counts will retry");
            self.meter.restore(pending);
        }
    }

    async fn write_counts(&self, pending: &HashMap<(String, String), i64>) -> db::Result<()> {
        let mut client = self.db.client().await?;
        let tx = client.transaction().await?;
        for ((day, name), value) in pending {
            tx.exec(
                "INSERT INTO daily_metrics(day,name,value) VALUES($1,$2,$3) ON CONFLICT(day,name) DO UPDATE SET value=daily_metrics.value+excluded.value",
                &[day, name, value],
            )
            .await?;
        }
        tx.commit().await?;
        Ok(())
    }

    /// Writes today's totals (accounts, active accounts, devices,
    /// connections, watches, documents) and accounts per school as
    /// `school:<id>`.
    pub(crate) async fn record_gauges(&self) {
        if let Err((metric, err)) = self.write_gauges().await {
            match metric {
                Some(metric) => tracing::warn!(metric, %err, "analytics totals skipped"),
                None => tracing::warn!(%err, "analytics totals skipped"),
            }
        }
    }

    /// Writes the gauges; a failure names the metric being read, if any.
    async fn write_gauges(&self) -> Result<(), (Option<&'static str>, db::Error)> {
        let now = self.now();
        let day = metric_day(now);
        let unix = now.timestamp();
        let mut client = self.db.client().await.map_err(|e| (None, e))?;
        let tx = client.transaction().await.map_err(|e| (None, e.into()))?;
        let set = "INSERT INTO daily_metrics(day,name,value) VALUES($1,$2,$3) ON CONFLICT(day,name) DO UPDATE SET value=excluded.value";
        for (name, query) in GAUGE_QUERIES {
            let read = async {
                let row = if query.contains("$1") {
                    tx.fetch_one(query, &[&unix]).await?
                } else {
                    tx.fetch_one(query, &[]).await?
                };
                let value: i64 = row.try_get(0)?;
                tx.exec(set, &[&day, name, &value]).await
            };
            read.await.map_err(|e| (Some(*name), e))?;
        }
        let rows = tx
            .fetch_all(
                "SELECT COALESCE(json_text(body,'$.inst'),''),COUNT(*) FROM docs WHERE id='prefs' AND deleted=0 GROUP BY 1",
                &[],
            )
            .await
            .map_err(|e| (None, e))?;
        let mut schools: HashMap<String, i64> = HashMap::new();
        for row in rows {
            if let (Ok(inst), Ok(n)) = (row.try_get::<_, String>(0), row.try_get::<_, i64>(1))
                && self.inst(&inst).is_some()
            {
                schools.insert(inst, n);
            }
        }
        for inst in &self.insts {
            let value = schools.get(&inst.id).copied().unwrap_or_default();
            tx.exec(set, &[&day, &format!("school:{}", inst.id), &value]).await.map_err(|e| (None, e))?;
        }
        tx.commit().await.map_err(|e| (None, e.into()))
    }

    /// Fills earlier days of `signups` and `accounts` from account creation
    /// dates, once (`kv` key `metrics_backfilled`).
    pub(crate) async fn backfill_metrics(&self) {
        if !self.kv_get("metrics_backfilled").await.is_empty() {
            return;
        }
        let today = metric_day(self.now());
        for query in [
            "INSERT INTO daily_metrics(day,name,value) SELECT day,'signups',n FROM (SELECT utc_day(created_at) AS day,COUNT(*) AS n FROM accounts WHERE created_at>0 GROUP BY 1) AS totals WHERE day<$1 ON CONFLICT DO NOTHING",
            "INSERT INTO daily_metrics(day,name,value) SELECT day,'accounts',SUM(n) OVER (ORDER BY day) FROM (SELECT utc_day(created_at) AS day,COUNT(*) AS n FROM accounts WHERE created_at>0 GROUP BY 1) AS totals WHERE day<$1 ON CONFLICT DO NOTHING",
        ] {
            if let Err(err) = self.db.exec(query, &[&today]).await {
                tracing::warn!(%err, "analytics history could not be filled");
                return;
            }
        }
        self.kv_set("metrics_backfilled", "1").await;
    }

    /// Backfills, records gauges, then flushes every minute and records
    /// gauges every ten until `stop` fires, flushing once more on the way out.
    pub async fn run_metrics(self: &Arc<Self>, stop: CancellationToken) {
        tokio::select! {
            () = stop.cancelled() => {}
            () = async { self.backfill_metrics().await; self.record_gauges().await } => {}
        }
        let mut tick = tokio::time::interval(METRICS_FLUSH);
        tick.tick().await;
        let mut last = Instant::now();
        loop {
            tokio::select! {
                () = stop.cancelled() => {
                    self.flush_counts().await;
                    return;
                }
                _ = tick.tick() => {}
            }
            self.flush_counts().await;
            if last.elapsed() >= GAUGE_EVERY {
                self.record_gauges().await;
                last = Instant::now();
            }
        }
    }
}
