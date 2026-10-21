//! A fixture-backed adapter for a fictional university. It lets the whole
//! system run without network access and serves as a template for real
//! adapters.
//!
//! The terms and sections come from `fixture.json`, embedded at build time.
//! The adapter makes no requests. Unless `static_seats` is set, each
//! availability check opens or closes a watched section's seats at random,
//! so alerts can be exercised.

use std::collections::HashSet;
use std::sync::Mutex;

use async_trait::async_trait;
use rand::RngExt;
use serde::Deserialize;

use crate::{Adapter, AdapterError, Seats, Section, Term};

const FIXTURE: &str = include_str!("fixture.json");

/// The demo institution. Seat counts persist in memory between checks.
pub struct Demo {
    /// When true, seat counts never change. Tests set it for deterministic
    /// results.
    pub static_seats: bool,
    terms: Vec<Term>,
    sections: Mutex<Vec<Section>>,
}

#[derive(Deserialize)]
struct Fixture {
    terms: Vec<Term>,
    sections: Vec<Section>,
}

impl Demo {
    /// Loads the embedded fixture with randomly changing seat counts.
    ///
    /// # Panics
    ///
    /// Panics if the embedded fixture is not valid, which is a build defect.
    pub fn new() -> Self {
        let fixture: Fixture = serde_json::from_str(FIXTURE).expect("the embedded demo fixture is valid");
        Self { static_seats: false, terms: fixture.terms, sections: Mutex::new(fixture.sections) }
    }
}

impl Default for Demo {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Adapter for Demo {
    async fn terms(&self) -> Result<Vec<Term>, AdapterError> {
        Ok(self.terms.clone())
    }

    async fn catalog(&self, _term: &str) -> Result<Vec<Section>, AdapterError> {
        Ok(self.sections.lock().expect("demo sections lock").clone())
    }

    /// Returns the watched sections' seats. Without `static_seats`, each
    /// watched section has a one-in-four chance of flipping between full and
    /// one to three open seats first.
    async fn availability(&self, _term: &str, ids: &[String]) -> Result<Vec<Seats>, AdapterError> {
        let want: HashSet<&str> = ids.iter().map(String::as_str).collect();
        let mut rng = rand::rng();
        let mut sections = self.sections.lock().expect("demo sections lock");
        let mut out = Vec::new();
        for s in sections.iter_mut().filter(|s| want.contains(s.id.as_str())) {
            if !self.static_seats && rng.random_range(0..4) == 0 {
                s.available = if s.available > 0 { 0 } else { 1 + rng.random_range(0..3) };
            }
            out.push(Seats { id: s.id.clone(), capacity: s.capacity, available: s.available, waitlist: s.waitlist });
        }
        Ok(out)
    }

    fn version(&self) -> &str {
        "demo-1"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn fixture() {
        let mut d = Demo::new();
        d.static_seats = true;
        let terms = d.terms().await.unwrap();
        assert!(!terms.is_empty(), "no terms");
        let secs = d.catalog(&terms[0].id).await.unwrap();
        assert!(secs.len() >= 10, "expected fixture sections, got {}", secs.len());
        for s in &secs {
            assert!(!s.id.is_empty() && !s.course.is_empty() && !s.meetings.is_empty(), "incomplete section {s:?}");
        }
        let seats = d.availability(&terms[0].id, &[secs[0].id.clone(), "nope".into()]).await.unwrap();
        assert!(seats.len() == 1 && seats[0].available == secs[0].available, "availability mismatch: {seats:?}");
    }
}
