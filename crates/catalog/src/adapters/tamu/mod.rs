//! Texas A&M's public class search. The Howdy portal's web component calls
//! an unauthenticated JSON API that returns a whole term at once.
//!
//! Terms come from `GET /api/all-terms`; the three most recent College
//! Station terms are offered. Sections come from `POST /api/course-sections`
//! with `{"startRow":0,"endRow":0,"termCode":...,"publicSearch":"Y"}`. The
//! public variant flags each section open or closed (`STUSEAT_OPEN`) but
//! hides exact seat counts, so sections read as open (`1`) or closed (`0`).
//! Catalog refreshes and availability checks both need the whole term, so a
//! term's response is reused for one minute, and seat polls run at most
//! every five minutes. Dated "Examination" rows become exam sittings. Each
//! request times out after three minutes.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use reqwest::StatusCode;
use reqwest::header::{ACCEPT, CONTENT_TYPE};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

use super::{decode_error, mdy, null_default as nd, status_text};
use crate::{Adapter, AdapterError, Exam, Http, Meeting, Seats, Section, Term};

const SEARCH_URL: &str = "https://howdyportal.tamu.edu/uPortal/p/public-class-search-ui.ctf1/max/render.uP";
const TIMEOUT: Duration = Duration::from_secs(180);
const CACHE_FOR: Duration = Duration::from_secs(60);

/// Texas A&M Howdy.
pub(super) struct Tamu {
    /// The API host. Tests point it at a fixture server.
    pub base: String,
    http: Http,
    /// Term dumps by term code, with the time they were fetched.
    cache: Mutex<HashMap<String, (Instant, Arc<Vec<Section>>)>>,
}

impl Tamu {
    pub(super) fn new(http: Http) -> Self {
        Self { base: "https://howdy.tamu.edu".into(), http, cache: Mutex::new(HashMap::new()) }
    }

    /// Returns a term's sections, reusing a response younger than a minute.
    async fn dump(&self, term: &str) -> Result<Arc<Vec<Section>>, AdapterError> {
        if let Some((at, secs)) = self.cache.lock().expect("tamu cache lock").get(term)
            && at.elapsed() < CACHE_FOR
        {
            return Ok(secs.clone());
        }
        let body = json!({"startRow": 0, "endRow": 0, "termCode": term, "publicSearch": "Y"});
        let rows: Option<Vec<Row>> = self.call("/api/course-sections", Some(body)).await?;
        let secs = Arc::new(rows.unwrap_or_default().into_iter().map(convert).collect::<Vec<_>>());
        self.cache.lock().expect("tamu cache lock").insert(term.to_owned(), (Instant::now(), secs.clone()));
        Ok(secs)
    }

    async fn call<T: DeserializeOwned>(&self, path: &str, body: Option<Value>) -> Result<T, AdapterError> {
        let url = format!("{}{path}", self.base);
        let request = match body {
            None => self.http.get(&url),
            Some(body) => {
                self.http.post(&url).header(CONTENT_TYPE, "application/json; charset=utf-8").body(body.to_string())
            }
        };
        let (status, body) = self.http.send_any(request.header(ACCEPT, "application/json"), TIMEOUT).await?;
        if status != StatusCode::OK {
            return Err(AdapterError::new(format!("tamu: {path} -> {}", status_text(status))));
        }
        serde_json::from_slice(&body).map_err(decode_error)
    }
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct RawTerm {
    #[serde(rename = "STVTERM_CODE", deserialize_with = "nd")]
    code: String,
    #[serde(rename = "STVTERM_DESC", deserialize_with = "nd")]
    desc: String,
    #[serde(rename = "STVTERM_START_DATE", deserialize_with = "nd")]
    start: String,
    #[serde(rename = "STVTERM_END_DATE", deserialize_with = "nd")]
    end: String,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Row {
    #[serde(rename = "SWV_CLASS_SEARCH_CRN", deserialize_with = "nd")]
    crn: String,
    #[serde(rename = "SWV_CLASS_SEARCH_TITLE", deserialize_with = "nd")]
    title: String,
    #[serde(rename = "SWV_CLASS_SEARCH_SUBJECT", deserialize_with = "nd")]
    subject: String,
    #[serde(rename = "SWV_CLASS_SEARCH_COURSE", deserialize_with = "nd")]
    course: String,
    #[serde(rename = "SWV_CLASS_SEARCH_SECTION", deserialize_with = "nd")]
    section: String,
    #[serde(rename = "SWV_CLASS_SEARCH_HOURS_LOW", deserialize_with = "nd")]
    hours: f64,
    #[serde(rename = "STUSEAT_OPEN", deserialize_with = "nd")]
    open: String,
    #[serde(rename = "SWV_CLASS_SEARCH_SCHD", deserialize_with = "nd")]
    schd: String,
    /// A JSON array of `{"NAME": ...}` encoded as a string.
    #[serde(rename = "SWV_CLASS_SEARCH_INSTRCTR_JSON", deserialize_with = "nd")]
    instructors: String,
    #[serde(rename = "SWV_CLASS_SEARCH_INST_TYPE", deserialize_with = "nd")]
    inst_type: String,
    /// Tags separated by `|`.
    #[serde(rename = "SWV_CLASS_SEARCH_ATTRIBUTES", deserialize_with = "nd")]
    attributes: String,
    /// A JSON array of meeting rows encoded as a string.
    #[serde(rename = "SWV_CLASS_SEARCH_JSON_CLOB", deserialize_with = "nd")]
    meetings: String,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct RawMeeting {
    #[serde(rename = "SSRMEET_SUN_DAY")]
    sun: Option<String>,
    #[serde(rename = "SSRMEET_MON_DAY")]
    mon: Option<String>,
    #[serde(rename = "SSRMEET_TUE_DAY")]
    tue: Option<String>,
    #[serde(rename = "SSRMEET_WED_DAY")]
    wed: Option<String>,
    #[serde(rename = "SSRMEET_THU_DAY")]
    thu: Option<String>,
    #[serde(rename = "SSRMEET_FRI_DAY")]
    fri: Option<String>,
    #[serde(rename = "SSRMEET_SAT_DAY")]
    sat: Option<String>,
    #[serde(rename = "SSRMEET_BEGIN_TIME", deserialize_with = "nd")]
    begin_time: String,
    #[serde(rename = "SSRMEET_END_TIME", deserialize_with = "nd")]
    end_time: String,
    #[serde(rename = "SSRMEET_START_DATE", deserialize_with = "nd")]
    start: String,
    #[serde(rename = "SSRMEET_END_DATE", deserialize_with = "nd")]
    finish: String,
    #[serde(rename = "SSRMEET_BLDG_CODE", deserialize_with = "nd")]
    bldg: String,
    #[serde(rename = "SSRMEET_ROOM_CODE", deserialize_with = "nd")]
    room: String,
    #[serde(rename = "SSRMEET_MTYP_CODE", deserialize_with = "nd")]
    kind: String,
}

/// Decodes a JSON array embedded in a string field. Malformed text yields
/// no elements and a malformed element yields its default.
fn embedded<T: DeserializeOwned + Default>(text: &str) -> Vec<T> {
    let items: Vec<Value> = serde_json::from_str(text).unwrap_or_default();
    items.into_iter().map(|v| serde_json::from_value(v).unwrap_or_default()).collect()
}

fn convert(r: Row) -> Section {
    let mut s = Section {
        id: r.crn,
        course: format!("{} {}", r.subject, r.course),
        subject: r.subject,
        title: r.title,
        number: r.section,
        kind: type_code(&r.schd),
        credits: r.hours,
        source_url: SEARCH_URL.into(),
        // Exact counts are not public; 1 means "open".
        available: i64::from(r.open == "Y"),
        ..Default::default()
    };
    let it = r.inst_type.to_lowercase();
    if it.contains("web") || it.contains("online") || it.contains("remote") {
        s.mode = "Online".into();
    } else if it.contains("blend") || it.contains("hybrid") {
        s.mode = "Hybrid".into();
    } else if !it.is_empty() {
        s.mode = "In person".into();
    }
    s.attributes = r.attributes.split('|').map(str::trim).filter(|a| !a.is_empty()).map(str::to_owned).collect();
    let people: Vec<Value> = serde_json::from_str(&r.instructors).unwrap_or_default();
    for p in people {
        let name = p.get("NAME").and_then(Value::as_str).unwrap_or("");
        let name = name.strip_suffix("(P)").unwrap_or(name).trim();
        if !name.is_empty() {
            s.instructors.push(name.to_owned());
        }
    }
    for m in embedded::<RawMeeting>(&r.meetings) {
        if m.begin_time.is_empty() {
            continue;
        }
        // Howdy lists midterms and finals as dated "Examination" rows.
        if m.kind == "Examination" {
            let date = mdy(&m.start);
            if !date.is_empty() && date == mdy(&m.finish) && hm(&m.end_time) > hm(&m.begin_time) {
                s.exams.push(Exam {
                    date,
                    start: hm(&m.begin_time),
                    end: hm(&m.end_time),
                    location: location(&m.bldg, &m.room),
                });
            }
            continue;
        }
        let days: String = [&m.mon, &m.tue, &m.wed, &m.thu, &m.fri, &m.sat, &m.sun]
            .iter()
            .zip("MTWRFSU".chars())
            .filter_map(|(d, c)| d.as_deref().is_some_and(|d| !d.is_empty()).then_some(c))
            .collect();
        let (start, end) = (hm(&m.begin_time), hm(&m.end_time));
        if days.is_empty() || end <= start {
            continue;
        }
        s.meetings.push(Meeting { days, start, end, location: location(&m.bldg, &m.room) });
        if s.start.is_empty() {
            s.start = mdy(&m.start);
            s.end = mdy(&m.finish);
        }
    }
    s
}

/// Parses an `HH:MM AM` time as minutes after midnight; zero otherwise.
fn hm(s: &str) -> i64 {
    let b = s.as_bytes();
    if b.len() < 6 || b[2] != b':' || ![b[0], b[1], b[3], b[4]].iter().all(u8::is_ascii_digit) {
        return 0;
    }
    let digit = |i: usize| i64::from(b[i] - b'0');
    let (mut hour, minute) = (digit(0) * 10 + digit(1), digit(3) * 10 + digit(4));
    if hour > 12 || minute > 59 {
        return 0;
    }
    // One or more spaces separate the time from the meridiem.
    let rest = &b[5..];
    let spaces = rest.iter().take_while(|&&c| c == b' ').count();
    if spaces == 0 {
        return 0;
    }
    match &rest[spaces..] {
        b"PM" if hour < 12 => hour += 12,
        b"PM" => {}
        b"AM" if hour == 12 => hour = 0,
        b"AM" => {}
        _ => return 0,
    }
    hour * 60 + minute
}

fn location(bldg: &str, room: &str) -> String {
    if bldg == "ONLINE" {
        return "Online".into();
    }
    format!("{bldg} {room}").trim().to_owned()
}

/// The first `n` bytes of `s`, or all of it when shorter.
fn first(s: &str, n: usize) -> String {
    s.get(..n).unwrap_or(s).to_owned()
}

fn type_code(schd: &str) -> String {
    match schd {
        "LL" | "LP" | "" => "LEC".into(),
        "INS" => "IND".into(),
        other => other.to_owned(),
    }
}

#[async_trait]
impl Adapter for Tamu {
    /// Returns the three most recent College Station terms.
    async fn terms(&self) -> Result<Vec<Term>, AdapterError> {
        let all: Option<Vec<RawTerm>> = self.call("/api/all-terms", None).await?;
        let mut cs: Vec<RawTerm> =
            all.unwrap_or_default().into_iter().filter(|t| t.desc.contains("College Station")).collect();
        cs.sort_by(|a, b| b.code.cmp(&a.code));
        Ok(cs
            .into_iter()
            .take(3)
            .map(|t| Term { start: first(&t.start, 10), end: first(&t.end, 10), id: t.code, name: t.desc })
            .collect())
    }

    async fn catalog(&self, term: &str) -> Result<Vec<Section>, AdapterError> {
        Ok(self.dump(term).await?.as_ref().clone())
    }

    /// Re-pulls the term; the public API has no filters.
    async fn availability(&self, term: &str, ids: &[String]) -> Result<Vec<Seats>, AdapterError> {
        let secs = self.dump(term).await?;
        let want: HashSet<&str> = ids.iter().map(String::as_str).collect();
        Ok(secs
            .iter()
            .filter(|s| want.contains(s.id.as_str()))
            .map(|s| Seats { id: s.id.clone(), available: s.available, capacity: s.capacity, waitlist: s.waitlist })
            .collect())
    }

    fn version(&self) -> &str {
        "tamu-howdy-1"
    }

    /// A seat check downloads the entire term from Howdy.
    fn minimum_poll_interval(&self) -> Option<Duration> {
        Some(Duration::from_secs(5 * 60))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::testserver::{Response, serve};

    const TERMS: &[u8] = include_bytes!("fixture_terms.json");
    const SECTIONS: &[u8] = include_bytes!("fixture_sections.json");

    #[tokio::test]
    async fn contract() {
        let base = serve(|r| match (r.method.as_str(), r.path.as_str()) {
            ("GET", "/api/all-terms") => Response::ok(TERMS),
            ("POST", "/api/course-sections") if r.header("content-type").is_empty() => {
                Response::error(400, "no content type")
            }
            ("POST", "/api/course-sections") => Response::ok(SECTIONS),
            _ => Response::error(404, "not found"),
        })
        .await;
        let mut a = Tamu::new(Http::unpaced());
        a.base = base;

        let ts = a.terms().await.unwrap();
        assert!(
            ts.len() == 2
                && ts[0].id == "202631"
                && ts[0].name == "Fall 2026 - College Station"
                && ts[0].start == "2026-08-24",
            "terms: {ts:?}"
        );
        let cs = a.catalog("202631").await.unwrap();
        assert_eq!(cs.len(), 3, "catalog");
        let lec = &cs[0];
        assert!(
            lec.id == "10009"
                && lec.course == "ACCT 230"
                && lec.number == "500"
                && lec.kind == "LEC"
                && lec.credits == 3.0
                && lec.available == 1,
            "bad lecture: {lec:?}"
        );
        assert!(
            lec.meetings.len() == 1
                && lec.meetings[0].days == "MW"
                && lec.meetings[0].start == 650
                && lec.meetings[0].end == 700
                && lec.meetings[0].location == "WCBA 113",
            "bad meetings: {:?}",
            lec.meetings
        );
        assert!(
            lec.instructors[0] == "Natalie L. Allen" && lec.start == "2026-08-24" && lec.end == "2026-12-10",
            "bad instructor/dates: {lec:?}"
        );
        let lab = &cs[1];
        assert!(lab.kind == "LAB" && lab.available == 0, "bad lab: {lab:?}");
        let online = &cs[2];
        assert!(
            online.meetings.is_empty() && online.available == 0,
            "online section should have no timed meetings: {online:?}"
        );
        let ex = &cs[2].exams;
        assert!(
            ex.len() == 3
                && ex[0].date == "2026-09-29"
                && ex[0].start == 19 * 60 + 5
                && ex[0].end == 20 * 60 + 20
                && ex[0].location == "Online"
                && ex[2].date == "2026-12-01",
            "examination rows should become dated exams: {ex:?}"
        );
        assert!(cs[0].exams.is_empty(), "lecture without exam rows has exams: {:?}", cs[0].exams);
        let seats = a.availability("202631", &["10009".into(), "65410".into(), "nope".into()]).await.unwrap();
        assert!(seats.len() == 2 && seats[0].available == 1 && seats[1].available == 0, "availability: {seats:?}");
    }

    #[test]
    fn clock_parsing() {
        assert_eq!(hm("10:50 AM"), 650);
        assert_eq!(hm("12:00 PM"), 720);
        assert_eq!(hm("12:10 AM"), 10);
        assert_eq!(hm("03:55 PM"), 955);
        assert_eq!(hm("3:55 PM"), 0);
        assert_eq!(hm("03:55PM"), 0);
        assert_eq!(hm(""), 0);
    }
}
