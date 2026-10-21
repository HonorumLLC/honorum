//! The push delivery queue: one `deliveries` row per destination, queued in
//! the same transaction as the event that caused it, claimed with a lease,
//! and retried with backoff.

use std::sync::Arc;
use std::time::Duration;

use sha2::{Digest, Sha256};
use tokio_util::sync::CancellationToken;

use crate::Server;
use crate::db::{self, Sql, Tx};
use crate::push::Alert;
use crate::session::new_id;
use crate::state::Institution;
use crate::watches::Watch;

/// A claimed delivery.
#[derive(Clone, Debug, Default)]
pub(crate) struct Delivery {
    pub(crate) id: String,
    pub(crate) event: String,
    /// `watch` or `reminder`.
    pub(crate) source: String,
    pub(crate) source_id: String,
    pub(crate) account: String,
    /// The session the destination belongs to, for account destinations.
    pub(crate) device: String,
    pub(crate) target: String,
    /// The [`Alert`] as JSON.
    pub(crate) payload: String,
    pub(crate) attempts: i64,
    pub(crate) expires: i64,
    /// The lease this claim holds (`lease_until`).
    pub(crate) lease: i64,
}

impl Server {
    /// Queues `alert` to each destination of `watch` inside `tx`: its own
    /// subscription, or every push-enabled device of its account. Delivery
    /// IDs hash `event|push|target`, so a repeat is a no-op.
    pub(crate) async fn enqueue(
        &self,
        tx: &Tx<'_>,
        source: &str,
        watch: &Watch,
        alert: &Alert,
        event: &str,
        expires: i64,
    ) -> db::Result<()> {
        // (device, target) pairs: the watch's own subscription, or every
        // device signed in to its account.
        let mut destinations = vec![(String::new(), watch.target.clone())];
        if !watch.account.is_empty() {
            destinations.clear();
            for row in
                tx.fetch_all("SELECT id,push FROM sessions WHERE account=$1 AND push<>''", &[&watch.account]).await?
            {
                destinations.push((row.try_get(0)?, row.try_get::<_, Option<String>>(1)?.unwrap_or_default()));
            }
        }
        let payload = serde_json::to_string(alert).expect("alerts serialize");
        let now = self.now().timestamp();
        for (device, target) in &destinations {
            let id = hex::encode(Sha256::digest(format!("{event}|push|{target}").as_bytes()));
            tx.exec(
                "INSERT INTO deliveries(id,event_key,source,source_id,account,device,channel,target,payload,due_at,expires_at) VALUES($1,$2,$3,$4,$5,$6,'push',$7,$8,$9,$10) ON CONFLICT DO NOTHING",
                &[&id, &event, &source, &watch.id, &watch.account, device, target, &payload, &now, &expires],
            )
            .await?;
        }
        Ok(())
    }

    /// Claims the earliest due delivery for 90 seconds, if any.
    pub(crate) async fn claim_delivery(&self) -> db::Result<Option<Delivery>> {
        let now = self.now().timestamp();
        let lease = now + 90;
        let row = self
            .db
            .fetch_opt(
                "UPDATE deliveries SET lease_until=$1 WHERE id=(SELECT id FROM deliveries WHERE delivered_at=0 AND due_at<=$2 AND lease_until<=$2 ORDER BY due_at LIMIT 1 FOR UPDATE SKIP LOCKED) RETURNING id,event_key,source,source_id,account,device,target,payload,attempts,expires_at",
                &[&lease, &now],
            )
            .await?;
        let Some(row) = row else { return Ok(None) };
        Ok(Some(Delivery {
            id: row.try_get(0)?,
            event: row.try_get(1)?,
            source: row.try_get(2)?,
            source_id: row.try_get(3)?,
            account: row.try_get(4)?,
            device: row.try_get(5)?,
            target: row.try_get(6)?,
            payload: row.try_get(7)?,
            attempts: row.try_get(8)?,
            expires: row.try_get(9)?,
            lease,
        }))
    }

    /// Records a delivery's end: `at` is the delivery time, or -1 when it
    /// stopped with `message`. Counts the notification or, unless the
    /// delivery was cancelled or its device removed, the failure.
    async fn finish_delivery(&self, d: &Delivery, at: i64, message: &str) {
        if let Err(err) = self
            .db
            .exec(
                "UPDATE deliveries SET delivered_at=$1,error=$2,lease_until=0 WHERE id=$3 AND lease_until=$4",
                &[&at, &message, &d.id, &d.lease],
            )
            .await
        {
            tracing::error!(%err, "delivery result could not be saved");
        }
        if at > 0 {
            self.count("notifications", 1);
        } else if message != "cancelled" && message != "device removed" {
            self.count("notification_failures", 1);
        }
    }

    /// Sends one claimed delivery and records the result: expired,
    /// cancelled (source gone), device removed, delivered, or a retry after
    /// 5 seconds doubling up to 5 minutes (at most ten attempts). A gone
    /// destination is forgotten.
    pub(crate) async fn deliver_job(&self, d: Delivery) {
        let now = self.now().timestamp();
        if d.expires <= now {
            return self.finish_delivery(&d, -1, "expired").await;
        }
        let table = if d.source == "reminder" { "reminders" } else { "watches" };
        let exists = if d.source == "reminder" && !d.account.is_empty() {
            // Account reminders come from the preferences document, not a registration.
            self.db
                .fetch_one(
                    "SELECT COUNT(*) FROM docs WHERE account=$1 AND id='prefs' AND deleted=0 AND json_text(body,'$.reminders')='true'",
                    &[&d.account],
                )
                .await
        } else {
            self.db.fetch_one(&format!("SELECT COUNT(*) FROM {table} WHERE id=$1"), &[&d.source_id]).await
        };
        let exists: i64 = match exists.and_then(|row| Ok(row.try_get(0)?)) {
            Ok(n) => n,
            Err(err) => {
                tracing::warn!(%err, "delivery source could not be checked; lease will retry");
                return;
            }
        };
        if exists == 0 {
            return self.finish_delivery(&d, -1, "cancelled").await;
        }
        if !d.device.is_empty() {
            let present = self
                .db
                .fetch_one(
                    "SELECT COUNT(*) FROM sessions WHERE id=$1 AND account=$2 AND push=$3",
                    &[&d.device, &d.account, &d.target],
                )
                .await
                .and_then(|row| Ok(row.try_get::<_, i64>(0)?));
            match present {
                Err(_) => return,
                Ok(0) => return self.finish_delivery(&d, -1, "device removed").await,
                Ok(_) => {}
            }
        }
        let Ok(alert) = serde_json::from_str::<Alert>(&d.payload) else {
            return self.finish_delivery(&d, -1, "invalid payload").await;
        };
        let (gone, result) = self.send_alert(&d.target, &alert).await;
        if gone {
            if d.device.is_empty() {
                self.exec_logged(&format!("DELETE FROM {table} WHERE id=$1 AND target=$2"), &[&d.source_id, &d.target])
                    .await;
            } else {
                self.exec_logged("UPDATE sessions SET push='' WHERE id=$1 AND push=$2", &[&d.device, &d.target]).await;
            }
        }
        let err = match result {
            Ok(()) => {
                self.finish_delivery(&d, now, "").await;
                if d.source == "watch" {
                    self.exec_logged("UPDATE alerts SET delivered_at=$1,error='' WHERE key=$2", &[&now, &d.event])
                        .await;
                }
                return;
            }
            Err(err) => err.to_string(),
        };
        if d.source == "watch" {
            self.exec_logged("UPDATE alerts SET error=$1 WHERE key=$2 AND delivered_at=0", &[&err, &d.event]).await;
        }
        if gone || d.attempts >= 9 {
            return self.finish_delivery(&d, -1, &err).await;
        }
        let retry = (5_i64 << d.attempts.clamp(0, 6)).min(300);
        if let Err(e) = self
            .db
            .exec(
                "UPDATE deliveries SET attempts=attempts+1,due_at=$1,lease_until=0,error=$2 WHERE id=$3 AND lease_until=$4",
                &[&(now + retry), &err, &d.id, &d.lease],
            )
            .await
        {
            tracing::error!(err = %e, "delivery retry could not be saved");
        }
    }

    /// [`Server::dispatch_until`] without a stop signal, so tests drive
    /// delivery without waiting.
    #[cfg(test)]
    pub(crate) async fn dispatch_pending(&self) {
        self.dispatch_until(&CancellationToken::new()).await;
    }

    /// Claims and sends up to 100 due deliveries, stopping between
    /// deliveries once `stop` fires. A send in progress finishes, so its
    /// result is recorded.
    async fn dispatch_until(&self, stop: &CancellationToken) {
        for _ in 0..100 {
            if stop.is_cancelled() {
                return;
            }
            match self.claim_delivery().await {
                Ok(Some(d)) => self.deliver_job(d).await,
                Ok(None) => return,
                Err(err) => {
                    tracing::warn!(%err, "delivery queue");
                    return;
                }
            }
        }
    }

    /// Runs four delivery workers, each dispatching every second, until
    /// `stop` fires.
    pub async fn run_deliveries(self: &Arc<Self>, stop: CancellationToken) {
        let workers = (0..4).map(|_| async {
            let mut ticker = tokio::time::interval(Duration::from_secs(1));
            ticker.tick().await;
            loop {
                self.dispatch_until(&stop).await;
                tokio::select! {
                    () = stop.cancelled() => return,
                    _ = ticker.tick() => {}
                }
            }
        });
        futures::future::join_all(workers).await;
    }

    /// Queues "COURSE NUM has an open seat" alerts inside `tx` for every
    /// watch of a section that opened (`detected` in Unix seconds), skipping
    /// watches alerted within the debounce window.
    pub(crate) async fn queue_open(
        &self,
        tx: &Tx<'_>,
        inst: &Institution,
        term: &str,
        section: &str,
        _available: i64,
        course: &str,
        number: &str,
        title: &str,
        detected: i64,
    ) -> db::Result<()> {
        let rows = tx
            .fetch_all(
                "SELECT id,target,last_alert,account FROM watches WHERE inst=$1 AND term=$2 AND section=$3",
                &[&inst.id, &term, &section],
            )
            .await?;
        let mut watches = Vec::with_capacity(rows.len());
        for row in rows {
            watches.push(Watch {
                id: row.try_get(0)?,
                target: row.try_get::<_, Option<String>>(1)?.unwrap_or_default(),
                last_alert: row.try_get::<_, Option<i64>>(2)?.unwrap_or_default(),
                account: row.try_get::<_, Option<String>>(3)?.unwrap_or_default(),
            });
        }
        for w in &watches {
            if detected - w.last_alert < self.cfg.debounce_minutes * 60 {
                continue;
            }
            let event = format!("{section}|{}|{}", w.id, new_id());
            tx.exec("INSERT INTO alerts(key,detected_at) VALUES($1,$2) ON CONFLICT DO NOTHING", &[&event, &detected])
                .await?;
            let alert = Alert {
                title: format!("{course} {number} has an open seat"),
                body: format!("{title} at {}. Register through your university now.", inst.name),
                url: format!("{}/#{}/{term}/{section}", self.cfg.base_url, inst.id),
            };
            self.enqueue(tx, "watch", w, &alert, &event, detected + 15 * 60).await?;
            // This timestamps queued transitions. Failed destinations retain their retry jobs.
            tx.exec("UPDATE watches SET last_alert=$1 WHERE id=$2", &[&detected, &w.id]).await?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

    use chrono::{TimeZone, Utc};
    use tokio_util::sync::CancellationToken;

    use crate::Server;
    use crate::db::Sql;
    use crate::push::{Alert, SendOutcome};
    use crate::snapshot::snapshot_key;
    use crate::test_support::{count, test_server};
    use crate::watches::Watch;

    /// Installs a sender that counts calls in `sent` and answers with `outcome`.
    fn counting(s: &Server, sent: &Arc<AtomicUsize>, fail: Option<&'static str>) {
        let sent = sent.clone();
        s.set_sender(Some(Arc::new(move |_: &str, _: &Alert| -> SendOutcome {
            sent.fetch_add(1, Ordering::SeqCst);
            (false, fail.map_or(Ok(()), |m| Err(anyhow::anyhow!(m))))
        })));
    }

    #[tokio::test]
    async fn durable_delivery_retries_and_cancellation() {
        let t = test_server().await;
        let s = &t.server;
        let inst = s.insts[0].clone();
        s.sync_catalog(&inst, None, true).await.unwrap();
        s.db.exec(
            "INSERT INTO watches(id,inst,term,section,channel,target,created_at) VALUES('w','demo','2026FA','10001','push','sub',0)",
            &[],
        )
        .await
        .unwrap();
        t.fake.set_seats("10001", 3);
        s.poll(&inst).await.unwrap();
        let sent = Arc::new(AtomicUsize::new(0));
        counting(s, &sent, Some("temporary outage"));
        s.dispatch_pending().await;
        let pending = count(s, "SELECT COUNT(*) FROM deliveries WHERE delivered_at=0 AND attempts=1").await;
        assert_eq!((sent.load(Ordering::SeqCst), pending), (1, 1));
        // Recreate the service: retry state survives process lifetime.
        let restored = Server::new(s.db.clone(), s.cfg.clone(), s.insts.clone(), s.data_dir.clone()).await.unwrap();
        counting(&restored, &sent, None);
        s.db.exec("UPDATE deliveries SET due_at=0", &[]).await.unwrap();
        restored.dispatch_pending().await;
        assert_eq!(sent.load(Ordering::SeqCst), 2, "retry not delivered");
        restored.dispatch_pending().await;
        assert_eq!(sent.load(Ordering::SeqCst), 2, "delivered job retried");
        // A separate queued transition is cancelled when the watch is removed.
        s.db.exec("UPDATE watches SET last_alert=0", &[]).await.unwrap();
        t.fake.set_seats("10001", 0);
        let _ = s.poll(&inst).await;
        t.fake.set_seats("10001", 2);
        let _ = s.poll(&inst).await;
        s.db.exec("DELETE FROM watches WHERE id='w'", &[]).await.unwrap();
        restored.dispatch_pending().await;
        assert_eq!(sent.load(Ordering::SeqCst), 2, "deleted watch delivered");
    }

    #[tokio::test]
    async fn reminder_catches_up_after_short_stall() {
        let t = test_server().await;
        let s = &t.server;
        s.clock.set(Utc.with_ymd_and_hms(2026, 8, 24, 13, 48, 0).unwrap());
        s.db.exec(
            r#"INSERT INTO reminders(id,target,tz,items,updated_at) VALUES('r','{}','America/Chicago','[{"label":"Math","days":"M","start":540}]',0)"#,
            &[],
        )
        .await
        .unwrap();
        let sent = Arc::new(AtomicUsize::new(0));
        counting(s, &sent, None);
        let stop = CancellationToken::new();
        // Two scheduler ticks per round.
        for _ in 0..2 {
            s.tick_scheduled_reminders(&stop).await;
            s.tick_scheduled_reminders(&stop).await;
            s.dispatch_pending().await;
        }
        assert_eq!(sent.load(Ordering::SeqCst), 1, "expected one catch-up reminder");
    }

    #[tokio::test]
    async fn seat_polling_does_not_rewrite_structural_snapshot() {
        let t = test_server().await;
        let s = &t.server;
        let inst = s.insts[0].clone();
        let _ = s.sync_catalog(&inst, None, true).await;
        let key = snapshot_key("structure", "demo", "2026FA");
        let hash = s.kv_get(&key).await;
        s.db.exec(
            "INSERT INTO watches(id,inst,term,section,channel,target,created_at) VALUES('w','demo','2026FA','10001','push','sub',0)",
            &[],
        )
        .await
        .unwrap();
        t.fake.set_seats("10001", 1);
        let _ = s.poll(&inst).await;
        let next = s.kv_get(&key).await;
        assert!(next == hash && !next.trim().is_empty(), "seat change replaced catalog");
    }

    #[tokio::test]
    async fn delivery_retries_only_failed_destination() {
        let t = test_server().await;
        let s = &t.server;
        for statement in [
            "INSERT INTO accounts(id,phone,created_at) VALUES('a','+12015550123',0)",
            "INSERT INTO sessions(id,account,push) VALUES('laptop','a','laptop-target'),('phone','a','phone-target')",
            r#"INSERT INTO docs(account,id,type,seq,body) VALUES('a','prefs','prefs',1,'{"reminders":true}')"#,
        ] {
            s.db.exec(statement, &[]).await.unwrap();
        }
        let mut client = s.db.client().await.unwrap();
        let tx = client.transaction().await.unwrap();
        let watch = Watch { id: "acct:a".into(), account: "a".into(), ..Watch::default() };
        let alert = Alert { title: "Class".into(), ..Alert::default() };
        s.enqueue(&tx, "reminder", &watch, &alert, "event", s.now().timestamp() + 3600).await.unwrap();
        tx.commit().await.unwrap();
        let calls: Arc<Mutex<HashMap<String, usize>>> = Arc::default();
        {
            let calls = calls.clone();
            s.set_sender(Some(Arc::new(move |target: &str, _: &Alert| -> SendOutcome {
                let mut calls = calls.lock().unwrap();
                let n = calls.entry(target.to_owned()).or_default();
                *n += 1;
                if target == "phone-target" && *n == 1 {
                    return (false, Err(anyhow::anyhow!("push service unavailable")));
                }
                (false, Ok(()))
            })));
        }
        s.dispatch_pending().await;
        s.db.exec("UPDATE deliveries SET due_at=0", &[]).await.unwrap();
        s.dispatch_pending().await;
        let calls = calls.lock().unwrap();
        assert_eq!((calls.get("laptop-target"), calls.get("phone-target")), (Some(&1), Some(&2)), "{calls:?}");
    }
}
