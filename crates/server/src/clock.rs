//! The server's clock. Tests move it; everything else reads it through
//! [`Server::now`](crate::Server::now) rather than the system clock, so a
//! test can stand at any instant without affecting other tests.

use std::sync::RwLock;

use chrono::{DateTime, TimeDelta, Utc};

#[derive(Default)]
pub(crate) struct Clock {
    fixed: RwLock<Option<DateTime<Utc>>>,
    offset: RwLock<TimeDelta>,
}

impl Clock {
    pub(crate) fn now(&self) -> DateTime<Utc> {
        let fixed = *self.fixed.read().expect("clock lock");
        fixed.unwrap_or_else(Utc::now) + *self.offset.read().expect("clock lock")
    }

    /// Stops the clock at `at`.
    #[cfg(test)]
    pub(crate) fn set(&self, at: DateTime<Utc>) {
        *self.fixed.write().expect("clock lock") = Some(at);
        *self.offset.write().expect("clock lock") = TimeDelta::zero();
    }

    /// Runs the clock `by` ahead of real time.
    #[cfg(test)]
    pub(crate) fn shift(&self, by: TimeDelta) {
        *self.fixed.write().expect("clock lock") = None;
        *self.offset.write().expect("clock lock") = by;
    }

    /// Returns to real time.
    #[cfg(test)]
    pub(crate) fn reset(&self) {
        *self.fixed.write().expect("clock lock") = None;
        *self.offset.write().expect("clock lock") = TimeDelta::zero();
    }
}
