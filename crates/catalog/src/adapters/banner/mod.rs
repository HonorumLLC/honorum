//! Ellucian Banner 9 Self-Service registration (`StudentRegistrationSsb`),
//! whose class search many universities expose without authentication.
//!
//! The adapter reads the public JSON endpoints under the configured base URL
//! (such as `https://reg.example.edu/StudentRegistrationSsb/ssb`):
//! `classSearch/getTerms` for terms, `term/search` to select a term in the
//! session, `classSearch/resetDataForm` before each query, and
//! `searchResults/searchResults` in pages of 500 sections. Banner keeps the
//! selected term and search state in a cookie session, so every catalog or
//! availability call starts a fresh session of its own. Each request times
//! out after 60 seconds.
//!
//! Availability queries watched CRNs one at a time when there are at most
//! 40, and scans the whole term otherwise. Meeting rows typed as an exam
//! with a single date become exam sittings.

use std::collections::{BTreeMap, HashSet};
use std::time::Duration;

use async_trait::async_trait;
use reqwest::header::{ACCEPT, CONTENT_TYPE, COOKIE, SET_COOKIE};
use reqwest::{RequestBuilder, StatusCode};
use serde::Deserialize;
use serde::de::DeserializeOwned;

use super::{decode_error, mdy, null_default as nd, status_text, upper_prefix3};
use crate::{Adapter, AdapterError, Http, Meeting, Seats, Section, Term};

const PAGE_SIZE: usize = 500;
const TIMEOUT: Duration = Duration::from_secs(60);

/// A Banner 9 school.
pub(super) struct Banner {
    /// The `.../StudentRegistrationSsb/ssb` root, without a trailing slash.
    pub base: String,
    http: Http,
}

impl Banner {
    pub(super) fn new(http: Http, base: &str) -> Self {
        Self { base: base.trim_end_matches('/').to_owned(), http }
    }

    fn session(&self) -> Session<'_> {
        Session { banner: self, cookies: BTreeMap::new() }
    }
}

#[derive(Deserialize)]
struct RawTerm {
    #[serde(default, deserialize_with = "nd")]
    code: String,
    #[serde(default, deserialize_with = "nd")]
    description: String,
}

#[derive(Deserialize)]
struct SearchResponse {
    #[serde(default, deserialize_with = "nd")]
    success: bool,
    #[serde(rename = "totalCount", default, deserialize_with = "nd")]
    total_count: i64,
    #[serde(default, deserialize_with = "nd")]
    data: Vec<RawSection>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct RawSection {
    #[serde(rename = "courseReferenceNumber", deserialize_with = "nd")]
    crn: String,
    #[serde(deserialize_with = "nd")]
    subject: String,
    #[serde(rename = "courseNumber", deserialize_with = "nd")]
    course_number: String,
    #[serde(rename = "courseTitle", deserialize_with = "nd")]
    title: String,
    #[serde(rename = "sequenceNumber", deserialize_with = "nd")]
    sequence: String,
    #[serde(rename = "scheduleTypeDescription", deserialize_with = "nd")]
    schedule_type: String,
    #[serde(rename = "linkIdentifier", deserialize_with = "nd")]
    link: String,
    #[serde(rename = "maximumEnrollment", deserialize_with = "nd")]
    max_enrollment: i64,
    #[serde(rename = "seatsAvailable", deserialize_with = "nd")]
    seats_available: i64,
    #[serde(rename = "waitCount", deserialize_with = "nd")]
    wait_count: i64,
    #[serde(rename = "creditHourLow", deserialize_with = "nd")]
    credits: f64,
    #[serde(rename = "instructionalMethodDescription", deserialize_with = "nd")]
    method: String,
    #[serde(deserialize_with = "nd")]
    faculty: Vec<RawFaculty>,
    #[serde(rename = "meetingsFaculty", deserialize_with = "nd")]
    meetings_faculty: Vec<RawMeetingFaculty>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct RawFaculty {
    #[serde(rename = "displayName", deserialize_with = "nd")]
    name: String,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct RawMeetingFaculty {
    #[serde(rename = "meetingTime", deserialize_with = "nd")]
    meeting_time: RawMeetingTime,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct RawMeetingTime {
    #[serde(rename = "beginTime", deserialize_with = "nd")]
    begin: String,
    #[serde(rename = "endTime", deserialize_with = "nd")]
    end: String,
    /// `CLAS`, `EXAM`, and so on.
    #[serde(rename = "meetingType", deserialize_with = "nd")]
    kind: String,
    #[serde(rename = "meetingTypeDescription", deserialize_with = "nd")]
    kind_description: String,
    /// `MM/DD/YYYY`.
    #[serde(rename = "startDate", deserialize_with = "nd")]
    start_date: String,
    #[serde(rename = "endDate", deserialize_with = "nd")]
    end_date: String,
    #[serde(deserialize_with = "nd")]
    building: String,
    #[serde(deserialize_with = "nd")]
    room: String,
    #[serde(deserialize_with = "nd")]
    monday: bool,
    #[serde(deserialize_with = "nd")]
    tuesday: bool,
    #[serde(deserialize_with = "nd")]
    wednesday: bool,
    #[serde(deserialize_with = "nd")]
    thursday: bool,
    #[serde(deserialize_with = "nd")]
    friday: bool,
    #[serde(deserialize_with = "nd")]
    saturday: bool,
    #[serde(deserialize_with = "nd")]
    sunday: bool,
}

/// One Banner cookie session. Banner requires a selected term before any
/// search and keeps search state server-side, so concurrent calls must not
/// share a session.
struct Session<'a> {
    banner: &'a Banner,
    cookies: BTreeMap<String, String>,
}

impl Session<'_> {
    /// Selects a term, which Banner requires before any search.
    async fn select_term(&mut self, term: &str) -> Result<(), AdapterError> {
        self.post("/term/search?mode=search", &[("term", term)]).await
    }

    async fn search(&mut self, params: &[(&str, &str)]) -> Result<Vec<Section>, AdapterError> {
        let mut out = Vec::new();
        let mut offset = 0;
        loop {
            // Banner keeps search state in the session; reset before each query.
            self.post("/classSearch/resetDataForm", &[]).await?;
            let offset_text = offset.to_string();
            let size_text = PAGE_SIZE.to_string();
            let mut q: Vec<(&str, &str)> = params.to_vec();
            q.push(("pageOffset", &offset_text));
            q.push(("pageMaxSize", &size_text));
            let resp: SearchResponse = self.get_json(&format!("/searchResults/searchResults?{}", encode(&q))).await?;
            if !resp.success {
                return Err(AdapterError::new("banner: search returned success=false"));
            }
            let count = resp.data.len();
            out.extend(resp.data.into_iter().map(|r| self.banner.convert(r)));
            if count == 0 || (offset + count) as i64 >= resp.total_count {
                return Ok(out);
            }
            offset += PAGE_SIZE;
        }
    }

    async fn get_json<T: DeserializeOwned>(&mut self, path: &str) -> Result<T, AdapterError> {
        let url = format!("{}{path}", self.banner.base);
        let request = self.banner.http.get(&url).header(ACCEPT, "application/json");
        let (status, body) = self.send(request).await?;
        if status != StatusCode::OK {
            return Err(AdapterError::new(format!("banner: {path} -> {}", status_text(status))));
        }
        serde_json::from_slice(&body).map_err(decode_error)
    }

    async fn post(&mut self, path: &str, form: &[(&str, &str)]) -> Result<(), AdapterError> {
        let url = format!("{}{path}", self.banner.base);
        let request =
            self.banner.http.post(&url).header(CONTENT_TYPE, "application/x-www-form-urlencoded").body(encode(form));
        let (status, _) = self.send(request).await?;
        if status != StatusCode::OK {
            return Err(AdapterError::new(format!("banner: {path} -> {}", status_text(status))));
        }
        Ok(())
    }

    /// Sends a request with the session's cookies and records the cookies
    /// the response sets. A `Max-Age` of zero or less deletes a cookie.
    async fn send(&mut self, mut request: RequestBuilder) -> Result<(StatusCode, bytes::Bytes), AdapterError> {
        if !self.cookies.is_empty() {
            let header = self.cookies.iter().map(|(k, v)| format!("{k}={v}")).collect::<Vec<_>>().join("; ");
            request = request.header(COOKIE, header);
        }
        let (status, headers, body) = self.banner.http.send_full(request, TIMEOUT).await?;
        for value in headers.get_all(SET_COOKIE) {
            let Ok(value) = value.to_str() else { continue };
            let mut parts = value.split(';');
            let Some((name, val)) = parts.next().and_then(|p| p.split_once('=')) else { continue };
            let name = name.trim();
            if name.is_empty() {
                continue;
            }
            let expired = parts.any(|attr| {
                attr.trim().split_once('=').is_some_and(|(k, v)| {
                    k.trim().eq_ignore_ascii_case("max-age") && v.trim().parse::<i64>().is_ok_and(|n| n <= 0)
                })
            });
            if expired {
                self.cookies.remove(name);
            } else {
                self.cookies.insert(name.to_owned(), val.trim().trim_matches('"').to_owned());
            }
        }
        Ok((status, body))
    }
}

/// Form-encodes parameters sorted by key.
fn encode(params: &[(&str, &str)]) -> String {
    let mut sorted = params.to_vec();
    sorted.sort_by(|a, b| a.0.cmp(b.0));
    let mut ser = url::form_urlencoded::Serializer::new(String::new());
    for (k, v) in sorted {
        ser.append_pair(k, v);
    }
    ser.finish()
}

impl Banner {
    fn convert(&self, r: RawSection) -> Section {
        let mut s = Section {
            id: r.crn,
            course: format!("{} {}", r.subject, r.course_number),
            subject: r.subject,
            title: r.title,
            number: r.sequence,
            kind: type_code(&r.schedule_type),
            link: r.link,
            capacity: r.max_enrollment,
            // Banner reports negative seats when a section is over-enrolled.
            available: r.seats_available.max(0),
            waitlist: r.wait_count,
            credits: r.credits,
            mode: mode(&r.method),
            source_url: format!("{}/classSearch/classSearch", self.base),
            instructors: r.faculty.into_iter().map(|f| f.name).collect(),
            ..Default::default()
        };
        for mf in r.meetings_faculty {
            let mt = mf.meeting_time;
            let days: String = [mt.monday, mt.tuesday, mt.wednesday, mt.thursday, mt.friday, mt.saturday, mt.sunday]
                .iter()
                .zip("MTWRFSU".chars())
                .filter_map(|(on, c)| on.then_some(c))
                .collect();
            if mt.begin.is_empty() {
                continue;
            }
            let location = format!("{} {}", mt.building, mt.room).trim().to_owned();
            // A dated exam row is a sitting, never a weekly pattern.
            if format!("{} {}", mt.kind, mt.kind_description).to_uppercase().contains("EXAM") {
                let date = mdy(&mt.start_date);
                if !date.is_empty() && date == mdy(&mt.end_date) && hhmm(&mt.end) > hhmm(&mt.begin) {
                    s.exams.push(crate::Exam { date, start: hhmm(&mt.begin), end: hhmm(&mt.end), location });
                }
                continue;
            }
            if days.is_empty() {
                continue;
            }
            s.meetings.push(Meeting { days, start: hhmm(&mt.begin), end: hhmm(&mt.end), location });
        }
        s
    }
}

fn mode(method: &str) -> String {
    let m = method.to_lowercase();
    if m.is_empty() {
        String::new()
    } else if ["online", "web", "distance", "remote"].iter().any(|k| m.contains(k)) {
        "Online".into()
    } else if m.contains("hybrid") || m.contains("blend") {
        "Hybrid".into()
    } else {
        "In person".into()
    }
}

/// Reads a four-digit `HHMM` time as minutes after midnight; zero otherwise.
fn hhmm(s: &str) -> i64 {
    if s.len() != 4 || !s.is_char_boundary(2) {
        return 0;
    }
    let h: i64 = s[..2].parse().unwrap_or(0);
    let m: i64 = s[2..].parse().unwrap_or(0);
    h * 60 + m
}

fn type_code(desc: &str) -> String {
    let d = desc.to_lowercase();
    if d.starts_with("lec") {
        "LEC".into()
    } else if d.starts_with("lab") {
        "LAB".into()
    } else if d.starts_with("dis") || d.starts_with("rec") {
        "DIS".into()
    } else if d.starts_with("sem") {
        "SEM".into()
    } else if d.is_empty() {
        "LEC".into()
    } else {
        upper_prefix3(desc)
    }
}

#[async_trait]
impl Adapter for Banner {
    /// Lists terms, leaving out view-only terms.
    async fn terms(&self) -> Result<Vec<Term>, AdapterError> {
        let raw: Vec<RawTerm> = self.session().get_json("/classSearch/getTerms?searchTerm=&offset=1&max=10").await?;
        Ok(raw
            .into_iter()
            .filter(|t| !t.description.contains("View Only"))
            .map(|t| Term { id: t.code, name: t.description, ..Default::default() })
            .collect())
    }

    async fn catalog(&self, term: &str) -> Result<Vec<Section>, AdapterError> {
        let mut session = self.session();
        session.select_term(term).await?;
        session.search(&[("txt_term", term)]).await
    }

    /// Queries watched CRNs one at a time when there are few, and falls back
    /// to a full catalog scan when a scan is cheaper.
    async fn availability(&self, term: &str, ids: &[String]) -> Result<Vec<Seats>, AdapterError> {
        let mut session = self.session();
        session.select_term(term).await?;
        let secs = if ids.len() > 40 {
            session.search(&[("txt_term", term)]).await?
        } else {
            let mut secs = Vec::new();
            for id in ids {
                secs.extend(session.search(&[("txt_term", term), ("txt_courseReferenceNumber", id)]).await?);
            }
            secs
        };
        let want: HashSet<&str> = ids.iter().map(String::as_str).collect();
        Ok(secs
            .into_iter()
            .filter(|s| want.contains(s.id.as_str()))
            .map(|s| Seats { id: s.id, capacity: s.capacity, available: s.available, waitlist: s.waitlist })
            .collect())
    }

    fn version(&self) -> &str {
        "banner9-1"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::testserver::{Response, serve};

    const SEARCH: &[u8] = include_bytes!("fixture_search.json");

    /// Serves the recorded Banner JSON shape under `/ssb`.
    async fn fixture_server() -> String {
        serve(|r| match r.path.as_str() {
            "/ssb/classSearch/getTerms" => Response::ok(
                r#"[{"code":"202610","description":"Fall 2026"},{"code":"202520","description":"Spring 2026 (View Only)"}]"#,
            ),
            "/ssb/term/search" => Response::ok("").with_header("Set-Cookie", "JSESSIONID=x; Path=/ssb"),
            "/ssb/classSearch/resetDataForm" => Response::ok(""),
            "/ssb/searchResults/searchResults" => {
                if !r.header("cookie").split(';').any(|c| c.trim() == "JSESSIONID=x") {
                    return Response::error(403, "no session");
                }
                let crn = r.q("txt_courseReferenceNumber");
                if !crn.is_empty() && !String::from_utf8_lossy(SEARCH).contains(crn) {
                    return Response::ok(r#"{"success":true,"totalCount":0,"data":[]}"#);
                }
                Response::ok(SEARCH)
            }
            _ => Response::error(404, "not found"),
        })
        .await
    }

    #[tokio::test]
    async fn contract() {
        let base = fixture_server().await;
        let b = Banner::new(Http::unpaced(), &format!("{base}/ssb"));

        let terms = b.terms().await.unwrap();
        assert!(terms.len() == 1 && terms[0].id == "202610", "terms: {terms:?}");
        let secs = b.catalog("202610").await.unwrap();
        assert_eq!(secs.len(), 2, "catalog: {secs:?}");
        let lec = &secs[0];
        assert!(
            lec.id == "20411"
                && lec.course == "CS 101"
                && lec.kind == "LEC"
                && lec.link == "A1"
                && lec.available == 0
                && lec.waitlist == 7
                && lec.credits == 3.0,
            "bad lecture: {lec:?}"
        );
        assert!(
            lec.meetings.len() == 1
                && lec.exams.len() == 1
                && lec.exams[0].date == "2026-12-10"
                && lec.exams[0].start == 19 * 60
                && lec.exams[0].end == 21 * 60
                && lec.exams[0].location == "ENG 110",
            "exam rows should become dated exams, not weekly meetings: {:?} {:?}",
            lec.meetings,
            lec.exams
        );
        let m = &lec.meetings[0];
        assert!(m.days == "MWF" && m.start == 540 && m.end == 590 && m.location == "ENG 110", "bad meeting: {m:?}");
        let lab = &secs[1];
        assert!(
            lab.kind == "LAB" && lab.meetings.len() == 1 && lab.meetings[0].days == "T" && lab.instructors.is_empty(),
            "bad lab: {lab:?}"
        );
        let seats = b.availability("202610", &["20412".into()]).await.unwrap();
        assert!(seats.len() == 1 && seats[0].available == 5, "availability: {seats:?}");
    }
}
