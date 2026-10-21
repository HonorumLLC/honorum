//! Live seat counts for saved schedules: `GET /api/seats` reads the stored
//! counts and exam sittings for named sections, so a schedule rendered from
//! its own copies stays current without the catalog download. Snapshot
//! files are served by [`crate::assets`].

use std::sync::Arc;

use axum::extract::State;
use axum::http::Uri;
use serde::Serialize;
use serde_json::value::RawValue;

use crate::Server;
use crate::api::{ApiResult, compact_json, fail, ok};
use crate::db::{self, Sql};

/// Sections one request may name.
const SEATS_MAX: usize = 100;

#[derive(Serialize)]
struct Seat {
    id: String,
    available: i64,
    capacity: i64,
    waitlist: i64,
    fetched_at: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    exams: Option<Box<RawValue>>,
}

/// `GET /api/seats?inst=&term=&ids=a,b,…`: current seat counts for up to
/// 100 sections, each `{"id", "available", "capacity", "waitlist",
/// "fetched_at"}` plus `"exams"` when the section is in the catalog. A
/// schedule rendered from its own compact copies shows live availability,
/// and current exam sittings, without downloading the catalog. Repeated
/// query parameters use the first value.
pub(crate) async fn handle_seats(State(s): State<Arc<Server>>, uri: Uri) -> ApiResult {
    let query = uri.query().unwrap_or_default();
    let get = |name: &str| {
        url::form_urlencoded::parse(query.as_bytes())
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.into_owned())
            .unwrap_or_default()
    };
    let (inst, term, ids) = (get("inst"), get("term"), get("ids"));
    let ids: Vec<&str> = ids.split(',').take(SEATS_MAX).filter(|id| !id.is_empty()).collect();
    if ids.is_empty() {
        return ok(&Vec::<Seat>::new());
    }
    let query = "SELECT a.id,a.available,a.capacity,a.waitlist,CASE WHEN COALESCE(t.updated_at,0)>a.checked_at THEN t.updated_at ELSE a.checked_at END,CASE WHEN c.id IS NULL THEN '' ELSE COALESCE(json_text(c.data,'$.exams'),'[]') END FROM section_seats a JOIN terms t ON t.inst=a.inst AND t.id=a.term LEFT JOIN sections c ON c.inst=a.inst AND c.term=a.term AND c.id=a.id WHERE a.inst=$1 AND a.term=$2 AND a.id=ANY($3)";
    let read = async {
        let rows = s.db.fetch_all(query, &[&inst, &term, &ids]).await?;
        rows.iter()
            .map(|row| {
                let exams: String = row.try_get(5)?;
                Ok(Seat {
                    id: row.try_get(0)?,
                    available: row.try_get(1)?,
                    capacity: row.try_get(2)?,
                    waitlist: row.try_get(3)?,
                    fetched_at: row.try_get(4)?,
                    exams: if exams.is_empty() {
                        None
                    } else {
                        Some(
                            RawValue::from_string(compact_json(&exams))
                                .map_err(|_| db::Error::Config("stored exams are not JSON".into()))?,
                        )
                    },
                })
            })
            .collect::<db::Result<Vec<_>>>()
    };
    match read.await {
        Ok(out) => ok(&out),
        Err(_) => fail(500, "could not read seat counts"),
    }
}
