//! Seat alerts: create, delete, and retarget. A guest alert names its push
//! destination (`channel`, `target`); a signed-in alert uses the account
//! channel instead.

use std::sync::Arc;

use axum::body::Body;
use axum::extract::{Path, Request, State};
use axum::http::StatusCode;
use serde::Deserialize;

use crate::Server;
use crate::api::{ApiError, ApiResult, empty, fail, json, null_default, ok, read_json};
use crate::db::{self, Sql};
use crate::push::push_endpoint;
use crate::session::{SESSION_TTL, new_id};

/// One seat alert or reminder destination: a push subscription in
/// `target`, or an account, which fans out to every device signed in to it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Watch {
    pub(crate) id: String,
    pub(crate) target: String,
    pub(crate) account: String,
    pub(crate) last_alert: i64,
}

/// The largest watch request body (8 KiB).
const WATCH_BODY_MAX: usize = 8 << 10;
/// Alerts one subscriber may hold.
const WATCH_LIMIT: i64 = 200;

#[derive(Default, Deserialize)]
#[serde(default)]
struct CreateWatch {
    #[serde(alias = "Inst", deserialize_with = "null_default")]
    inst: String,
    #[serde(alias = "Term", deserialize_with = "null_default")]
    term: String,
    #[serde(alias = "Section", deserialize_with = "null_default")]
    section: String,
    #[serde(alias = "Channel", deserialize_with = "null_default")]
    channel: String,
    #[serde(alias = "Target", deserialize_with = "null_default")]
    target: String,
}

/// `POST /api/watches`: creates a seat alert (`inst`, `term`, `section`,
/// `channel`, `target`; 8 KiB). The body is decoded before the session is read,
/// so a malformed body is 400 even with a bad session; a signed-in request
/// always uses the account channel. At most 200 alerts per subscriber. Answers
/// 201 `{"id"}`, or 200 with the existing alert's id for an account.
pub(crate) async fn handle_create_watch(State(s): State<Arc<Server>>, request: Request) -> ApiResult {
    let (parts, body) = request.into_parts();
    let Some(mut input) = read_json::<CreateWatch>(body, WATCH_BODY_MAX).await else {
        return fail(400, "bad json");
    };
    let mut account = String::new();
    if let Some(se) = s.session(&parts).await? {
        account = se.account;
        input.channel = "account".into();
        input.target = String::new();
    }
    match input.channel.as_str() {
        "account" => {
            if account.is_empty() {
                return fail(401, "sign in required");
            }
            let since = s.now().timestamp() - SESSION_TTL.as_secs() as i64;
            let push =
                s.db.fetch_one(
                    "SELECT COUNT(*) FROM sessions WHERE account=$1 AND push<>'' AND last_seen>$2",
                    &[&account, &since],
                )
                .await
                .and_then(|row| Ok(row.try_get::<_, i64>(0)?));
            match push {
                Err(_) => return fail(500, "could not read notification devices"),
                Ok(0) => return fail(400, "turn on notifications on a device first"),
                Ok(_) => {}
            }
        }
        "push" => {
            if push_endpoint(&input.target).is_empty() {
                return fail(400, "bad push subscription");
            }
        }
        _ => return fail(400, "channel must be push"),
    }
    let Ok(mut client) = s.db.client().await else { return fail(500, "could not create alert") };
    let Ok(tx) = client.transaction().await else { return fail(500, "could not create alert") };
    let create = || ApiError::new(500, "could not create alert");
    // Serialize account writes before checking duplicates and subscriber limits.
    tx.exec("UPDATE accounts SET seq=seq WHERE id=$1", &[&account]).await.map_err(|_| create())?;
    if account.is_empty() {
        tx.exec("SELECT pg_advisory_xact_lock(hashtextextended($1,0))", &[&format!("watch:{}", input.target)])
            .await
            .map_err(|_| create())?;
    }
    let count = |row: db::Row| -> db::Result<i64> { Ok(row.try_get(0)?) };
    let sections = tx
        .fetch_one(
            "SELECT COUNT(*) FROM sections WHERE inst=$1 AND term=$2 AND id=$3",
            &[&input.inst, &input.term, &input.section],
        )
        .await
        .and_then(count)
        .map_err(|_| ApiError::new(500, "could not read section"))?;
    if sections == 0 {
        return fail(404, "unknown section");
    }
    let held = if !account.is_empty() {
        let existing = tx
            .fetch_opt(
                "SELECT id FROM watches WHERE account=$1 AND inst=$2 AND term=$3 AND section=$4",
                &[&account, &input.inst, &input.term, &input.section],
            )
            .await
            .and_then(|row| row.map(|row| row.try_get::<_, String>(0)).transpose().map_err(Into::into));
        match existing {
            Err(_) => return fail(500, "could not read existing alerts"),
            Ok(Some(id)) => return ok(&serde_json::json!({ "id": id })),
            Ok(None) => {}
        }
        tx.fetch_one("SELECT COUNT(*) FROM watches WHERE account=$1", &[&account]).await.and_then(count)
    } else {
        tx.fetch_one("SELECT COUNT(*) FROM watches WHERE target=$1", &[&input.target]).await.and_then(count)
    };
    let held = held.map_err(|_| ApiError::new(500, "could not count alerts"))?;
    if held >= WATCH_LIMIT {
        return fail(429, "too many watches for this subscriber");
    }
    let id = new_id();
    tx.exec(
        "INSERT INTO watches(id,inst,term,section,channel,target,endpoint,created_at,account) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9)",
        &[
            &id,
            &input.inst,
            &input.term,
            &input.section,
            &input.channel,
            &input.target,
            &push_endpoint(&input.target),
            &s.now().timestamp(),
            &account,
        ],
    )
    .await
    .map_err(|_| create())?;
    tx.commit().await.map_err(|_| create())?;
    s.changed(&account);
    s.count("watches_created", 1);
    Ok(json(StatusCode::CREATED, &serde_json::json!({ "id": id })))
}

/// `DELETE /api/watches/{id}`: removes an alert (204). An account's alert
/// needs that account's session, read only then ("not your alert"
/// otherwise); others are keyed by an unguessable id only the subscriber
/// holds.
pub(crate) async fn handle_delete_watch(
    State(s): State<Arc<Server>>,
    Path(id): Path<String>,
    request: Request,
) -> ApiResult {
    let (parts, _) = request.into_parts();
    let owner = match s.db.fetch_opt("SELECT account FROM watches WHERE id=$1", &[&id]).await {
        Ok(None) => String::new(),
        Ok(Some(row)) => match row.try_get::<_, String>(0) {
            Ok(owner) => owner,
            Err(_) => return fail(500, "could not read alert"),
        },
        Err(_) => return fail(500, "could not read alert"),
    };
    if !owner.is_empty() {
        match s.session(&parts).await? {
            Some(se) if se.account == owner => {}
            _ => return fail(403, "not your alert"),
        }
    }
    if s.db.exec("DELETE FROM watches WHERE id=$1 AND account=$2", &[&id, &owner]).await.is_err() {
        return fail(500, "could not remove alert");
    }
    s.changed(&owner);
    Ok(empty(204))
}

#[derive(Default, Deserialize)]
#[serde(default)]
struct Retarget {
    #[serde(alias = "From", deserialize_with = "null_default")]
    from: String,
    #[serde(alias = "To", deserialize_with = "null_default")]
    to: String,
}

/// `POST /api/push/retarget`: moves every watch and reminder on one push
/// subscription to another (`from`, `to`; 8 KiB); answers `{"moved": n}`.
/// Browsers replace a subscription now and then (the endpoint expires, or
/// the VAPID key changed); the page and the service worker report the new
/// one here so alerts keep arriving.
pub(crate) async fn handle_retarget(State(s): State<Arc<Server>>, body: Body) -> ApiResult {
    let Some(input) = read_json::<Retarget>(body, WATCH_BODY_MAX).await else {
        return fail(400, "bad json");
    };
    let (from, to) = (push_endpoint(&input.from), push_endpoint(&input.to));
    if from.is_empty() || to.is_empty() {
        return fail(400, "bad push subscription");
    }
    let mut moved: i64 = 0;
    for table in ["watches", "reminders"] {
        let query = format!("UPDATE {table} SET target=$1,endpoint=$2 WHERE endpoint=$3");
        match s.db.exec(&query, &[&input.to, &to, &from]).await {
            Ok(n) => moved += n as i64,
            Err(_) => return fail(500, "could not move subscriptions"),
        }
    }
    ok(&serde_json::json!({ "moved": moved }))
}
