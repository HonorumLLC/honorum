//! In-process wakeups for an account's live connections.
//!
//! The hub is a signal only. The document sequence in the database is the
//! durable record, and a client that misses a wakeup catches up on its next
//! poll. PostgreSQL `LISTEN/NOTIFY` carries the same signal between replicas
//! (see `events`), and the process's heartbeat delivers each stream a frame
//! read for every stream at once.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use tokio::sync::Notify;

#[derive(Default)]
pub(crate) struct Hub {
    subs: Mutex<HashMap<String, Vec<Arc<Slot>>>>,
    /// Subscriptions made, which assigns each its phase.
    made: AtomicU64,
}

/// One live connection's signals.
pub(crate) struct Slot {
    session: String,
    /// Which of the heartbeat period's ticks reads this stream.
    phase: u64,
    wake: Notify,
    /// The account changed since the stream last woke.
    changed: AtomicBool,
    /// The latest heartbeat for the stream, not yet taken.
    beat: Mutex<Option<Beat>>,
}

/// A heartbeat: the account's sequence and Google Calendar version, or
/// `None` when the session or account no longer exists.
pub(crate) type Beat = Option<(i64, i64)>;

/// Why a subscription woke.
#[derive(Debug)]
pub(crate) enum Wakeup {
    /// The account changed; read it again.
    Changed,
    /// A heartbeat read for this stream.
    Beat(Beat),
}

impl Slot {
    /// Hands the stream a heartbeat, replacing one it has not taken.
    pub(crate) fn beat(&self, beat: Beat) {
        *self.beat.lock().expect("beat lock") = Some(beat);
        self.wake.notify_one();
    }

    /// The session the stream belongs to.
    pub(crate) fn session(&self) -> &str {
        &self.session
    }
}

/// One subscription. Dropping it unsubscribes.
pub(crate) struct Subscription<'a> {
    hub: &'a Hub,
    account: String,
    slot: Arc<Slot>,
}

impl Subscription<'_> {
    /// Waits for the next wakeup. Wakeups that arrive while nobody waits
    /// collapse into one, and a change outranks a heartbeat, which may have
    /// been read before it.
    pub(crate) async fn woken(&self) -> Wakeup {
        self.slot.wake.notified().await;
        let changed = self.slot.changed.swap(false, Ordering::AcqRel);
        let beat = self.slot.beat.lock().expect("beat lock").take();
        match beat {
            Some(beat) if !changed => Wakeup::Beat(beat),
            _ => Wakeup::Changed,
        }
    }
}

impl Drop for Subscription<'_> {
    fn drop(&mut self) {
        let mut subs = self.hub.subs.lock().expect("hub lock");
        if let Some(list) = subs.get_mut(&self.account) {
            list.retain(|n| !Arc::ptr_eq(n, &self.slot));
            if list.is_empty() {
                subs.remove(&self.account);
            }
        }
    }
}

impl Hub {
    /// Subscribes a connection of `session` to `account`'s wakeups.
    pub(crate) fn subscribe(&self, account: &str, session: &str) -> Subscription<'_> {
        let slot = Arc::new(Slot {
            session: session.to_owned(),
            phase: self.made.fetch_add(1, Ordering::Relaxed),
            wake: Notify::new(),
            changed: AtomicBool::new(false),
            beat: Mutex::new(None),
        });
        self.subs.lock().expect("hub lock").entry(account.to_owned()).or_default().push(slot.clone());
        Subscription { hub: self, account: account.to_owned(), slot }
    }

    /// Wakes every live connection of `account`.
    pub(crate) fn notify(&self, account: &str) {
        if account.is_empty() {
            return;
        }
        if let Some(list) = self.subs.lock().expect("hub lock").get(account) {
            for slot in list {
                slot.changed.store(true, Ordering::Release);
                slot.wake.notify_one();
            }
        }
    }

    /// The accounts with live connections in this process.
    pub(crate) fn accounts(&self) -> Vec<String> {
        self.subs.lock().expect("hub lock").keys().cloned().collect()
    }

    /// The live connections whose turn `tick` is when connections take
    /// turns across `phases` ticks, with their accounts.
    pub(crate) fn slots(&self, tick: u64, phases: u64) -> Vec<(String, Arc<Slot>)> {
        let subs = self.subs.lock().expect("hub lock");
        subs.iter()
            .flat_map(|(account, list)| {
                list.iter()
                    .filter(|slot| slot.phase % phases == tick % phases)
                    .map(|slot| (account.clone(), slot.clone()))
            })
            .collect()
    }
}
