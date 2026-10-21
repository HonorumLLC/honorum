//! School integrations. Each module adapts one provider's public class
//! search; see `docs/adapters.md` for provider limits.
//!
//! Every adapter sends its requests through the shared [`Http`] client, so
//! all adapters that reach one host share its pacing budget. Each request
//! carries the timeout the provider's adapter documents.

use std::sync::Arc;

use chrono::NaiveDate;
use reqwest::StatusCode;

use crate::{Adapter, AdapterError, Http};

mod banner;
mod civitas;
pub mod demo;
mod fose;
mod illinois;
mod ou;
mod rutgers;
mod tamu;
mod uci;

pub(crate) use crate::types::null_default;

/// Builds the adapter a school's configuration names. `url` configures
/// `banner` and `fose`; `env` configures `civitas` (the subdomain) and
/// `rutgers` (the campus). Returns `None` for an unknown kind.
pub fn build(kind: &str, url: &str, env: &str, http: &Http) -> Option<Arc<dyn Adapter>> {
    let http = http.clone();
    let adapter: Arc<dyn Adapter> = match kind {
        "demo" => Arc::new(demo::Demo::new()),
        "banner" => Arc::new(banner::Banner::new(http, url)),
        "ou" => Arc::new(ou::Ou::new(http)),
        "tamu" => Arc::new(tamu::Tamu::new(http)),
        "civitas" => Arc::new(civitas::Civitas::new(http, env)),
        "fose" => Arc::new(fose::Fose::new(http, url)),
        "illinois" => Arc::new(illinois::Illinois::new(http)),
        "uci" => Arc::new(uci::Uci::new(http)),
        "rutgers" => Arc::new(rutgers::Rutgers::new(http, env)),
        _ => return None,
    };
    Some(adapter)
}

/// Formats a status as `404 Not Found`, the form provider errors report.
pub(crate) fn status_text(status: StatusCode) -> String {
    match status.canonical_reason() {
        Some(reason) => format!("{} {reason}", status.as_u16()),
        None => status.as_u16().to_string(),
    }
}

/// Reads exactly `n` ASCII digits from the front of `s`.
fn fixed_digits(s: &str, n: usize) -> Option<(u32, &str)> {
    let head = s.get(..n)?;
    if !head.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    Some((head.parse().ok()?, &s[n..]))
}

/// Parses a `MM/DD/YYYY` date. Every field has a fixed width and the date
/// must exist on the calendar.
pub(crate) fn parse_mdy(s: &str) -> Option<NaiveDate> {
    let (month, rest) = fixed_digits(s, 2)?;
    let rest = rest.strip_prefix('/')?;
    let (day, rest) = fixed_digits(rest, 2)?;
    let rest = rest.strip_prefix('/')?;
    let (year, rest) = fixed_digits(rest, 4)?;
    if !rest.is_empty() {
        return None;
    }
    NaiveDate::from_ymd_opt(i32::try_from(year).ok()?, month, day)
}

/// Rewrites a `MM/DD/YYYY` date as `YYYY-MM-DD`; empty when it does not parse.
pub(crate) fn mdy(s: &str) -> String {
    parse_mdy(s).map(iso).unwrap_or_default()
}

/// Formats a date as `YYYY-MM-DD`.
pub(crate) fn iso(date: NaiveDate) -> String {
    date.format("%Y-%m-%d").to_string()
}

/// Upper-cases the first three bytes of a provider's component name, the
/// fallback section type for unrecognized components.
pub(crate) fn upper_prefix3(s: &str) -> String {
    let end = s.len().min(3);
    String::from_utf8_lossy(&s.as_bytes()[..end]).to_uppercase()
}

/// Decodes HTML character references such as `&amp;` in a text fragment.
pub(crate) fn unescape_html(s: &str) -> String {
    if !s.contains('&') {
        return s.to_owned();
    }
    scraper::Html::parse_fragment(s).root_element().text().collect()
}

/// Wraps a JSON decoding failure in an adapter error.
pub(crate) fn decode_error(err: serde_json::Error) -> AdapterError {
    AdapterError::new(err.to_string())
}

/// Element lookups with fixed field rules: names match
/// local names, a path like `a>b` walks direct children, and a scalar field
/// takes the last matching element's own character data.
pub(crate) mod xml {
    use roxmltree::{Document, Node, ParsingOptions};

    /// Parses a document. A DOCTYPE is accepted and not fetched.
    pub(crate) fn parse(text: &str) -> Result<Document<'_>, roxmltree::Error> {
        Document::parse_with_options(text, ParsingOptions { allow_dtd: true, ..Default::default() })
    }

    /// Every element reached from `node` by following `path` through direct
    /// children, in document order.
    pub(crate) fn all<'a, 'i>(node: Node<'a, 'i>, path: &[&str]) -> Vec<Node<'a, 'i>> {
        let mut level = vec![node];
        for name in path {
            level = level
                .into_iter()
                .flat_map(|n| n.children().filter(|c| c.is_element() && c.tag_name().name() == *name))
                .collect();
        }
        level
    }

    /// The character data directly inside an element, without nested
    /// elements' text.
    pub(crate) fn own_text(node: Node<'_, '_>) -> String {
        node.children().filter(|c| c.is_text()).filter_map(|c| c.text()).collect()
    }

    /// The own text of the last element at `path`, or empty when none.
    pub(crate) fn text(node: Node<'_, '_>, path: &[&str]) -> String {
        all(node, path).last().map(|n| own_text(*n)).unwrap_or_default()
    }

    /// An attribute by local name, or empty.
    pub(crate) fn attr(node: Node<'_, '_>, name: &str) -> String {
        node.attributes().find(|a| a.name() == name).map(|a| a.value().to_owned()).unwrap_or_default()
    }

    /// Reads an integer field: empty text is zero and other text must be an
    /// integer after trimming spaces.
    pub(crate) fn int(node: Node<'_, '_>, path: &[&str]) -> Result<i64, String> {
        let raw = text(node, path);
        if raw.is_empty() {
            return Ok(0);
        }
        raw.trim().parse().map_err(|_| format!("strconv.ParseInt: parsing {:?}: invalid syntax", raw.trim()))
    }
}

#[cfg(test)]
pub(crate) mod testserver {
    //! A minimal HTTP/1.1 server for fixture tests. Each connection carries
    //! one request; the handler sees the method, path, decoded query,
    //! lower-cased headers, and body.

    use std::collections::HashMap;
    use std::sync::Arc;

    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    pub(crate) struct Request {
        pub method: String,
        pub path: String,
        pub query: HashMap<String, String>,
        pub headers: HashMap<String, String>,
        pub body: Vec<u8>,
    }

    impl Request {
        /// The first value of a query parameter, or empty.
        pub(crate) fn q(&self, key: &str) -> &str {
            self.query.get(key).map(String::as_str).unwrap_or("")
        }

        /// A request header, or empty.
        pub(crate) fn header(&self, key: &str) -> &str {
            self.headers.get(&key.to_ascii_lowercase()).map(String::as_str).unwrap_or("")
        }

        pub(crate) fn body_text(&self) -> String {
            String::from_utf8_lossy(&self.body).into_owned()
        }
    }

    pub(crate) struct Response {
        pub status: u16,
        pub headers: Vec<(String, String)>,
        pub body: Vec<u8>,
    }

    impl Response {
        pub(crate) fn ok(body: impl Into<Vec<u8>>) -> Self {
            Self { status: 200, headers: Vec::new(), body: body.into() }
        }

        pub(crate) fn error(status: u16, body: &str) -> Self {
            Self { status, headers: Vec::new(), body: body.as_bytes().to_vec() }
        }

        pub(crate) fn with_header(mut self, key: &str, value: &str) -> Self {
            self.headers.push((key.to_owned(), value.to_owned()));
            self
        }
    }

    /// Serves `handler` on a loopback port and returns the base URL, such as
    /// `http://127.0.0.1:5000`. The server runs until the test runtime ends.
    pub(crate) async fn serve<F>(handler: F) -> String
    where
        F: Fn(Request) -> Response + Send + Sync + 'static,
    {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let handler = Arc::new(handler);
        tokio::spawn(async move {
            loop {
                let Ok((mut socket, _)) = listener.accept().await else { return };
                let handler = handler.clone();
                tokio::spawn(async move {
                    let Some(request) = read_request(&mut socket).await else { return };
                    let response = handler(request);
                    let mut head = format!(
                        "HTTP/1.1 {} X\r\ncontent-length: {}\r\nconnection: close\r\n",
                        response.status,
                        response.body.len()
                    );
                    for (k, v) in &response.headers {
                        head.push_str(&format!("{k}: {v}\r\n"));
                    }
                    head.push_str("\r\n");
                    let _ = socket.write_all(head.as_bytes()).await;
                    let _ = socket.write_all(&response.body).await;
                    let _ = socket.shutdown().await;
                });
            }
        });
        format!("http://{addr}")
    }

    async fn read_request(socket: &mut tokio::net::TcpStream) -> Option<Request> {
        let mut buf = Vec::new();
        let mut chunk = [0u8; 8192];
        let header_end = loop {
            if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                break i;
            }
            let n = socket.read(&mut chunk).await.ok()?;
            if n == 0 {
                return None;
            }
            buf.extend_from_slice(&chunk[..n]);
        };
        let head = String::from_utf8_lossy(&buf[..header_end]).into_owned();
        let mut lines = head.split("\r\n");
        let mut start = lines.next()?.split(' ');
        let method = start.next()?.to_owned();
        let target = start.next()?.to_owned();
        let mut headers = HashMap::new();
        for line in lines {
            if let Some((k, v)) = line.split_once(':') {
                headers.insert(k.trim().to_ascii_lowercase(), v.trim().to_owned());
            }
        }
        let length: usize = headers.get("content-length").and_then(|v| v.parse().ok()).unwrap_or(0);
        let mut body = buf[header_end + 4..].to_vec();
        while body.len() < length {
            let n = socket.read(&mut chunk).await.ok()?;
            if n == 0 {
                break;
            }
            body.extend_from_slice(&chunk[..n]);
        }
        let (path, raw_query) = target.split_once('?').unwrap_or((&target, ""));
        let mut query = HashMap::new();
        for (k, v) in url::form_urlencoded::parse(raw_query.as_bytes()) {
            query.entry(k.into_owned()).or_insert_with(|| v.into_owned());
        }
        Some(Request { method, path: path.to_owned(), query, headers, body })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mdy_requires_fixed_width_calendar_dates() {
        assert_eq!(mdy("12/10/2026"), "2026-12-10");
        assert_eq!(mdy("02/30/2026"), "");
        assert_eq!(mdy("2/3/2026"), "");
        assert_eq!(mdy("12/10/2026 "), "");
        assert_eq!(mdy(""), "");
    }

    #[test]
    fn build_maps_configured_kinds() {
        let http = Http::unpaced();
        for (kind, version) in [
            ("demo", "demo-1"),
            ("banner", "banner9-1"),
            ("ou", "ou-classnav-1"),
            ("tamu", "tamu-howdy-1"),
            ("civitas", "civitas-1"),
            ("fose", "fose-1"),
            ("illinois", "illinois-cis-1"),
            ("uci", "uci-websoc-1"),
            ("rutgers", "rutgers-soc-1"),
        ] {
            let adapter = build(kind, "https://example.edu", "x", &http).expect(kind);
            assert_eq!(adapter.version(), version);
        }
        assert!(build("nope", "", "", &http).is_none());
    }

    #[test]
    fn unescape_html_decodes_references() {
        assert_eq!(unescape_html("A &amp; B"), "A & B");
        assert_eq!(unescape_html("Fall 2026"), "Fall 2026");
        assert_eq!(unescape_html("&lt;x&gt; &#39;"), "<x> '");
    }
}
