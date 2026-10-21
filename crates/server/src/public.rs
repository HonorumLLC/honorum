//! The public configuration (`/api/config`): schools, terms with their
//! published snapshot hashes, closures, and server capabilities. It changes
//! with collection or deployment, never per user, so one serialization is
//! cached until invalidated.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use axum::body::Body;
use axum::extract::State;
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::Response;
use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::Server;
use crate::api::{ApiError, ApiResult};
use crate::assets::matches_etag;
use crate::closures::Closure;
use crate::config::Lms;
use crate::db::{self, Sql};

/// The cached serialization and the revision counter that invalidates it.
/// The mutex is held while a new value is computed, so concurrent requests
/// compute it once.
#[derive(Default)]
pub(crate) struct PublicConfigCache {
    pub(crate) revision: AtomicU64,
    pub(crate) value: tokio::sync::Mutex<Option<Arc<PublicConfig>>>,
}

/// One serialization and what it depended on.
#[derive(Clone, Debug, Default)]
pub(crate) struct PublicConfig {
    pub(crate) revision: u64,
    pub(crate) version: String,
    pub(crate) vapid: String,
    pub(crate) phone: bool,
    pub(crate) google: bool,
    pub(crate) body: bytes::Bytes,
    /// A quoted SHA-256 of `body`.
    pub(crate) etag: String,
}

/// One term in the public configuration.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub(crate) struct PublicTerm {
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) start: String,
    pub(crate) end: String,
    pub(crate) updated: i64,
    pub(crate) sections: i64,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub(crate) structure_snapshot: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub(crate) seats_snapshot: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub(crate) descriptions_snapshot: String,
}

/// One school in the public configuration.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub(crate) struct PublicInstitution {
    #[serde(skip_serializing_if = "String::is_empty")]
    pub(crate) timezone: String,
    pub(crate) id: String,
    pub(crate) name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) lms: Option<Lms>,
    pub(crate) terms: Vec<PublicTerm>,
    pub(crate) closures: Vec<Closure>,
}

impl Server {
    /// Marks the cached configuration stale after a catalog or manifest change.
    pub(crate) fn invalidate_public_config(&self) {
        self.public_config.revision.fetch_add(1, Ordering::Relaxed);
    }

    /// The current serialization, recomputed when the revision, version,
    /// VAPID key, phone sign-in, or Google availability changed. Terms that
    /// were collected but hold no sections are omitted.
    pub(crate) async fn config_publication(&self) -> db::Result<Arc<PublicConfig>> {
        let mut cached = self.public_config.value.lock().await;
        let revision = self.public_config.revision.load(Ordering::Relaxed);
        let phone = self.verifier().is_some();
        let google = self.google_available();
        if let Some(v) = cached.as_ref()
            && v.revision == revision
            && v.version == self.version
            && v.vapid == self.vapid.public
            && v.phone == phone
            && v.google == google
        {
            return Ok(v.clone());
        }
        // One query observes all manifest pointers and term metadata together.
        let rows = self
            .db
            .fetch_all(
                r#"SELECT t.inst,t.id,t.name,t.start,t."end",t.updated_at,
 (SELECT COUNT(*) FROM sections s WHERE s.inst=t.inst AND s.term=t.id),
 COALESCE((SELECT v FROM kv WHERE k='snap:structure:' || t.inst || ':' || t.id),''),
 COALESCE((SELECT v FROM kv WHERE k='snap:seats:' || t.inst || ':' || t.id),''),
 COALESCE((SELECT v FROM kv WHERE k='snap:desc:' || t.inst || ':' || t.id),'')
 FROM terms t ORDER BY t.inst,t.id DESC"#,
                &[],
            )
            .await?;
        let mut terms: std::collections::HashMap<String, Vec<PublicTerm>> = std::collections::HashMap::new();
        for row in rows {
            let inst: String = row.try_get(0)?;
            let t = PublicTerm {
                id: row.try_get(1)?,
                name: row.try_get::<_, Option<String>>(2)?.unwrap_or_default(),
                start: row.try_get::<_, Option<String>>(3)?.unwrap_or_default(),
                end: row.try_get::<_, Option<String>>(4)?.unwrap_or_default(),
                updated: row.try_get::<_, Option<i64>>(5)?.unwrap_or_default(),
                sections: row.try_get(6)?,
                structure_snapshot: row.try_get(7)?,
                seats_snapshot: row.try_get(8)?,
                descriptions_snapshot: row.try_get(9)?,
            };
            if t.updated > 0 && t.sections == 0 {
                continue;
            }
            terms.entry(inst).or_default().push(t);
        }
        let institutions: Vec<PublicInstitution> = self
            .insts
            .iter()
            .map(|inst| PublicInstitution {
                timezone: inst.timezone.clone(),
                id: inst.id.clone(),
                name: inst.name.clone(),
                lms: inst.lms.clone(),
                terms: terms.remove(&inst.id).unwrap_or_default(),
                closures: inst.closures.clone(),
            })
            .collect();
        // Top-level keys are in sorted order; nested structs keep their
        // field order.
        #[derive(Serialize)]
        struct ConfigBody<'a> {
            apns: bool,
            google_calendar: bool,
            institutions: &'a [PublicInstitution],
            phone_auth: bool,
            vapid: &'a str,
            version: &'a str,
        }
        let body = serde_json::to_vec(&ConfigBody {
            apns: self.apns().is_some(),
            google_calendar: google,
            institutions: &institutions,
            phone_auth: phone,
            vapid: &self.vapid.public,
            version: &self.version,
        })
        .expect("the public configuration serializes");
        let etag = format!("\"{}\"", hex::encode(Sha256::digest(&body)));
        let value = Arc::new(PublicConfig {
            revision,
            version: self.version.clone(),
            vapid: self.vapid.public.clone(),
            phone,
            google,
            body: body.into(),
            etag,
        });
        *cached = Some(value.clone());
        Ok(value)
    }
}

/// `GET /api/config`: the public configuration, cacheable for 30 seconds
/// (`public, max-age=30, s-maxage=60, must-revalidate`) with an ETag (304
/// when it matches).
pub(crate) async fn handle_config(State(s): State<Arc<Server>>, headers: HeaderMap) -> ApiResult {
    let value = s.config_publication().await.map_err(|_| ApiError::new(500, "could not read term configuration"))?;
    let matched =
        headers.get(header::IF_NONE_MATCH).and_then(|v| v.to_str().ok()).is_some_and(|v| matches_etag(v, &value.etag));
    let mut response = if matched {
        let mut r = Response::new(Body::empty());
        *r.status_mut() = StatusCode::NOT_MODIFIED;
        r
    } else {
        Response::new(Body::from(value.body.clone()))
    };
    let h = response.headers_mut();
    h.insert(header::CONTENT_TYPE, HeaderValue::from_static("application/json"));
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static("public, max-age=30, s-maxage=60, must-revalidate"));
    h.insert(header::ETAG, HeaderValue::from_str(&value.etag).expect("a hex ETag is a valid header"));
    Ok(response)
}

#[cfg(test)]
mod tests {
    use crate::db::Sql;
    use crate::phone::PhoneVerifier;
    use crate::test_support::test_server;

    struct AcceptAll;

    #[async_trait::async_trait]
    impl PhoneVerifier for AcceptAll {
        async fn send(&self, _: &str) -> anyhow::Result<()> {
            Ok(())
        }
        async fn check(&self, _: &str, _: &str) -> anyhow::Result<()> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn public_configuration_publication() {
        let t = test_server().await;
        let s = &t.server;
        s.sync_catalog(&s.insts[0], None, true).await.unwrap();
        let c = t.client();
        let first = c.get("/api/config").await;
        assert!(
            first.status == 200 && first.header("cache-control").contains("public") && !first.header("etag").is_empty(),
            "public config: {} {:?}",
            first.status,
            first.headers
        );
        let body = first.text();
        assert!(
            body.contains(r#""structure_snapshot""#) && body.contains(r#""seats_snapshot""#),
            "missing independent publications: {body}"
        );
        let cached = s.public_config.value.lock().await.clone().unwrap();
        let etag = first.header("etag").to_owned();
        let revalidate = c.with_header("if-none-match", &etag);
        let same = revalidate.get("/api/config").await;
        let still = s.public_config.value.lock().await.clone().unwrap();
        assert!(
            same.status == 304 && same.body.is_empty() && std::sync::Arc::ptr_eq(&cached, &still),
            "revalidation recomputed or returned a body: {}",
            same.status
        );
        s.db.exec("UPDATE terms SET name='Changed term'", &[]).await.unwrap();
        s.invalidate_public_config();
        let changed = revalidate.get("/api/config").await;
        assert!(
            changed.status == 200 && changed.header("etag") != etag && changed.text().contains("Changed term"),
            "publication did not change: {} {}",
            changed.status,
            changed.text()
        );
        // Deployment flags also invalidate serialized data even without a
        // catalog edit. The server's version is fixed once shared, so this
        // changes phone sign-in, another such flag.
        s.set_verifier(Some(std::sync::Arc::new(AcceptAll)));
        let flagged = c.get("/api/config").await;
        assert!(flagged.text().contains(r#""phone_auth":true"#), "stale deployment flags");
    }
}
