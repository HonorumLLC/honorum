//! The operator dashboard at `/admin`: `daily_metrics` charts beside the
//! live status, and every metric by day as JSON. `HONORUM_ADMIN_TOKEN`
//! opens it, as the password of HTTP Basic authentication or as a bearer
//! token; without one the routes do not exist (404).

use std::collections::{BTreeSet, HashMap};
use std::fmt::Write;
use std::sync::Arc;

use axum::extract::{Query, Request, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use chrono::{DateTime, NaiveDate, TimeDelta, Utc};
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;

use crate::Server;
use crate::analytics::GAUGE_QUERIES;
use crate::api::{ApiError, ApiResult, fail, ok};
use crate::db::{self, Sql};
use crate::metrics::metric_day;
use crate::status::{InstStatus, ago, html_escape, html_response, template_escape};

/// Guards the admin routes: 404 without a configured token, 60 requests per
/// address per minute, then the token (constant-time compared) or 401 with
/// `WWW-Authenticate: Basic realm="Honorum", charset="UTF-8"`. Allowed
/// responses get `Cache-Control: no-store` and `X-Robots-Tag: noindex`.
pub(crate) async fn require_admin(State(s): State<Arc<Server>>, request: Request, next: Next) -> Response {
    if s.cfg.admin_token.is_empty() {
        let mut response = (StatusCode::NOT_FOUND, "404 page not found\n").into_response();
        let h = response.headers_mut();
        h.insert(header::CONTENT_TYPE, HeaderValue::from_static("text/plain; charset=utf-8"));
        h.insert("x-content-type-options", HeaderValue::from_static("nosniff"));
        return response;
    }
    match s.rate_allow(&format!("admin:{}", s.client_ip(&request)), 60, 60).await {
        Err(_) => return ApiError::new(503, "request limits unavailable; try again shortly").into_response(),
        Ok(false) => {
            return ApiError::new(429, "too many requests; try again shortly").retry_after(60).into_response();
        }
        Ok(true) => {}
    }
    let authorization =
        request.headers().get(header::AUTHORIZATION).and_then(|v| v.to_str().ok()).unwrap_or_default().to_owned();
    let given = basic_password(&authorization).or_else(|| authorization.strip_prefix("Bearer ").map(str::to_owned));
    let want = Sha256::digest(s.cfg.admin_token.as_bytes());
    let allowed = given.is_some_and(|given| bool::from(want.ct_eq(&Sha256::digest(given.as_bytes()))));
    if !allowed {
        let mut response = ApiError::new(401, "admin token required").into_response();
        response
            .headers_mut()
            .insert(header::WWW_AUTHENTICATE, HeaderValue::from_static(r#"Basic realm="Honorum", charset="UTF-8""#));
        return response;
    }
    let mut response = next.run(request).await;
    let h = response.headers_mut();
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    h.insert("x-robots-tag", HeaderValue::from_static("noindex"));
    response
}

/// The password of an HTTP Basic `Authorization` value: the scheme matches
/// case-insensitively and the credentials are standard base64 of
/// `user:password`.
fn basic_password(authorization: &str) -> Option<String> {
    const PREFIX: &str = "Basic ";
    let scheme = authorization.get(..PREFIX.len())?;
    if !scheme.eq_ignore_ascii_case(PREFIX) {
        return None;
    }
    let decoded = STANDARD.decode(&authorization[PREFIX.len()..]).ok()?;
    let decoded = String::from_utf8_lossy(&decoded);
    let (_, password) = decoded.split_once(':')?;
    Some(password.to_owned())
}

/// Every stored day of every metric; two years of a few dozen metrics is
/// small enough to read whole.
#[derive(Default)]
struct MetricRows {
    /// Values by metric name, then day.
    values: HashMap<String, HashMap<String, f64>>,
    /// The first day each metric was recorded.
    first: HashMap<String, String>,
}

impl Server {
    async fn load_metrics(&self) -> db::Result<MetricRows> {
        let mut m = MetricRows::default();
        for row in self.db.fetch_all("SELECT day,name,value FROM daily_metrics", &[]).await? {
            let day: String = row.try_get(0)?;
            let name: String = row.try_get(1)?;
            let value: i64 = row.try_get(2)?;
            let first = m.first.entry(name.clone()).or_default();
            if first.is_empty() || day < *first {
                first.clone_from(&day);
            }
            m.values.entry(name).or_default().insert(day, value as f64);
        }
        Ok(m)
    }
}

fn is_gauge(name: &str) -> bool {
    name.starts_with("school:") || GAUGE_QUERIES.iter().any(|(g, _)| *g == name)
}

impl MetricRows {
    /// One value per day, NaN before the metric was first recorded. A
    /// counter with no row counts nothing that day; a gauge keeps its last
    /// reading. Counters start no later than `requests`, which every running
    /// day records.
    fn daily(&self, name: &str, days: &[String]) -> Vec<f64> {
        let gauge = is_gauge(name);
        let mut start = self.first.get(name).map(String::as_str).unwrap_or_default();
        if let Some(r) = self.first.get("requests")
            && !gauge
            && !r.is_empty()
            && (start.is_empty() || r.as_str() < start)
        {
            start = r;
        }
        let values = self.values.get(name);
        let (mut last, mut last_day) = (f64::NAN, "");
        if gauge && let Some(values) = values {
            for (d, v) in values {
                if *d < days[0] && d.as_str() > last_day {
                    (last, last_day) = (*v, d);
                }
            }
        }
        days.iter()
            .map(|d| match values.and_then(|v| v.get(d)) {
                Some(v) => {
                    last = *v;
                    *v
                }
                None if start.is_empty() || d.as_str() < start => f64::NAN,
                None if gauge => last,
                None => 0.0,
            })
            .collect()
    }
}

/// The chosen range and the equal period before it, in UTC days. Ranges
/// past 90 days are charted by whole weeks.
struct Span {
    n: usize,
    size: usize,
    /// The previous period, then the current one.
    days: Vec<String>,
}

fn new_span(n: usize, now: DateTime<Utc>) -> Span {
    let (n, size) = if n > 90 { (n / 7 * 7, 7) } else { (n, 1) };
    let days = (0..2 * n).rev().map(|i| metric_day(now - TimeDelta::days(i as i64))).collect();
    Span { n, size, days }
}

impl Span {
    /// `[from, to)` indexes into `days` covering the current period.
    fn buckets(&self) -> Vec<(usize, usize)> {
        let mut out = Vec::new();
        let mut end = self.days.len();
        while end > self.n {
            out.push((self.n.max(end.saturating_sub(self.size)), end));
            end = end.saturating_sub(self.size);
        }
        out.reverse();
        out
    }

    /// Folds daily values into the buckets: the last reading of a gauge, the
    /// sum of a counter.
    fn group(&self, v: &[f64], gauge: bool) -> Vec<f64> {
        self.buckets().into_iter().map(|(a, b)| if gauge { last_of(v, a, b) } else { sum_of(v, a, b) }).collect()
    }

    fn labels(&self) -> Vec<String> {
        let label = |day: &str| {
            NaiveDate::parse_from_str(day, "%Y-%m-%d").map(|d| d.format("%b %-d").to_string()).unwrap_or_default()
        };
        self.buckets()
            .into_iter()
            .map(|(a, b)| {
                if b - a == 1 {
                    label(&self.days[b - 1])
                } else {
                    format!("{} – {}", label(&self.days[a]), label(&self.days[b - 1]))
                }
            })
            .collect()
    }
}

/// The sum of the non-NaN values in `v[from..to]`; NaN when there are none.
fn sum_of(v: &[f64], from: usize, to: usize) -> f64 {
    let mut values = v[from..to].iter().filter(|x| !x.is_nan()).peekable();
    if values.peek().is_none() {
        return f64::NAN;
    }
    values.sum()
}

/// The last non-NaN value in `v[from..to]`; NaN when there is none.
fn last_of(v: &[f64], from: usize, to: usize) -> f64 {
    v[from..to].iter().rev().find(|x| !x.is_nan()).copied().unwrap_or(f64::NAN)
}

// Numbers

/// An integer with thousands separators and a minus sign (U+2212).
fn grouped(n: i64) -> String {
    let mut s = n.unsigned_abs().to_string();
    let mut i = s.len() as isize - 3;
    while i > 0 {
        s.insert(i as usize, ',');
        i -= 3;
    }
    if n < 0 { format!("−{s}") } else { s }
}

/// A value in at most about four characters: `1,284`, `12.9K`, `4.2M`, or
/// `—` for NaN.
fn compact(v: f64) -> String {
    let a = v.abs();
    if v.is_nan() {
        "—".into()
    } else if a >= 1e6 {
        format!("{}M", format_fixed(v / 1e6, 1).trim_end_matches(".0").to_owned())
    } else if a >= 1e4 {
        format!("{}K", format_fixed(v / 1e3, 1).trim_end_matches(".0").to_owned())
    } else {
        grouped(v.round() as i64)
    }
}

/// `v` with `precision` decimals; infinities are `+Inf` and `-Inf`, and NaN
/// is `NaN`.
fn format_fixed(v: f64, precision: usize) -> String {
    if v.is_infinite() {
        return if v > 0.0 { "+Inf".into() } else { "-Inf".into() };
    }
    if v.is_nan() {
        return "NaN".into();
    }
    format!("{v:.precision$}")
}

fn percent_text(v: f64) -> String {
    if v.is_nan() {
        return "—".into();
    }
    if v != 0.0 && v.abs() < 0.1 {
        return format!("{}%", format_fixed(v, 2));
    }
    format!("{}%", format_fixed(v, 1))
}

// Tiles

#[derive(Default)]
struct Tile {
    label: String,
    value: String,
    delta: String,
    good: bool,
    bad: bool,
}

/// Sets a tile's delta: the signed difference, or a percentage when `pct`.
/// `up_good` says which direction is good news.
fn change(t: &mut Tile, now: f64, before: f64, pct: bool, up_good: bool) {
    if now.is_nan() || before.is_nan() {
        return;
    }
    let mut d = now - before;
    if pct && before == 0.0 {
        return;
    } else if pct {
        d = d / before * 100.0;
        t.delta = format!("{}%", format_fixed(d.abs(), 0));
    } else {
        t.delta = compact(d.abs());
    }
    if d.round() == 0.0 {
        t.delta = "No change".into();
        return;
    }
    t.delta = format!("{} {}", if d > 0.0 { "▲" } else { "▼" }, t.delta);
    (t.good, t.bad) = ((d > 0.0) == up_good, (d > 0.0) != up_good);
}

// Charts

/// A series' figure, beside its legend key, is the range's total for a
/// counter and the latest reading for a gauge.
struct Series {
    label: String,
    values: Vec<f64>,
    figure: f64,
}

#[derive(Default)]
struct Chart {
    title: String,
    columns: bool,
    /// After each value in the tooltip and headline.
    unit: String,
    series: Vec<Series>,
    labels: Vec<String>,
}

const CHART_W: i64 = 480;
const CHART_H: i64 = 176;
const PAD_L: i64 = 40;
const PAD_R: i64 = 6;
const PAD_T: i64 = 8;
const PAD_B: i64 = 24;
const PLOT_W: i64 = CHART_W - PAD_L - PAD_R;
const PLOT_H: i64 = CHART_H - PAD_T - PAD_B;
const TIP_LINE: f64 = 15.0;
const TIP_PAD: f64 = 8.0;
const TIP_CHAR: f64 = 6.3;

/// A top and a step of 1, 2, 2.5 or 5 times a power of ten giving two to
/// five gridlines.
fn nice_scale(top: f64) -> (f64, f64) {
    // NaN, zero, and negative tops all take the default scale.
    if top.partial_cmp(&0.0) != Some(std::cmp::Ordering::Greater) {
        return (4.0, 1.0);
    }
    let raw = top / 4.0;
    let mag = 10f64.powf(raw.log10().floor());
    let mut step = 10.0 * mag;
    for m in [1.0, 2.0, 2.5, 5.0, 10.0] {
        if raw <= m * mag {
            step = m * mag;
            break;
        }
    }
    if step < 1.0 {
        step = 1.0;
    }
    ((top / step).ceil() * step, step)
}

impl Chart {
    fn value(&self, v: f64) -> String {
        if v.is_nan() {
            return "—".into();
        }
        format!("{}{}", compact(v), self.unit)
    }

    fn latest(&self, s: &Series) -> String {
        self.value(s.figure)
    }

    /// Draws the chart: zero-based, recessive gridlines, 2px lines with an
    /// end dot or columns with rounded tops, and a hover band per bucket
    /// whose tooltip lists every series. It needs no script.
    fn svg(&self) -> String {
        let n = self.labels.len();
        let mut top = 0.0f64;
        for s in &self.series {
            for v in &s.values {
                if !v.is_nan() {
                    top = top.max(*v);
                }
            }
        }
        let (top, step) = nice_scale(top);
        let band = PLOT_W as f64 / n as f64;
        let x = |i: usize| PAD_L as f64 + band * (i as f64 + 0.5);
        let y = |v: f64| (PAD_T + PLOT_H) as f64 - v / top * PLOT_H as f64;
        let mut b = String::new();
        let mut summary = self.title.clone();
        for s in &self.series {
            let _ = write!(summary, ", {} {}", s.label, self.latest(s));
        }
        let _ =
            write!(b, r#"<svg viewBox="0 0 {CHART_W} {CHART_H}" role="img" aria-label="{}">"#, html_escape(&summary));
        let mut v = 0.0;
        while v <= top + step / 2.0 {
            let base = if v == 0.0 { " base" } else { "" };
            let _ = write!(
                b,
                r#"<line class="gl{base}" x1="{PAD_L}" x2="{}" y1="{:.1}" y2="{:.1}"/>"#,
                CHART_W - PAD_R,
                y(v),
                y(v)
            );
            let _ = write!(
                b,
                r#"<text class="ax" text-anchor="end" x="{}" y="{:.1}">{}</text>"#,
                PAD_L - 6,
                y(v) + 3.5,
                compact(v)
            );
            v += step;
        }
        if n > 0 {
            for i in [0, n / 2, n - 1] {
                if n < 3 && i == n / 2 {
                    continue;
                }
                let anchor = if i == n - 1 {
                    "end"
                } else if i == 0 {
                    "start"
                } else {
                    "middle"
                };
                let label = self.labels[i].split(" – ").next().unwrap_or_default();
                let _ = write!(
                    b,
                    r#"<text class="ax" text-anchor="{anchor}" x="{:.1}" y="{}">{}</text>"#,
                    (PAD_L as f64).max(x(i).min((CHART_W - PAD_R) as f64)),
                    CHART_H - 6,
                    html_escape(label)
                );
            }
        }
        let floor = (PAD_T + PLOT_H) as f64;
        for (k, s) in self.series.iter().enumerate() {
            let slot = k + 1;
            if self.columns {
                let mut w = 24f64.min(band - 2.0);
                if band < 4.0 {
                    w = (band - 1.0).max(1.0);
                }
                for (i, &v) in s.values.iter().enumerate() {
                    if v.is_nan() || v <= 0.0 {
                        continue;
                    }
                    let h = PLOT_H as f64 * v / top;
                    let r = 4f64.min(w / 2.0).min(h);
                    let (x0, y0) = (x(i) - w / 2.0, y(v));
                    let _ = write!(
                        b,
                        r#"<path class="s{slot}" d="M{:.1} {:.1}V{:.1}Q{:.1} {:.1} {:.1} {:.1}H{:.1}Q{:.1} {:.1} {:.1} {:.1}V{:.1}Z"/>"#,
                        x0,
                        floor,
                        y0 + r,
                        x0,
                        y0,
                        x0 + r,
                        y0,
                        x0 + w - r,
                        x0 + w,
                        y0,
                        x0 + w,
                        y0 + r,
                        floor
                    );
                }
                continue;
            }
            let mut d = String::new();
            let mut pen = "M";
            for (i, &v) in s.values.iter().enumerate() {
                if v.is_nan() {
                    pen = "M";
                    continue;
                }
                let _ = write!(d, "{pen}{:.1} {:.1}", x(i), y(v));
                pen = "L";
            }
            if self.series.len() == 1 {
                // A wash under a lone line; it closes each unbroken run to the baseline.
                let mut area = String::new();
                let mut open = false;
                for i in 0..=s.values.len() {
                    if i < s.values.len() && !s.values[i].is_nan() {
                        if !open {
                            open = true;
                            let _ = write!(area, "M{:.1} {}", x(i), PAD_T + PLOT_H);
                        }
                        let _ = write!(area, "L{:.1} {:.1}", x(i), y(s.values[i]));
                    } else if open {
                        let _ = write!(area, "L{:.1} {}Z", x(i - 1), PAD_T + PLOT_H);
                        open = false;
                    }
                }
                let _ = write!(b, r#"<path class="wash s{slot}" d="{area}"/>"#);
            }
            let _ = write!(b, r#"<path class="line s{slot}" d="{d}"/>"#);
            if let Some(i) = s.values.iter().rposition(|v| !v.is_nan()) {
                let _ = write!(b, r#"<circle class="dot s{slot}" cx="{:.1}" cy="{:.1}" r="4"/>"#, x(i), y(s.values[i]));
            }
        }
        // Hover bands come last so each tooltip is drawn over the marks.
        for i in 0..n {
            let _ = write!(
                b,
                r#"<g class="hit"><rect class="band" x="{:.1}" y="{PAD_T}" width="{:.1}" height="{PLOT_H}"/>"#,
                PAD_L as f64 + band * i as f64,
                band
            );
            if !self.columns {
                for (k, s) in self.series.iter().enumerate() {
                    let v = s.values[i];
                    if !v.is_nan() {
                        let _ =
                            write!(b, r#"<circle class="dot s{}" cx="{:.1}" cy="{:.1}" r="4"/>"#, k + 1, x(i), y(v));
                    }
                }
            }
            let mut lines = vec![self.labels[i].clone()];
            let mut width = self.labels[i].chars().count() as f64;
            for s in &self.series {
                let line = if self.series.len() == 1 {
                    self.value(s.values[i])
                } else {
                    format!("{}  {}", s.label, self.value(s.values[i]))
                };
                width = width.max(line.chars().count() as f64 + 2.0);
                lines.push(line);
            }
            let (tw, th) = (width * TIP_CHAR + 2.0 * TIP_PAD, lines.len() as f64 * TIP_LINE + TIP_PAD);
            let mut tx = x(i) + band / 2.0 + 6.0;
            if tx + tw > CHART_W as f64 {
                tx = x(i) - band / 2.0 - 6.0 - tw;
            }
            let _ =
                write!(b, r#"<g class="tip"><rect x="{tx:.1}" y="{PAD_T}" width="{tw:.1}" height="{th:.1}" rx="6"/>"#);
            for (j, line) in lines.iter().enumerate() {
                let ty = PAD_T as f64 + TIP_PAD / 2.0 + TIP_LINE * (j + 1) as f64 - 3.0;
                if j == 0 || self.series.len() == 1 {
                    let class = if j == 0 { "head" } else { "val" };
                    let _ = write!(
                        b,
                        r#"<text class="{class}" x="{:.1}" y="{ty:.1}">{}</text>"#,
                        tx + TIP_PAD,
                        html_escape(line)
                    );
                    continue;
                }
                let s = &self.series[j - 1];
                let _ =
                    write!(b, r#"<circle class="s{j}" cx="{:.1}" cy="{:.1}" r="3.5"/>"#, tx + TIP_PAD + 3.5, ty - 3.5);
                let _ = write!(
                    b,
                    r#"<text class="key" x="{:.1}" y="{ty:.1}">{}</text><text class="val" text-anchor="end" x="{:.1}" y="{ty:.1}">{}</text>"#,
                    tx + TIP_PAD + 12.0,
                    html_escape(&s.label),
                    tx + tw - TIP_PAD,
                    self.value(s.values[i])
                );
            }
            b.push_str("</g></g>");
        }
        b.push_str("</svg>");
        b
    }
}

// Page

struct AdminSchool {
    status: InstStatus,
    accounts: String,
    change: String,
    state: &'static str,
    class: &'static str,
}

struct AdminPage {
    range: usize,
    ranges: [usize; 4],
    tiles: Vec<Tile>,
    sections: Vec<(String, Vec<Chart>)>,
    schools: Vec<AdminSchool>,
    queue: String,
    head: Vec<String>,
    rows: Vec<Vec<String>>,
    version: String,
    generated_at: String,
}

fn range_label(n: usize) -> String {
    if n == 365 { "1Y".into() } else { format!("{n}D") }
}

/// The `?days=` range: 7, 30, 90, or 365, else 30.
fn admin_range(query: &HashMap<String, String>) -> usize {
    match query.get("days").and_then(|d| d.parse::<i64>().ok()) {
        Some(n @ (7 | 30 | 90 | 365)) => n as usize,
        _ => 30,
    }
}

/// Every stored metric, in the order of the table, with its column label.
const METRIC_COLUMNS: &[(&str, &str)] = &[
    ("accounts", "Accounts"),
    ("active_day", "Active day"),
    ("active_week", "Active week"),
    ("active_month", "Active month"),
    ("signups", "New"),
    ("signins", "Sign-ins"),
    ("codes", "Codes"),
    ("deletions", "Deleted"),
    ("devices", "Devices"),
    ("push_devices", "Push devices"),
    ("schedules", "Schedules"),
    ("tasks", "Tasks"),
    ("edits", "Edits"),
    ("shares", "Shares"),
    ("watches", "Watches"),
    ("watches_created", "Watches added"),
    ("notifications", "Notified"),
    ("notification_failures", "Notify failed"),
    ("canvas", "Canvas"),
    ("feeds", "Feeds"),
    ("google", "Google"),
    ("canvas_imports", "Canvas imports"),
    ("canvas_failures", "Canvas failed"),
    ("feed_imports", "Feed imports"),
    ("feed_failures", "Feed failed"),
    ("requests", "Requests"),
    ("api_requests", "API"),
    ("api_ms", "API ms"),
    ("api_slow", "API > 1 s"),
    ("server_errors", "5xx"),
    ("client_errors", "4xx"),
];

impl Server {
    async fn admin_page(&self, n: usize) -> db::Result<AdminPage> {
        let m = self.load_metrics().await?;
        let st = self.status().await;
        let now = self.now();
        let sp = new_span(n, now);
        let n = sp.n;
        let labels = sp.labels();
        let daily = |name: &str| m.daily(name, &sp.days);
        let series1 = |label: &str, name: &str| {
            let gauge = is_gauge(name);
            let values = sp.group(&daily(name), gauge);
            let figure = if gauge { last_of(&values, 0, values.len()) } else { sum_of(&values, 0, values.len()) };
            Series { label: label.into(), values, figure }
        };
        let cur = |v: &[f64]| sum_of(v, n, 2 * n);
        let prev = |v: &[f64]| sum_of(v, 0, n);

        let mut p = AdminPage {
            range: n,
            ranges: [7, 30, 90, 365],
            tiles: Vec::new(),
            sections: Vec::new(),
            schools: Vec::new(),
            queue: String::new(),
            head: Vec::new(),
            rows: Vec::new(),
            version: self.version.clone(),
            generated_at: now.format("%b %-d, %H:%M UTC").to_string(),
        };

        for (label, name) in [
            ("Accounts", "accounts"),
            ("Active today", "active_day"),
            ("Active this week", "active_week"),
            ("Devices", "devices"),
        ] {
            let v = daily(name);
            let mut t = Tile { label: label.into(), value: compact(last_of(&v, n, 2 * n)), ..Tile::default() };
            change(&mut t, last_of(&v, n, 2 * n), last_of(&v, 0, n), false, true);
            p.tiles.push(t);
        }
        let signups = daily("signups");
        let mut t = Tile { label: "New accounts".into(), value: compact(cur(&signups)), ..Tile::default() };
        change(&mut t, cur(&signups), prev(&signups), true, true);
        p.tiles.push(t);
        let (requests, errors) = (daily("requests"), daily("server_errors"));
        let rate = |e: f64, r: f64| if r > 0.0 { e / r * 100.0 } else { f64::NAN };
        let mut t = Tile {
            label: "Error rate".into(),
            value: percent_text(rate(cur(&errors), cur(&requests))),
            ..Tile::default()
        };
        let (now_rate, before_rate) = (rate(cur(&errors), cur(&requests)), rate(prev(&errors), prev(&requests)));
        if !now_rate.is_nan() && !before_rate.is_nan() && (now_rate - before_rate).abs() >= 0.01 {
            let arrow = if now_rate > before_rate { "▲ " } else { "▼ " };
            t.delta = format!("{arrow}{}", percent_text((now_rate - before_rate).abs()));
            (t.good, t.bad) = (now_rate < before_rate, now_rate > before_rate);
        }
        p.tiles.push(t);

        // API time is the bucket's total over its count.
        let (ms, calls) = (sp.group(&daily("api_ms"), false), sp.group(&daily("api_requests"), false));
        let mean: Vec<f64> =
            ms.iter().zip(&calls).map(|(ms, calls)| if *calls > 0.0 { ms / calls } else { f64::NAN }).collect();
        let api_mean = sum_of(&ms, 0, ms.len()) / sum_of(&calls, 0, calls.len());

        let chart = |title: &str, columns: bool, series: Vec<Series>| Chart {
            title: title.into(),
            columns,
            series,
            labels: labels.clone(),
            ..Chart::default()
        };
        p.sections.push((
            "Usage".into(),
            vec![
                chart("Accounts", false, vec![series1("Accounts", "accounts")]),
                chart(
                    "Active accounts",
                    false,
                    vec![
                        series1("Day", "active_day"),
                        series1("Week", "active_week"),
                        series1("Month", "active_month"),
                    ],
                ),
                chart("Sign-ins", false, vec![series1("Sign-ins", "signins"), series1("New", "signups")]),
                chart("Edits synced", true, vec![series1("Edits", "edits")]),
                chart("Seat watches", false, vec![series1("Watches", "watches")]),
                chart("Shared schedules", true, vec![series1("Shares", "shares")]),
            ],
        ));
        p.sections.push((
            "Service".into(),
            vec![
                chart("Requests", true, vec![series1("Requests", "requests")]),
                Chart {
                    unit: " ms".into(),
                    ..chart("API time", false, vec![Series { label: "Mean".into(), values: mean, figure: api_mean }])
                },
                chart("Server errors", true, vec![series1("Errors", "server_errors")]),
                chart("Slow requests", true, vec![series1("Over 1 s", "api_slow")]),
            ],
        ));
        p.sections.push((
            "Integrations".into(),
            vec![
                chart(
                    "Connections",
                    false,
                    vec![series1("Canvas", "canvas"), series1("Feeds", "feeds"), series1("Google", "google")],
                ),
                chart("Imports", false, vec![series1("Canvas", "canvas_imports"), series1("Feeds", "feed_imports")]),
                chart(
                    "Import failures",
                    false,
                    vec![series1("Canvas", "canvas_failures"), series1("Feeds", "feed_failures")],
                ),
                chart(
                    "Notifications",
                    false,
                    vec![series1("Sent", "notifications"), series1("Failed", "notification_failures")],
                ),
            ],
        ));

        for inst in st.institutions.iter().flatten() {
            let v = daily(&format!("school:{}", inst.id));
            let mut delta = Tile::default();
            change(&mut delta, last_of(&v, n, 2 * n), last_of(&v, 0, n), false, true);
            let (state, class) = if inst.ok {
                ("Healthy", "good")
            } else if inst.message.starts_with("fetching") {
                ("Syncing", "warn")
            } else if inst.paused_until > 0 {
                ("Paused", "bad")
            } else {
                ("Down", "bad")
            };
            p.schools.push(AdminSchool {
                status: inst.clone(),
                accounts: compact(last_of(&v, n, 2 * n)),
                change: delta.delta,
                state,
                class,
            });
        }
        p.queue = format!("{} pending · {} failed", grouped(st.pending_deliveries), grouped(st.failed_deliveries));

        // Every metric by day, newest first, for reading exact values.
        p.head.push("Day".into());
        let mut cols = Vec::new();
        for (name, label) in METRIC_COLUMNS {
            p.head.push((*label).into());
            cols.push(daily(name));
        }
        for i in (n..2 * n).rev() {
            let mut row = vec![sp.days[i].clone()];
            row.extend(cols.iter().map(|v| compact(v[i])));
            p.rows.push(row);
        }
        Ok(p)
    }
}

/// Renders the dashboard.
fn render_page(p: &AdminPage) -> String {
    let e = template_escape;
    let mut out = String::from(PAGE_HEAD);
    out.push_str("<header><h1>Honorum</h1><nav aria-label=Range>");
    for r in p.ranges {
        let current = if r == p.range { r#" aria-current="page""# } else { "" };
        let _ = write!(out, r#"<a href="?days={r}"{current}>{}</a>"#, e(&range_label(r)));
    }
    out.push_str("</nav></header>\n<section class=tiles>");
    for t in &p.tiles {
        let class = if t.good {
            " good"
        } else if t.bad {
            " bad"
        } else {
            ""
        };
        let delta = if t.delta.is_empty() { "&nbsp;".to_owned() } else { e(&t.delta) };
        let _ = write!(
            out,
            r#"<div class="card tile"><div class=label>{}</div><div class=value>{}</div><div class="delta{class}">{delta}</div></div>"#,
            e(&t.label),
            e(&t.value)
        );
    }
    out.push_str("</section>\n");
    for (title, charts) in &p.sections {
        let _ = write!(out, "<h2>{}</h2><section class=grid>", e(title));
        for c in charts {
            let _ = write!(
                out,
                r#"<figure class="card chart" style="margin:0"><div class=top><h3>{}</h3><div class=legend>"#,
                e(&c.title)
            );
            if c.series.len() == 1 {
                let _ = write!(out, "<span><b>{}</b></span>", e(&c.latest(&c.series[0])));
            } else {
                for (i, s) in c.series.iter().enumerate() {
                    let _ = write!(
                        out,
                        r#"<span><i class="s{}"></i>{} <b>{}</b></span>"#,
                        i + 1,
                        e(&s.label),
                        e(&c.latest(s))
                    );
                }
            }
            let _ = write!(out, "</div></div>{}</figure>", c.svg());
        }
        out.push_str("</section>");
    }
    out.push_str("\n<h2>Schools</h2>\n");
    out.push_str(r#"<div class="card scroll"><table><tr><th>School</th><th>Accounts</th><th>Change</th><th>Watches</th><th>Sections</th><th>Catalog</th><th>Fetched</th></tr>"#);
    out.push('\n');
    for school in &p.schools {
        let i = &school.status;
        let _ = write!(
            out,
            r#"<tr><td>{}</td><td>{}</td><td class=delta>{}</td><td>{}</td><td>{}</td><td><span class="state {}" title="{}"><i></i>{}</span></td><td>{}</td></tr>"#,
            e(&i.name),
            e(&school.accounts),
            e(&school.change),
            i.watches,
            i.sections,
            e(school.class),
            e(&i.message),
            e(school.state),
            e(&ago(i.last_ok))
        );
    }
    out.push_str("\n</table></div>\n");
    out.push_str(r#"<details><summary>All Numbers</summary><div class="card scroll"><table><tr>"#);
    for h in &p.head {
        let _ = write!(out, "<th>{}</th>", e(h));
    }
    out.push_str("</tr>");
    for row in &p.rows {
        out.push_str("<tr>");
        for cell in row {
            let _ = write!(out, "<td>{}</td>", e(cell));
        }
        out.push_str("</tr>");
    }
    out.push_str("</table></div></details>\n");
    let _ = write!(
        out,
        r#"<footer>Notifications queue {} · Days in UTC · Version {} · {} · <a href="/api/admin/metrics?days={}">JSON</a></footer>"#,
        e(&p.queue),
        e(&p.version),
        e(&p.generated_at),
        p.range
    );
    out.push_str("\n</main>");
    out
}

/// `GET /admin`: the dashboard; `?days=` picks the range.
pub(crate) async fn handle_admin_html(
    State(s): State<Arc<Server>>,
    Query(query): Query<HashMap<String, String>>,
) -> ApiResult {
    let Ok(page) = s.admin_page(admin_range(&query)).await else {
        return fail(500, "could not read analytics");
    };
    Ok(html_response(render_page(&page)))
}

/// `GET /api/admin/metrics`: `{"days": [...], "metrics": {name: [value or
/// null, ...]}}` for the chosen range.
pub(crate) async fn handle_admin_json(
    State(s): State<Arc<Server>>,
    Query(query): Query<HashMap<String, String>>,
) -> ApiResult {
    let Ok(m) = s.load_metrics().await else {
        return fail(500, "could not read analytics");
    };
    let sp = new_span(admin_range(&query), s.now());
    let days = &sp.days[sp.n..];
    let names: BTreeSet<&str> =
        m.values.keys().map(String::as_str).chain(METRIC_COLUMNS.iter().map(|(name, _)| *name)).collect();
    let metrics: serde_json::Map<String, serde_json::Value> = names
        .into_iter()
        .map(|name| {
            let list = m.daily(name, days).into_iter().map(json_number).collect();
            (name.to_owned(), serde_json::Value::Array(list))
        })
        .collect();
    ok(&serde_json::json!({ "days": days, "metrics": metrics }))
}

/// A metric value as JSON: `null` for NaN, and integral values below 10^15
/// without a fraction.
fn json_number(v: f64) -> serde_json::Value {
    if v.is_nan() {
        serde_json::Value::Null
    } else if v.fract() == 0.0 && v.abs() < 1e15 {
        serde_json::Value::from(v as i64)
    } else {
        serde_json::Value::from(v)
    }
}
/// The page head: metadata and styles. Series colors are the first three
/// slots of a palette checked for color-vision deficiency, stepped
/// separately for dark.
const PAGE_HEAD: &str = r#"<!doctype html><html lang=en><meta charset=utf-8><meta name=viewport content="width=device-width,initial-scale=1"><meta name=robots content=noindex><title>Honorum Admin</title>
<style>
:root{color-scheme:light;--page:#f9f9f7;--surface:#fcfcfb;--ink:#0b0b0b;--ink2:#52514e;--muted:#898781;--grid:#e1e0d9;--axis:#c3c2b7;--ring:rgba(11,11,11,.1);--s1:#2a78d6;--s2:#eb6834;--s3:#1baf7a;--good:#006300;--bad:#d03b3b;--warn:#b07800;--dot-good:#0ca30c;--dot-warn:#fab219;--dot-bad:#d03b3b}
@media (prefers-color-scheme:dark){:root{color-scheme:dark;--page:#0d0d0d;--surface:#1a1a19;--ink:#fff;--ink2:#c3c2b7;--muted:#898781;--grid:#2c2c2a;--axis:#383835;--ring:rgba(255,255,255,.1);--s1:#3987e5;--s2:#d95926;--s3:#199e70;--good:#0ca30c;--bad:#e66767;--warn:#fab219}}
*{box-sizing:border-box}
body{margin:0;background:var(--page);color:var(--ink);font:14px/1.45 system-ui,-apple-system,"Segoe UI",sans-serif}
main{max-width:1120px;margin:0 auto;padding:20px 16px 40px}
header{display:flex;align-items:center;justify-content:space-between;gap:12px;flex-wrap:wrap;margin-bottom:16px}
h1{margin:0;font-size:1.25rem;font-weight:650}
h2{margin:28px 0 10px;font-size:.8rem;font-weight:650;color:var(--ink2);text-transform:uppercase;letter-spacing:.04em}
h3{margin:0;font-size:.9rem;font-weight:600}
nav{display:inline-flex;padding:2px;border-radius:8px;background:var(--ring)}
nav a{padding:3px 10px;border-radius:6px;color:var(--ink2);font-size:.8rem;font-weight:600;text-decoration:none}
nav a[aria-current]{background:var(--surface);color:var(--ink);box-shadow:0 1px 2px rgba(0,0,0,.12)}
.card{background:var(--surface);border-radius:10px;box-shadow:0 0 0 1px var(--ring)}
.tiles{display:grid;grid-template-columns:repeat(auto-fit,minmax(150px,1fr));gap:10px}
.tile{padding:12px 14px}
.tile .label{color:var(--ink2);font-size:.8rem}
.tile .value{font-size:1.6rem;font-weight:600;line-height:1.25}
.delta{font-size:.78rem;color:var(--muted);font-variant-numeric:tabular-nums}
.delta.good{color:var(--good)}.delta.bad{color:var(--bad)}
.grid{display:grid;grid-template-columns:repeat(auto-fill,minmax(min(100%,440px),1fr));gap:10px}
.chart{padding:12px 14px 8px;container-type:inline-size}
.chart .top{display:flex;align-items:baseline;justify-content:space-between;gap:10px;flex-wrap:wrap;margin-bottom:4px}
.legend{display:flex;gap:12px;flex-wrap:wrap;color:var(--ink2);font-size:.78rem;font-variant-numeric:tabular-nums}
.legend span{display:inline-flex;align-items:center;gap:5px}
.legend i{width:8px;height:8px;border-radius:50%}
.legend b{color:var(--ink);font-weight:600}
svg{display:block;width:100%;height:auto;overflow:visible}
svg text{font-size:10.5px;fill:var(--muted);font-variant-numeric:tabular-nums}
@container (max-width:420px){svg .ax{font-size:14px}}
svg .gl{stroke:var(--grid);stroke-width:1}svg .base{stroke:var(--axis)}
.s1{fill:var(--s1);stroke:var(--s1)}.s2{fill:var(--s2);stroke:var(--s2)}.s3{fill:var(--s3);stroke:var(--s3)}
i.s1{background:var(--s1)}i.s2{background:var(--s2)}i.s3{background:var(--s3)}
svg path[class^=s]{stroke:none}
svg .line{fill:none;stroke-width:2;stroke-linejoin:round;stroke-linecap:round}
svg .wash{stroke:none;opacity:.1}
svg .dot{stroke:var(--surface);stroke-width:2}
.hit .band{fill:var(--ink);opacity:0}
.hit .dot,.hit .tip{visibility:hidden}
.hit:hover .band{opacity:.05}.hit:hover .dot,.hit:hover .tip{visibility:visible}
.tip{pointer-events:none}.tip rect{fill:var(--surface);stroke:var(--ring);filter:drop-shadow(0 2px 6px rgba(0,0,0,.15))}
.tip circle{stroke:none}.tip .head{fill:var(--ink2);font-weight:600}.tip .key{fill:var(--ink2)}.tip .val{fill:var(--ink);font-weight:600}
table{width:100%;border-collapse:collapse;font-variant-numeric:tabular-nums}
th,td{padding:7px 12px;text-align:right;white-space:nowrap;border-bottom:1px solid var(--grid)}
th:first-child,td:first-child{text-align:left}
th{color:var(--ink2);font-weight:600;font-size:.78rem}
tr:last-child td{border-bottom:0}
.state{display:inline-flex;align-items:center;gap:6px}.state i{width:8px;height:8px;border-radius:50%}
.state.good i{background:var(--dot-good)}.state.warn i{background:var(--dot-warn)}.state.bad i{background:var(--dot-bad)}
.scroll{overflow-x:auto}
details{margin-top:28px}summary{cursor:pointer;color:var(--ink2);font-size:.8rem;font-weight:650;text-transform:uppercase;letter-spacing:.04em}
details .card{margin-top:10px}
footer{margin-top:24px;color:var(--muted);font-size:.78rem}footer a{color:inherit}
</style>
<main>
"#;

#[cfg(test)]
mod tests {
    use base64::Engine;
    use base64::engine::general_purpose::STANDARD;
    use chrono::{TimeDelta, TimeZone, Utc};

    use super::*;
    use crate::test_support::{test_server, test_server_with};

    const TEST_ADMIN_TOKEN: &str = "0123456789abcdef0123456789abcdef";

    fn basic(user: &str, password: &str) -> String {
        format!("Basic {}", STANDARD.encode(format!("{user}:{password}")))
    }

    #[tokio::test]
    async fn admin_needs_its_token() {
        let t = test_server().await;
        assert_eq!(t.client().get("/admin").await.status.as_u16(), 404, "without a configured token");

        let t = test_server_with(|cfg| cfg.admin_token = TEST_ADMIN_TOKEN.into()).await;
        let c = t.client();
        let w = c.get("/admin").await;
        assert!(
            w.status.as_u16() == 401 && w.header("www-authenticate").starts_with("Basic "),
            "without credentials: {} {:?}",
            w.status,
            w.header("www-authenticate")
        );
        let w = c.with_header("authorization", &basic("admin", "wrong")).get("/admin").await;
        assert_eq!(w.status.as_u16(), 401, "wrong token");
        let w = c.with_header("authorization", &basic("anyone", TEST_ADMIN_TOKEN)).get("/admin?days=90").await;
        assert!(
            w.status.as_u16() == 200 && w.text().contains("Active accounts") && w.header("cache-control") == "no-store",
            "basic auth: {} {:?}",
            w.status,
            w.header("cache-control")
        );
        let w = c.bearer(TEST_ADMIN_TOKEN).get("/api/admin/metrics?days=7").await;
        assert_eq!(w.status.as_u16(), 200, "bearer JSON: {}", w.text());
        let out = w.json();
        assert_eq!(out["days"].as_array().map(Vec::len), Some(7), "{out}");
        assert_eq!(out["metrics"]["accounts"].as_array().map(Vec::len), Some(7), "{out}");
    }

    // Token length is checked by config::tests::short_admin_tokens_are_refused,
    // since reading the token happens in Config's environment step.

    #[tokio::test]
    async fn metrics_count_and_total() {
        let t = test_server().await;
        let s = &t.server;
        let day = Utc.with_ymd_and_hms(2026, 10, 1, 12, 0, 0).unwrap();
        s.clock.set(day);
        let (unix, three_ago, ten_ago) =
            (day.timestamp(), (day - TimeDelta::days(3)).timestamp(), (day - TimeDelta::days(10)).timestamp());
        s.db.exec(
            "INSERT INTO accounts(id,phone,created_at) VALUES('a','+15550000001',$1),('b','+15550000002',$2)",
            &[&three_ago, &unix],
        )
        .await
        .unwrap();
        s.db.exec(
            "INSERT INTO sessions(id,account,token_hash,created_at,last_seen) VALUES('s1','a','h1',0,$1),('s2','a','h2',0,$2),('s3','b','h3',0,$3)",
            &[&unix, &unix, &ten_ago],
        )
        .await
        .unwrap();
        s.db.exec(
            r#"INSERT INTO docs(account,id,type,seq,updated_at,body) VALUES('a','prefs','prefs',1,0,'{"inst":"demo"}'),('b','prefs','prefs',1,0,'{"inst":"elsewhere"}')"#,
            &[],
        )
        .await
        .unwrap();
        s.backfill_metrics().await;
        s.count("requests", 3);
        s.count("signins", 1);
        s.flush_counts().await;
        s.count("requests", 2);
        s.flush_counts().await;
        s.record_gauges().await;

        let m = s.load_metrics().await.unwrap();
        let today = metric_day(day);
        for (name, want) in [
            ("requests", 5.0),
            ("signins", 1.0),
            ("accounts", 2.0),
            ("active_day", 1.0),
            ("active_week", 1.0),
            ("active_month", 2.0),
            ("devices", 3.0),
            ("school:demo", 1.0),
        ] {
            let got = m.values.get(name).and_then(|v| v.get(&today)).copied().unwrap_or_default();
            assert_eq!(got, want, "{name}");
        }
        // History from creation dates: one account three days ago, none since.
        let days: Vec<String> = [4, 3, 2, 0].iter().map(|d| metric_day(day - TimeDelta::days(*d))).collect();
        let (accounts, signups) = (m.daily("accounts", &days), m.daily("signups", &days));
        assert!(
            accounts[0].is_nan() && accounts[1] == 1.0 && accounts[2] == 1.0 && accounts[3] == 2.0,
            "accounts carry forward: {accounts:?}"
        );
        assert!(
            signups[0].is_nan() && signups[1] == 1.0 && signups[2] == 0.0,
            "signups count from their first day: {signups:?}"
        );
        let errors = m.daily("server_errors", &days);
        assert!(errors[2].is_nan() && errors[3] == 0.0, "counters start with requests: {errors:?}");
    }

    #[test]
    fn span_buckets() {
        let now = Utc::now();
        let sp = new_span(365, now);
        let b = sp.buckets();
        assert!(
            sp.n == 364 && b.len() == 52 && b[0] == (364, 371) && b[51].1 == 728,
            "year buckets: n={} {:?}",
            sp.n,
            &b[..1]
        );
        let sp = new_span(7, now);
        assert!(sp.buckets().len() == 7 && sp.labels().len() == 7 && sp.days[13] == metric_day(now), "week buckets");
        let v = [f64::NAN, 2.0, f64::NAN, 3.0];
        assert!(sum_of(&v, 0, 4) == 5.0 && last_of(&v, 0, 3) == 2.0 && sum_of(&v, 0, 1).is_nan(), "folding");
        assert_eq!(nice_scale(2465.0), (3000.0, 1000.0), "scale");
        assert_eq!(
            (compact(1284.0), compact(12900.0), compact(4.2e6)),
            ("1,284".into(), "12.9K".into(), "4.2M".into())
        );
    }
}
