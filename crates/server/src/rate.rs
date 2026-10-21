//! Limits shared by every replica through the database: sliding-window
//! request rates keyed by hashed identifiers, and four leased connection
//! slots per provider host for account imports. A slot a crashed process
//! held frees itself when its lease ends.
//!
//! General API writes go through [`LocalLimits`] first, which admits a
//! key's requests in process while it is well under budget and records them
//! in the database with the key's next read. Sign-in limits read the
//! database on every request.

use std::collections::{HashMap, VecDeque};
use std::sync::Mutex;
use std::time::Duration;

use sha2::{Digest, Sha256};

use crate::Server;
use crate::db::{self, Sql};
use crate::session::new_id;

impl Server {
    /// Counts one request against `key`'s sliding window of `period_secs`,
    /// across replicas. Keys are hashed so the table holds no phone numbers,
    /// account IDs, or addresses.
    pub(crate) async fn rate_allow(&self, key: &str, budget: i32, period_secs: i64) -> db::Result<bool> {
        let hashed = hex::encode(Sha256::digest(key.as_bytes()));
        let now = self.now().timestamp();
        let row = self
            .db
            .fetch_one(
                "SELECT rate_allow($1,$2,$3,$4,$5)",
                &[&hashed, &budget, &(now - period_secs), &now, &(now + period_secs)],
            )
            .await?;
        Ok(row.try_get(0)?)
    }

    /// Counts one API request against `key`'s sliding window of
    /// `period_secs`, like [`Server::rate_allow`], but reads the database
    /// only when this process's view of the key is more than
    /// [`LOCAL_FRESH_SECS`] old or past half of `budget`. Requests admitted
    /// in between are recorded with the next read.
    ///
    /// Each replica admits at most half the budget on its own, so two
    /// replicas stay within it; near the limit every request reads the
    /// database.
    pub(crate) async fn request_allow(&self, key: &str, budget: i32, period_secs: i64) -> db::Result<bool> {
        let now = self.now().timestamp();
        let cutoff = now - period_secs;
        let unsent = {
            let mut limits = self.limits.lock();
            let w = limits.window(key, now, period_secs);
            w.forget(cutoff);
            if w.read_at > 0 && now - w.read_at < LOCAL_FRESH_SECS && w.others + w.admitted.len() < budget as usize / 2
            {
                w.admitted.push_back(now);
                w.unsent.push(now);
                return Ok(true);
            }
            std::mem::take(&mut w.unsent)
        };
        let hashed = hex::encode(Sha256::digest(key.as_bytes()));
        let read = self
            .db
            .fetch_one(
                "SELECT rate_record($1,$2,$3,$4,$5,$6)",
                &[&hashed, &budget, &cutoff, &unsent, &now, &(now + period_secs)],
            )
            .await
            .and_then(|row| Ok(row.try_get::<_, i32>(0)?));
        let mut limits = self.limits.lock();
        let w = limits.window(key, now, period_secs);
        let recorded = match read {
            Ok(recorded) => recorded,
            Err(err) => {
                // Keep admitted requests for the next read.
                w.unsent.extend(unsent);
                return Err(err);
            }
        };
        let allowed = recorded < budget;
        if allowed {
            w.admitted.push_back(now);
        }
        w.others = (recorded.max(0) as usize + usize::from(allowed)).saturating_sub(w.admitted.len());
        w.read_at = now;
        Ok(allowed)
    }

    /// Waits for one of four import slots for a provider host, shared by every
    /// process. An import has a 30-second request budget, shorter than the
    /// two-minute lease a crash leaves behind. Dropping the returned
    /// [`ProviderSlot`] frees it.
    pub(crate) async fn provider_slot(&self, host: &str) -> db::Result<ProviderSlot<'_>> {
        self.db
            .exec(
                "INSERT INTO provider_slots(host,slot) VALUES($1,0),($2,1),($3,2),($4,3) ON CONFLICT DO NOTHING",
                &[&host, &host, &host, &host],
            )
            .await?;
        let owner = new_id();
        loop {
            let now = self.now().timestamp();
            let row = self
                .db
                .fetch_opt(
                    "UPDATE provider_slots SET owner=$1,lease_until=$2 WHERE (host,slot)=(SELECT host,slot FROM provider_slots WHERE host=$3 AND lease_until<=$4 ORDER BY slot LIMIT 1 FOR UPDATE SKIP LOCKED) RETURNING slot",
                    &[&owner, &(now + 120), &host, &now],
                )
                .await?;
            if let Some(row) = row {
                return Ok(ProviderSlot { server: self, host: host.to_owned(), slot: row.try_get(0)?, owner });
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }
}

/// A held provider slot.
pub(crate) struct ProviderSlot<'a> {
    server: &'a Server,
    host: String,
    slot: i64,
    owner: String,
}

impl Drop for ProviderSlot<'_> {
    fn drop(&mut self) {
        let db = self.server.db.clone();
        let (host, slot, owner) = (std::mem::take(&mut self.host), self.slot, std::mem::take(&mut self.owner));
        tokio::spawn(async move {
            let release = async {
                db.exec(
                    "UPDATE provider_slots SET owner='',lease_until=0 WHERE host=$1 AND slot=$2 AND owner=$3",
                    &[&host, &slot, &owner],
                )
                .await
            };
            if let Ok(Err(err)) = tokio::time::timeout(Duration::from_secs(5), release).await {
                tracing::error!(%err, "provider slot could not be released");
            }
        });
    }
}

/// Seconds a key's count of other replicas' requests is trusted before the
/// next request reads the database.
const LOCAL_FRESH_SECS: i64 = 5;
/// Keys [`LocalLimits`] holds before it drops idle ones.
const LOCAL_KEYS: usize = 10_000;

/// This process's view of API request limits (see
/// [`Server::request_allow`]).
#[derive(Default)]
pub(crate) struct LocalLimits {
    state: Mutex<LocalState>,
}

impl LocalLimits {
    fn lock(&self) -> std::sync::MutexGuard<'_, LocalState> {
        self.state.lock().expect("limits lock")
    }
}

#[derive(Default)]
struct LocalState {
    keys: HashMap<String, Window>,
    /// The key count that triggers the next sweep of idle keys.
    sweep_at: usize,
}

impl LocalState {
    /// The window for `key`, extended to `now + period`. Sweeps idle keys
    /// when the map has doubled since the last sweep.
    fn window(&mut self, key: &str, now: i64, period: i64) -> &mut Window {
        if self.keys.len() >= self.sweep_at.max(LOCAL_KEYS) {
            // A key idle for its whole period has nothing left to send.
            self.keys.retain(|_, w| w.expires > now);
            self.sweep_at = self.keys.len() * 2;
        }
        let w = self.keys.entry(key.to_owned()).or_default();
        w.expires = now + period;
        w
    }
}

/// One key's requests as this process knows them.
#[derive(Default)]
struct Window {
    /// Requests this process admitted within the period, oldest first.
    admitted: VecDeque<i64>,
    /// Admitted requests the database has not recorded yet.
    unsent: Vec<i64>,
    /// Requests other replicas had recorded at the last read.
    others: usize,
    /// When the database was last read; 0 before the first read.
    read_at: i64,
    /// When the key's last request leaves the window.
    expires: i64,
}

impl Window {
    /// Drops requests at or before `cutoff`.
    fn forget(&mut self, cutoff: i64) {
        while self.admitted.front().is_some_and(|&t| t <= cutoff) {
            self.admitted.pop_front();
        }
        self.unsent.retain(|&t| t > cutoff);
    }
}
