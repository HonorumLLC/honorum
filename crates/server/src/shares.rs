//! Shared schedules: immutable opaque blobs keyed by a client-chosen
//! 32-hex-digit id. The server never reads a blob; a stored id is never
//! overwritten, so a link that has been passed around always shows what its
//! author shared.

use std::sync::{Arc, LazyLock};

use axum::body::Body;
use axum::extract::{Path, State};
use axum::http::{HeaderValue, header};
use axum::response::Response;
use http_body_util::BodyExt;
use regex::Regex;

use crate::Server;
use crate::api::{ApiResult, empty, fail};
use crate::db::Sql;

/// Share and guest reminder ids: 32 lowercase hex digits.
pub(crate) static SYNC_ID: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[0-9a-f]{32}$").expect("valid pattern"));

/// The largest share (256 KiB).
pub(crate) const SHARE_MAX: usize = 256 << 10;

/// `GET /api/sync/{id}`: the blob as `application/octet-stream` with
/// `X-Updated-At` (RFC 3339 UTC); 404 "not found".
pub(crate) async fn handle_sync_get(State(s): State<Arc<Server>>, Path(id): Path<String>) -> ApiResult {
    let row = s.db.fetch_one("SELECT blob, updated_at FROM sync WHERE id=$1", &[&id]).await;
    let read = row.and_then(|row| Ok((row.try_get::<_, Option<Vec<u8>>>(0)?, row.try_get::<_, i64>(1)?)));
    let Ok((blob, updated)) = read else { return fail(404, "not found") };
    let at = chrono::DateTime::from_timestamp(updated, 0).unwrap_or_default();
    let mut response = Response::new(Body::from(blob.unwrap_or_default()));
    let h = response.headers_mut();
    h.insert(
        "x-updated-at",
        HeaderValue::from_str(&at.format("%Y-%m-%dT%H:%M:%SZ").to_string()).expect("a timestamp is header-safe"),
    );
    h.insert(header::CONTENT_TYPE, HeaderValue::from_static("application/octet-stream"));
    Ok(response)
}

/// `PUT /api/sync/{id}`: stores a new share (204). A share never changes,
/// so a link passed around cannot be overwritten by anyone else who holds
/// it: an existing id is 409 "this shared schedule already exists"; over
/// [`SHARE_MAX`] is 413 "blob too large".
pub(crate) async fn handle_sync_put(State(s): State<Arc<Server>>, Path(id): Path<String>, body: Body) -> ApiResult {
    if !SYNC_ID.is_match(&id) {
        return fail(400, "bad id");
    }
    let Ok(collected) = http_body_util::Limited::new(body, SHARE_MAX).collect().await else {
        return fail(413, "blob too large");
    };
    let blob = collected.to_bytes().to_vec();
    // Shares are stamped with the system clock, not the server's clock.
    let now = chrono::Utc::now().timestamp();
    let inserted = s
        .db
        .exec("INSERT INTO sync(id,blob,updated_at) VALUES($1,$2,$3) ON CONFLICT(id) DO NOTHING", &[&id, &blob, &now])
        .await;
    match inserted {
        Err(_) => fail(500, "could not save shared schedule"),
        Ok(0) => fail(409, "this shared schedule already exists"),
        Ok(_) => {
            s.count("shares", 1);
            Ok(empty(204))
        }
    }
}
