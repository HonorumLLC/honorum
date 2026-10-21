//! The signed-in account: details, devices, push destinations, seat alert
//! list, and deletion.

use std::sync::Arc;

use axum::body::Body;
use axum::extract::{Path, State};
use axum::http::header::SET_COOKIE;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::Server;
use crate::api::{ApiResult, empty, fail, null_default, ok, read_json};
use crate::auth::clean_device;
use crate::db::{self, Sql};
use crate::push::apns::APNS_TOKEN;
use crate::push::push_endpoint;
use crate::session::{Auth, SESSION_TTL, Session};

/// One signed-in device as `GET /api/me` lists it.
#[derive(Serialize)]
struct Device {
    id: String,
    name: String,
    created: i64,
    last_seen: i64,
    push: bool,
    current: bool,
}

/// Reads `{"id", "phone", "seq", "device", "devices"}` for the session's
/// account, with devices seen within the session TTL before `now`, most
/// recent first, the current one marked. Works on the pool or inside a
/// transaction. Fails with `NoRows` for an unknown account.
pub(crate) async fn account_details(db: &dyn Sql, se: &Session, now: DateTime<Utc>) -> db::Result<Map<String, Value>> {
    let row = db.fetch_one("SELECT phone,seq FROM accounts WHERE id=$1", &[&se.account]).await?;
    let (phone, seq): (String, i64) = (row.try_get(0)?, row.try_get(1)?);
    let since = now.timestamp() - SESSION_TTL.as_secs() as i64;
    let rows = db
        .fetch_all(
            "SELECT id,name,created_at,last_seen,push<>'' FROM sessions WHERE account=$1 AND last_seen>$2 ORDER BY last_seen DESC,created_at,id",
            &[&se.account, &since],
        )
        .await?;
    let mut devices = Vec::with_capacity(rows.len());
    for row in rows {
        let id: String = row.try_get(0)?;
        devices.push(Device {
            current: id == se.id,
            id,
            name: row.try_get(1)?,
            created: row.try_get(2)?,
            last_seen: row.try_get(3)?,
            push: row.try_get(4)?,
        });
    }
    let mut out = Map::new();
    out.insert("id".into(), Value::String(se.account.clone()));
    out.insert("phone".into(), Value::String(phone));
    out.insert("seq".into(), Value::from(seq));
    out.insert("device".into(), Value::String(se.id.clone()));
    out.insert("devices".into(), serde_json::to_value(devices).expect("devices serialize"));
    Ok(out)
}

/// One of the account's seat alerts with its class label.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub(crate) struct WatchOut {
    pub(crate) id: String,
    pub(crate) inst: String,
    pub(crate) term: String,
    pub(crate) section: String,
    /// "COURSE-NUM", or empty when the section is gone.
    pub(crate) label: String,
    pub(crate) title: String,
    /// Always "account".
    pub(crate) channel: String,
}

impl Server {
    /// The account's seat alerts, oldest first.
    pub(crate) async fn account_watches(&self, account: &str) -> db::Result<Vec<WatchOut>> {
        let rows = self
            .db
            .fetch_all(
                "SELECT w.id,w.inst,w.term,w.section,CASE WHEN json_valid(s.data) THEN COALESCE(json_text(s.data,'$.course'),'') ELSE '' END,CASE WHEN json_valid(s.data) THEN COALESCE(json_text(s.data,'$.num'),'') ELSE '' END,CASE WHEN json_valid(s.data) THEN COALESCE(json_text(s.data,'$.title'),'') ELSE '' END FROM watches w LEFT JOIN sections s ON s.inst=w.inst AND s.term=w.term AND s.id=w.section WHERE w.account=$1 ORDER BY w.created_at,w.id",
                &[&account],
            )
            .await?;
        let mut out = Vec::with_capacity(rows.len());
        for row in rows {
            let (course, number): (String, String) = (row.try_get(4)?, row.try_get(5)?);
            out.push(WatchOut {
                id: row.try_get(0)?,
                inst: row.try_get(1)?,
                term: row.try_get(2)?,
                section: row.try_get(3)?,
                label: if course.is_empty() { String::new() } else { format!("{course}-{number}") },
                title: row.try_get(6)?,
                channel: "account".into(),
            });
        }
        Ok(out)
    }
}

/// `GET /api/me`: the account details.
pub(crate) async fn handle_me(State(s): State<Arc<Server>>, Auth(se): Auth) -> ApiResult {
    match account_details(&s.db, &se, s.now()).await {
        Ok(out) => ok(&out),
        Err(_) => fail(500, "could not read account"),
    }
}

#[derive(Default, Deserialize)]
#[serde(default)]
struct PatchRequest {
    #[serde(alias = "Device", deserialize_with = "null_default")]
    device: String,
}

/// `PATCH /api/me`: renames this device (`device`; 4 KiB) and returns the
/// details.
pub(crate) async fn handle_me_patch(State(s): State<Arc<Server>>, Auth(se): Auth, body: Body) -> ApiResult {
    let Some(input) = read_json::<PatchRequest>(body, 4 << 10).await else {
        return fail(400, "bad json");
    };
    if !input.device.is_empty()
        && s.db.exec("UPDATE sessions SET name=$1 WHERE id=$2", &[&clean_device(&input.device), &se.id]).await.is_err()
    {
        return fail(500, "could not rename device");
    }
    match account_details(&s.db, &se, s.now()).await {
        Ok(out) => ok(&out),
        Err(_) => fail(500, "could not read account"),
    }
}

/// Every table that holds an account's data, emptied when it is deleted;
/// the account row goes last.
const ACCOUNT_DELETES: &[&str] = &[
    "DELETE FROM account_streak_cache WHERE account=$1",
    "DELETE FROM account_streak_rollups WHERE account=$1",
    "DELETE FROM account_activity WHERE account=$1",
    "DELETE FROM account_timezones WHERE account=$1",
    "DELETE FROM account_streaks WHERE account=$1",
    "DELETE FROM account_streak_days WHERE account=$1",
    "DELETE FROM canvas_items WHERE account=$1",
    "DELETE FROM canvas_connections WHERE account=$1",
    "DELETE FROM feed_connections WHERE account=$1",
    "DELETE FROM google_connections WHERE account=$1",
    "DELETE FROM google_published WHERE account=$1",
    "DELETE FROM google_calendars WHERE account=$1",
    "DELETE FROM google_states WHERE account=$1",
    "DELETE FROM deliveries WHERE account=$1",
    "DELETE FROM docs WHERE account=$1",
    "DELETE FROM doc_history WHERE account=$1",
    "DELETE FROM watches WHERE account=$1",
    "DELETE FROM sessions WHERE account=$1",
    "DELETE FROM accounts WHERE id=$1",
];

/// `DELETE /api/me`: disconnects Google, deletes the account and
/// everything it holds, and clears the cookie (204).
pub(crate) async fn handle_me_delete(State(s): State<Arc<Server>>, Auth(se): Auth) -> ApiResult {
    // Google keeps no copy either: its calendar and access go first.
    {
        let Ok(lock) = s.lock_google(&se.account).await else {
            return fail(503, "Google Calendar is busy; try again");
        };
        if let Ok(Some(mut c)) = s.google_connection(&se.account).await {
            let lost = lock.lease.lost();
            tokio::select! {
                _ = s.forget_google(&se.account, &mut c) => {}
                _ = lost.cancelled() => {}
            }
        }
    }
    let delete = async {
        let mut client = s.db.client().await?;
        let tx = client.transaction().await?;
        tx.exec("UPDATE accounts SET seq=seq WHERE id=$1", &[&se.account]).await?;
        for query in ACCOUNT_DELETES {
            tx.exec(query, &[&se.account]).await?;
        }
        tx.commit().await?;
        Ok::<(), db::Error>(())
    };
    if delete.await.is_err() {
        return fail(500, "could not delete account");
    }
    s.count("deletions", 1);
    let mut response = empty(204);
    response.headers_mut().insert(SET_COOKIE, s.session_cookie("", -1));
    Ok(response)
}

/// `DELETE /api/me/devices/{id}`: signs a device out (204), clearing the
/// cookie when it is this one.
pub(crate) async fn handle_delete_device(
    State(s): State<Arc<Server>>,
    Auth(se): Auth,
    Path(id): Path<String>,
) -> ApiResult {
    if s.db.exec("DELETE FROM sessions WHERE id=$1 AND account=$2", &[&id, &se.account]).await.is_err() {
        return fail(500, "could not remove device");
    }
    let mut response = empty(204);
    if id == se.id {
        response.headers_mut().insert(SET_COOKIE, s.session_cookie("", -1));
    }
    Ok(response)
}

#[derive(Default, Deserialize)]
#[serde(default)]
struct PushRequest {
    #[serde(alias = "Subscription", deserialize_with = "null_default")]
    subscription: String,
    #[serde(alias = "APNs", alias = "Apns", alias = "APNS", deserialize_with = "null_default")]
    apns: String,
}

/// `PUT /api/me/push`: stores this device's Web Push subscription or APNs
/// token (`subscription`, `apns`; 8 KiB); empty clears it (204).
pub(crate) async fn handle_me_push(State(s): State<Arc<Server>>, Auth(se): Auth, body: Body) -> ApiResult {
    let Some(input) = read_json::<PushRequest>(body, 8 << 10).await else {
        return fail(400, "bad json");
    };
    let mut target = input.subscription.clone();
    if !input.apns.is_empty() {
        if !APNS_TOKEN.is_match(&input.apns) {
            return fail(400, "bad device token");
        }
        if s.apns().is_none() {
            return fail(503, "iPhone notifications are not configured on this server");
        }
        target = serde_json::json!({ "apns": input.apns.to_lowercase() }).to_string();
    } else if !input.subscription.is_empty() && push_endpoint(&input.subscription).is_empty() {
        return fail(400, "bad push subscription");
    }
    if s.db.exec("UPDATE sessions SET push=$1 WHERE id=$2", &[&target, &se.id]).await.is_err() {
        return fail(500, "could not save push subscription");
    }
    Ok(empty(204))
}

/// `GET /api/me/watches`: the account's seat alerts with their class labels.
pub(crate) async fn handle_me_watches(State(s): State<Arc<Server>>, Auth(se): Auth) -> ApiResult {
    match s.account_watches(&se.account).await {
        Ok(out) => ok(&out),
        Err(_) => fail(500, "could not read alerts"),
    }
}
