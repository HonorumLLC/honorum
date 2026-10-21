//! Serving files: published catalog snapshots (`/catalog/{inst}/{file}`)
//! with prepared gzip and Brotli encodings, and the embedded web client
//! with compressed text assets.

use std::borrow::Cow;
use std::collections::HashMap;
use std::io::Write;
use std::sync::{Arc, LazyLock};

use axum::body::Body;
use axum::extract::{Path, Request, State};
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode, header};
use axum::response::Response;
use chrono::{DateTime, Utc};
use regex::Regex;

use crate::Server;
use crate::api::ApiResult;
use crate::db::Sql;
use crate::snapshot::with_suffix;

/// The web client's files, supplied by the binary (rust-embed of
/// `apps/web/dist`). Paths are relative and `/`-separated, without a
/// leading slash, such as `index.html` or `assets/app-1234.js`.
pub trait WebAssets: Send + Sync {
    /// Every file path.
    fn names(&self) -> Vec<String>;
    /// A file's bytes.
    fn get(&self, path: &str) -> Option<Cow<'static, [u8]>>;
    /// A file's modification time, Unix seconds, when known.
    fn modified(&self, path: &str) -> Option<i64>;
}

/// One prepared text asset: the raw bytes, gzip (best compression), the
/// build's Brotli copy when present, and the modification time.
#[derive(Clone, Debug, Default)]
pub(crate) struct PreparedAsset {
    pub(crate) raw: bytes::Bytes,
    pub(crate) gzip: bytes::Bytes,
    pub(crate) brotli: Option<bytes::Bytes>,
    pub(crate) modified: Option<i64>,
}

/// The web client as served: text assets (`.html`, `.js`, `.css`, `.json`,
/// `.svg`, `.webmanifest`) prepared once, keyed by URL path (`/index.html`),
/// and every other file served as is.
///
/// Standalone hosting thus gets compressed text assets without compressing
/// on requests: the build's Brotli copies where it made them, and gzip made
/// here otherwise.
pub(crate) struct StaticSite {
    pub(crate) files: Arc<dyn WebAssets>,
    pub(crate) prepared: HashMap<String, PreparedAsset>,
}

const PREPARED_EXTENSIONS: [&str; 6] = ["html", "js", "css", "json", "svg", "webmanifest"];

impl StaticSite {
    /// Prepares the text assets of `files`.
    pub(crate) fn new(files: Arc<dyn WebAssets>) -> Self {
        let mut prepared = HashMap::new();
        for name in files.names() {
            if !PREPARED_EXTENSIONS.contains(&extension(&name)) {
                continue;
            }
            let Some(raw) = files.get(&name) else { continue };
            let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::best());
            if encoder.write_all(&raw).is_err() {
                continue;
            }
            let Ok(gzip) = encoder.finish() else { continue };
            let asset = PreparedAsset {
                raw: bytes::Bytes::copy_from_slice(&raw),
                gzip: gzip.into(),
                brotli: files.get(&format!("{name}.br")).map(|b| bytes::Bytes::copy_from_slice(&b)),
                modified: files.modified(&name),
            };
            prepared.insert(format!("/{name}"), asset);
        }
        Self { files, prepared }
    }

    /// Serves one request: `/` is `/index.html`; prepared assets negotiate
    /// `br`/`gzip` (406 "no acceptable encoding" when identity is refused)
    /// with `Vary: Accept-Encoding` and conditional GET support; other files
    /// are served with their MIME type, and missing ones are 404.
    pub(crate) async fn serve(&self, request: Request) -> Response {
        let mut name = request.uri().path().to_owned();
        if name == "/" {
            name = "/index.html".into();
        }
        let Some(asset) = self.prepared.get(&name) else { return self.serve_file(&request) };
        let accept = header_str(request.headers(), header::ACCEPT_ENCODING);
        let mut body = asset.raw.clone();
        let mut chosen = None;
        for encoding in accepted_encodings(accept) {
            match (encoding, &asset.brotli) {
                ("br", Some(br)) => body = br.clone(),
                ("gzip", _) => body = asset.gzip.clone(),
                _ => continue,
            }
            chosen = Some(encoding);
            break;
        }
        let mut headers = HeaderMap::new();
        headers.append(header::VARY, HeaderValue::from_static("Accept-Encoding"));
        if chosen.is_none() && encoding_quality(accept, "identity") == 0.0 {
            return plain_error(headers, StatusCode::NOT_ACCEPTABLE, "no acceptable encoding\n");
        }
        if let Some(encoding) = chosen {
            headers.insert(header::CONTENT_ENCODING, HeaderValue::from_static(encoding));
        }
        headers.insert(header::CONTENT_TYPE, HeaderValue::from_static(mime_type(&name)));
        serve_content(&request, headers, asset.modified, body)
    }

    /// Serves a file that has no prepared encodings: `index.html` for
    /// directories, 404 for unknown names.
    fn serve_file(&self, request: &Request) -> Response {
        let path = request.uri().path().trim_start_matches('/');
        let mut name = clean_path(path);
        if name.is_empty() || path.ends_with('/') {
            name = if name.is_empty() { "index.html".into() } else { format!("{name}/index.html") };
        }
        let Some(data) = self.files.get(&name) else {
            return plain_error(HeaderMap::new(), StatusCode::NOT_FOUND, "404 page not found\n");
        };
        let mut headers = HeaderMap::new();
        headers.insert(header::CONTENT_TYPE, HeaderValue::from_static(mime_type(&name)));
        serve_content(request, headers, self.files.modified(&name), bytes::Bytes::copy_from_slice(&data))
    }
}

/// Resolves `.` and `..` segments, as `path.Clean` does, without leaving the
/// root.
fn clean_path(path: &str) -> String {
    let mut parts: Vec<&str> = Vec::new();
    for segment in path.split('/') {
        match segment {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            s => parts.push(s),
        }
    }
    parts.join("/")
}

fn extension(name: &str) -> &str {
    let base = name.rsplit('/').next().unwrap_or(name);
    base.rsplit_once('.').map(|(_, ext)| ext).unwrap_or("")
}

/// The content type for a file name, by extension.
fn mime_type(name: &str) -> &'static str {
    match extension(name).to_ascii_lowercase().as_str() {
        "html" | "htm" => "text/html; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "json" | "map" => "application/json",
        "webmanifest" => "application/manifest+json",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "avif" => "image/avif",
        "ico" => "image/vnd.microsoft.icon",
        "wasm" => "application/wasm",
        "pdf" => "application/pdf",
        "xml" => "text/xml; charset=utf-8",
        "txt" => "text/plain; charset=utf-8",
        "woff" => "font/woff",
        "woff2" => "font/woff2",
        "ttf" => "font/ttf",
        _ => "application/octet-stream",
    }
}

fn header_str(headers: &HeaderMap, name: header::HeaderName) -> &str {
    headers.get(name).and_then(|v| v.to_str().ok()).unwrap_or_default()
}

/// A plain-text error (`text/plain; charset=utf-8`, `nosniff`), keeping
/// `headers`.
fn plain_error(mut headers: HeaderMap, status: StatusCode, body: &'static str) -> Response {
    headers.insert(header::CONTENT_TYPE, HeaderValue::from_static("text/plain; charset=utf-8"));
    headers.insert("x-content-type-options", HeaderValue::from_static("nosniff"));
    let mut response = Response::new(Body::from(body));
    *response.status_mut() = status;
    *response.headers_mut() = headers;
    response
}

/// Conditional and range serving: `Last-Modified` and `If-Modified-Since`
/// (when no `If-None-Match` is sent), a single byte range,
/// `Content-Length`, and no body for HEAD.
fn serve_content(request: &Request, mut headers: HeaderMap, modified: Option<i64>, body: bytes::Bytes) -> Response {
    let head = request.method() == Method::HEAD;
    let modified = modified.filter(|&m| m > 0).and_then(|m| DateTime::<Utc>::from_timestamp(m, 0));
    if let Some(m) = modified {
        let value = m.format("%a, %d %b %Y %H:%M:%S GMT").to_string();
        headers.insert(header::LAST_MODIFIED, HeaderValue::from_str(&value).expect("a date is a valid header"));
    }
    let requests = request.headers();
    let safe = request.method() == Method::GET || head;
    if safe
        && !requests.contains_key(header::IF_NONE_MATCH)
        && let Some(m) = modified
        && let Some(since) = requests.get(header::IF_MODIFIED_SINCE).and_then(|v| v.to_str().ok())
        && let Ok(since) = DateTime::parse_from_rfc2822(since)
        && m.timestamp() <= since.timestamp()
    {
        headers.remove(header::CONTENT_TYPE);
        headers.remove(header::CONTENT_ENCODING);
        headers.remove(header::CONTENT_LENGTH);
        let mut response = Response::new(Body::empty());
        *response.status_mut() = StatusCode::NOT_MODIFIED;
        *response.headers_mut() = headers;
        return response;
    }
    headers.insert(header::ACCEPT_RANGES, HeaderValue::from_static("bytes"));
    let size = body.len() as u64;
    let mut status = StatusCode::OK;
    let mut body = body;
    let range = requests.get(header::RANGE).and_then(|v| v.to_str().ok());
    if let Some(range) = range
        && !requests.contains_key(header::IF_RANGE)
    {
        match single_range(range, size) {
            Some(Ok((start, end))) => {
                status = StatusCode::PARTIAL_CONTENT;
                headers.insert(
                    header::CONTENT_RANGE,
                    HeaderValue::from_str(&format!("bytes {start}-{end}/{size}")).expect("a valid header"),
                );
                body = body.slice(start as usize..=end as usize);
            }
            Some(Err(())) => {
                headers.insert(
                    header::CONTENT_RANGE,
                    HeaderValue::from_str(&format!("bytes */{size}")).expect("a valid header"),
                );
                return plain_error(headers, StatusCode::RANGE_NOT_SATISFIABLE, "invalid range: failed to overlap\n");
            }
            None => {}
        }
    }
    headers.insert(header::CONTENT_LENGTH, HeaderValue::from(body.len() as u64));
    let mut response = Response::new(if head { Body::empty() } else { Body::from(body) });
    *response.status_mut() = status;
    *response.headers_mut() = headers;
    response
}

/// Parses a `Range` header naming one byte range. `None` ignores the header
/// (malformed or several ranges, which are served whole); `Some(Err)` is a
/// range outside the content.
fn single_range(header: &str, size: u64) -> Option<Result<(u64, u64), ()>> {
    let spec = header.strip_prefix("bytes=")?.trim();
    if spec.contains(',') {
        return None;
    }
    let (start, end) = spec.split_once('-')?;
    let (start, end) = (start.trim(), end.trim());
    if start.is_empty() {
        let n: u64 = end.parse().ok()?;
        if n == 0 || size == 0 {
            return Some(Err(()));
        }
        return Some(Ok((size.saturating_sub(n), size - 1)));
    }
    let start: u64 = start.parse().ok()?;
    if start >= size {
        return Some(Err(()));
    }
    let end = if end.is_empty() { size - 1 } else { end.parse::<u64>().ok()?.min(size - 1) };
    if end < start {
        return None;
    }
    Some(Ok((start, end)))
}

/// The q-value `header` (an `Accept-Encoding` value) gives `encoding`;
/// identity is acceptable unless refused explicitly or by `*;q=0`.
pub(crate) fn encoding_quality(header: &str, encoding: &str) -> f64 {
    let mut wildcard = -1.0;
    for item in header.split(',') {
        let mut parts = item.trim().split(';');
        let name = parts.next().unwrap_or_default().trim().to_ascii_lowercase();
        let mut q = 1.0;
        for param in parts {
            if let Some((k, v)) = param.trim().split_once('=')
                && k.eq_ignore_ascii_case("q")
            {
                q = match v.parse::<f64>() {
                    Ok(value) if (0.0..=1.0).contains(&value) => value,
                    _ => 0.0,
                };
            }
        }
        if name == encoding {
            return q;
        }
        if name == "*" {
            wildcard = q;
        }
    }
    if encoding == "identity" {
        return if wildcard == 0.0 { 0.0 } else { 1.0 };
    }
    if wildcard >= 0.0 { wildcard } else { 0.0 }
}

/// `br` and `gzip` in the client's order of preference, omitting refused ones.
pub(crate) fn accepted_encodings(header: &str) -> Vec<&'static str> {
    let mut encodings = vec!["br", "gzip"];
    // Stable: equal preference keeps Brotli first.
    encodings.sort_by(|a, b| encoding_quality(header, b).total_cmp(&encoding_quality(header, a)));
    encodings.retain(|e| encoding_quality(header, e) > 0.0);
    encodings
}

/// Reports whether an `If-None-Match` value matches `etag` (weak
/// comparison; `*` matches anything).
pub(crate) fn matches_etag(header: &str, etag: &str) -> bool {
    let etag = etag.strip_prefix("W/").unwrap_or(etag);
    header.split(',').map(str::trim).any(|value| value == "*" || value.strip_prefix("W/").unwrap_or(value) == etag)
}

/// Only published, content-addressed JSON files are exposed, never
/// arbitrary paths in the data directory.
static CATALOG_FILENAME: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^([A-Za-z0-9_-]+)\.(?:structure|seats|desc)\.([a-f0-9]{64})\.json$").expect("valid pattern")
});
static INSTITUTION_PATH: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[A-Za-z0-9_-]+$").expect("valid pattern"));

impl Server {
    /// Publishes a term's files when a request finds them missing (a fresh
    /// disk). Unknown schools and terms never collected are left alone, so
    /// unknown paths stay cheap 404s.
    pub(crate) async fn rebuild_snapshot(&self, inst: &str, term: &str) -> bool {
        if self.inst(inst).is_none() {
            return false;
        }
        let known = self
            .db
            .fetch_one("SELECT COUNT(*) FROM terms WHERE inst=$1 AND id=$2 AND updated_at>0", &[&inst, &term])
            .await
            .and_then(|row| Ok(row.try_get::<_, i64>(0)?));
        if !matches!(known, Ok(n) if n > 0) {
            return false;
        }
        if let Err(err) = self.write_snapshot(inst, term).await {
            tracing::warn!(%inst, %term, err = %format!("{err:#}"), "catalog rebuild on request failed");
            return false;
        }
        true
    }
}

/// `GET /catalog/{inst}/{file}`: a published, content-addressed snapshot
/// object `<term>.<structure|seats|desc>.<sha256>.json`, rebuilt on demand,
/// in the best prepared encoding, immutable, with an ETag of
/// `"HASH-ENCODING"`. `X-Honorum-Store-Encoded: 1` (the Cloudflare Worker)
/// receives the encoded bytes as `application/octet-stream` with
/// `X-Honorum-Stored-Encoding`. Other names are 404.
pub(crate) async fn handle_catalog(
    State(s): State<Arc<Server>>,
    Path((inst, file)): Path<(String, String)>,
    request: Request,
) -> ApiResult {
    let not_found = || Ok(plain_error(HeaderMap::new(), StatusCode::NOT_FOUND, "404 page not found\n"));
    let Some(found) = CATALOG_FILENAME.captures(&file) else { return not_found() };
    if !INSTITUTION_PATH.is_match(&inst) {
        return not_found();
    }
    let mut filename = s.data_dir.join("catalog").join(&inst).join(&file);
    let mut info = tokio::fs::metadata(&filename).await;
    if info.is_err() && s.rebuild_snapshot(&inst, &found[1]).await {
        info = tokio::fs::metadata(&filename).await;
    }
    let info = match info {
        Ok(info) if info.is_file() => info,
        _ => return not_found(),
    };
    let headers = request.headers();
    let accept = header_str(headers, header::ACCEPT_ENCODING);
    let mut encoding = "";
    for candidate in accepted_encodings(accept) {
        let compressed = with_suffix(&filename, if candidate == "br" { ".br" } else { ".gz" });
        if tokio::fs::metadata(&compressed).await.is_ok_and(|m| m.is_file()) {
            encoding = candidate;
            filename = compressed;
            break;
        }
    }
    if encoding.is_empty() && encoding_quality(accept, "identity") == 0.0 {
        return Ok(plain_error(HeaderMap::new(), StatusCode::NOT_ACCEPTABLE, "no acceptable encoding\n"));
    }
    let mut out = HeaderMap::new();
    out.insert(header::CONTENT_TYPE, HeaderValue::from_static("application/json"));
    out.insert(header::VARY, HeaderValue::from_static("Accept-Encoding, X-Honorum-Store-Encoded"));
    out.insert("x-raw-length", HeaderValue::from(info.len()));
    let store_encoded =
        !encoding.is_empty() && header_str(headers, header::HeaderName::from_static("x-honorum-store-encoded")) == "1";
    if !encoding.is_empty() {
        if store_encoded {
            // The Worker stores these prepared bytes as an opaque body: the
            // Cache API otherwise decodes Content-Encoding before serving a
            // cached response. It restores the JSON type and encoding only
            // when returning to clients.
            out.insert(header::CONTENT_TYPE, HeaderValue::from_static("application/octet-stream"));
            out.insert("x-honorum-stored-encoding", HeaderValue::from_static(encoding));
        } else {
            out.insert(header::CONTENT_ENCODING, HeaderValue::from_static(encoding));
        }
    }
    let etag = format!("\"{}-{encoding}\"", &found[2]);
    let cache = if store_encoded {
        "public, max-age=31536000, immutable, no-transform"
    } else {
        "public, max-age=31536000, immutable"
    };
    out.insert(header::CACHE_CONTROL, HeaderValue::from_static(cache));
    out.insert(header::ETAG, HeaderValue::from_str(&etag).expect("a hex ETag is a valid header"));
    if matches_etag(header_str(headers, header::IF_NONE_MATCH), &etag) {
        let mut response = Response::new(Body::empty());
        *response.status_mut() = StatusCode::NOT_MODIFIED;
        *response.headers_mut() = out;
        return Ok(response);
    }
    let Ok(body) = tokio::fs::read(&filename).await else { return not_found() };
    let modified = tokio::fs::metadata(&filename)
        .await
        .ok()
        .and_then(|m| m.modified().ok())
        .map(|t| DateTime::<Utc>::from(t).timestamp());
    Ok(serve_content(&request, out, modified, body.into()))
}

#[cfg(test)]
mod tests {
    use std::io::Read;

    use super::*;
    use crate::landing::tests::MapFs;
    use crate::snapshot::write_gzipped;
    use crate::test_support::test_server;

    struct Reply {
        status: StatusCode,
        headers: HeaderMap,
        body: bytes::Bytes,
    }

    impl Reply {
        fn header(&self, name: &str) -> &str {
            self.headers.get(name).and_then(|v| v.to_str().ok()).unwrap_or_default()
        }
    }

    async fn collect(response: Response) -> Reply {
        let (parts, body) = response.into_parts();
        let body = http_body_util::BodyExt::collect(body).await.unwrap().to_bytes();
        Reply { status: parts.status, headers: parts.headers, body }
    }

    fn request(method: &str, path: &str, headers: &[(&str, &str)]) -> Request {
        let mut request = axum::http::Request::builder().method(method).uri(path);
        for (k, v) in headers {
            request = request.header(*k, *v);
        }
        request.body(Body::empty()).unwrap()
    }

    /// Calls the catalog handler directly, without the router.
    async fn catalog(s: &Arc<Server>, file: &str, method: &str, headers: &[(&str, &str)]) -> Reply {
        let response = handle_catalog(
            State(s.clone()),
            Path(("ou".to_owned(), file.to_owned())),
            request(method, &format!("/catalog/ou/{file}"), headers),
        )
        .await
        .unwrap_or_else(axum::response::IntoResponse::into_response);
        collect(response).await
    }

    fn decode(encoding: &str, body: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        match encoding {
            "br" => brotli::Decompressor::new(body, 4096).read_to_end(&mut out).unwrap(),
            "gzip" => flate2::read::GzDecoder::new(body).read_to_end(&mut out).unwrap(),
            _ => return body.to_vec(),
        };
        out
    }

    #[tokio::test]
    async fn published_catalog_encodings_and_caching() {
        let t = test_server().await;
        let s = &t.server;
        let dir = s.data_dir.join("catalog").join("ou");
        std::fs::create_dir_all(&dir).unwrap();
        let raw = br#"{"courses":[],"test":"preserve data"}"#;
        let name = format!("202610.structure.{}.json", "a".repeat(64));
        write_gzipped(&dir.join(&name), raw).unwrap();
        for (accept, encoding) in [
            ("br,gzip", "br"),
            ("gzip", "gzip"),
            ("br;q=0,gzip", "gzip"),
            ("gzip;q=0.5,br;q=0.1", "gzip"),
            ("br;q=0,gzip;q=0", ""),
            ("", ""),
        ] {
            let result = catalog(s, &name, "GET", &[("accept-encoding", accept)]).await;
            assert!(
                result.status == 200
                    && result.header("content-encoding") == encoding
                    && result.header("cache-control").contains("immutable"),
                "{accept}: {} {:?}",
                result.status,
                result.headers
            );
            assert_eq!(decode(encoding, &result.body), raw, "decoded body");
            let etag = result.header("etag").to_owned();
            let unchanged = catalog(s, &name, "GET", &[("accept-encoding", accept), ("if-none-match", &etag)]).await;
            assert!(unchanged.status == 304 && unchanged.body.is_empty(), "immutable validator not honored");
        }
        let refused = catalog(s, &name, "GET", &[("accept-encoding", "*;q=0")]).await;
        assert_eq!(refused.status, 406, "encoding exclusion");
        let escaped = catalog(s, "../secret.json", "GET", &[("accept-encoding", "*;q=0")]).await;
        assert_eq!(escaped.status, 404, "accepted non-public filename");
    }

    #[tokio::test]
    async fn published_catalog_opaque_encoded_storage() {
        let t = test_server().await;
        let s = &t.server;
        let dir = s.data_dir.join("catalog").join("ou");
        std::fs::create_dir_all(&dir).unwrap();
        let raw = br#"{"courses":[],"test":"preserve publication-time compressed bytes"}"#;
        let name = format!("202610.structure.{}.json", "a".repeat(64));
        write_gzipped(&dir.join(&name), raw).unwrap();
        for encoding in ["br", "gzip"] {
            let suffix = if encoding == "gzip" { ".gz" } else { ".br" };
            let prepared = std::fs::read(with_suffix(&dir.join(&name), suffix)).unwrap();
            let normal = catalog(s, &name, "GET", &[("accept-encoding", encoding)]).await;
            let store = [("accept-encoding", encoding), ("x-honorum-store-encoded", "1")];
            let stored = catalog(s, &name, "GET", &store).await;
            assert!(
                stored.status == 200 && stored.body == prepared,
                "{encoding}: stored body differs: {}",
                stored.status
            );
            assert!(
                stored.header("content-encoding").is_empty()
                    && stored.header("content-type") == "application/octet-stream"
                    && stored.header("x-honorum-stored-encoding") == encoding,
                "{encoding}: opaque encoding headers: {:?}",
                stored.headers
            );
            assert!(
                stored.header("content-length") == prepared.len().to_string()
                    && stored.header("x-raw-length") == raw.len().to_string(),
                "{encoding}: body length headers: {:?}",
                stored.headers
            );
            assert!(
                stored.header("etag") == normal.header("etag")
                    && stored.header("vary").contains("X-Honorum-Store-Encoded")
                    && stored.header("cache-control").contains("no-transform"),
                "{encoding}: opaque cache headers: {:?}",
                stored.headers
            );
            assert!(
                normal.header("vary").contains("X-Honorum-Store-Encoded")
                    && normal.header("x-honorum-stored-encoding").is_empty()
                    && normal.header("content-encoding") == encoding
                    && normal.header("content-type") == "application/json",
                "{encoding}: ordinary response changed: {:?}",
                normal.headers
            );
            let head = catalog(s, &name, "HEAD", &store).await;
            assert!(
                head.status == 200
                    && head.body.is_empty()
                    && head.header("content-length") == stored.header("content-length")
                    && head.header("x-honorum-stored-encoding") == encoding,
                "{encoding}: opaque HEAD: {} {:?}",
                head.status,
                head.headers
            );
            let etag = stored.header("etag").to_owned();
            let unchanged = catalog(
                s,
                &name,
                "GET",
                &[("accept-encoding", encoding), ("x-honorum-store-encoded", "1"), ("if-none-match", &etag)],
            )
            .await;
            assert!(
                unchanged.status == 304
                    && unchanged.body.is_empty()
                    && unchanged.header("content-encoding").is_empty()
                    && unchanged.header("x-honorum-stored-encoding") == encoding,
                "{encoding}: opaque validator: {} {:?}",
                unchanged.status,
                unchanged.headers
            );
            let invalid =
                catalog(s, &name, "GET", &[("accept-encoding", encoding), ("x-honorum-store-encoded", "true")]).await;
            assert!(
                invalid.header("content-encoding") == encoding
                    && invalid.header("x-honorum-stored-encoding").is_empty(),
                "{encoding}: unsupported storage request accepted: {:?}",
                invalid.headers
            );
            let plain =
                catalog(s, &name, "GET", &[("accept-encoding", "identity"), ("x-honorum-store-encoded", "1")]).await;
            assert!(
                plain.status == 200
                    && plain.body.as_ref() == raw
                    && plain.header("content-type") == "application/json"
                    && plain.header("x-honorum-stored-encoding").is_empty(),
                "{encoding}: identity fallback: {} {:?}",
                plain.status,
                plain.headers
            );
        }
    }

    #[tokio::test]
    async fn standalone_static_compression() {
        let raw = b"<!doctype html><title>Saved app</title>";
        let css = b"body { color: red }";
        let mut prepared = Vec::new();
        {
            let mut writer = brotli::CompressorWriter::new(&mut prepared, 4096, 11, 22);
            writer.write_all(css).unwrap();
        }
        let site = StaticSite::new(Arc::new(MapFs::new(&[
            ("index.html", &raw[..]),
            ("app.css", &css[..]),
            ("app.css.br", &prepared[..]),
        ])));
        // The build's Brotli copy wins where it exists and the browser accepts it.
        for (path, accept, encoding) in [
            ("/app.css", "gzip, br", "br"),
            ("/app.css", "br;q=0.5, gzip", "gzip"),
            ("/", "br, gzip", "gzip"),
            ("/app.css", "", ""),
        ] {
            let result = collect(site.serve(request("GET", path, &[("accept-encoding", accept)])).await).await;
            assert!(
                result.status == 200
                    && result.header("content-encoding") == encoding
                    && result.header("vary") == "Accept-Encoding",
                "{path} with {accept:?}: {} {:?}",
                result.status,
                result.headers
            );
            if encoding == "br" {
                assert_eq!(result.body, prepared, "prepared copy changed");
            }
        }
        let result = collect(site.serve(request("GET", "/", &[("accept-encoding", "gzip")])).await).await;
        assert!(result.status == 200 && result.header("content-encoding") == "gzip", "static: {:?}", result.headers);
        assert_eq!(decode("gzip", &result.body), raw, "body");
        let plain = collect(site.serve(request("GET", "/", &[])).await).await;
        assert!(plain.body.as_ref() == raw && plain.header("content-encoding").is_empty(), "identity response changed");
    }
}
