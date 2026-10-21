//! Process-local counters, the day's analytics counters, and API timing.
//!
//! Analytics keep one number per metric per UTC day in `daily_metrics`, for
//! the operator dashboard. Counters gather here and flush every minute (see
//! `analytics`). Nothing here holds an account, device, phone number, or
//! address.

use std::collections::HashMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicI64, Ordering};
use std::time::Duration;

use chrono::{DateTime, Utc};

use crate::Server;

/// The UTC day of an instant, `YYYY-MM-DD`.
pub(crate) fn metric_day(t: DateTime<Utc>) -> String {
    t.format("%Y-%m-%d").to_string()
}

/// Counters gathered since the last flush, keyed by day and name.
#[derive(Default)]
pub(crate) struct Meter {
    counts: Mutex<HashMap<(String, String), i64>>,
}

impl Meter {
    pub(crate) fn add(&self, day: String, name: &str, n: i64) {
        *self.counts.lock().expect("meter lock").entry((day, name.to_owned())).or_default() += n;
    }

    /// Removes and returns everything gathered.
    pub(crate) fn take(&self) -> HashMap<(String, String), i64> {
        std::mem::take(&mut *self.counts.lock().expect("meter lock"))
    }

    /// Puts back counts a failed flush could not write.
    pub(crate) fn restore(&self, pending: HashMap<(String, String), i64>) {
        let mut counts = self.counts.lock().expect("meter lock");
        for (k, v) in pending {
            *counts.entry(k).or_default() += v;
        }
    }
}

/// Cumulative request durations, in six buckets: up to 10 ms, 50 ms, 100 ms,
/// 500 ms, one second, and longer.
#[derive(Default)]
pub(crate) struct DurationStats {
    pub(crate) count: AtomicI64,
    pub(crate) nanos: AtomicI64,
    pub(crate) buckets: [AtomicI64; 6],
}

impl DurationStats {
    pub(crate) fn observe(&self, elapsed: Duration) {
        self.count.fetch_add(1, Ordering::Relaxed);
        self.nanos.fetch_add(elapsed.as_nanos() as i64, Ordering::Relaxed);
        let bounds = [10, 50, 100, 500, 1000].map(Duration::from_millis);
        let bucket = bounds.iter().position(|b| elapsed <= *b).unwrap_or(5);
        self.buckets[bucket].fetch_add(1, Ordering::Relaxed);
    }
}

/// Process-wide totals shown on the status page.
#[derive(Default)]
pub(crate) struct Counters {
    pub(crate) requests: AtomicI64,
    pub(crate) errors: AtomicI64,
    pub(crate) snapshot_skips: AtomicI64,
    pub(crate) snapshot_publications: AtomicI64,
    pub(crate) collection_queue_nanos: AtomicI64,
}

impl Server {
    /// Adds `n` to today's counter `name`.
    pub(crate) fn count(&self, name: &str, n: i64) {
        if n != 0 {
            self.meter.add(metric_day(self.now()), name, n);
        }
    }

    /// Adds one API request's time to the day's analytics.
    pub(crate) fn observe_api(&self, elapsed: Duration) {
        self.count("api_requests", 1);
        self.count("api_ms", elapsed.as_millis() as i64);
        if elapsed > Duration::from_secs(1) {
            self.count("api_slow", 1);
        }
    }
}
