//! Account document sync: schedules (`s_…`), tasks (`t_…`), and `prefs`,
//! each with an account-scoped revision. Uploads and downloads share one
//! transaction, so an upload never hides a remote change behind its own
//! cursor.
//!
//! Document bodies are stored compacted, with `<`, `>`, `&`, U+2028, and
//! U+2029 escaped (see [`crate::api::compact_json`]), the form responses
//! carry, so downloads copy them unchanged. Bodies stored verbatim by
//! earlier releases are converted as they are read. Page byte budgets count
//! that encoding.

use std::collections::HashMap;
use std::sync::{Arc, LazyLock};

use axum::body::Body;
use axum::extract::{Path, State};
use chrono::{DateTime, Utc};
use regex::Regex;
use serde::de::{self, IgnoredAny, MapAccess, Visitor};
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::value::RawValue;
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};

use crate::Server;
use crate::api::{ApiResult, compact_json, fail, final_json, null_default, ok, read_json};
use crate::db::{self, Param, Sql, Tx};
use crate::session::Auth;

/// Document IDs: 1–64 of `A-Za-z0-9_-`.
pub(crate) static DOC_ID: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[A-Za-z0-9_-]{1,64}$").expect("valid pattern"));
/// Document types: 1–16 lowercase letters.
pub(crate) static DOC_TYPE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[a-z]{1,16}$").expect("valid pattern"));

/// Documents one download returns at most.
pub(crate) const PULL_LIMIT: usize = 500;
/// Documents one upload may carry.
pub(crate) const PUSH_LIMIT: usize = 200;
/// The largest document body (64 KiB).
pub(crate) const DOC_BODY_MAX: usize = 64 << 10;
/// The largest sync request (4 MiB).
pub(crate) const PUSH_BODY_MAX: usize = 4 << 20;
/// The encoded size a download page stays within (4 MiB).
pub(crate) const PULL_BODY_MAX: usize = 4 << 20;
/// Earlier versions one history request returns.
pub(crate) const HISTORY_LIMIT: i64 = 30;

/// Revisions read per query while checking an upload's bases.
const REVISION_BATCH: usize = 200;
/// Rows written per multi-row insert.
const INSERT_BATCH: usize = 100;

/// One document revision, uploaded or downloaded. `base` is the revision an
/// upload was edited from. A JSON `null` body is kept as the text `null`.
///
/// `body` is written to responses as it is. [`push_docs`] and stored reads
/// put it in the encoding of [`compact_json`] first.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct Doc {
    #[serde(deserialize_with = "null_default")]
    pub(crate) id: String,
    #[serde(rename = "type", deserialize_with = "null_default")]
    pub(crate) kind: String,
    #[serde(deserialize_with = "null_default")]
    pub(crate) rev: i64,
    #[serde(skip_serializing_if = "is_zero", deserialize_with = "null_default")]
    pub(crate) base: i64,
    #[serde(deserialize_with = "null_default")]
    pub(crate) updated: i64,
    #[serde(skip_serializing_if = "std::ops::Not::not", deserialize_with = "null_default")]
    pub(crate) deleted: bool,
    #[serde(skip_serializing_if = "Option::is_none", deserialize_with = "raw_some")]
    pub(crate) body: Option<Box<RawValue>>,
}

fn is_zero(v: &i64) -> bool {
    *v == 0
}

/// Deserializes any JSON value, `null` included, as `Some(raw)`.
fn raw_some<'de, D: Deserializer<'de>>(d: D) -> Result<Option<Box<RawValue>>, D::Error> {
    Box::<RawValue>::deserialize(d).map(Some)
}

/// A stored body as raw JSON in the encoding of [`compact_json`]; an empty
/// one is no body.
fn stored_body(body: String) -> db::Result<Option<Box<RawValue>>> {
    if body.is_empty() {
        return Ok(None);
    }
    RawValue::from_string(final_json(body))
        .map(Some)
        .map_err(|_| db::Error::Config("a stored document body is not JSON".into()))
}

/// Puts an uploaded body in the encoding of [`compact_json`].
fn final_body(raw: Box<RawValue>) -> db::Result<Box<RawValue>> {
    RawValue::from_string(final_json(Box::<str>::from(raw).into_string()))
        .map_err(|_| db::Error::Config("an uploaded document body is not JSON".into()))
}

/// Counts the bytes written to it.
struct ByteCount(usize);

impl std::io::Write for ByteCount {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0 += buf.len();
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// An accepted upload and the revision it received (0 for a deletion of a
/// document the server never had).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct AppliedDoc {
    pub(crate) id: String,
    pub(crate) rev: i64,
}

/// The `POST /api/me/sync` request.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct DocPush {
    #[serde(deserialize_with = "null_default")]
    pub(crate) docs: Vec<Doc>,
    #[serde(deserialize_with = "null_default")]
    pub(crate) since: i64,
    #[serde(deserialize_with = "null_default")]
    pub(crate) watches_version: String,
}

/// The `id` member of a document body, matched case-insensitively with the
/// last occurrence winning. `null` and objects decode; any other JSON value
/// is an error.
struct BodyId(Option<String>);

impl<'de> Deserialize<'de> for BodyId {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct Probe;
        impl<'de> Visitor<'de> for Probe {
            type Value = BodyId;
            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("an object or null")
            }
            fn visit_unit<E: de::Error>(self) -> Result<BodyId, E> {
                Ok(BodyId(None))
            }
            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<BodyId, A::Error> {
                let mut id = None;
                while let Some(key) = map.next_key::<String>()? {
                    if key.eq_ignore_ascii_case("id") {
                        id = map.next_value::<Option<String>>()?;
                    } else {
                        map.next_value::<IgnoredAny>()?;
                    }
                }
                Ok(BodyId(id))
            }
        }
        d.deserialize_any(Probe)
    }
}

/// Checks shape and identity: at most [`PUSH_LIMIT`] documents, valid IDs
/// and types, bodies within [`DOC_BODY_MAX`] that are JSON unless deleted,
/// the type the ID prefix names, and a body `id` matching the document ID
/// without its prefix. The prefix names the type, and a body must describe
/// the document it is stored under, so one client's mistake cannot
/// overwrite another document or store a schedule as a task.
pub(crate) fn valid_docs(docs: &[Doc]) -> bool {
    if docs.len() > PUSH_LIMIT {
        return false;
    }
    docs.iter().all(|d| {
        let body_len = d.body.as_ref().map_or(0, |b| b.get().len());
        if !DOC_ID.is_match(&d.id)
            || !DOC_TYPE.is_match(&d.kind)
            || body_len > DOC_BODY_MAX
            || (!d.deleted && d.body.is_none())
        {
            return false;
        }
        let expected = if d.id.starts_with("s_") {
            "schedule"
        } else if d.id.starts_with("t_") {
            "task"
        } else if d.id == "prefs" {
            "prefs"
        } else {
            return false;
        };
        if d.kind != expected {
            return false;
        }
        if d.deleted {
            return true;
        }
        let Some(body) = &d.body else { return false };
        let Ok(BodyId(id)) = serde_json::from_str::<BodyId>(body.get()) else { return false };
        match id {
            Some(id) if expected != "prefs" => id == d.id[2..],
            _ => true,
        }
    })
}

/// One row bound for `docs`.
struct DocRow {
    id: String,
    kind: String,
    seq: i64,
    updated: i64,
    deleted: i64,
    body: String,
}

/// One replaced version bound for `doc_history`.
struct HistoryRow {
    id: String,
    rev: i64,
    updated: i64,
    body: String,
}

/// Runs `prefix VALUES (…),(…) suffix` for `rows` in batches of
/// [`INSERT_BATCH`], each row contributing `width` parameters. Query text
/// comes only from constants; every value is a parameter.
pub(crate) async fn insert_rows(
    tx: &Tx<'_>,
    prefix: &str,
    suffix: &str,
    width: usize,
    rows: &[Vec<Param<'_>>],
) -> db::Result<()> {
    for batch in rows.chunks(INSERT_BATCH) {
        let marks = (0..batch.len())
            .map(|row| {
                let params = (1..=width).map(|column| format!("${}", row * width + column)).collect::<Vec<_>>();
                format!("({})", params.join(","))
            })
            .collect::<Vec<_>>()
            .join(",");
        let params: Vec<Param> = batch.iter().flatten().copied().collect();
        tx.exec(&format!("{prefix} VALUES {marks} {suffix}"), &params).await?;
    }
    Ok(())
}

/// Reads one `docs` row selected as `id,type,seq,updated_at,deleted,body`.
fn read_doc(row: &db::Row) -> db::Result<Doc> {
    Ok(Doc {
        id: row.try_get(0)?,
        kind: row.try_get(1)?,
        rev: row.try_get(2)?,
        base: 0,
        updated: row.try_get(3)?,
        deleted: row.try_get::<_, i64>(4)? == 1,
        body: stored_body(row.try_get(5)?)?,
    })
}

/// Applies uploads inside `tx`: each document whose stored revision equals
/// its `base` gets the next account sequence; others are returned as
/// conflicts with the stored version. Replaced bodies go to `doc_history`
/// (archived at `now`). Returns the accepted documents, the conflicts, and
/// the account sequence afterwards.
///
/// One upload may carry several revisions of a document: each is checked
/// against the previous accepted one, every intermediate body is kept in
/// history, and only the last accepted body is written.
pub(crate) async fn push_docs(
    tx: &Tx<'_>,
    account: &str,
    docs: Vec<Doc>,
    now: DateTime<Utc>,
) -> db::Result<(Vec<AppliedDoc>, Vec<Doc>, i64)> {
    let (mut applied, mut conflicts) = (Vec::new(), Vec::new());
    // Take the account row lock before reading revisions.
    if !docs.is_empty() {
        tx.exec("UPDATE accounts SET seq=seq WHERE id=$1", &[&account]).await?;
    }
    let mut seq: i64 = tx.fetch_one("SELECT seq FROM accounts WHERE id=$1", &[&account]).await?.try_get(0)?;
    if docs.is_empty() {
        return Ok((applied, conflicts, seq));
    }
    // Read revisions in bounded batches before assigning sequences, rather
    // than one round trip per document.
    let mut current: HashMap<String, Doc> = HashMap::new();
    for batch in docs.chunks(REVISION_BATCH) {
        let ids: Vec<&str> = batch.iter().map(|d| d.id.as_str()).collect();
        let rows = tx
            .fetch_all(
                "SELECT id,type,seq,updated_at,deleted,body FROM docs WHERE account=$1 AND id=ANY($2)",
                &[&account, &ids],
            )
            .await?;
        for row in &rows {
            let d = read_doc(row)?;
            current.insert(d.id.clone(), d);
        }
    }
    let archived = now.timestamp();
    let mut history = Vec::new();
    // PostgreSQL forbids an upsert batch affecting the same row twice, so
    // only the final accepted body of each document is written.
    let mut pending: HashMap<String, DocRow> = HashMap::new();
    let mut order = Vec::new();
    for mut d in docs {
        let cur = current.get(&d.id);
        if let Some(cur) = cur
            && cur.rev != d.base
        {
            conflicts.push(cur.clone());
            continue;
        }
        if cur.is_none() && d.deleted {
            applied.push(AppliedDoc { id: d.id, rev: 0 });
            continue;
        }
        if let Some(cur) = cur
            && !cur.deleted
        {
            history.push(HistoryRow {
                id: d.id.clone(),
                rev: cur.rev,
                updated: cur.updated,
                body: cur.body.as_ref().map(|b| b.get().to_owned()).unwrap_or_default(),
            });
        }
        if !d.deleted
            && let Some(raw) = d.body.take()
        {
            d.body = Some(final_body(raw)?);
        }
        seq += 1;
        let (body, deleted) = if d.deleted {
            (String::new(), 1)
        } else {
            (d.body.as_ref().map(|b| b.get().to_owned()).unwrap_or_default(), 0)
        };
        if !pending.contains_key(&d.id) {
            order.push(d.id.clone());
        }
        pending.insert(
            d.id.clone(),
            DocRow { id: d.id.clone(), kind: d.kind.clone(), seq, updated: d.updated, deleted, body },
        );
        d.rev = seq;
        d.base = 0;
        if d.deleted {
            d.body = None;
        }
        applied.push(AppliedDoc { id: d.id.clone(), rev: seq });
        current.insert(d.id.clone(), d);
    }
    let zero: i64 = 0;
    let history_params: Vec<Vec<Param>> =
        history.iter().map(|h| vec![&account as Param, &h.id, &h.rev, &h.updated, &zero, &h.body, &archived]).collect();
    insert_rows(
        tx,
        "INSERT INTO doc_history(account,id,rev,updated_at,deleted,body,archived_at)",
        "ON CONFLICT DO NOTHING",
        7,
        &history_params,
    )
    .await?;
    let updates: Vec<&DocRow> = order.iter().filter_map(|id| pending.get(id)).collect();
    let update_params: Vec<Vec<Param>> = updates
        .iter()
        .map(|r| vec![&account as Param, &r.id, &r.kind, &r.seq, &r.updated, &r.deleted, &r.body])
        .collect();
    insert_rows(
        tx,
        "INSERT INTO docs(account,id,type,seq,updated_at,deleted,body)",
        "ON CONFLICT(account,id) DO UPDATE SET type=excluded.type,seq=excluded.seq,updated_at=excluded.updated_at,deleted=excluded.deleted,body=excluded.body",
        7,
        &update_params,
    )
    .await?;
    tx.exec("UPDATE accounts SET seq=$1 WHERE id=$2", &[&seq, &account]).await?;
    Ok((applied, conflicts, seq))
}

/// Reads changes after `since` inside `tx`, at most [`PULL_LIMIT`] rows and
/// [`PULL_BODY_MAX`] encoded bytes. Rows this request just uploaded
/// (`acknowledged` id → rev) advance the cursor without being returned.
/// Returns the documents, the cursor, and whether more remain. The cursor
/// never passes a row left for the next page.
pub(crate) async fn pull_docs(
    tx: &Tx<'_>,
    account: &str,
    since: i64,
    acknowledged: &HashMap<String, i64>,
) -> db::Result<(Vec<Doc>, i64, bool)> {
    let seq: i64 = tx.fetch_one("SELECT seq FROM accounts WHERE id=$1", &[&account]).await?.try_get(0)?;
    let limit = PULL_LIMIT as i64 + 1;
    let rows = tx
        .fetch_all(
            "SELECT id,type,seq,updated_at,deleted,body FROM docs WHERE account=$1 AND seq>$2 ORDER BY seq LIMIT $3",
            &[&account, &since, &limit],
        )
        .await?;
    let mut docs = Vec::new();
    let (mut scanned, mut size, mut cursor) = (0usize, 2usize, since);
    for row in &rows {
        let d = read_doc(row)?;
        if scanned == PULL_LIMIT {
            return Ok((docs, cursor, true));
        }
        if acknowledged.get(&d.id).copied().unwrap_or(0) == d.rev {
            scanned += 1;
            cursor = d.rev;
            continue;
        }
        // Count the encoded document so the response itself stays within
        // the bound.
        let mut encoded = ByteCount(0);
        serde_json::to_writer(&mut encoded, &d).map_err(|e| db::Error::Config(e.to_string()))?;
        let encoded = encoded.0;
        if size + encoded + 1 > PULL_BODY_MAX && scanned > 0 {
            return Ok((docs, cursor, true));
        }
        scanned += 1;
        cursor = d.rev;
        size += encoded + 1;
        docs.push(d);
    }
    Ok((docs, seq, false))
}

impl Server {
    /// Adds `watches_version` (SHA-256 of the account's alerts as JSON) to
    /// `out`, and `watches` when it differs from the client's `version`.
    /// Alerts change independently of document revisions (class labels
    /// included); the hash spares unchanged lists the transfer.
    pub(crate) async fn attach_watches(
        &self,
        account: &str,
        version: &str,
        out: &mut Map<String, Value>,
    ) -> db::Result<()> {
        let watches = self.account_watches(account).await?;
        let data = compact_json(&serde_json::to_string(&watches).expect("alerts serialize"));
        let current = hex::encode(Sha256::digest(data.as_bytes()));
        if version != current {
            out.insert("watches".into(), serde_json::to_value(&watches).expect("alerts serialize"));
        }
        out.insert("watches_version".into(), Value::String(current));
        Ok(())
    }
}

/// The `POST /api/me/sync` response, its fields in sorted order.
#[derive(Serialize)]
struct SyncReply {
    applied: Vec<AppliedDoc>,
    conflicts: Vec<Doc>,
    docs: Vec<Doc>,
    more: bool,
    seq: i64,
    #[serde(flatten)]
    watches: Map<String, Value>,
}

/// `POST /api/me/sync`: uploads changed documents and downloads the rest
/// ([`DocPush`]; 4 MiB). Answers `{"seq", "applied", "conflicts", "docs",
/// "more"}` plus watches on the last page. A request without uploads reads
/// one repeatable-read snapshot.
pub(crate) async fn handle_sync_docs(State(s): State<Arc<Server>>, Auth(se): Auth, body: Body) -> ApiResult {
    let input = match read_json::<DocPush>(body, PUSH_BODY_MAX).await {
        Some(input) if valid_docs(&input.docs) && input.since >= 0 => input,
        _ => return fail(400, "bad push"),
    };
    let (applied, conflicts, docs, cursor, more) = {
        let Ok(mut client) = s.db.client().await else { return fail(500, "could not sync documents") };
        let started = if input.docs.is_empty() {
            client
                .build_transaction()
                .isolation_level(tokio_postgres::IsolationLevel::RepeatableRead)
                .read_only(true)
                .start()
                .await
        } else {
            client.transaction().await
        };
        let Ok(tx) = started else { return fail(500, "could not sync documents") };
        let Ok((applied, conflicts, _)) = push_docs(&tx, &se.account, input.docs, s.now()).await else {
            return fail(500, "could not save documents");
        };
        let acknowledged: HashMap<String, i64> = applied.iter().map(|d| (d.id.clone(), d.rev)).collect();
        let Ok((docs, cursor, more)) = pull_docs(&tx, &se.account, input.since, &acknowledged).await else {
            return fail(500, "could not read changes");
        };
        if tx.commit().await.is_err() {
            return fail(500, "could not finish sync");
        }
        (applied, conflicts, docs, cursor, more)
    };
    let mut watches = Map::new();
    if !more && s.attach_watches(&se.account, &input.watches_version, &mut watches).await.is_err() {
        return fail(500, "could not read alerts");
    }
    if !applied.is_empty() {
        s.count("edits", applied.len() as i64);
        s.changed(&se.account);
    }
    ok(&SyncReply { applied, conflicts, docs, more, seq: cursor, watches })
}

/// One earlier version in `GET /api/me/history/{id}`.
#[derive(Serialize)]
struct Version {
    rev: i64,
    updated: i64,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    deleted: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    body: Option<Box<RawValue>>,
    archived: i64,
}

#[derive(Serialize)]
struct History {
    id: String,
    versions: Vec<Version>,
}

/// `GET /api/me/history/{id}`: earlier versions of one document, newest
/// first. Restoring one is an ordinary upload of that body.
pub(crate) async fn handle_history(State(s): State<Arc<Server>>, Auth(se): Auth, Path(id): Path<String>) -> ApiResult {
    if !DOC_ID.is_match(&id) {
        return fail(400, "bad document");
    }
    let read = async {
        let rows = s
            .db
            .fetch_all(
                "SELECT rev,updated_at,deleted,body,archived_at FROM doc_history WHERE account=$1 AND id=$2 ORDER BY rev DESC LIMIT $3",
                &[&se.account, &id, &HISTORY_LIMIT],
            )
            .await?;
        rows.iter()
            .map(|row| {
                Ok(Version {
                    rev: row.try_get(0)?,
                    updated: row.try_get(1)?,
                    deleted: row.try_get::<_, i64>(2)? == 1,
                    body: stored_body(row.try_get(3)?)?,
                    archived: row.try_get(4)?,
                })
            })
            .collect::<db::Result<Vec<_>>>()
    };
    match read.await {
        Ok(versions) => ok(&History { id, versions }),
        Err(_) => fail(500, "could not read history"),
    }
}
