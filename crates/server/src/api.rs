//! Conventions every handler shares: JSON responses and errors, bounded
//! request bodies, and the middleware that applies security headers, write
//! origin checks, rate limits, and timing.
//!
//! A handler returns [`ApiResult`]. Failures are [`ApiError`] values, which
//! render as `{"error": "<message>"}` and count toward the day's client or
//! server error metrics.

use std::sync::Arc;
use std::time::Instant;

use axum::body::Body;
use axum::extract::{Request, State};
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::Server;
use crate::session::{self, ResolvedSession};

pub(crate) type ApiResult<T = Response> = Result<T, ApiError>;

/// An error response: a status and a message safe to show a student.
#[derive(Debug, Clone)]
pub(crate) struct ApiError {
    pub(crate) status: StatusCode,
    pub(crate) message: String,
    /// Seconds for the `Retry-After` header.
    pub(crate) retry_after: Option<u64>,
}

impl ApiError {
    pub(crate) fn new(status: u16, message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::from_u16(status).expect("a valid status"),
            message: message.into(),
            retry_after: None,
        }
    }

    pub(crate) fn retry_after(mut self, seconds: u64) -> Self {
        self.retry_after = Some(seconds);
        self
    }
}

/// Marks a response produced by [`ApiError`], so the request middleware can
/// count it.
#[derive(Clone, Copy)]
pub(crate) struct Failed;

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let mut response = json(self.status, &serde_json::json!({ "error": self.message }));
        if let Some(seconds) = self.retry_after {
            response.headers_mut().insert(header::RETRY_AFTER, HeaderValue::from(seconds));
        }
        response.extensions_mut().insert(Failed);
        response
    }
}

/// Shorthand for `Err(ApiError::new(status, message))`.
pub(crate) fn fail<T>(status: u16, message: impl Into<String>) -> ApiResult<T> {
    Err(ApiError::new(status, message))
}

/// A JSON response. Serialization of server-built values cannot fail.
pub(crate) fn json<T: Serialize + ?Sized>(status: StatusCode, value: &T) -> Response {
    let mut body = serde_json::to_vec(value).expect("response values serialize");
    body.push(b'\n');
    let mut response = Response::new(Body::from(body));
    *response.status_mut() = status;
    response.headers_mut().insert(header::CONTENT_TYPE, HeaderValue::from_static("application/json"));
    response
}

/// A `200 OK` JSON response.
pub(crate) fn ok<T: Serialize + ?Sized>(value: &T) -> ApiResult {
    Ok(json(StatusCode::OK, value))
}

/// An empty response with `status`, such as `204 No Content`.
pub(crate) fn empty(status: u16) -> Response {
    let mut response = Response::new(Body::empty());
    *response.status_mut() = StatusCode::from_u16(status).expect("a valid status");
    response
}

/// Reads at most `limit` bytes of a request body. A longer body is cut off at
/// the limit, as `io.LimitReader` did, so JSON in it fails to parse.
pub(crate) async fn read_body(body: Body, limit: usize) -> Option<bytes::Bytes> {
    use http_body_util::BodyExt;
    let limited = http_body_util::Limited::new(body, limit);
    match limited.collect().await {
        Ok(collected) => Some(collected.to_bytes()),
        Err(err) if err.downcast_ref::<http_body_util::LengthLimitError>().is_some() => Some(bytes::Bytes::new()),
        Err(_) => None,
    }
}

/// Reads the body as one JSON value of at most `limit` bytes; trailing data
/// after the first value is ignored. `None` means the body was missing,
/// too long, or not the expected shape.
///
/// Field names are matched exactly. Give request structs
/// `#[serde(default)]` so missing fields take their zero value.
pub(crate) async fn read_json<T: DeserializeOwned>(body: Body, limit: usize) -> Option<T> {
    let bytes = read_body(body, limit).await?;
    parse_json(&bytes)
}

/// Parses the first JSON value in `bytes`.
pub(crate) fn parse_json<T: DeserializeOwned>(bytes: &[u8]) -> Option<T> {
    serde_json::Deserializer::from_slice(bytes).into_iter::<T>().next()?.ok()
}

/// Adds conservative security headers and cache policy to every response.
pub(crate) async fn secure(request: Request, next: Next) -> Response {
    let path = request.uri().path().to_owned();
    let mut response = next.run(request).await;
    let h = response.headers_mut();
    h.insert("x-content-type-options", HeaderValue::from_static("nosniff"));
    h.insert("x-frame-options", HeaderValue::from_static("DENY"));
    h.insert("referrer-policy", HeaderValue::from_static("no-referrer"));
    h.insert(
        "content-security-policy",
        HeaderValue::from_static(
            "default-src 'self'; img-src 'self' data:; style-src 'self' 'unsafe-inline'; connect-src 'self'",
        ),
    );
    // These are defaults: a handler's own cache policy (such as
    // `/api/config`'s) takes precedence.
    let policy = if path == "/" || path == "/index.html" || path == "/sw.js" {
        Some("no-cache")
    } else if path.starts_with("/api/") {
        Some("no-store")
    } else if path.starts_with("/assets/") {
        Some("public, max-age=31536000, immutable")
    } else {
        None
    };
    if let Some(policy) = policy {
        h.entry(header::CACHE_CONTROL).or_insert(HeaderValue::from_static(policy));
    }
    response
}

/// Counts requests and failures, applies write rules, and times API calls.
///
/// Writes from another origin are refused. Resource-consuming writes have
/// bounded rates: signed-in writes count against the account, since many
/// students on campus Wi-Fi share an address; the session read here is kept
/// in the request for the handler.
pub(crate) async fn admit(State(s): State<Arc<Server>>, mut request: Request, next: Next) -> Response {
    s.counters.requests.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    s.count("requests", 1);
    let path = request.uri().path().to_owned();
    if let Err(err) = s.allow_request(&mut request).await {
        return s.counted(err.into_response());
    }
    let started = Instant::now();
    let response = next.run(request).await;
    if path.starts_with("/api/") {
        let elapsed = started.elapsed();
        s.api_timing.observe(elapsed);
        // The event stream stays open for as long as a page does.
        if path != "/api/me/events" {
            s.observe_api(elapsed);
        }
    }
    s.counted(response)
}

impl Server {
    /// Records a failure response in the error counters.
    fn counted(&self, response: Response) -> Response {
        if response.extensions().get::<Failed>().is_some() {
            self.counters.errors.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            if response.status().is_server_error() {
                self.count("server_errors", 1);
            } else {
                self.count("client_errors", 1);
            }
        }
        response
    }

    async fn allow_request(&self, request: &mut Request) -> ApiResult<()> {
        let path = request.uri().path();
        // Google's change notifications all come from Google, carry no
        // origin, and each is checked against its channel's secret.
        if request.method() == Method::GET || request.method() == Method::HEAD || path == "/api/google/notify" {
            return Ok(());
        }
        if let Some(origin) = request.headers().get(header::ORIGIN) {
            let host = request.headers().get(header::HOST).and_then(|h| h.to_str().ok()).unwrap_or_default();
            if !self.origin_allowed(origin.to_str().unwrap_or_default(), host) {
                return fail(403, "request origin is not allowed");
            }
        }
        let ip = self.client_ip(request);
        let (key, limit, period) = if path.starts_with("/api/lms/") {
            (format!("lms:{ip}"), 30, 60)
        } else if path.starts_with("/api/sync/") {
            (format!("share:{ip}"), 100, 3600)
        } else if session::has_credential(request.headers()) {
            let resolved = self.lookup_session(request.headers()).await;
            let key = match &resolved {
                Ok(Some(se)) => format!("account:{}", se.account),
                _ => format!("write:{ip}"),
            };
            request.extensions_mut().insert(ResolvedSession(Arc::new(resolved)));
            (key, 120, 60)
        } else {
            (format!("write:{ip}"), 120, 60)
        };
        match self.request_allow(&key, limit, period).await {
            Err(_) => fail(503, "request limits unavailable; try again shortly"),
            Ok(false) => Err(ApiError::new(429, "too many requests; try again shortly").retry_after(60)),
            Ok(true) => Ok(()),
        }
    }

    fn origin_allowed(&self, origin: &str, host: &str) -> bool {
        let Ok(u) = url::Url::parse(origin) else { return false };
        let origin_host = host_port(&u);
        if origin_host.is_empty() || (u.scheme() != "http" && u.scheme() != "https") {
            return false;
        }
        let base = url::Url::parse(&self.cfg.base_url).map(|b| host_port(&b)).unwrap_or_default();
        origin_host == host || origin_host == base
    }

    /// The address of the browser. Forwarded addresses count only when the
    /// proxy is trusted.
    pub(crate) fn client_ip(&self, request: &Request) -> String {
        let remote = request
            .extensions()
            .get::<axum::extract::ConnectInfo<std::net::SocketAddr>>()
            .map(|c| c.0.ip().to_string())
            .unwrap_or_default();
        if self.cfg.trust_proxy { forwarded_ip(request.headers()).unwrap_or(remote) } else { remote }
    }
}

/// `host[:port]` as it appears in a `Host` header.
fn host_port(u: &url::Url) -> String {
    match (u.host_str(), u.port()) {
        (Some(h), Some(p)) => format!("{h}:{p}"),
        (Some(h), None) => h.to_owned(),
        _ => String::new(),
    }
}

fn forwarded_ip(headers: &HeaderMap) -> Option<String> {
    ["cf-connecting-ip", "x-forwarded-for"].iter().find_map(|name| {
        let value = headers.get(*name)?.to_str().ok()?;
        (!value.is_empty()).then(|| value.split(',').next().unwrap_or_default().trim().to_owned())
    })
}

/// Returns valid JSON text in the encoding [`compact_json`] writes, without
/// copying text already in it.
pub(crate) fn final_json(raw: String) -> String {
    if is_compact_json(&raw) { raw } else { compact_json(&raw) }
}

/// Reports whether valid JSON text is already in the encoding
/// [`compact_json`] writes: no whitespace outside strings, and no `<`, `>`,
/// `&`, U+2028, or U+2029 inside them. Reads bytes; UTF-8 continuation bytes
/// never equal a quote or backslash.
pub(crate) fn is_compact_json(raw: &str) -> bool {
    let bytes = raw.as_bytes();
    let (mut quoted, mut escaped) = (false, false);
    for (i, &b) in bytes.iter().enumerate() {
        if !quoted {
            match b {
                b' ' | b'\t' | b'\n' | b'\r' => return false,
                b'"' => quoted = true,
                _ => {}
            }
            continue;
        }
        if escaped {
            escaped = false;
            continue;
        }
        match b {
            b'\\' => escaped = true,
            b'"' => quoted = false,
            b'<' | b'>' | b'&' => return false,
            // U+2028 and U+2029 are E2 80 A8 and E2 80 A9.
            0xE2 if bytes.get(i + 1) == Some(&0x80) && matches!(bytes.get(i + 2), Some(0xA8 | 0xA9)) => return false,
            _ => {}
        }
    }
    true
}

/// Rewrites valid JSON text compactly: insignificant whitespace removed, and
/// `<`, `>`, `&`, U+2028, and U+2029 inside strings escaped as `\u`
/// sequences, as [`escaped_json`] writes them. Document responses and their
/// page budgets use this encoding.
pub(crate) fn compact_json(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let (mut quoted, mut escaped) = (false, false);
    for c in raw.chars() {
        if !quoted {
            match c {
                ' ' | '\t' | '\n' | '\r' => {}
                '"' => {
                    quoted = true;
                    out.push(c);
                }
                _ => out.push(c),
            }
            continue;
        }
        if escaped {
            escaped = false;
            out.push(c);
            continue;
        }
        match c {
            '\\' => {
                escaped = true;
                out.push(c);
            }
            '"' => {
                quoted = false;
                out.push(c);
            }
            '<' => out.push_str("\\u003c"),
            '>' => out.push_str("\\u003e"),
            '&' => out.push_str("\\u0026"),
            '\u{2028}' => out.push_str("\\u2028"),
            '\u{2029}' => out.push_str("\\u2029"),
            _ => out.push(c),
        }
    }
    out
}

/// Serializes `value` compactly with struct fields in declaration order, map
/// keys sorted, and `<`, `>`, `&`, U+2028, and U+2029 escaped as `\u003c`
/// and so on. Stored Google event body hashes and event lists were computed
/// over this exact encoding by earlier releases, so changing it republishes
/// every event.
pub(crate) fn escaped_json<T: serde::Serialize + ?Sized>(value: &T) -> Vec<u8> {
    let mut out = Vec::new();
    let mut ser = serde_json::Serializer::with_formatter(&mut out, EscapingFormatter);
    value.serialize(&mut ser).expect("server values serialize");
    out
}

/// Escapes HTML-significant characters and line separators in strings.
struct EscapingFormatter;

impl serde_json::ser::Formatter for EscapingFormatter {
    fn write_string_fragment<W: ?Sized + std::io::Write>(&mut self, w: &mut W, fragment: &str) -> std::io::Result<()> {
        let mut start = 0;
        for (i, c) in fragment.char_indices() {
            let escaped = match c {
                '<' => "\\u003c",
                '>' => "\\u003e",
                '&' => "\\u0026",
                '\u{2028}' => "\\u2028",
                '\u{2029}' => "\\u2029",
                _ => continue,
            };
            w.write_all(&fragment.as_bytes()[start..i])?;
            w.write_all(escaped.as_bytes())?;
            start = i + c.len_utf8();
        }
        w.write_all(&fragment.as_bytes()[start..])
    }
}

/// Deserializes JSON `null` as the type's default, so a field written as
/// `null` reads the same as an omitted one. Use with
/// `#[serde(default, deserialize_with = "null_default")]`.
pub(crate) fn null_default<'de, D, T>(d: D) -> Result<T, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Default + serde::Deserialize<'de>,
{
    use serde::Deserialize;
    Ok(Option::<T>::deserialize(d)?.unwrap_or_default())
}

/// The browser's address, as [`Server::client_ip`] reads it, for handlers
/// with their own rate limits (phone codes, the admin page). Forwarded
/// addresses count only when the proxy is trusted; a request without
/// connection information, as in tests, yields an empty address.
pub(crate) struct ClientIp(pub String);

impl axum::extract::FromRequestParts<Arc<Server>> for ClientIp {
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(
        parts: &mut axum::http::request::Parts,
        s: &Arc<Server>,
    ) -> Result<Self, Self::Rejection> {
        let remote = parts
            .extensions
            .get::<axum::extract::ConnectInfo<std::net::SocketAddr>>()
            .map(|c| c.0.ip().to_string())
            .unwrap_or_default();
        let ip = if s.cfg.trust_proxy { forwarded_ip(&parts.headers).unwrap_or(remote) } else { remote };
        Ok(ClientIp(ip))
    }
}
