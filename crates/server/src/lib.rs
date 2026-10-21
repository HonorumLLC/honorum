//! The Honorum service: the HTTP API, catalog collection, account sync,
//! imports, and notifications, stored in PostgreSQL.
//!
//! `docs/architecture.md` describes the boundaries and contracts this crate
//! implements; `docs/application.md` describes the product behavior.

// Foundation.
mod api;
mod clock;
mod closures;
mod config;
mod db;
mod hub;
mod jobs;
mod metrics;
mod outbound;
mod rate;
mod schema;
mod session;
mod state;

// HTTP surface.
mod admin;
pub mod assets;
mod catalog_api;
mod landing;
mod public;
mod router;
mod shares;
mod status;
mod watches;

// Accounts and documents.
mod accounts;
mod auth;
mod docs;
mod events;
mod phone;
mod streaks;

// Integrations and imports.
mod canvas_account;
mod canvas_classes;
mod feeds;
mod google;
mod ical;
mod imports;
mod lms;
mod seal;

// Catalog collection and publication.
mod monitor;
mod object_store;
mod snapshot;
mod snapshot_cleanup;

// Notifications.
mod deliveries;
mod push;
mod reminder_schedule;
mod reminders;

// Background work.
mod analytics;
mod worker;

// Development labs (`honorum-lab`).
#[cfg(feature = "lab")]
pub mod lab;

#[cfg(test)]
mod accounts_tests;
#[cfg(test)]
mod auth_tests;
#[cfg(test)]
mod boundaries_tests;
#[cfg(test)]
mod db_tests;
#[cfg(test)]
mod docs_tests;
#[cfg(test)]
mod events_tests;
#[cfg(test)]
mod phone_tests;
#[cfg(test)]
mod postgres_tests;
#[cfg(test)]
mod test_support;
#[cfg(any(test, feature = "testing"))]
mod testing;

pub use config::Config;
pub use db::Db;
pub use router::router;
pub use state::{Institution, Server};
