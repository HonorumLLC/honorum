//! PostgreSQL access.
//!
//! The server keeps two bounded pools on one database. The query pool serves
//! requests and background work. The lock pool holds session advisory locks
//! for provider operations that span network calls, so a queue of waiting
//! imports cannot starve ordinary queries.
//!
//! Queries use PostgreSQL's numbered `$n` parameters. Each connection
//! prepares and caches its statements.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use deadpool_postgres::{GenericClient, Manager, ManagerConfig, Pool, RecyclingMethod};
use tokio::task::JoinHandle;
use tokio_postgres::types::ToSql;
use tokio_util::sync::CancellationToken;

pub(crate) use tokio_postgres::Row;

use crate::schema;

/// One query parameter. Use `i64` values for `BIGINT` columns.
pub(crate) type Param<'a> = &'a (dyn ToSql + Sync);

/// A database failure. Messages never include the connection URL.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{0}")]
    Postgres(#[from] tokio_postgres::Error),
    #[error("database pool: {0}")]
    Pool(String),
    #[error("expected one row, found none")]
    NoRows,
    #[error("{0}")]
    Config(String),
}

impl From<deadpool_postgres::PoolError> for Error {
    fn from(err: deadpool_postgres::PoolError) -> Self {
        match err {
            deadpool_postgres::PoolError::Backend(e) => Self::Postgres(e),
            other => Self::Pool(other.to_string()),
        }
    }
}

pub(crate) type Result<T, E = Error> = std::result::Result<T, E>;

/// Query methods shared by the pool, a pooled connection, and a transaction.
#[async_trait]
pub(crate) trait Sql: Send + Sync {
    /// Runs a statement and returns the number of rows it changed.
    async fn exec(&self, query: &str, params: &[Param<'_>]) -> Result<u64>;
    /// Returns every row.
    async fn fetch_all(&self, query: &str, params: &[Param<'_>]) -> Result<Vec<Row>>;
    /// Returns the first row, or [`Error::NoRows`].
    async fn fetch_one(&self, query: &str, params: &[Param<'_>]) -> Result<Row> {
        self.fetch_opt(query, params).await?.ok_or(Error::NoRows)
    }
    /// Returns the first row, if any.
    async fn fetch_opt(&self, query: &str, params: &[Param<'_>]) -> Result<Option<Row>> {
        Ok(self.fetch_all(query, params).await?.into_iter().next())
    }
}

#[async_trait]
impl<C: GenericClient + Send + Sync> Sql for C {
    async fn exec(&self, query: &str, params: &[Param<'_>]) -> Result<u64> {
        let statement = self.prepare_cached(query).await?;
        Ok(self.execute(&statement, params).await?)
    }

    async fn fetch_all(&self, query: &str, params: &[Param<'_>]) -> Result<Vec<Row>> {
        let statement = self.prepare_cached(query).await?;
        Ok(self.query(&statement, params).await?)
    }
}

/// A pooled connection.
pub(crate) type Client = deadpool_postgres::Object;
/// A transaction on a pooled connection. Dropping it without committing
/// rolls it back.
pub(crate) type Tx<'a> = deadpool_postgres::Transaction<'a>;

/// The query and lock pools.
#[derive(Clone)]
pub struct Db {
    pool: Pool,
    locks: Pool,
    config: tokio_postgres::Config,
    tls: tokio_postgres_rustls::MakeRustlsConnect,
}

impl Db {
    /// Opens both pools and creates or updates the schema. The URL belongs in
    /// environment secrets; errors deliberately omit it. TLS follows the
    /// URL's `sslmode`.
    pub async fn open(url: &str, connections: usize, lock_connections: usize) -> Result<Self> {
        if !(1..=8).contains(&lock_connections) {
            return Err(Error::Config("PostgreSQL lock connections must be between 1 and 8".into()));
        }
        let (config, tls) = connection_config(url)?;
        let pool = new_pool(&config, &tls, connections.max(1));
        let locks = new_pool(&config, &tls, lock_connections);
        let db = Self { pool, locks, config, tls };
        schema::initialize(&db).await?;
        Ok(db)
    }

    /// Takes a connection from the query pool.
    pub(crate) async fn client(&self) -> Result<Client> {
        Ok(self.pool.get().await?)
    }

    /// Checks that a connection can be made.
    pub(crate) async fn ping(&self) -> Result<()> {
        self.client().await?.simple_query("SELECT 1").await?;
        Ok(())
    }

    /// Opens a dedicated connection outside both pools and listens on
    /// `channel`. Notifications arrive on the receiver, which closes when the
    /// connection ends; dropping the client closes the connection.
    pub(crate) async fn listen(
        &self,
        channel: &str,
    ) -> Result<(tokio_postgres::Client, tokio::sync::mpsc::UnboundedReceiver<tokio_postgres::Notification>)> {
        let (client, mut connection) = self.config.connect(self.tls.clone()).await?;
        let (sender, receiver) = tokio::sync::mpsc::unbounded_channel();
        tokio::spawn(async move {
            let mut messages = futures::stream::poll_fn(move |cx| connection.poll_message(cx));
            while let Some(Ok(message)) = futures::StreamExt::next(&mut messages).await {
                if let tokio_postgres::AsyncMessage::Notification(n) = message
                    && sender.send(n).is_err()
                {
                    return;
                }
            }
        });
        client.batch_execute(&format!("LISTEN {channel}")).await?;
        Ok((client, receiver))
    }

    /// Query and lock pool occupancy, for the capacity lab's statistics:
    /// `(max_size, open, idle, waiting)` for each pool.
    #[cfg(feature = "lab")]
    pub(crate) fn pool_status(&self) -> [(usize, usize, usize, usize); 2] {
        [&self.pool, &self.locks].map(|p| {
            let st = p.status();
            (st.max_size, st.size, st.available, st.waiting)
        })
    }

    /// Closes both pools. Connections in use close when they are returned.
    pub fn close(&self) {
        self.pool.close();
        self.locks.close();
    }

    /// Waits for the session advisory lock `scope:key` on a connection from
    /// the lock pool. The lock is held until the returned [`Lease`] is
    /// dropped, or until its connection dies, which also releases it in
    /// PostgreSQL. Long operations should stop when [`Lease::lost`] fires.
    ///
    /// Waiting can be cancelled by dropping the future.
    pub(crate) async fn lock(&self, scope: &str, key: &str) -> Result<Lease> {
        let client = self.locks.get().await?;
        let identity = format!("{scope}:{key}");
        loop {
            let held: bool = client
                .fetch_one("SELECT pg_try_advisory_lock(hashtextextended($1,0))", &[&identity])
                .await?
                .try_get(0)?;
            if held {
                break;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        let client = Arc::new(client);
        let lost = CancellationToken::new();
        let watcher = {
            let client = client.clone();
            let lost = lost.clone();
            tokio::spawn(async move {
                let mut ticker = tokio::time::interval(Duration::from_secs(5));
                ticker.tick().await;
                loop {
                    ticker.tick().await;
                    let ping = tokio::time::timeout(Duration::from_secs(2), client.simple_query("SELECT 1")).await;
                    if !matches!(ping, Ok(Ok(_))) {
                        lost.cancel();
                        return;
                    }
                }
            })
        };
        Ok(Lease { client: Some(client), identity, lost, watcher: Some(watcher) })
    }
}

#[async_trait]
impl Sql for Db {
    async fn exec(&self, query: &str, params: &[Param<'_>]) -> Result<u64> {
        self.client().await?.exec(query, params).await
    }

    async fn fetch_all(&self, query: &str, params: &[Param<'_>]) -> Result<Vec<Row>> {
        self.client().await?.fetch_all(query, params).await
    }
}

/// A held session advisory lock.
pub(crate) struct Lease {
    client: Option<Arc<Client>>,
    identity: String,
    lost: CancellationToken,
    watcher: Option<JoinHandle<()>>,
}

impl Lease {
    /// Fires when the lock's connection stops answering, after which the lock
    /// may already belong to another process.
    pub(crate) fn lost(&self) -> CancellationToken {
        self.lost.clone()
    }
}

impl Drop for Lease {
    fn drop(&mut self) {
        let (Some(client), Some(watcher)) = (self.client.take(), self.watcher.take()) else { return };
        let identity = std::mem::take(&mut self.identity);
        watcher.abort();
        tokio::spawn(async move {
            let _ = watcher.await;
            let Ok(client) = Arc::try_unwrap(client) else { return };
            let released = tokio::time::timeout(
                Duration::from_secs(2),
                client.fetch_one("SELECT pg_advisory_unlock(hashtextextended($1,0))", &[&identity]),
            )
            .await;
            let released = matches!(released, Ok(Ok(ref row)) if row.try_get::<_, bool>(0).unwrap_or(false));
            if !released {
                // A connection whose lock state is unknown must not return to
                // the pool; closing it releases every lock it held.
                drop(deadpool_postgres::Object::take(client));
            }
        });
    }
}

fn new_pool(config: &tokio_postgres::Config, tls: &tokio_postgres_rustls::MakeRustlsConnect, size: usize) -> Pool {
    let manager =
        Manager::from_config(config.clone(), tls.clone(), ManagerConfig { recycling_method: RecyclingMethod::Fast });
    Pool::builder(manager)
        .max_size(size)
        .runtime(deadpool_postgres::Runtime::Tokio1)
        .wait_timeout(Some(Duration::from_secs(30)))
        .create_timeout(Some(Duration::from_secs(10)))
        .post_recycle(deadpool_postgres::Hook::sync_fn(|_, metrics| {
            // Replace connections after thirty minutes to bound server memory use.
            if metrics.age() > Duration::from_secs(30 * 60) {
                return Err(deadpool_postgres::HookError::message("connection lifetime reached"));
            }
            Ok(())
        }))
        .build()
        .expect("the pool configuration is valid")
}

/// How the server checks the database's certificate.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Verify {
    /// `sslmode=disable`, `prefer`, or `require`: encrypt when asked, but do
    /// not check the certificate, as libpq does.
    None,
    /// `verify-ca` or `verify-full`: check against the web roots and
    /// `sslrootcert`, when given.
    Full,
}

fn connection_config(url: &str) -> Result<(tokio_postgres::Config, tokio_postgres_rustls::MakeRustlsConnect)> {
    let invalid = || Error::Config("invalid PostgreSQL connection configuration".into());
    let mut parsed = url::Url::parse(url).map_err(|_| invalid())?;
    let mut verify = Verify::None;
    let mut root_cert = None;
    let mut kept = Vec::new();
    for (key, value) in parsed.query_pairs() {
        match (&*key, &*value) {
            ("sslmode", "verify-ca" | "verify-full") => {
                verify = Verify::Full;
                kept.push(("sslmode".to_owned(), "require".to_owned()));
            }
            ("sslrootcert", path) => root_cert = Some(path.to_owned()),
            _ => kept.push((key.into_owned(), value.into_owned())),
        }
    }
    parsed.query_pairs_mut().clear().extend_pairs(kept);
    let mut config: tokio_postgres::Config = parsed.as_str().trim_end_matches('?').parse().map_err(|_| invalid())?;
    config.options("-c search_path=public,catalog -c timezone=UTC -c statement_timeout=30000 -c idle_in_transaction_session_timeout=30000");
    config.connect_timeout(Duration::from_secs(10));
    let tls = tls_config(verify, root_cert.as_deref())?;
    Ok((config, tokio_postgres_rustls::MakeRustlsConnect::new(tls)))
}

fn tls_config(verify: Verify, root_cert: Option<&str>) -> Result<rustls::ClientConfig> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let builder = rustls::ClientConfig::builder_with_provider(provider.clone())
        .with_safe_default_protocol_versions()
        .map_err(|e| Error::Config(e.to_string()))?;
    let config = match verify {
        Verify::None => {
            builder.dangerous().with_custom_certificate_verifier(Arc::new(AcceptAny(provider))).with_no_client_auth()
        }
        Verify::Full => {
            let mut roots = rustls::RootCertStore::empty();
            roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
            if let Some(path) = root_cert {
                let pem = std::fs::read(path).map_err(|_| Error::Config("sslrootcert could not be read".into()))?;
                for cert in rustls::pki_types::pem::PemObject::pem_slice_iter(&pem) {
                    let cert: rustls::pki_types::CertificateDer =
                        cert.map_err(|_| Error::Config("sslrootcert is not PEM".into()))?;
                    roots.add(cert).map_err(|_| Error::Config("sslrootcert holds an invalid certificate".into()))?;
                }
            }
            builder.with_root_certificates(roots).with_no_client_auth()
        }
    };
    Ok(config)
}

/// Encrypts without authenticating the server, matching libpq's `require`.
#[derive(Debug)]
struct AcceptAny(Arc<rustls::crypto::CryptoProvider>);

impl rustls::client::danger::ServerCertVerifier for AcceptAny {
    fn verify_server_cert(
        &self,
        _: &rustls::pki_types::CertificateDer<'_>,
        _: &[rustls::pki_types::CertificateDer<'_>],
        _: &rustls::pki_types::ServerName<'_>,
        _: &[u8],
        _: rustls::pki_types::UnixTime,
    ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        Ok(rustls::client::danger::ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &rustls::pki_types::CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(message, cert, dss, &self.0.signature_verification_algorithms)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &rustls::pki_types::CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(message, cert, dss, &self.0.signature_verification_algorithms)
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        self.0.signature_verification_algorithms.supported_schemes()
    }
}
