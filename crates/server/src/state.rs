//! The server: configuration, storage, and the state every module shares.
//!
//! [`Server`] is built once per process and shared as `Arc<Server>`.
//! Replaceable parts, such as the push sender and the phone verifier, sit
//! behind locks.

use std::path::PathBuf;
use std::sync::{Arc, RwLock};

use chrono::{DateTime, Utc};
use honorum_catalog::Adapter;

use crate::canvas_account::AccountLocks;
use crate::clock::Clock;
use crate::closures::{Closure, load_closures};
use crate::config::{Config, Lms};
use crate::db::{Db, Param, Sql};
use crate::google::GoogleState;
use crate::hub::Hub;
use crate::metrics::{Counters, DurationStats, Meter};
use crate::object_store::CatalogObjects;
use crate::outbound::Outbound;
use crate::phone::PhoneVerifier;
use crate::public::PublicConfigCache;
use crate::push::Sender;
use crate::push::apns::ApnsSender;
use crate::push::webpush::VapidKeys;
use crate::rate::LocalLimits;
use crate::seal::Key;
use crate::session::SeenSessions;
use crate::status::StatusCache;

/// One configured school.
#[derive(Clone)]
pub struct Institution {
    pub id: String,
    pub name: String,
    /// IANA time zone for campus dates; empty means UTC.
    pub timezone: String,
    /// `None` for schools served without collection, as in the labs.
    pub adapter: Option<Arc<dyn Adapter>>,
    pub lms: Option<Lms>,
    /// Days classes do not meet: the configuration's list, or the built-in
    /// one from `closures.json`.
    pub closures: Vec<Closure>,
}

/// Shared server state. Handlers receive it as `State<Arc<Server>>`.
pub struct Server {
    pub(crate) db: Db,
    pub(crate) cfg: Config,
    pub(crate) insts: Vec<Institution>,
    /// Holds catalog snapshot files under `catalog/`.
    pub(crate) data_dir: PathBuf,
    /// Shown on the status page; set at build time.
    pub version: String,
    pub(crate) clock: Clock,
    pub(crate) meter: Meter,
    pub(crate) counters: Counters,
    pub(crate) api_timing: DurationStats,
    pub(crate) events: Hub,
    /// This process's view of API request limits.
    pub(crate) limits: LocalLimits,
    /// Session activity waiting to be written (see [`Server::run_sessions`]).
    pub(crate) seen: SeenSessions,
    pub(crate) outbound: Outbound,

    /// The per-process Google Calendar client: endpoints, locks, the
    /// worker's wakeup, and cached access tokens.
    pub(crate) google: GoogleState,
    /// Serializes every import kind (Canvas, feeds) for one account within
    /// this process; the `imports` advisory lock covers other processes.
    pub(crate) canvas_locks: AccountLocks,
    /// Seals account Canvas tokens, feed URLs, and Google refresh tokens.
    /// `None` when `HONORUM_CANVAS_KEY` is unset, which turns those
    /// integrations off.
    pub(crate) canvas_key: Option<Key>,
    /// APNs delivery, when configured. Tests replace it with `set_apns`.
    pub(crate) apns: RwLock<Option<Arc<ApnsSender>>>,
    /// The cached public status (`/api/status`, `/status`).
    pub(crate) status_cache: StatusCache,
    /// Catalog object storage (R2), when `HONORUM_CATALOG_OBJECTS=1`.
    pub(crate) objects: Option<CatalogObjects>,
    /// The embedded web client, when the binary carries one.
    pub web: Option<Arc<dyn crate::assets::WebAssets>>,
    /// Replaces push delivery when set (tests and labs); `None` delivers
    /// through [`Server::deliver`]. See [`Server::send_alert`].
    pub(crate) sender: RwLock<Option<Arc<dyn Sender>>>,
    /// Sends and checks phone codes; `None` turns phone sign-in off. Tests
    /// replace it with `set_verifier`.
    pub(crate) verify: RwLock<Option<Arc<dyn PhoneVerifier>>>,
    /// The VAPID key pair Web Push messages are signed with.
    pub(crate) vapid: VapidKeys,
    /// One catalog write (sections, seats) at a time in this process.
    pub(crate) write_lock: tokio::sync::Mutex<()>,
    /// One snapshot file publication at a time in this process.
    pub(crate) snapshot_lock: tokio::sync::Mutex<()>,
    /// The serialized public configuration (`/api/config`).
    pub(crate) public_config: PublicConfigCache,
}

impl Server {
    /// [`Server::build`], shared in an `Arc`, for tests and labs.
    #[cfg(any(test, feature = "testing"))]
    pub(crate) async fn new(
        db: Db,
        cfg: Config,
        insts: Vec<Institution>,
        data_dir: PathBuf,
    ) -> anyhow::Result<Arc<Server>> {
        Ok(Arc::new(Self::build(db, cfg, insts, data_dir).await?))
    }

    /// Builds a server: object storage from the environment, the Google
    /// client, the credential key, closures (the built-in list for schools
    /// whose configuration names none), the phone verifier, APNs, and VAPID
    /// keys (stored in the database when not configured). Creates
    /// `data_dir/catalog`. The binary sets `version` and `web` before
    /// sharing the server.
    pub async fn build(db: Db, cfg: Config, mut insts: Vec<Institution>, data_dir: PathBuf) -> anyhow::Result<Server> {
        let objects = CatalogObjects::from_env(|key| std::env::var(key).unwrap_or_default())?;
        let google = GoogleState::new(&cfg);
        let canvas_key = crate::seal::decode_canvas_key(&cfg.canvas_key)?;
        let builtin = load_closures()?;
        for inst in &mut insts {
            // A configuration that names a list, even an empty one, keeps it.
            let configured = cfg.institutions.iter().any(|c| c.id == inst.id && c.closures.is_some());
            if inst.closures.is_empty() && !configured {
                inst.closures = builtin.get(&inst.id).cloned().unwrap_or_default();
            }
        }
        let verify = crate::phone::new_phone_verifier(&cfg.twilio);
        let apns = ApnsSender::new(&cfg.apns)?.map(Arc::new);
        let vapid = crate::push::webpush::initialize(&db, &cfg).await?;
        std::fs::create_dir_all(data_dir.join("catalog"))?;
        Ok(Server {
            db,
            cfg,
            insts,
            data_dir,
            version: "dev".into(),
            clock: Clock::default(),
            meter: Meter::default(),
            counters: Counters::default(),
            api_timing: DurationStats::default(),
            events: Hub::default(),
            limits: LocalLimits::default(),
            seen: SeenSessions::default(),
            outbound: Outbound::new(),
            google,
            canvas_locks: AccountLocks::default(),
            canvas_key,
            apns: RwLock::new(apns),
            status_cache: StatusCache::default(),
            objects,
            web: None,
            sender: RwLock::new(None),
            verify: RwLock::new(verify),
            vapid,
            write_lock: tokio::sync::Mutex::new(()),
            snapshot_lock: tokio::sync::Mutex::new(()),
            public_config: PublicConfigCache::default(),
        })
    }

    /// The current instant on the server's clock.
    pub(crate) fn now(&self) -> DateTime<Utc> {
        self.clock.now()
    }

    /// The configured school with `id`.
    pub(crate) fn inst(&self, id: &str) -> Option<&Institution> {
        self.insts.iter().find(|i| i.id == id)
    }

    /// Wakes the account's live event streams in this process.
    pub(crate) fn changed(&self, account: &str) {
        self.events.notify(account);
    }

    /// The phone verifier, if phone sign-in is configured.
    pub(crate) fn verifier(&self) -> Option<Arc<dyn PhoneVerifier>> {
        self.verify.read().expect("verifier lock").clone()
    }

    /// Replaces the phone verifier, as tests do.
    #[cfg(test)]
    pub(crate) fn set_verifier(&self, verifier: Option<Arc<dyn PhoneVerifier>>) {
        *self.verify.write().expect("verifier lock") = verifier;
    }

    /// The APNs sender, if iPhone notifications are configured.
    pub(crate) fn apns(&self) -> Option<Arc<ApnsSender>> {
        self.apns.read().expect("apns lock").clone()
    }

    /// Replaces the APNs sender, as tests do.
    #[cfg(test)]
    pub(crate) fn set_apns(&self, sender: Option<Arc<ApnsSender>>) {
        *self.apns.write().expect("apns lock") = sender;
    }

    /// Replaces push delivery for every alert, as tests do; `None` restores
    /// real delivery.
    #[cfg(any(test, feature = "testing"))]
    pub(crate) fn set_sender(&self, sender: Option<Arc<dyn Sender>>) {
        *self.sender.write().expect("sender lock") = sender;
    }

    /// Runs a write whose failure the caller cannot act on, such as a sweep,
    /// a status note, or an acknowledgement, and logs a failure.
    pub(crate) async fn exec_logged(&self, query: &str, params: &[Param<'_>]) {
        if let Err(err) = self.db.exec(query, params).await {
            let statement = query.trim().lines().next().unwrap_or_default();
            let statement: String = statement.chars().take(80).collect();
            tracing::error!(%statement, %err, "database write failed");
        }
    }

    /// Reads a value from the `kv` table; empty when missing.
    pub(crate) async fn kv_get(&self, key: &str) -> String {
        match self.db.fetch_opt("SELECT v FROM kv WHERE k=$1", &[&key]).await {
            Ok(Some(row)) => row.try_get::<_, Option<String>>(0).ok().flatten().unwrap_or_default(),
            _ => String::new(),
        }
    }

    /// Writes a value to the `kv` table, logging a failure.
    pub(crate) async fn kv_set(&self, key: &str, value: &str) {
        self.exec_logged(
            "INSERT INTO kv(k,v) VALUES($1,$2) ON CONFLICT(k) DO UPDATE SET v=excluded.v",
            &[&key, &value],
        )
        .await;
    }
}
