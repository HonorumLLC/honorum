//! Catalog data model, the paced provider HTTP client, and school adapters.
//!
//! Every adapter turns one school's public class search into the same
//! [`Term`], [`Section`], and [`Seats`] values. The server stores those values,
//! publishes them as catalog snapshots, and compares seat counts for alerts.

pub mod adapters;
mod http;
mod text;
mod types;

pub use http::{Http, HttpError};
pub(crate) use text::title_case;
pub use types::{Adapter, AdapterError, AdapterState, Exam, Meeting, Seats, Section, Term};
