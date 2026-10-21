//! Account Canvas connections: the sealed token, background and manual
//! refreshes, publication of imported tasks into account documents, and
//! completion write-back. Sealing is in [`crate::seal`].
//!
//! Imports of every kind (Canvas, feeds) for one account are serialized by
//! [`Server::lock_canvas`]: an in-process [`AccountLocks`] entry plus the
//! `imports` advisory lock shared by every process. Database transactions
//! protect published document revisions.

use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::body::Body;
use axum::extract::State;
use axum::http::{HeaderValue, header};
use serde::{Deserialize, Serialize};
use serde_json::value::RawValue;
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};

use crate::Server;
use crate::api::{ApiError, ApiResult, fail, null_default, ok, read_json};
use crate::db::{self, Lease, Param, Sql};
use crate::docs::{DOC_BODY_MAX, DOC_ID, Doc, push_docs};
use crate::lms::{CanvasCompletion, Task};
use crate::session::{Auth, new_token};

/// A task or other account document body: a JSON object kept with every
/// field, including ones a newer client added.
pub(crate) type DocBody = Map<String, Value>;

/// How often a refresh resolves Canvas module routing again.
pub(crate) const MODULES_INTERVAL: Duration = Duration::from_secs(24 * 3600);
/// A manual refresh within this window of the last import is skipped
/// unless forced.
pub(crate) const IMPORT_FRESHNESS: Duration = Duration::from_secs(5 * 60);
/// Background imports repeat at this cadence.
pub(crate) const IMPORT_INTERVAL: Duration = Duration::from_secs(3600);
/// A failed background import is retried after this long.
pub(crate) const IMPORT_RETRY: Duration = Duration::from_secs(15 * 60);
/// Background imports run only for accounts with a device seen this recently.
pub(crate) const IMPORT_DEVICE: Duration = Duration::from_secs(30 * 24 * 3600);

/// An account's Canvas connection as clients see it. The sealed credential
/// and module timestamp are never serialized.
#[derive(Clone, Debug, Default, Serialize)]
pub(crate) struct CanvasConnection {
    pub(crate) id: String,
    pub(crate) base: String,
    pub(crate) connected: bool,
    pub(crate) available: bool,
    pub(crate) synced: i64,
    pub(crate) error: String,
    #[serde(skip)]
    pub(crate) credential: Vec<u8>,
    /// When module routing was last resolved, Unix seconds.
    #[serde(skip)]
    pub(crate) modules_at: i64,
}

/// `force` fetches fresh assignments while keeping recent module routing;
/// `rescan_modules` also refreshes module routing.
#[derive(Clone, Copy, Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct CanvasRefreshOptions {
    pub(crate) force: bool,
    pub(crate) rescan_modules: bool,
}

/// An import failure with the HTTP status a client-triggered refresh
/// reports, or an internal failure, which is logged and reported as 500
/// "the import could not finish".
#[derive(Debug)]
pub(crate) enum ImportError {
    Status { status: u16, message: String },
    Other(anyhow::Error),
}

impl ImportError {
    /// A failure reported to the client with `status` and `message`.
    pub(crate) fn new(status: u16, message: impl Into<String>) -> Self {
        Self::Status { status, message: message.into() }
    }
}

impl std::fmt::Display for ImportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Status { message, .. } => f.write_str(message),
            Self::Other(err) => err.fmt(f),
        }
    }
}

impl std::error::Error for ImportError {}

impl From<anyhow::Error> for ImportError {
    fn from(err: anyhow::Error) -> Self {
        Self::Other(err)
    }
}

impl From<db::Error> for ImportError {
    fn from(err: db::Error) -> Self {
        Self::Other(err.into())
    }
}

impl From<ImportError> for ApiError {
    fn from(err: ImportError) -> Self {
        match err {
            ImportError::Status { status, message } => ApiError::new(status, message),
            ImportError::Other(err) => {
                tracing::error!(%err, "import failed");
                ApiError::new(500, "the import could not finish")
            }
        }
    }
}

/// One account's slot in [`AccountLocks`]: the lock and how many holders and
/// waiters use it.
struct Held {
    lock: Arc<tokio::sync::Mutex<()>>,
    refs: usize,
}

type HeldMap = Arc<Mutex<HashMap<String, Held>>>;

/// In-process per-account locks for imports, replacement, disconnection,
/// and completion. An entry exists only while someone holds or waits for
/// it; a waiter whose future is dropped gives up its place.
#[derive(Default)]
pub(crate) struct AccountLocks {
    held: HeldMap,
}

/// A held [`AccountLocks`] entry; dropping it releases the lock.
pub(crate) struct AccountGuard {
    _guard: tokio::sync::OwnedMutexGuard<()>,
    locks: HeldMap,
    account: String,
}

impl Drop for AccountGuard {
    fn drop(&mut self) {
        release(&self.locks, &self.account);
    }
}

/// Gives up one reference to an account's entry, removing it at zero.
fn release(locks: &HeldMap, account: &str) {
    let mut held = locks.lock().expect("account locks");
    if let Some(entry) = held.get_mut(account) {
        entry.refs -= 1;
        if entry.refs == 0 {
            held.remove(account);
        }
    }
}

/// A reference taken while waiting; released if the wait is abandoned.
struct Waiting<'a> {
    locks: &'a HeldMap,
    account: &'a str,
    armed: bool,
}

impl Drop for Waiting<'_> {
    fn drop(&mut self) {
        if self.armed {
            release(self.locks, self.account);
        }
    }
}

impl AccountLocks {
    /// Waits for the account's lock. Cancel by dropping the future.
    pub(crate) async fn lock(&self, account: &str) -> AccountGuard {
        let lock = {
            let mut held = self.held.lock().expect("account locks");
            let entry = held
                .entry(account.to_owned())
                .or_insert_with(|| Held { lock: Arc::new(tokio::sync::Mutex::new(())), refs: 0 });
            entry.refs += 1;
            entry.lock.clone()
        };
        let mut waiting = Waiting { locks: &self.held, account, armed: true };
        let guard = lock.lock_owned().await;
        waiting.armed = false;
        AccountGuard { _guard: guard, locks: self.held.clone(), account: account.to_owned() }
    }
}

/// The import lock for one account: local, then shared. Long work should
/// stop when [`ImportLock::lease`]'s `lost()` token fires.
pub(crate) struct ImportLock {
    pub(crate) lease: Lease,
    pub(crate) _local: AccountGuard,
}

/// Runs `work` unless the lease is lost first, in which case `work` is
/// dropped and `lost` is returned.
async fn under_lease<T>(lease: &Lease, lost: T, work: impl Future<Output = T>) -> T {
    let token = lease.lost();
    tokio::select! {
        result = work => result,
        () = token.cancelled() => lost,
    }
}

impl Server {
    /// Takes the account's import lock: the in-process entry, then the
    /// `imports` advisory lock. Cancel by dropping the future.
    pub(crate) async fn lock_canvas(&self, account: &str) -> db::Result<ImportLock> {
        let local = self.canvas_locks.lock(account).await;
        let lease = self.db.lock("imports", account).await?;
        Ok(ImportLock { lease, _local: local })
    }

    /// Seals a Canvas token for the connection (scope: its base URL).
    pub(crate) fn encrypt_canvas(&self, account: &str, c: &CanvasConnection, token: &str) -> anyhow::Result<Vec<u8>> {
        self.seal(account, &c.id, &c.base, token)
    }

    /// Opens the connection's sealed Canvas token.
    pub(crate) fn decrypt_canvas(&self, account: &str, c: &CanvasConnection) -> anyhow::Result<String> {
        self.open(account, &c.id, &c.base, &c.credential)
    }

    /// Reads the account's Canvas connection; a missing row is a
    /// disconnected connection. `available` reports a configured key.
    pub(crate) async fn canvas_connection(&self, account: &str) -> db::Result<CanvasConnection> {
        let mut c = CanvasConnection { available: self.integrations_available(), ..CanvasConnection::default() };
        let row = self
            .db
            .fetch_opt(
                "SELECT id,base,credential,synced,error,modules FROM canvas_connections WHERE account=$1",
                &[&account],
            )
            .await?;
        if let Some(row) = row {
            c.id = row.try_get(0)?;
            c.base = row.try_get(1)?;
            c.credential = row.try_get(2)?;
            c.synced = row.try_get(3)?;
            c.error = row.try_get(4)?;
            c.modules_at = row.try_get(5)?;
        }
        c.connected = !c.credential.is_empty();
        Ok(c)
    }

    /// Imports the account's Canvas planner unless a recent import covers
    /// it. The refresh endpoint and the background queue share it, so a
    /// device and the server never import twice.
    pub(crate) async fn refresh_canvas_connection(
        &self,
        account: &str,
        options: CanvasRefreshOptions,
    ) -> Result<CanvasConnection, ImportError> {
        let started = self.now().timestamp();
        let lock = self.lock_canvas(account).await.map_err(|_| ImportError::new(503, "the refresh was cancelled"))?;
        let cancelled = || Err(ImportError::new(503, "the refresh was cancelled"));
        under_lease(&lock.lease, cancelled(), self.refresh_canvas_locked(account, options, started)).await
    }

    async fn refresh_canvas_locked(
        &self,
        account: &str,
        options: CanvasRefreshOptions,
        started: i64,
    ) -> Result<CanvasConnection, ImportError> {
        let mut c = self
            .canvas_connection(account)
            .await
            .map_err(|_| ImportError::new(500, "could not read Canvas connection"))?;
        if !c.connected {
            return Ok(c);
        }
        // Requests waiting for the same account share the refresh just
        // completed, provided it included modules when this one needs them.
        if c.synced >= started && (!options.rescan_modules || c.modules_at >= started) {
            return Ok(c);
        }
        let now = self.now().timestamp();
        if !options.force && !options.rescan_modules && now - c.synced < IMPORT_FRESHNESS.as_secs() as i64 {
            return Ok(c);
        }
        self.db
            .exec("UPDATE canvas_connections SET attempted=$1 WHERE account=$2", &[&started, &account])
            .await
            .map_err(|_| ImportError::new(500, "could not record the refresh"))?;
        let token = self
            .decrypt_canvas(account, &c)
            .map_err(|_| ImportError::new(503, "Canvas credential is unavailable; reconnect Canvas"))?;
        let base =
            self.outbound.safe_url(&c.base).map_err(|_| ImportError::new(500, "invalid saved Canvas address"))?;
        // Manual refreshes bypass assignment freshness but reuse module
        // routing. Only an explicit rescan or the daily reconcile repeats
        // per-course lookups.
        let with_modules =
            options.rescan_modules || self.now().timestamp() - c.modules_at >= MODULES_INTERVAL.as_secs() as i64;
        let Ok(tasks) = self.fetch_canvas_tasks(&base, &token, None, with_modules).await else {
            self.exec_logged(
                "UPDATE canvas_connections SET error=$1 WHERE account=$2",
                &[&"Canvas refresh failed. Reconnect if your token has expired.", &account],
            )
            .await;
            self.count("canvas_failures", 1);
            return Err(ImportError::new(502, "Canvas refresh failed. Try again or reconnect Canvas."));
        };
        c.synced = self.now().timestamp();
        if with_modules {
            c.modules_at = self.now().timestamp();
        }
        c.error.clear();
        if self.publish_canvas(account, &c, &tasks, false).await.is_err() {
            return Err(ImportError::new(500, "could not save Canvas assignments"));
        }
        self.count("canvas_imports", 1);
        Ok(c)
    }

    /// Records the connection (inserting it when `connect`) and reconciles
    /// its assignments in one transaction.
    pub(crate) async fn publish_canvas(
        &self,
        account: &str,
        c: &CanvasConnection,
        tasks: &[Task],
        connect: bool,
    ) -> anyhow::Result<()> {
        if connect {
            let params: &[Param] = &[&account, &c.id, &c.base, &c.credential, &c.synced, &c.modules_at];
            self.publish_import(account, "canvas", &c.id, tasks, Some(("INSERT INTO canvas_connections(account,id,base,credential,synced,modules,error) VALUES($1,$2,$3,$4,$5,$6,'') ON CONFLICT(account) DO UPDATE SET id=excluded.id,base=excluded.base,credential=excluded.credential,synced=excluded.synced,modules=excluded.modules,error=''", params))).await
        } else {
            // modules keeps its stored value unless this import refreshed it.
            let params: &[Param] = &[&c.synced, &c.modules_at, &c.modules_at, &account, &c.id];
            self.publish_import(account, "canvas", &c.id, tasks, Some(("UPDATE canvas_connections SET synced=$1,modules=CASE WHEN CAST($2 AS BIGINT)>0 THEN $3 ELSE modules END,error='' WHERE account=$4 AND id=$5", params))).await
        }
    }

    /// Publishes a complete import atomically: existing edits keep their IDs,
    /// new items get an ID derived from the connection and upstream ID,
    /// duplicates and tasks of a replaced connection become recoverable
    /// manual copies, a user-deleted task stays deleted, and open tasks the
    /// source no longer lists are removed. `persist` is one statement run in
    /// the same transaction first, such as the connection's own update.
    /// Fails with "account no longer exists" for a deleted account.
    ///
    /// Canvas and calendar feeds share this reconciliation; a task remembers
    /// its connection in `canvasConnection`, whichever kind of source it
    /// came from.
    pub(crate) async fn publish_import(
        &self,
        account: &str,
        source: &str,
        connection: &str,
        tasks: &[Task],
        persist: Option<(&str, &[Param<'_>])>,
    ) -> anyhow::Result<()> {
        let mut client = self.db.client().await?;
        let tx = client.transaction().await?;
        tx.exec("UPDATE accounts SET seq=seq WHERE id=$1", &[&account]).await?;
        let exists: i64 = tx.fetch_one("SELECT COUNT(*) FROM accounts WHERE id=$1", &[&account]).await?.try_get(0)?;
        if exists != 1 {
            anyhow::bail!("account no longer exists");
        }
        if let Some((statement, params)) = persist {
            tx.exec(statement, params).await?;
        }
        let mut identities: HashMap<String, String> = HashMap::new();
        for row in tx
            .fetch_all(
                "SELECT uid,document FROM canvas_items WHERE account=$1 AND connection=$2",
                &[&account, &connection],
            )
            .await?
        {
            identities.insert(row.try_get(0)?, row.try_get(1)?);
        }
        struct Saved {
            id: String,
            rev: i64,
            body: DocBody,
        }
        let now = self.now();
        let updated = now.timestamp_millis();
        let mut existing: HashMap<String, Saved> = HashMap::new();
        let mut updates: Vec<Doc> = Vec::new();
        let recovered_title = if source == "canvas" { "Recovered Canvas copy: " } else { "Recovered copy: " };
        let rows = tx
            .fetch_all(
                "SELECT id,seq,body FROM docs WHERE account=$1 AND type='task' AND deleted=0 ORDER BY seq,id",
                &[&account],
            )
            .await?;
        for row in rows {
            let id: String = row.try_get(0)?;
            let rev: i64 = row.try_get::<_, Option<i64>>(1)?.unwrap_or_default();
            let raw: String = row.try_get::<_, Option<String>>(2)?.unwrap_or_default();
            let mut body = task_body(raw.as_bytes());
            if body.get("source").and_then(Value::as_str) != Some(source) {
                continue;
            }
            let uid = string_value(body.get("uid")).to_owned();
            if uid.is_empty() {
                continue;
            }
            let other = present(&body, "canvasConnection")
                && body.get("canvasConnection").and_then(Value::as_str) != Some(connection);
            if other && source != "canvas" {
                continue; // another feed's item
            }
            let known = identities.get(&uid).map(String::as_str).unwrap_or_default();
            if existing.contains_key(&uid) || (!known.is_empty() && known != id) || other {
                body.insert("source".into(), "manual".into());
                body.insert("recoveredCanvasUID".into(), uid.clone().into());
                for key in ["uid", "pendingDone", "pendingCanvasDone", "canvasConnection"] {
                    body.remove(key);
                }
                let title = format!("{recovered_title}{}", string_value(body.get("title")));
                body.insert("title".into(), title.into());
                let raw = serde_json::to_string(&body)?;
                if raw.len() > DOC_BODY_MAX {
                    anyhow::bail!("recovery copy exceeds the document limit");
                }
                updates.push(task_doc(id, rev, updated, Some(raw)));
            } else {
                existing.insert(uid, Saved { id, rev, body });
            }
        }
        let mut seen: HashSet<&str> = HashSet::new();
        for task in tasks {
            if !seen.insert(&task.uid) {
                continue;
            }
            let found = existing.contains_key(&task.uid);
            let (id, rev, mut body) = match existing.get(&task.uid) {
                Some(old) => (old.id.clone(), old.rev, old.body.clone()),
                None => {
                    let deleted = tx
                        .fetch_opt(
                            "SELECT document FROM canvas_items WHERE account=$1 AND connection=$2 AND uid=$3",
                            &[&account, &connection, &task.uid],
                        )
                        .await?;
                    if deleted.is_some() {
                        continue; // A user-deleted task stays deleted.
                    }
                    let hash = Sha256::digest(format!("{connection}\0{}", task.uid).as_bytes());
                    let id = format!("t_c_{}", hex::encode(&hash[..24]));
                    // A task an earlier import removed returns over its tombstone.
                    let rev = match tx
                        .fetch_opt("SELECT seq FROM docs WHERE account=$1 AND id=$2 AND deleted=1", &[&account, &id])
                        .await?
                    {
                        Some(row) => row.try_get::<_, Option<i64>>(0)?.unwrap_or_default(),
                        None => 0,
                    };
                    (id, rev, DocBody::new())
                }
            };
            let before = serde_json::to_string(&body)?;
            // A guest completion from iOS records only the desired state; the
            // account retries it under a durable intent like every other client's.
            if let Some(desired) = body.get("pendingDone").and_then(Value::as_bool)
                && !present(&body, "pendingCanvasDone")
            {
                body.insert("done".into(), desired.into());
                body.insert("pendingCanvasDone".into(), new_token().into());
                body.remove("pendingDone");
            }
            let incoming = match serde_json::to_value(task)? {
                Value::Object(map) => map,
                _ => DocBody::new(),
            };
            let pending = present(&body, "pendingCanvasDone") || present(&body, "pendingDone");
            let was_done = body.get("done") == Some(&Value::Bool(true));
            // A due date the student changed stays until the source moves its
            // own. importedDue is what the source last said; a task imported
            // before it was recorded counts as unchanged.
            let keep_due = match body.get("importedDue").and_then(Value::as_str) {
                Some(base) => !same_instant(string_value(body.get("due")), base) && same_instant(&task.due, base),
                None => false,
            };
            for key in
                ["uid", "title", "course", "courseId", "courseCode", "due", "done", "url", "overrideId", "reason"]
            {
                if (key == "done" || key == "reason") && pending {
                    continue;
                }
                if key == "course" && body.get("courseEdited") == Some(&Value::Bool(true)) {
                    continue;
                }
                if key == "due" && keep_due {
                    continue;
                }
                // A feed reports completion only one way; a personal tick stays.
                if key == "done" && source == "feed" && found && body.get("done") == Some(&Value::Bool(true)) {
                    continue;
                }
                match incoming.get(key) {
                    Some(value) => {
                        body.insert(key.into(), value.clone());
                    }
                    None => {
                        body.remove(key);
                    }
                }
            }
            // A completion the import reports carries no time of its own.
            if (body.get("done") == Some(&Value::Bool(true))) != was_done {
                body.remove("completedAt");
            }
            if let Some(module_item) = incoming.get("moduleItem") {
                body.insert("moduleItem".into(), module_item.clone());
            }
            body.insert("importedDue".into(), task.due.clone().into());
            body.insert("id".into(), id.strip_prefix("t_").unwrap_or(&id).into());
            body.insert("source".into(), source.into());
            body.insert("canvasConnection".into(), connection.into());
            let after = serde_json::to_string(&body)?;
            if after.len() > DOC_BODY_MAX {
                anyhow::bail!("imported task exceeds the document limit");
            }
            tx.exec(
                "INSERT INTO canvas_items(account,connection,uid,document) VALUES($1,$2,$3,$4) ON CONFLICT(account,connection,uid) DO UPDATE SET document=excluded.document",
                &[&account, &connection, &task.uid, &id],
            )
            .await?;
            if before != after {
                updates.push(task_doc(id, rev, updated, Some(after)));
            }
        }
        // A finished task stays when the import no longer lists it: the
        // source looks back only a week or two, so its silence says nothing
        // about history. An open one goes, and so does its identity, because
        // the student did not delete it: it comes back if the source lists
        // it again.
        for (uid, old) in &existing {
            if !seen.contains(uid.as_str())
                && old.body.get("done") != Some(&Value::Bool(true))
                && !present(&old.body, "pendingCanvasDone")
                && !present(&old.body, "pendingDone")
            {
                updates.push(task_doc(old.id.clone(), old.rev, updated, None));
                tx.exec(
                    "DELETE FROM canvas_items WHERE account=$1 AND connection=$2 AND uid=$3",
                    &[&account, &connection, uid],
                )
                .await?;
            }
        }
        let changed = !updates.is_empty();
        push_docs(&tx, account, updates, now).await?;
        tx.commit().await?;
        if changed {
            self.changed(account);
        }
        Ok(())
    }

    /// Removes every task a connection imported, and its identity records,
    /// when the student removes that connection.
    pub(crate) async fn delete_import_tasks(
        &self,
        account: &str,
        source: &str,
        connection: &str,
    ) -> anyhow::Result<()> {
        let mut client = self.db.client().await?;
        let tx = client.transaction().await?;
        tx.exec("UPDATE accounts SET seq=seq WHERE id=$1", &[&account]).await?;
        let now = self.now();
        let mut removals = Vec::new();
        for row in tx
            .fetch_all("SELECT id,seq,body FROM docs WHERE account=$1 AND type='task' AND deleted=0", &[&account])
            .await?
        {
            let id: String = row.try_get(0)?;
            let rev: i64 = row.try_get::<_, Option<i64>>(1)?.unwrap_or_default();
            let raw: String = row.try_get::<_, Option<String>>(2)?.unwrap_or_default();
            let body = task_body(raw.as_bytes());
            if body.get("source").and_then(Value::as_str) == Some(source)
                && body.get("canvasConnection").and_then(Value::as_str) == Some(connection)
            {
                removals.push(task_doc(id, rev, now.timestamp_millis(), None));
            }
        }
        tx.exec("DELETE FROM canvas_items WHERE account=$1 AND connection=$2", &[&account, &connection]).await?;
        let changed = !removals.is_empty();
        push_docs(&tx, account, removals, now).await?;
        tx.commit().await?;
        if changed {
            self.changed(account);
        }
        Ok(())
    }
}

/// A task document revision based on `rev`: a body, or a deletion.
fn task_doc(id: String, rev: i64, updated: i64, body: Option<String>) -> Doc {
    let deleted = body.is_none();
    Doc {
        id,
        kind: "task".into(),
        base: rev,
        updated,
        deleted,
        body: body.map(|b| RawValue::from_string(b).expect("serialized JSON is valid")),
        ..Doc::default()
    }
}

/// Reports whether a field is present and not null.
fn present(body: &DocBody, key: &str) -> bool {
    !matches!(body.get(key), None | Some(Value::Null))
}

/// The string value of a JSON field; empty for a missing or non-string one.
pub(crate) fn string_value(v: Option<&Value>) -> &str {
    v.and_then(Value::as_str).unwrap_or_default()
}

/// Reports whether two RFC 3339 times name the same instant, or are equal
/// strings (including both empty).
pub(crate) fn same_instant(a: &str, b: &str) -> bool {
    if a == b {
        return true;
    }
    match (crate::streaks::parse_instant(a), crate::streaks::parse_instant(b)) {
        (Some(x), Some(y)) => x == y,
        _ => false,
    }
}

/// Parses an account document body, keeping every field, including ones a
/// newer client added; anything but a JSON object yields an empty map.
pub(crate) fn task_body(raw: &[u8]) -> DocBody {
    match serde_json::from_slice::<Value>(raw) {
        Ok(Value::Object(map)) => map,
        _ => DocBody::new(),
    }
}

/// `GET /api/me/canvas`: the account's Canvas connection.
pub(crate) async fn handle_account_canvas(State(s): State<Arc<Server>>, Auth(se): Auth) -> ApiResult {
    write_connection(&s, &se.account).await
}

async fn write_connection(s: &Server, account: &str) -> ApiResult {
    let c = s.canvas_connection(account).await.map_err(|_| ApiError::new(500, "could not read Canvas connection"))?;
    let mut response = ok(&c)?;
    response.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    Ok(response)
}

#[derive(Default, Deserialize)]
#[serde(default)]
struct ConnectRequest {
    #[serde(alias = "Base", deserialize_with = "null_default")]
    base: String,
    #[serde(alias = "Token", deserialize_with = "null_default")]
    token: String,
}

/// The base URL with path, query, and fragment cleared:
/// `scheme://host[:port]`, with the authority as the student wrote it when
/// it is plain, else as the URL parser normalized it. Stored bases are part
/// of each credential's associated data, so this form must not change:
/// credentials sealed by earlier releases would no longer open.
fn origin_string(raw: &str, parsed: &url::Url) -> String {
    let authority = raw
        .split_once("://")
        .map(|(_, rest)| rest.split(['/', '?', '#']).next().unwrap_or_default())
        .filter(|a| !a.is_empty() && !a.contains('@') && !a.contains('\\'));
    match authority {
        Some(authority) => format!("{}://{authority}", parsed.scheme()),
        None => parsed[..url::Position::BeforePath].to_owned(),
    }
}

/// `PUT /api/me/canvas`: connects Canvas with a base URL and token (4 KiB),
/// importing at once with module routing so completion works immediately.
pub(crate) async fn handle_connect_canvas(State(s): State<Arc<Server>>, Auth(se): Auth, body: Body) -> ApiResult {
    let input = read_json::<ConnectRequest>(body, 4 << 10).await.filter(|i| !i.token.trim().is_empty());
    let Some(input) = input else { return fail(400, "Canvas address and token are required") };
    let raw = input.base.trim();
    let mut base = s.outbound.safe_url(raw).map_err(|e| ApiError::new(400, e))?;
    // The origin, not a user-supplied path, identifies this Canvas instance.
    base.set_path("");
    base.set_query(None);
    base.set_fragment(None);
    let origin = origin_string(raw, &base);
    if !s.integrations_available() {
        return fail(503, "Account Canvas connections are not configured on this server");
    }
    let Ok(lock) = s.lock_canvas(&se.account).await else { return fail(503, "the request was cancelled") };
    let cancelled = fail(503, "the request was cancelled");
    under_lease(&lock.lease, cancelled, async {
        let mut c = s
            .canvas_connection(&se.account)
            .await
            .map_err(|_| ApiError::new(500, "could not read Canvas connection"))?;
        if c.connected && c.base != origin {
            return fail(409, "This account already uses a different Canvas address");
        }
        if c.id.is_empty() || c.base != origin {
            c.id = new_token();
            c.base = origin.clone();
        }
        let credential = s
            .encrypt_canvas(&se.account, &c, &input.token)
            .map_err(|_| ApiError::new(500, "could not protect Canvas credential"))?;
        let tasks = s
            .fetch_canvas_tasks(&base, &input.token, None, true)
            .await
            .map_err(|e| ApiError::new(502, e.to_string()))?;
        c.credential = credential;
        c.connected = true;
        c.synced = s.now().timestamp();
        c.modules_at = s.now().timestamp();
        c.error.clear();
        if s.publish_canvas(&se.account, &c, &tasks, true).await.is_err() {
            return fail(500, "could not save Canvas connection");
        }
        ok(&c)
    })
    .await
}

/// `DELETE /api/me/canvas`: forgets the token. Imported assignments remain
/// available offline and in the account.
pub(crate) async fn handle_disconnect_canvas(State(s): State<Arc<Server>>, Auth(se): Auth) -> ApiResult {
    let Ok(lock) = s.lock_canvas(&se.account).await else { return fail(503, "the request was cancelled") };
    let cancelled = fail(503, "the request was cancelled");
    under_lease(&lock.lease, cancelled, async {
        let empty: &[u8] = &[];
        s.db.exec("UPDATE canvas_connections SET credential=$1,error='' WHERE account=$2", &[&empty, &se.account])
            .await
            .map_err(|_| ApiError::new(500, "could not disconnect Canvas"))?;
        write_connection(&s, &se.account).await
    })
    .await
}

/// `POST /api/me/canvas/refresh`: imports now, per [`CanvasRefreshOptions`]
/// (1 KiB body).
pub(crate) async fn handle_refresh_canvas(State(s): State<Arc<Server>>, Auth(se): Auth, body: Body) -> ApiResult {
    let Some(options) = read_json::<CanvasRefreshOptions>(body, 1024).await else {
        return fail(400, "invalid refresh");
    };
    let c = s.refresh_canvas_connection(&se.account, options).await?;
    ok(&c)
}

#[derive(Default, Deserialize)]
#[serde(default)]
struct CompleteRequest {
    #[serde(deserialize_with = "null_default")]
    document: String,
    #[serde(deserialize_with = "null_default")]
    intent: String,
}

/// `POST /api/me/canvas/complete`: sends a task's pending completion intent
/// to Canvas and acknowledges it in the document. The current document is
/// read after the client uploads its intent, so stale device metadata
/// cannot select another item; both clients write the same durable intent
/// ID, so repeated delivery is harmless.
pub(crate) async fn handle_account_canvas_complete(
    State(s): State<Arc<Server>>,
    Auth(se): Auth,
    body: Body,
) -> ApiResult {
    let input = read_json::<CompleteRequest>(body, 1024).await.filter(|i| DOC_ID.is_match(&i.document));
    let Some(input) = input else { return fail(400, "task document is required") };
    let Ok(lock) = s.lock_canvas(&se.account).await else { return fail(503, "the request was cancelled") };
    let cancelled = fail(503, "the request was cancelled");
    under_lease(&lock.lease, cancelled, async {
        let c = match s.canvas_connection(&se.account).await {
            Ok(c) if c.connected => c,
            _ => return fail(409, "Reconnect Canvas to send completion changes"),
        };
        let row =
            s.db.fetch_opt(
                "SELECT body,seq FROM docs WHERE account=$1 AND id=$2 AND deleted=0",
                &[&se.account, &input.document],
            )
            .await;
        let (raw, rev) = match row {
            Ok(Some(row)) => match (row.try_get::<_, Option<String>>(0), row.try_get::<_, Option<i64>>(1)) {
                (Ok(Some(raw)), Ok(Some(rev))) => (raw, rev),
                _ => return fail(404, "Canvas task not found"),
            },
            _ => return fail(404, "Canvas task not found"),
        };
        let mut body = task_body(raw.as_bytes());
        let uid = string_value(body.get("uid")).to_owned();
        let parts: Vec<&str> = uid.splitn(3, ':').collect();
        if body.get("source").and_then(Value::as_str) != Some("canvas")
            || parts.len() != 3
            || parts[0] != "canvas"
            || (present(&body, "canvasConnection")
                && body.get("canvasConnection").and_then(Value::as_str) != Some(&c.id))
        {
            return fail(400, "not a Canvas task");
        }
        let intent = string_value(body.get("pendingCanvasDone"));
        if intent.is_empty() || intent != input.intent {
            return ok(&serde_json::json!({ "applied": false }));
        }
        let token = s
            .decrypt_canvas(&se.account, &c)
            .map_err(|_| ApiError::new(503, "Canvas credential is unavailable; reconnect Canvas"))?;
        let base = s.outbound.safe_url(&c.base).map_err(|_| ApiError::new(500, "invalid saved Canvas address"))?;
        let completion = CanvasCompletion {
            token,
            kind: parts[1].to_owned(),
            id: parts[2].to_owned(),
            override_id: string_value(body.get("overrideId")).to_owned(),
            course_id: string_value(body.get("courseId")).to_owned(),
            module_item: string_value(body.get("moduleItem")).to_owned(),
            done: body.get("done") == Some(&Value::Bool(true)),
            ..CanvasCompletion::default()
        };
        let mut out = s
            .complete_canvas(&base, &completion)
            .await
            .map_err(|_| ApiError::new(502, "Canvas could not be updated; the change will retry"))?;
        body.insert("overrideId".into(), out.get("overrideId").cloned().unwrap_or(Value::Null));
        if out.get("module") == Some(&Value::Bool(true)) {
            body.remove("pendingCanvasDone");
            body.remove("pendingDone");
        }
        let encoded = serde_json::to_string(&body).expect("document bodies serialize");
        let acknowledge = async {
            let mut client = s.db.client().await?;
            let tx = client.transaction().await?;
            let doc = task_doc(input.document.clone(), rev, s.now().timestamp_millis(), Some(encoded));
            let (_, conflicts, _) = push_docs(&tx, &se.account, vec![doc], s.now()).await?;
            tx.exec("UPDATE canvas_connections SET synced=0 WHERE account=$1", &[&se.account]).await?;
            tx.commit().await?;
            Ok::<bool, db::Error>(conflicts.is_empty())
        };
        let applied = acknowledge.await.map_err(|_| ApiError::new(500, "could not acknowledge Canvas change"))?;
        s.changed(&se.account);
        out.insert("applied".into(), Value::Bool(applied));
        ok(&out)
    })
    .await
}

#[cfg(test)]
#[path = "canvas_account_tests.rs"]
pub(crate) mod tests;
