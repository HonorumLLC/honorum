//! The public status page and its JSON, cached for 30 seconds, plus
//! process-local performance figures (request timing itself is in
//! [`crate::metrics`]). Counts that cannot be read show as zero rather than
//! failing the page.

use std::fmt::Write;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::extract::State;
use axum::http::{HeaderValue, header};
use axum::response::{IntoResponse, Response};
use serde::Serialize;

use crate::Server;
use crate::api::{ApiResult, ok};
use crate::db::Sql;

/// How long a computed status is reused.
const STATUS_TTL: Duration = Duration::from_secs(30);

/// Cumulative process-local measurements; no request bodies or user IDs.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub(crate) struct PerformanceStatus {
    pub(crate) api_requests: i64,
    pub(crate) api_mean_ms: f64,
    pub(crate) api_duration_buckets: [i64; 6],
    pub(crate) snapshot_skips: i64,
    pub(crate) snapshot_publications: i64,
    pub(crate) collection_queue_ms: f64,
}

/// One school's collection health.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub(crate) struct InstStatus {
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) message: String,
    pub(crate) ok: bool,
    pub(crate) last_ok: i64,
    pub(crate) paused_until: i64,
    pub(crate) failures: i64,
    pub(crate) sections: i64,
    pub(crate) watches: i64,
}

/// The public status.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub(crate) struct Status {
    pub(crate) performance: PerformanceStatus,
    pub(crate) pending_deliveries: i64,
    pub(crate) failed_deliveries: i64,
    /// `null` when no school is configured.
    pub(crate) institutions: Option<Vec<InstStatus>>,
    pub(crate) median_alert_ms: i64,
    pub(crate) alerts_delivered: i64,
    pub(crate) alerts_failed: i64,
    pub(crate) accounts: i64,
    pub(crate) phone_auth: bool,
    pub(crate) requests: i64,
    pub(crate) errors: i64,
    pub(crate) version: String,
    pub(crate) generated_at: i64,
}

/// The last computed [`Status`] and when it was computed; `None` forces a
/// recount.
#[derive(Default)]
pub(crate) struct StatusCache {
    pub(crate) value: Mutex<Option<(Instant, Status)>>,
}

/// "never" for 0, else the rounded duration since `t` (Unix seconds) and
/// " ago", formatted by [`format_seconds`].
pub(crate) fn ago(t: i64) -> String {
    if t == 0 {
        return "never".into();
    }
    let elapsed = chrono::Utc::now().timestamp_millis() - t.saturating_mul(1000);
    format!("{} ago", format_seconds(round_seconds(elapsed)))
}

/// The rounded duration until `t` (Unix seconds), formatted by
/// [`format_seconds`].
pub(crate) fn until(t: i64) -> String {
    let remaining = t.saturating_mul(1000) - chrono::Utc::now().timestamp_millis();
    format_seconds(round_seconds(remaining))
}

/// Milliseconds rounded to whole seconds, halves away from zero.
fn round_seconds(ms: i64) -> i64 {
    if ms >= 0 { (ms + 500) / 1000 } else { -((-ms + 500) / 1000) }
}

/// Whole seconds as hours, minutes, and seconds: `0s`, `45s`,
/// `2m0s`, `1h2m3s`, with a leading `-` when negative.
pub(crate) fn format_seconds(secs: i64) -> String {
    if secs == 0 {
        return "0s".into();
    }
    let sign = if secs < 0 { "-" } else { "" };
    let a = secs.unsigned_abs();
    let (h, m, s) = (a / 3600, a % 3600 / 60, a % 60);
    if h > 0 {
        format!("{sign}{h}h{m}m{s}s")
    } else if m > 0 {
        format!("{sign}{m}m{s}s")
    } else {
        format!("{sign}{s}s")
    }
}

/// Escapes text for HTML text and quoted attributes: `"&'+<>` become
/// numeric or named references, and NUL becomes U+FFFD.
pub(crate) fn template_escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '\0' => out.push('\u{FFFD}'),
            '"' => out.push_str("&#34;"),
            '&' => out.push_str("&amp;"),
            '\'' => out.push_str("&#39;"),
            '+' => out.push_str("&#43;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            c => out.push(c),
        }
    }
    out
}

/// Escapes `<`, `>`, `&`, `'`, and `"` for HTML text and attributes.
pub(crate) fn html_escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '&' => out.push_str("&amp;"),
            '\'' => out.push_str("&#39;"),
            '"' => out.push_str("&#34;"),
            c => out.push(c),
        }
    }
    out
}

/// An HTML response with `Content-Type: text/html; charset=utf-8`.
pub(crate) fn html_response(body: String) -> Response {
    let mut response = body.into_response();
    response.headers_mut().insert(header::CONTENT_TYPE, HeaderValue::from_static("text/html; charset=utf-8"));
    response
}

impl Server {
    /// The process's request timing and publication counters.
    pub(crate) fn performance_status(&self) -> PerformanceStatus {
        let timing = &self.api_timing;
        let mut out = PerformanceStatus {
            api_requests: timing.count.load(Ordering::Relaxed),
            snapshot_skips: self.counters.snapshot_skips.load(Ordering::Relaxed),
            snapshot_publications: self.counters.snapshot_publications.load(Ordering::Relaxed),
            collection_queue_ms: self.counters.collection_queue_nanos.load(Ordering::Relaxed) as f64 / 1e6,
            ..PerformanceStatus::default()
        };
        if out.api_requests > 0 {
            out.api_mean_ms = timing.nanos.load(Ordering::Relaxed) as f64 / out.api_requests as f64 / 1e6;
        }
        for (slot, bucket) in out.api_duration_buckets.iter_mut().zip(&timing.buckets) {
            *slot = bucket.load(Ordering::Relaxed);
        }
        out
    }

    /// The status, recounted at most every 30 seconds; live counters are
    /// always current.
    pub(crate) async fn status(&self) -> Status {
        let cached = self.status_cache.value.lock().expect("status lock").clone();
        if let Some((at, mut st)) = cached
            && at.elapsed() < STATUS_TTL
        {
            st.requests = self.counters.requests.load(Ordering::Relaxed);
            st.errors = self.counters.errors.load(Ordering::Relaxed);
            st.performance = self.performance_status();
            return st;
        }
        let st = self.compute_status().await;
        *self.status_cache.value.lock().expect("status lock") = Some((Instant::now(), st.clone()));
        st
    }

    /// The first column of a count query's row; 0 when the query fails.
    async fn count_of(&self, query: &str, params: &[crate::db::Param<'_>]) -> i64 {
        match self.db.fetch_one(query, params).await {
            Ok(row) => row.try_get::<_, Option<i64>>(0).ok().flatten().unwrap_or_default(),
            Err(_) => 0,
        }
    }

    async fn compute_status(&self) -> Status {
        let mut st = Status {
            generated_at: chrono::Utc::now().timestamp(),
            requests: self.counters.requests.load(Ordering::Relaxed),
            errors: self.counters.errors.load(Ordering::Relaxed),
            version: self.version.clone(),
            performance: self.performance_status(),
            ..Status::default()
        };
        st.pending_deliveries = self.count_of("SELECT COUNT(*) FROM deliveries WHERE delivered_at=0", &[]).await;
        st.failed_deliveries = self
            .count_of(
                "SELECT COUNT(*) FROM deliveries WHERE delivered_at=-1 AND error NOT IN ('cancelled','device removed')",
                &[],
            )
            .await;
        for inst in &self.insts {
            let mut i = InstStatus { id: inst.id.clone(), name: inst.name.clone(), ..InstStatus::default() };
            if let Ok(Some(row)) = self
                .db
                .fetch_opt(
                    "SELECT ok,message,last_ok,failures,paused_until FROM adapter_status WHERE inst=$1",
                    &[&inst.id],
                )
                .await
            {
                // Columns are read in order; the first unreadable one leaves it
                // and the rest at their defaults.
                let mut read = || -> Result<(), tokio_postgres::Error> {
                    i.ok = row.try_get::<_, Option<i64>>(0)?.unwrap_or_default() == 1;
                    i.message = row.try_get::<_, Option<String>>(1)?.unwrap_or_default();
                    i.last_ok = row.try_get::<_, Option<i64>>(2)?.unwrap_or_default();
                    i.failures = row.try_get::<_, Option<i64>>(3)?.unwrap_or_default();
                    i.paused_until = row.try_get::<_, Option<i64>>(4)?.unwrap_or_default();
                    Ok(())
                };
                let _ = read();
            }
            i.sections = self.count_of("SELECT COUNT(*) FROM sections WHERE inst=$1", &[&inst.id]).await;
            i.watches = self.count_of("SELECT COUNT(*) FROM watches WHERE inst=$1", &[&inst.id]).await;
            st.institutions.get_or_insert_with(Vec::new).push(i);
        }
        if let Ok(rows) = self
            .db
            .fetch_all(
                "SELECT detected_at, delivered_at FROM alerts WHERE delivered_at>0 ORDER BY detected_at DESC LIMIT 200",
                &[],
            )
            .await
        {
            let mut latencies: Vec<i64> = rows
                .iter()
                .map(|row| {
                    let detected = row.try_get::<_, Option<i64>>(0).ok().flatten().unwrap_or_default();
                    let delivered = row.try_get::<_, Option<i64>>(1).ok().flatten().unwrap_or_default();
                    (delivered - detected) * 1000
                })
                .collect();
            if !latencies.is_empty() {
                latencies.sort_unstable();
                st.median_alert_ms = latencies[latencies.len() / 2];
            }
        }
        st.alerts_delivered = self.count_of("SELECT COUNT(*) FROM alerts WHERE delivered_at>0", &[]).await;
        st.alerts_failed = self.count_of("SELECT COUNT(*) FROM alerts WHERE delivered_at=0 AND error<>''", &[]).await;
        st.accounts = self.count_of("SELECT COUNT(*) FROM accounts", &[]).await;
        st.phone_auth = self.verifier().is_some();
        st
    }

    /// Forces the next [`Server::status`] to recount.
    pub(crate) fn invalidate_status(&self) {
        *self.status_cache.value.lock().expect("status lock") = None;
    }
}

/// `GET /api/status`: the status as JSON.
pub(crate) async fn handle_status_json(State(s): State<Arc<Server>>) -> ApiResult {
    ok(&s.status().await)
}

/// `GET /status`: the status page.
pub(crate) async fn handle_status_html(State(s): State<Arc<Server>>) -> ApiResult {
    Ok(html_response(status_page(&s.status().await)))
}

/// Renders the status page.
fn status_page(st: &Status) -> String {
    let e = template_escape;
    let mut out = String::from(concat!(
        r#"<!doctype html><meta charset=utf-8><meta name=viewport content="width=device-width"><title>Honorum status</title>"#,
        "\n",
        r#"<style>body{font:15px/1.5 system-ui,sans-serif;max-width:52rem;margin:2rem auto;padding:0 1rem;color:#222}table{border-collapse:collapse;width:100%}td,th{text-align:left;padding:.4rem .6rem;border-bottom:1px solid #ddd}.ok{color:#1a7f37}.bad{color:#b42318}.busy{color:#b45309}small{color:#666}:root{color-scheme:light dark}@media (prefers-color-scheme:dark){body{background:#111418;color:#e6e8eb}td,th{border-color:#2a2f37}small{color:#8a8f98}.ok{color:#4ade80}.bad{color:#f87171}.busy{color:#fbbf24}}</style>"#,
        "\n<h1>Honorum status</h1>\n",
    ));
    let _ = writeln!(
        out,
        "<p>Median alert latency (detection to delivery): <b>{} ms</b> · delivered {} · failed {}</p>",
        st.median_alert_ms, st.alerts_delivered, st.alerts_failed
    );
    let _ = writeln!(
        out,
        "<p>Delivery queue: {} pending · {} stopped or expired</p>",
        st.pending_deliveries, st.failed_deliveries
    );
    out.push_str("<table><tr><th>University</th><th>Adapter</th><th>Last successful fetch</th><th>Sections</th><th>Watches</th></tr>\n");
    for i in st.institutions.iter().flatten() {
        let _ = write!(out, "<tr><td>{}</td>\n<td>", e(&i.name));
        if i.ok {
            out.push_str("<span class=ok>healthy</span>");
            if !i.message.is_empty() {
                let _ = write!(out, "<br><small>{}</small>", e(&i.message));
            }
        } else if i.message.starts_with("fetching") {
            let _ = write!(out, "<span class=busy>syncing</span><br><small>{}</small>", e(&i.message));
        } else {
            out.push_str("<span class=bad>");
            if i.paused_until > 0 {
                let _ = write!(out, "paused, retrying in {}", e(&until(i.paused_until)));
            } else {
                out.push_str("down");
            }
            let _ = write!(out, "</span><br><small>{}</small>", e(&i.message));
        }
        let _ = write!(out, "</td>\n<td>{}</td><td>{}</td><td>{}</td></tr>", e(&ago(i.last_ok)), i.sections, i.watches);
    }
    out.push_str("\n</table>\n");
    let _ = write!(
        out,
        r#"<p><small>Version {} · accounts: {} · phone sign-in: {} · requests served: {} · errors: {}. Provider error details are available in server logs. <a href="/">Back to Honorum</a></small></p>"#,
        e(&st.version),
        st.accounts,
        if st.phone_auth { "configured" } else { "off" },
        st.requests,
        st.errors
    );
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durations_and_escaping() {
        assert_eq!(format_seconds(0), "0s");
        assert_eq!(format_seconds(45), "45s");
        assert_eq!(format_seconds(120), "2m0s");
        assert_eq!(format_seconds(3723), "1h2m3s");
        assert_eq!(format_seconds(-65), "-1m5s");
        assert_eq!((round_seconds(1499), round_seconds(1500), round_seconds(-1500)), (1, 2, -2));
        assert_eq!(ago(0), "never");
        assert_eq!(template_escape(r#"<a href="x">&'+</a>"#), "&lt;a href=&#34;x&#34;&gt;&amp;&#39;&#43;&lt;/a&gt;");
    }
}
