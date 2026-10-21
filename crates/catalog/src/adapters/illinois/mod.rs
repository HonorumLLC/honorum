//! The University of Illinois Course Explorer's official public XML API
//! (`courses.illinois.edu/cisapp/explorer/schedule`).
//!
//! `schedule.xml` lists calendar years and `<year>.xml` lists each year's
//! terms; the adapter reads the two newest years and offers the three most
//! recent terms whose schedules are published. A term's subjects come from
//! `<year>/<term>.xml`, and each subject's sections from
//! `<year>/<term>/<subject>.xml?mode=cascade`, fetched one at a time with a
//! 150 ms pause. Term IDs look like `2026-fall`. Each request times out after
//! 90 seconds.
//!
//! The API carries every section and meeting but no seat counts or
//! open/closed status, so the catalog marks seats as unpublished and
//! availability returns nothing.

use std::collections::HashSet;
use std::sync::LazyLock;
use std::time::Duration;

use async_trait::async_trait;
use chrono::NaiveDate;
use regex::Regex;
use reqwest::StatusCode;

use super::xml;
use super::{iso, status_text, upper_prefix3};
use crate::{Adapter, AdapterError, Http, Meeting, Seats, Section, Term};

const PAUSE: Duration = Duration::from_millis(150);
const TIMEOUT: Duration = Duration::from_secs(90);

static RE_YEAR: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"<calendarYear id="([0-9]{4})""#).expect("valid regex"));
static RE_TERM: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"<term id="([0-9]+)" href="[^"]*/([0-9]{4})/([0-9A-Za-z_]+)\.xml">([^<]+)<"#).expect("valid regex")
});
static RE_SUBJECT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"<subject id="([^"]+)""#).expect("valid regex"));

/// The Urbana-Champaign Course Explorer.
pub(super) struct Illinois {
    /// The schedule root, without `.xml`. Tests point it at a fixture server.
    pub base: String,
    http: Http,
}

impl Illinois {
    pub(super) fn new(http: Http) -> Self {
        Self { base: "https://courses.illinois.edu/cisapp/explorer/schedule".into(), http }
    }

    async fn get(&self, url: &str) -> Result<String, AdapterError> {
        let (status, body) = self.http.send_any(self.http.get(url), TIMEOUT).await?;
        if status != StatusCode::OK {
            return Err(AdapterError::new(format!("illinois: {url} -> {}", status_text(status))));
        }
        Ok(String::from_utf8_lossy(&body).into_owned())
    }
}

/// Converts one subject's cascade document into sections.
fn subject_sections(raw: &str, subject: &str, path: &str) -> Result<Vec<Section>, String> {
    let doc = xml::parse(raw).map_err(|err| err.to_string())?;
    let root = doc.root_element();
    let mut out = Vec::new();
    for c in xml::all(root, &["cascadingCourses", "cascadingCourse"]) {
        let course_id = xml::attr(c, "id");
        let label = xml::text(c, &["label"]);
        let description = xml::text(c, &["description"]);
        let credit_hours = xml::text(c, &["creditHours"]);
        let gen_ed: Vec<String> = xml::all(c, &["genEdCategories", "category", "genEdAttributes", "genEdAttribute"])
            .into_iter()
            .map(xml::own_text)
            .collect();
        for sec in xml::all(c, &["detailedSections", "detailedSection"]) {
            let mut s = Section {
                id: xml::attr(sec, "id"),
                course: course_id.clone(),
                subject: subject.to_owned(),
                title: label.clone(),
                number: xml::text(sec, &["sectionNumber"]),
                kind: "LEC".into(),
                available: -1,
                credits: credits(&credit_hours),
                start: date(&xml::text(sec, &["startDate"])),
                end: date(&xml::text(sec, &["endDate"])),
                source_url: format!(
                    "{path}/{subject}/{}",
                    course_id.strip_prefix(&format!("{subject} ")).unwrap_or(&course_id)
                ),
                description: description.trim().to_owned(),
                attributes: gen_ed.clone(),
                ..Default::default()
            };
            let mut seen = HashSet::new();
            for (j, mt) in xml::all(sec, &["meetings", "meeting"]).into_iter().enumerate() {
                let code = xml::all(mt, &["type"]).into_iter().map(|t| xml::attr(t, "code")).rfind(|c| !c.is_empty());
                if j == 0
                    && let Some(code) = code
                {
                    s.kind = upper_prefix3(&code);
                }
                for instructor in xml::all(mt, &["instructors", "instructor"]) {
                    let name = format!(
                        "{} {}",
                        xml::attr(instructor, "firstName").trim(),
                        xml::attr(instructor, "lastName").trim()
                    )
                    .trim()
                    .to_owned();
                    if !name.is_empty() && seen.insert(name.clone()) {
                        s.instructors.push(name);
                    }
                }
                let days = xml::text(mt, &["daysOfTheWeek"]).trim().to_owned();
                let (start, end) = (hm(&xml::text(mt, &["start"])), hm(&xml::text(mt, &["end"])));
                if days.is_empty() || end <= start {
                    continue;
                }
                let location = format!("{} {}", xml::text(mt, &["buildingName"]), xml::text(mt, &["roomNumber"]))
                    .trim()
                    .to_owned();
                s.meetings.push(Meeting { days, start, end, location });
            }
            out.push(s);
        }
    }
    Ok(out)
}

/// Reads the leading number of a credit note such as `3 TO 4 hours.`.
fn credits(s: &str) -> f64 {
    s.split_whitespace().next().and_then(|f| f.parse().ok()).unwrap_or(0.0)
}

/// Parses an `HH:MMAM` time as minutes after midnight; zero otherwise.
fn hm(s: &str) -> i64 {
    let b = s.trim().as_bytes();
    if b.len() != 7 || b[2] != b':' || ![b[0], b[1], b[3], b[4]].iter().all(u8::is_ascii_digit) {
        return 0;
    }
    let digit = |i: usize| i64::from(b[i] - b'0');
    let (mut hour, minute) = (digit(0) * 10 + digit(1), digit(3) * 10 + digit(4));
    if hour > 12 || minute > 59 {
        return 0;
    }
    match &b[5..] {
        b"PM" if hour < 12 => hour += 12,
        b"PM" => {}
        b"AM" if hour == 12 => hour = 0,
        b"AM" => {}
        _ => return 0,
    }
    hour * 60 + minute
}

/// Rewrites an `MM-DD-YYZ` date as `YYYY-MM-DD`; empty when it does not
/// parse. Two-digit years from 69 are in the 1900s.
fn date(s: &str) -> String {
    let b = s.trim().as_bytes();
    if b.len() != 9 || b[2] != b'-' || b[5] != b'-' || b[8] != b'Z' {
        return String::new();
    }
    let num = |i: usize| -> Option<u32> {
        (b[i].is_ascii_digit() && b[i + 1].is_ascii_digit())
            .then(|| u32::from(b[i] - b'0') * 10 + u32::from(b[i + 1] - b'0'))
    };
    let (Some(month), Some(day), Some(yy)) = (num(0), num(3), num(6)) else { return String::new() };
    let year = if yy >= 69 { 1900 + yy } else { 2000 + yy };
    NaiveDate::from_ymd_opt(year as i32, month, day).map(iso).unwrap_or_default()
}

#[async_trait]
impl Adapter for Illinois {
    /// Returns the three most recent published terms; IDs look like
    /// `2026-fall`. Future terms are listed before their schedules are
    /// published, so each candidate is probed.
    async fn terms(&self) -> Result<Vec<Term>, AdapterError> {
        let root = self.get(&format!("{}.xml", self.base)).await?;
        let years: Vec<String> = RE_YEAR.captures_iter(&root).take(2).map(|m| m[1].to_owned()).collect();
        let mut all: Vec<(i64, Term)> = Vec::new();
        for year in years {
            let body = self.get(&format!("{}/{year}.xml", self.base)).await?;
            for m in RE_TERM.captures_iter(&body) {
                let id = m[1].parse().unwrap_or(0);
                all.push((
                    id,
                    Term { id: format!("{}-{}", &m[2], &m[3]), name: m[4].to_owned(), ..Default::default() },
                ));
            }
        }
        all.sort_by_key(|t| std::cmp::Reverse(t.0));
        let mut out = Vec::new();
        for (_, term) in all {
            if self.get(&format!("{}/{}.xml", self.base, term.id.replacen('-', "/", 1))).await.is_err() {
                continue;
            }
            out.push(term);
            if out.len() == 3 {
                break;
            }
        }
        Ok(out)
    }

    async fn catalog(&self, term: &str) -> Result<Vec<Section>, AdapterError> {
        let path = format!("{}/{}", self.base, term.replacen('-', "/", 1));
        let body = self.get(&format!("{path}.xml")).await?;
        let subjects: Vec<String> = RE_SUBJECT.captures_iter(&body).map(|m| m[1].to_owned()).collect();
        let mut out = Vec::new();
        for (i, subject) in subjects.iter().enumerate() {
            if i > 0 {
                tokio::time::sleep(PAUSE).await;
            }
            let raw = self
                .get(&format!("{path}/{subject}.xml?mode=cascade"))
                .await
                .map_err(|err| AdapterError::new(format!("illinois: {subject}: {err}")))?;
            let secs = subject_sections(&raw, subject, &path)
                .map_err(|err| AdapterError::new(format!("illinois: {subject}: {err}")))?;
            out.extend(secs);
        }
        Ok(out)
    }

    /// Returns nothing: the API publishes no enrollment data.
    async fn availability(&self, _term: &str, _ids: &[String]) -> Result<Vec<Seats>, AdapterError> {
        Ok(Vec::new())
    }

    fn version(&self) -> &str {
        "illinois-cis-1"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::testserver::{Response, serve};

    #[tokio::test]
    async fn contract() {
        let base = serve(|r| {
            let file: &[u8] = match r.path.as_str() {
                "/schedule.xml" => include_bytes!("fixture_schedule.xml"),
                "/schedule/2026.xml" => include_bytes!("fixture_2026.xml"),
                "/schedule/2027.xml" => include_bytes!("fixture_2027.xml"),
                "/schedule/2026/fall.xml" => include_bytes!("fixture_fall.xml"),
                "/schedule/2026/fall/CS.xml" if r.q("mode") != "cascade" => {
                    return Response::error(400, "need cascade");
                }
                "/schedule/2026/fall/CS.xml" => include_bytes!("fixture_cs.xml"),
                _ => return Response::error(404, "404 page not found"),
            };
            Response::ok(file)
        })
        .await;
        let mut u = Illinois::new(Http::unpaced());
        u.base = format!("{base}/schedule");
        let terms = u.terms().await.unwrap();
        // 2027-spring is listed but unpublished.
        assert!(terms.len() == 1 && terms[0].id == "2026-fall" && terms[0].name == "Fall 2026", "terms: {terms:?}");
        let secs = u.catalog("2026-fall").await.unwrap();
        assert!(!secs.is_empty(), "catalog");
        let s = &secs[0];
        assert!(
            s.course == "CS 100"
                && s.subject == "CS"
                && !s.title.is_empty()
                && s.available == -1
                && s.credits == 1.0
                && s.start == "2026-08-24",
            "bad section: {s:?}"
        );
        assert!(
            !s.meetings.is_empty()
                && s.meetings[0].start != 0
                && !s.meetings[0].days.is_empty()
                && !s.meetings[0].location.is_empty(),
            "bad meetings: {:?}",
            s.meetings
        );
        assert!(
            !s.instructors.is_empty() && s.instructors[0].contains("Cunningham"),
            "bad instructors: {:?}",
            s.instructors
        );
        assert_eq!(hm("09:00AM"), 540);
        assert_eq!(hm("10:50AM"), 650);
        assert_eq!(date("12-09-26Z"), "2026-12-09");
        assert_eq!(credits("3 TO 4 hours."), 3.0);
    }

    #[test]
    fn time_parsing_matches_twelve_hour_rules() {
        assert_eq!(hm("12:00AM"), 0);
        assert_eq!(hm("12:30PM"), 750);
        assert_eq!(hm("01:15PM"), 795);
        assert_eq!(hm("13:00PM"), 0);
        assert_eq!(hm("9:00AM"), 0);
        assert_eq!(hm("ARRANGED"), 0);
        assert_eq!(date("02-30-26Z"), "");
        assert_eq!(date("08-24-70Z"), "1970-08-24");
    }
}
