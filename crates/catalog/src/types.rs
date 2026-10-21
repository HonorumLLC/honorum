//! Values shared by adapters, the server, and the snapshot format clients read.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

/// One registration period at an institution.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Term {
    pub id: String,
    pub name: String,
    /// First day, `YYYY-MM-DD`, when the school publishes it.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub start: String,
    /// Last day, `YYYY-MM-DD`, when the school publishes it.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub end: String,
}

/// One weekly meeting pattern. `days` uses the letters `MTWRFSU`; times are
/// minutes after midnight in the institution's time zone.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Meeting {
    pub days: String,
    pub start: i64,
    pub end: i64,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub location: String,
}

/// One dated sitting, such as a midterm or final published with the section.
/// `date` is `YYYY-MM-DD`; times are minutes after midnight, campus time.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Exam {
    pub date: String,
    pub start: i64,
    pub end: i64,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub location: String,
}

/// One enrollable section of a course.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Section {
    /// Upstream identifier, such as a CRN.
    pub id: String,
    pub course: String,
    pub subject: String,
    pub title: String,
    #[serde(rename = "num")]
    pub number: String,
    /// Component kind: `LEC`, `LAB`, `DIS`, and so on.
    #[serde(rename = "type")]
    pub kind: String,
    /// Sections that share a link must be taken together.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub link: String,
    #[serde(default, deserialize_with = "null_default")]
    pub instructors: Vec<String>,
    #[serde(default, deserialize_with = "null_default")]
    pub meetings: Vec<Meeting>,
    /// Dated sittings, when the school publishes them.
    #[serde(default, skip_serializing_if = "Vec::is_empty", deserialize_with = "null_default")]
    pub exams: Vec<Exam>,
    /// Zero when the school does not publish capacity.
    pub capacity: i64,
    /// `-1` when not published; `1` with a zero capacity means "open".
    pub available: i64,
    pub waitlist: i64,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub credits: f64,
    /// `YYYY-MM-DD` when a section runs on dates other than the term's.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub start: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub end: String,
    /// `In person`, `Online`, or `Hybrid`.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub mode: String,
    /// General-education and similar tags.
    #[serde(default, skip_serializing_if = "Vec::is_empty", deserialize_with = "null_default")]
    pub attributes: Vec<String>,
    /// Lifted to the course in snapshots.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub prereqs: String,
    #[serde(rename = "url", default, skip_serializing_if = "String::is_empty")]
    pub source_url: String,
}

/// The availability of one section.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Seats {
    pub id: String,
    pub capacity: i64,
    pub available: i64,
    pub waitlist: i64,
}

/// An adapter failure. The message is written to the status page, so it must
/// not contain credentials or personal data.
#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub struct AdapterError(pub String);

impl AdapterError {
    pub fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

impl From<crate::HttpError> for AdapterError {
    fn from(err: crate::HttpError) -> Self {
        Self(err.to_string())
    }
}

/// Fetches catalog and seat data for one institution.
///
/// Cancellation is by dropping the returned future; adapters hold no state
/// that a dropped request can leave inconsistent.
#[async_trait]
pub trait Adapter: Send + Sync {
    async fn terms(&self) -> Result<Vec<Term>, AdapterError>;
    async fn catalog(&self, term: &str) -> Result<Vec<Section>, AdapterError>;
    async fn availability(&self, term: &str, ids: &[String]) -> Result<Vec<Seats>, AdapterError>;
    /// Changes when the adapter's output format changes, so stored catalogs
    /// are collected again.
    fn version(&self) -> &str;
    /// The shortest seat-poll interval the provider tolerates, when it is
    /// longer than the configured one. `None` imposes no floor.
    fn minimum_poll_interval(&self) -> Option<std::time::Duration> {
        None
    }
    /// Routing metadata the server persists across restarts, for adapters
    /// that keep it. `None` for stateless adapters.
    fn state(&self) -> Option<&dyn AdapterState> {
        None
    }
}

/// Routing metadata an adapter learns while collecting catalogs and needs to
/// poll watched sections after a restart. The server stores the bytes under
/// the adapter's version and hands them back on startup.
pub trait AdapterState: Send + Sync {
    /// Serializes the routing metadata after a catalog collection.
    fn save_state(&self) -> Result<Vec<u8>, AdapterError>;
    /// Replaces the routing metadata with previously saved bytes.
    fn load_state(&self, data: &[u8]) -> Result<(), AdapterError>;
    /// Serializes the routing metadata after a seat poll. An empty result
    /// means nothing changed and nothing needs to be written.
    fn save_polling_state(&self) -> Result<Vec<u8>, AdapterError> {
        Ok(Vec::new())
    }
}

fn is_zero(v: &f64) -> bool {
    *v == 0.0
}

/// Reads JSON `null` as the type's default, so a field written as `null`
/// reads the same as an omitted one.
pub(crate) fn null_default<'de, D, T>(d: D) -> Result<T, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Default + Deserialize<'de>,
{
    Ok(Option::<T>::deserialize(d)?.unwrap_or_default())
}
