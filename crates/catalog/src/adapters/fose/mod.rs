//! CourseLeaf CLSS class-search sites ("FOSE"), which many universities host
//! at `https://classes.<school>.edu`.
//!
//! Terms come from the `srcdb` options on the site's index page; the first
//! three real terms are offered. One POST to `/api/?page=fose&route=search`
//! returns every section in a term. Seat counts come only from a
//! per-section POST to `route=details`, so the catalog marks seats as
//! unpublished (`-1`, or `0` when the section is full or cancelled) and
//! availability polls fetch details for watched sections only, pausing
//! 100 ms between them. The section-to-course routing that detail requests
//! need is the adapter's persisted state. Each request times out after
//! three minutes.

use std::collections::{BTreeMap, HashMap};
use std::sync::{LazyLock, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use regex::Regex;
use reqwest::StatusCode;
use reqwest::header::CONTENT_TYPE;
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

use super::{decode_error, null_default as nd, status_text, unescape_html};
use crate::{Adapter, AdapterError, AdapterState, Http, Meeting, Seats, Section, Term};

const PAUSE: Duration = Duration::from_millis(100);
const TIMEOUT: Duration = Duration::from_secs(180);

static RE_TERM: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"<option value="([0-9]{4,6})"[^>]*>([^<]+)<"#).expect("valid regex"));

/// Section routing by term, then CRN, to the course code.
type Routing = BTreeMap<String, BTreeMap<String, String>>;

/// A CourseLeaf class-search site.
pub(super) struct Fose {
    /// The site root without a trailing slash, such as
    /// `https://classes.oregonstate.edu`.
    pub base: String,
    http: Http,
    code_of: Mutex<Routing>,
}

impl Fose {
    pub(super) fn new(http: Http, base: &str) -> Self {
        Self { base: base.trim_end_matches('/').to_owned(), http, code_of: Mutex::new(Routing::new()) }
    }
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct SearchResponse {
    #[serde(deserialize_with = "nd")]
    results: Vec<SearchResult>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct SearchResult {
    #[serde(deserialize_with = "nd")]
    code: String,
    #[serde(deserialize_with = "nd")]
    title: String,
    #[serde(deserialize_with = "nd")]
    crn: String,
    #[serde(deserialize_with = "nd")]
    no: String,
    #[serde(deserialize_with = "nd")]
    schd: String,
    #[serde(deserialize_with = "nd")]
    instr: String,
    #[serde(deserialize_with = "nd")]
    stat: String,
    /// A JSON array encoded as a string.
    #[serde(rename = "meetingTimes", deserialize_with = "nd")]
    meeting_times: String,
    #[serde(rename = "start_date", deserialize_with = "nd")]
    start_date: String,
    #[serde(rename = "end_date", deserialize_with = "nd")]
    end_date: String,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Details {
    #[serde(rename = "ssbsect_seats_avail", deserialize_with = "nd")]
    avail: String,
    #[serde(rename = "max_enroll", deserialize_with = "nd")]
    max_enroll: String,
    #[serde(rename = "ssbsect_wait_count", deserialize_with = "nd")]
    wait_count: String,
}

impl Fose {
    fn convert(&self, r: &SearchResult) -> Section {
        let mut s = Section {
            id: r.crn.clone(),
            course: r.code.clone(),
            subject: r.code.split_whitespace().next().unwrap_or("").to_owned(),
            title: r.title.clone(),
            number: r.no.clone(),
            kind: type_code(&r.schd),
            available: -1,
            start: r.start_date.clone(),
            end: r.end_date.clone(),
            source_url: format!("{}/", self.base),
            ..Default::default()
        };
        if !r.instr.is_empty() {
            s.instructors.push(r.instr.clone());
        }
        // Full or cancelled; other codes carry no seat information.
        if r.stat == "F" || r.stat == "C" {
            s.available = 0;
        }
        // Meetings with the same times on several days merge into one
        // pattern, in first-seen order.
        let raw: Vec<Value> = serde_json::from_str(&r.meeting_times).unwrap_or_default();
        let mut by_time: HashMap<(i64, i64), String> = HashMap::new();
        let mut order = Vec::new();
        for m in &raw {
            let field = |k: &str| m.get(k).and_then(Value::as_str).unwrap_or("").to_owned();
            let day: i64 = field("meet_day").parse().unwrap_or(0);
            let (start, end) = (hhmm(&field("start_time")), hhmm(&field("end_time")));
            if !(0..=6).contains(&day) || end <= start {
                continue;
            }
            let key = (start, end);
            let days = by_time.entry(key).or_insert_with(|| {
                order.push(key);
                String::new()
            });
            days.push(char::from(b"MTWRFSU"[day as usize]));
        }
        for key in order {
            s.meetings.push(Meeting {
                days: by_time.remove(&key).unwrap_or_default(),
                start: key.0,
                end: key.1,
                location: String::new(),
            });
        }
        s
    }

    async fn get(&self, url: &str) -> Result<String, AdapterError> {
        let (status, body) = self.http.send_any(self.http.get(url), TIMEOUT).await?;
        if status != StatusCode::OK {
            return Err(AdapterError::new(format!("fose: {url} -> {}", status_text(status))));
        }
        Ok(String::from_utf8_lossy(&body).into_owned())
    }

    async fn post<T: DeserializeOwned>(&self, route: &str, body: Value) -> Result<T, AdapterError> {
        let url = format!("{}/api/?page=fose&route={route}", self.base);
        let request = self.http.post(&url).header(CONTENT_TYPE, "application/json").body(body.to_string());
        let (status, body) = self.http.send_any(request, TIMEOUT).await?;
        if status != StatusCode::OK {
            return Err(AdapterError::new(format!("fose: {route} -> {}", status_text(status))));
        }
        serde_json::from_slice(&body).map_err(decode_error)
    }
}

/// Reads an integer `HMM` or `HHMM` time as minutes after midnight; zero
/// when it is not a non-negative integer.
fn hhmm(s: &str) -> i64 {
    match s.parse::<i64>() {
        Ok(n) if n >= 0 => n / 100 * 60 + n % 100,
        _ => 0,
    }
}

fn type_code(schd: &str) -> String {
    match schd.to_uppercase().as_str() {
        "" | "A" | "L" | "LEC" => "LEC".into(),
        "B" | "LAB" => "LAB".into(),
        "D" | "DIS" | "R" | "REC" => "DIS".into(),
        "S" | "SEM" => "SEM".into(),
        other => other.to_owned(),
    }
}

#[async_trait]
impl Adapter for Fose {
    /// Returns the first three terms on the index page, skipping the
    /// all-terms entry and combined options.
    async fn terms(&self) -> Result<Vec<Term>, AdapterError> {
        let body = self.get(&format!("{}/", self.base)).await?;
        let mut out = Vec::new();
        for m in RE_TERM.captures_iter(&body) {
            let name = unescape_html(&m[2]).trim().to_owned();
            if &m[1] == "999999" || name.contains('&') || name.starts_with("All") || name.starts_with("Any") {
                continue;
            }
            out.push(Term { id: m[1].to_owned(), name, ..Default::default() });
            if out.len() == 3 {
                break;
            }
        }
        if out.is_empty() {
            return Err(AdapterError::new(format!("fose: no terms found at {}", self.base)));
        }
        Ok(out)
    }

    async fn catalog(&self, term: &str) -> Result<Vec<Section>, AdapterError> {
        let res: SearchResponse = self.post("search", json!({"other": {"srcdb": term}, "criteria": []})).await?;
        let mut codes = BTreeMap::new();
        let mut out = Vec::with_capacity(res.results.len());
        for r in &res.results {
            let s = self.convert(r);
            codes.insert(s.id.clone(), r.code.clone());
            out.push(s);
        }
        self.code_of.lock().expect("fose routing lock").insert(term.to_owned(), codes);
        Ok(out)
    }

    /// Fetches the detail record for each watched section the last catalog
    /// recorded; unknown sections are skipped.
    async fn availability(&self, term: &str, ids: &[String]) -> Result<Vec<Seats>, AdapterError> {
        let mut out = Vec::new();
        for (i, id) in ids.iter().enumerate() {
            let code = {
                let routing = self.code_of.lock().expect("fose routing lock");
                routing.get(term).and_then(|m| m.get(id)).cloned().unwrap_or_default()
            };
            if code.is_empty() {
                continue;
            }
            if i > 0 {
                tokio::time::sleep(PAUSE).await;
            }
            let body = json!({"group": format!("code:{code}"), "key": format!("crn:{id}"), "srcdb": term, "matched": format!("crn:{id}")});
            let d: Details = self.post("details", body).await?;
            out.push(Seats {
                id: id.clone(),
                available: d.avail.parse().unwrap_or(0),
                capacity: d.max_enroll.parse().unwrap_or(0),
                waitlist: d.wait_count.parse().unwrap_or(0),
            });
        }
        Ok(out)
    }

    fn version(&self) -> &str {
        "fose-1"
    }

    fn state(&self) -> Option<&dyn AdapterState> {
        Some(self)
    }
}

/// The persisted state is the section-to-course routing, as JSON
/// `{term: {crn: code}}`.
impl AdapterState for Fose {
    fn save_state(&self) -> Result<Vec<u8>, AdapterError> {
        let routing = self.code_of.lock().expect("fose routing lock");
        serde_json::to_vec(&*routing).map_err(|err| AdapterError::new(err.to_string()))
    }

    fn load_state(&self, data: &[u8]) -> Result<(), AdapterError> {
        let state: Option<Routing> = serde_json::from_slice(data).map_err(|err| AdapterError::new(err.to_string()))?;
        *self.code_of.lock().expect("fose routing lock") = state.unwrap_or_default();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::testserver::{Response, serve};

    const INDEX: &[u8] = include_bytes!("fixture_index.html");
    const SEARCH: &[u8] = include_bytes!("fixture_search.json");
    const DETAILS: &[u8] = include_bytes!("fixture_details.json");

    #[tokio::test]
    async fn contract() {
        let base = serve(|r| {
            if r.path == "/" {
                return Response::ok(INDEX);
            }
            let body = r.body_text();
            match r.q("route") {
                "search" if !body.contains(r#""srcdb":"202701""#) => Response::error(400, "bad term"),
                "search" => Response::ok(SEARCH),
                "details" if !body.contains(r#""key":"crn:12047""#) => Response::error(400, "bad key"),
                "details" => Response::ok(DETAILS),
                _ => Response::error(404, "bad route"),
            }
        })
        .await;
        let f = Fose::new(Http::unpaced(), &base);
        let terms = f.terms().await.unwrap();
        assert!(terms.len() == 3 && terms[0].id == "202603" && terms[2].name == "Fall 2026", "terms: {terms:?}");
        let secs = f.catalog("202701").await.unwrap();
        assert_eq!(secs.len(), 4, "catalog");
        let s = &secs[3];
        assert!(
            s.id == "12047"
                && s.course == "CS 162"
                && s.number == "011"
                && s.available == -1
                && s.start == "2026-09-23"
                && s.instructors[0] == "A. Guyer",
            "bad section: {s:?}"
        );
        assert!(
            s.meetings.len() == 1
                && s.meetings[0].days == "T"
                && s.meetings[0].start == 480
                && s.meetings[0].end == 590,
            "bad meeting: {:?}",
            s.meetings
        );
        let seats = f.availability("202701", &["12047".into(), "nope".into()]).await.unwrap();
        assert!(seats.len() == 1 && seats[0].available == 1 && seats[0].capacity == 20, "availability: {seats:?}");
    }

    #[test]
    fn meeting_days_merge() {
        let f = Fose::new(Http::unpaced(), "http://x");
        let s = f.convert(&SearchResult {
            code: "CS 101".into(),
            crn: "1".into(),
            meeting_times: r#"[{"meet_day":"0","start_time":"1100","end_time":"1150"},{"meet_day":"2","start_time":"1100","end_time":"1150"},{"meet_day":"4","start_time":"1100","end_time":"1150"}]"#.into(),
            ..Default::default()
        });
        assert!(
            s.meetings.len() == 1 && s.meetings[0].days == "MWF" && s.meetings[0].start == 660,
            "expected merged MWF meeting, got {:?}",
            s.meetings
        );
        let full =
            f.convert(&SearchResult { code: "CS 1".into(), crn: "2".into(), stat: "F".into(), ..Default::default() });
        assert_eq!(full.available, 0, "stat F should read as full");
    }
}
