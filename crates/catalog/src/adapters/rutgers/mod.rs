//! The Rutgers Schedule of Classes JSON feeds (`classes.rutgers.edu/soc/api`).
//!
//! `courses.json` is one file per term and campus with every course and
//! section; `openSections.json` is a small list of the section indexes that
//! are open now, which makes availability polling one cheap request per
//! term. Seat counts are not published, so sections read as open (`1`) or
//! closed (`0`). Term IDs look like `2026-9` (year and term code). The
//! campus is `NB`, `NK`, or `CM`. The course feed is served gzip-compressed
//! without a `Content-Encoding` header and is decompressed here. Each
//! section's final exam note becomes an exam sitting. Each request times out
//! after three minutes.

use std::collections::HashSet;
use std::io::Read;
use std::sync::LazyLock;
use std::time::Duration;

use async_trait::async_trait;
use chrono::Datelike;
use regex::Regex;
use reqwest::StatusCode;
use reqwest::header::ACCEPT;
use serde::Deserialize;
use serde::de::DeserializeOwned;

use super::{decode_error, iso, null_default as nd, parse_mdy, status_text};
use crate::{Adapter, AdapterError, Exam, Http, Meeting, Seats, Section, Term, title_case};

const TIMEOUT: Duration = Duration::from_secs(180);

static RE_TAG: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"<[^>]+>").expect("valid regex"));
static RE_FINAL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^([0-9]{2}/[0-9]{2}/[0-9]{4}) - ([0-9]{4}) - ([0-9]{4}) (AM|PM)$").expect("valid regex")
});

fn term_name(code: &str) -> &'static str {
    match code {
        "9" => "Fall",
        "1" => "Spring",
        "7" => "Summer",
        "0" => "Winter",
        _ => "",
    }
}

/// One Rutgers campus.
pub(super) struct Rutgers {
    /// The API root. Tests point it at a fixture server.
    pub base: String,
    /// `NB`, `NK`, or `CM`.
    pub campus: String,
    http: Http,
}

impl Rutgers {
    /// An empty campus selects New Brunswick (`NB`).
    pub(super) fn new(http: Http, campus: &str) -> Self {
        let campus = if campus.is_empty() { "NB" } else { campus };
        Self { base: "https://classes.rutgers.edu/soc/api".into(), campus: campus.to_owned(), http }
    }

    async fn get_json<T: DeserializeOwned>(&self, path: &str) -> Result<T, AdapterError> {
        let request = self.http.get(&format!("{}{path}", self.base)).header(ACCEPT, "application/json");
        let (status, body) = self.http.send_any(request, TIMEOUT).await?;
        if status != StatusCode::OK {
            return Err(AdapterError::new(format!("rutgers: {path} -> {}", status_text(status))));
        }
        // The feed is served gzip-compressed without a Content-Encoding header.
        if body.starts_with(&[0x1f, 0x8b]) {
            let mut text = Vec::new();
            flate2::read::GzDecoder::new(&body[..])
                .read_to_end(&mut text)
                .map_err(|err| AdapterError::new(err.to_string()))?;
            return serde_json::from_slice(&text).map_err(decode_error);
        }
        serde_json::from_slice(&body).map_err(decode_error)
    }

    async fn open_sections(&self, year: &str, code: &str) -> Result<Vec<String>, AdapterError> {
        let open: Option<Vec<String>> =
            self.get_json(&format!("/openSections.json?year={year}&term={code}&campus={}", self.campus)).await?;
        Ok(open.unwrap_or_default())
    }
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Course {
    #[serde(rename = "subjectDescription", deserialize_with = "nd")]
    subject_description: String,
    #[serde(deserialize_with = "nd")]
    title: String,
    #[serde(rename = "courseString", deserialize_with = "nd")]
    course_string: String,
    #[serde(rename = "courseDescription", deserialize_with = "nd")]
    course_description: String,
    #[serde(rename = "preReqNotes", deserialize_with = "nd")]
    prereq_notes: String,
    #[serde(deserialize_with = "nd")]
    credits: f64,
    #[serde(rename = "coreCodes", deserialize_with = "nd")]
    core_codes: Vec<CoreCode>,
    #[serde(deserialize_with = "nd")]
    sections: Vec<RawSection>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct CoreCode {
    #[serde(rename = "coreCode", deserialize_with = "nd")]
    code: String,
    #[serde(rename = "coreCodeDescription", deserialize_with = "nd")]
    description: String,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct RawSection {
    #[serde(deserialize_with = "nd")]
    index: String,
    #[serde(deserialize_with = "nd")]
    number: String,
    #[serde(rename = "openStatus", deserialize_with = "nd")]
    open_status: bool,
    #[serde(rename = "finalExam", deserialize_with = "nd")]
    final_exam: String,
    #[serde(deserialize_with = "nd")]
    instructors: Vec<Instructor>,
    #[serde(rename = "meetingTimes", deserialize_with = "nd")]
    meeting_times: Vec<MeetingTime>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Instructor {
    #[serde(deserialize_with = "nd")]
    name: String,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct MeetingTime {
    #[serde(rename = "meetingDay", deserialize_with = "nd")]
    day: String,
    #[serde(rename = "startTimeMilitary", deserialize_with = "nd")]
    start: String,
    #[serde(rename = "endTimeMilitary", deserialize_with = "nd")]
    end: String,
    #[serde(rename = "buildingCode", deserialize_with = "nd")]
    building: String,
    #[serde(rename = "roomNumber", deserialize_with = "nd")]
    room: String,
    #[serde(rename = "campusAbbrev", deserialize_with = "nd")]
    campus: String,
    #[serde(rename = "meetingModeDesc", deserialize_with = "nd")]
    mode: String,
}

fn type_code(mode: &str) -> String {
    let m = mode.to_uppercase();
    let code = if m.starts_with("LEC") {
        "LEC"
    } else if m.starts_with("REC") {
        "DIS"
    } else if m.starts_with("LAB") {
        "LAB"
    } else if m.starts_with("ONLINE") {
        "ONL"
    } else {
        "LEC"
    };
    code.into()
}

/// Reads the schedule of classes' `12/16/2026 - 0400 - 0700 PM` note. The
/// meridiem belongs to the end; a start past the end is morning.
fn final_exam(note: &str) -> Option<Exam> {
    let m = RE_FINAL.captures(note.trim())?;
    let date = parse_mdy(&m[1])?;
    let (mut start, mut end) = (hhmm(&m[2]), hhmm(&m[3]));
    if m[4].eq_ignore_ascii_case("PM") {
        if end < 12 * 60 {
            end += 12 * 60;
        }
        if start < 12 * 60 {
            start += 12 * 60;
        }
    }
    if start >= end {
        start -= 12 * 60;
    }
    if start < 0 {
        return None;
    }
    Some(Exam { date: iso(date), start, end, location: String::new() })
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

#[async_trait]
impl Adapter for Rutgers {
    /// Probes the current and next academic year and returns up to three
    /// terms that have open sections, newest first.
    async fn terms(&self) -> Result<Vec<Term>, AdapterError> {
        let y = chrono::Local::now().year();
        let (this, next) = (y.to_string(), (y + 1).to_string());
        let mut out = Vec::new();
        for (year, code) in [(&next, "1"), (&next, "0"), (&this, "9"), (&this, "7"), (&this, "1")] {
            match self.open_sections(year, code).await {
                Ok(open) if !open.is_empty() => {}
                _ => continue,
            }
            out.push(Term {
                id: format!("{year}-{code}"),
                name: format!("{} {year}", term_name(code)),
                ..Default::default()
            });
            if out.len() == 3 {
                break;
            }
        }
        if out.is_empty() {
            return Err(AdapterError::new("rutgers: no terms with open sections found"));
        }
        Ok(out)
    }

    async fn catalog(&self, term: &str) -> Result<Vec<Section>, AdapterError> {
        let (year, code) = term.split_once('-').unwrap_or((term, ""));
        let courses: Option<Vec<Course>> =
            self.get_json(&format!("/courses.json?year={year}&term={code}&campus={}", self.campus)).await?;
        let mut out = Vec::new();
        for c in courses.unwrap_or_default() {
            let attributes: Vec<String> = c
                .core_codes
                .iter()
                .map(|cc| format!("{} {}", cc.code, title_case(&cc.description)).trim().to_owned())
                .collect();
            for sec in &c.sections {
                let mut s = Section {
                    id: sec.index.clone(),
                    course: c.course_string.clone(),
                    subject: title_case(&c.subject_description),
                    title: title_case(&c.title),
                    number: sec.number.clone(),
                    kind: "LEC".into(),
                    credits: c.credits,
                    source_url: format!(
                        "https://classes.rutgers.edu/soc/#keyword?keyword={}&semester={code}{year}&campus={}",
                        c.course_string, self.campus
                    ),
                    // Rutgers publishes open or closed only.
                    available: i64::from(sec.open_status),
                    description: c.course_description.trim().to_owned(),
                    prereqs: RE_TAG.replace_all(&c.prereq_notes, " ").trim().to_owned(),
                    attributes: attributes.clone(),
                    mode: "In person".into(),
                    ..Default::default()
                };
                s.instructors =
                    sec.instructors.iter().filter(|i| !i.name.is_empty()).map(|i| title_case(&i.name)).collect();
                for (i, m) in sec.meeting_times.iter().enumerate() {
                    if i == 0 && !m.mode.is_empty() {
                        s.kind = type_code(&m.mode);
                    }
                    if m.mode.to_uppercase().contains("ONLINE") {
                        s.mode = "Online".into();
                    }
                    let day = m.day.replace('H', "R");
                    let (start, end) = (hhmm(&m.start), hhmm(&m.end));
                    if day.is_empty() || end <= start {
                        continue;
                    }
                    let mut location = format!("{} {}", m.building, m.room).trim().to_owned();
                    if !m.campus.is_empty() && !location.is_empty() {
                        location = format!("{location} ({})", m.campus);
                    }
                    s.meetings.push(Meeting { days: day, start, end, location });
                }
                if let Some(exam) = final_exam(&sec.final_exam) {
                    s.exams.push(exam);
                }
                out.push(s);
            }
        }
        Ok(out)
    }

    /// Reads the open-sections list: one small request per term. Every
    /// watched ID gets a result, open (`1`) or closed (`0`).
    async fn availability(&self, term: &str, ids: &[String]) -> Result<Vec<Seats>, AdapterError> {
        let (year, code) = term.split_once('-').unwrap_or((term, ""));
        let open = self.open_sections(year, code).await?;
        let is_open: HashSet<&str> = open.iter().map(String::as_str).collect();
        Ok(ids
            .iter()
            .map(|id| Seats {
                id: id.clone(),
                available: i64::from(is_open.contains(id.as_str())),
                ..Default::default()
            })
            .collect())
    }

    fn version(&self) -> &str {
        "rutgers-soc-1"
    }
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use super::*;
    use crate::adapters::testserver::{Response, serve};

    const COURSES: &[u8] = include_bytes!("fixture_courses.json");
    const OPEN: &[u8] = include_bytes!("fixture_open.json");

    #[tokio::test]
    async fn contract() {
        let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        gz.write_all(COURSES).unwrap();
        let gz = gz.finish().unwrap();
        let base = serve(move |r| {
            if r.q("campus") != "NB" || r.q("year").is_empty() {
                return Response::error(400, "bad params");
            }
            match r.path.as_str() {
                "/soc/api/openSections.json" if r.q("term") == "9" => Response::ok(OPEN),
                "/soc/api/openSections.json" => Response::ok("[]"),
                // A gzip body without Content-Encoding, like the real feed.
                "/soc/api/courses.json" => Response::ok(gz.clone()),
                _ => Response::error(404, "not found"),
            }
        })
        .await;
        let mut r = Rutgers::new(Http::unpaced(), "NB");
        r.base = format!("{base}/soc/api");
        let terms = r.terms().await.unwrap();
        assert!(terms.len() == 1 && terms[0].name.starts_with("Fall"), "terms: {terms:?}");
        let secs = r.catalog(&terms[0].id).await.unwrap();
        assert_eq!(secs.len(), 4, "catalog");
        let s = &secs[0];
        assert!(
            s.id == "11389"
                && s.course == "01:198:111"
                && s.subject == "Computer Science"
                && s.credits == 4.0
                && s.available == 0
                && s.kind == "LEC",
            "bad section: {s:?}"
        );
        assert!(
            s.meetings.len() >= 2
                && s.meetings[0].days == "T"
                && s.meetings[0].start == 840
                && s.meetings[0].end == 920
                && s.meetings[1].days == "R"
                && s.meetings[0].location == "TIL 232 (LIV)",
            "bad meetings: {:?}",
            s.meetings
        );
        assert!(
            s.exams.len() == 1
                && s.exams[0].date == "2026-12-16"
                && s.exams[0].start == 16 * 60
                && s.exams[0].end == 19 * 60,
            "bad final exam: {:?}",
            s.exams
        );
        let seats = r.availability(&terms[0].id, &["11389".into(), "00007".into()]).await.unwrap();
        assert!(seats.len() == 2 && seats[0].available == 0 && seats[1].available == 1, "availability: {seats:?}");
    }

    #[test]
    fn final_exam_notes() {
        let cases: [(&str, Option<(i64, i64)>); 6] = [
            ("12/16/2026 - 0400 - 0700 PM", Some((16 * 60, 19 * 60))),
            ("12/18/2026 - 0800 - 1100 AM", Some((8 * 60, 11 * 60))),
            // The meridiem belongs to the end.
            ("12/18/2026 - 1100 - 0200 PM", Some((11 * 60, 14 * 60))),
            ("12/18/2026 - 1200 - 0300 PM", Some((12 * 60, 15 * 60))),
            ("", None),
            ("TBA", None),
        ];
        for (note, want) in cases {
            let got = final_exam(note).map(|e| (e.start, e.end));
            assert_eq!(got, want, "{note:?}");
        }
    }
}
