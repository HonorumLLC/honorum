//! The University of Oklahoma's ClassNav (`classnav.ou.edu`), a public class
//! search whose DataTables backend returns JSON without any login or
//! session. It is not run by the Registrar, so the adapter treats it as a
//! public machine-readable endpoint behind the catalog and fetches politely.
//!
//! Term codes and subject codes come from the search page, read once and
//! cached. `index_ajax.php` returns 21-column rows: course information,
//! meeting patterns (column 13), dates, seat data, and exam sittings
//! (column 17, in the meeting pattern format with kind `EXAM`). The endpoint
//! has no CRN filter, so a catalog fetches every subject one request at a
//! time with a 200 ms pause, and availability re-queries only the subjects
//! that hold watched sections. That section-to-subject routing is the
//! adapter's persisted state. Credits come from the last digit of the
//! course number. Each request times out after 90 seconds.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::sync::{LazyLock, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use chrono::NaiveDate;
use regex::Regex;
use reqwest::StatusCode;
use reqwest::header::ACCEPT;
use serde::Deserialize;
use serde_json::Value;
use tokio::sync::OnceCell;

use super::{decode_error, iso, null_default as nd, status_text, upper_prefix3};
use crate::{Adapter, AdapterError, AdapterState, Exam, Http, Meeting, Seats, Section, Term};

/// The pause between per-subject requests.
const PAUSE: Duration = Duration::from_millis(200);
const TIMEOUT: Duration = Duration::from_secs(90);

static RE_TERM: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"value="([0-9]{6})"[^\n]*>([^<\n]+)</option>"#).expect("valid regex"));
static RE_SUBJECT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"value="([^"]+)" <% if\(subject_code"#).expect("valid regex"));
static RE_SEATS: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"failsafe='([0-9]+) out of ([0-9]+)'").expect("valid regex"));
static RE_WAIT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"([0-9]+) Waiting").expect("valid regex"));
static RE_LAB: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^Lab-.*-([0-9]{3})$").expect("valid regex"));
static RE_DATES: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^([0-9A-Za-z_]{3}) ([0-9]{1,2}) - ([0-9A-Za-z_]{3}) ([0-9]{1,2})$").expect("valid regex")
});
static RE_TAG: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"<[^>]+>").expect("valid regex"));

/// Section routing by term, then CRN, to the subject code.
type Routing = BTreeMap<String, BTreeMap<String, String>>;

/// The search form's term and subject lists.
struct Form {
    /// Subject codes in page order.
    subjects: Vec<String>,
    /// Term code to label.
    terms: HashMap<String, String>,
}

/// OU ClassNav.
pub(super) struct Ou {
    /// The site root. Tests point it at a fixture server.
    pub base: String,
    http: Http,
    form: OnceCell<Form>,
    subject_of: Mutex<Routing>,
}

impl Ou {
    pub(super) fn new(http: Http) -> Self {
        Self {
            base: "https://classnav.ou.edu".into(),
            http,
            form: OnceCell::new(),
            subject_of: Mutex::new(Routing::new()),
        }
    }

    /// Loads the term and subject lists from the search page once. A failed
    /// load is retried on the next call.
    async fn form(&self) -> Result<&Form, AdapterError> {
        self.form
            .get_or_try_init(|| async {
                let html = self.get(&format!("{}/", self.base), None).await?;
                let terms: HashMap<String, String> =
                    RE_TERM.captures_iter(&html).map(|m| (m[1].to_owned(), m[2].trim().to_owned())).collect();
                let subjects: Vec<String> = RE_SUBJECT.captures_iter(&html).map(|m| m[1].to_owned()).collect();
                if terms.is_empty() || subjects.is_empty() {
                    return Err(AdapterError::new("ou: could not parse ClassNav search form"));
                }
                Ok(Form { subjects, terms })
            })
            .await
    }

    fn params(term: &str, subject: &str, length: usize) -> Vec<(&'static str, String)> {
        let mut params = vec![
            ("sEcho", "1".to_owned()),
            ("iDisplayStart", "0".to_owned()),
            ("iDisplayLength", length.to_string()),
            ("semester", term.to_owned()),
            ("subject", subject.to_owned()),
            ("schedule", "all".to_owned()),
            ("delivery", "all".to_owned()),
            ("gened", "all".to_owned()),
            ("term", "all".to_owned()),
            // available=true would mean "open seats only" and hide every full section.
            ("available", "false".to_owned()),
            ("waitlist", "true".to_owned()),
            ("attribute", "all".to_owned()),
            ("gradCredit", "all".to_owned()),
        ];
        params.sort_by_key(|p| p.0);
        params
    }

    async fn count(&self, term: &str) -> Result<i64, AdapterError> {
        let body = self.get(&format!("{}/index_ajax.php", self.base), Some(Self::params(term, "all", 1))).await?;
        let r: AjaxResponse = serde_json::from_str(&body).map_err(decode_error)?;
        Ok(r.total)
    }

    async fn rows(&self, term: &str, subject: &str, length: usize) -> Result<Vec<Section>, AdapterError> {
        let body =
            self.get(&format!("{}/index_ajax.php", self.base), Some(Self::params(term, subject, length))).await?;
        let r: AjaxResponse = serde_json::from_str(&body).map_err(decode_error)?;
        let year = term_year(term);
        Ok(r.data.iter().filter(|row| row.len() >= 21).map(|row| self.convert(row, year)).collect())
    }

    fn remember(&self, term: &str, secs: &[Section]) {
        let m = secs.iter().map(|s| (s.id.clone(), s.subject.clone())).collect();
        self.subject_of.lock().expect("ou routing lock").insert(term.to_owned(), m);
    }

    fn convert(&self, row: &[Value], year: i32) -> Section {
        let (crn, subj, num, sec, title) = (cell(row, 1), cell(row, 2), cell(row, 3), cell(row, 4), cell(row, 5));
        let kind = cell(row, 7);
        let mut s = Section {
            id: crn,
            course: format!("{subj} {num}"),
            subject: subj,
            title: title.clone(),
            number: sec.clone(),
            kind: type_code(&kind),
            source_url: format!("{}/", self.base),
            ..Default::default()
        };
        s.instructors = cell(row, 15).split(',').map(str::trim).filter(|n| !n.is_empty()).map(str::to_owned).collect();
        if let Some(m) = RE_SEATS.captures(&cell(row, 12)) {
            s.available = m[1].parse().unwrap_or(0);
            s.capacity = m[2].parse().unwrap_or(0);
        }
        if let Some(m) = RE_WAIT.captures(&cell(row, 16)) {
            s.waitlist = m[1].parse().unwrap_or(0);
        }
        // OU encodes credit hours in the last digit of the course number.
        if !kind.contains("No Credit") && num.len() == 4 {
            s.credits = f64::from(num.as_bytes()[3].wrapping_sub(b'0'));
        }
        if let Some(m) = RE_DATES.captures(&cell(row, 11)) {
            s.start = date(year, &m[1], &m[2]);
            s.end = date(year, &m[3], &m[4]);
        }
        // Labs are titled "Lab-SUBJ NUM-SEC" after their lecture section;
        // lectures with a co-requisite note anchor the group.
        let notes = cell(row, 14);
        if let Some(m) = RE_LAB.captures(&title) {
            s.link = m[1].to_owned();
        } else if notes.to_uppercase().contains("CO-REQUISITE") {
            s.link = sec;
        }
        let delivery = cell(row, 8);
        s.mode = mode(&delivery);
        s.attributes = [cell(row, 9), cell(row, 20)].into_iter().filter(|a| !a.is_empty()).collect();
        (s.description, s.prereqs) = split_prereqs(&notes);
        let online = delivery.to_lowercase().contains("online");
        // Column 13 holds the weekly patterns and column 17 the dated exam
        // sittings, both as "start, end, from, to, building, room, days,
        // kind" rows joined with "#".
        let patterns = format!("{}#{}", cell(row, 13), cell(row, 17));
        for part in patterns.split('#') {
            let f: Vec<&str> = part.split(',').map(str::trim).collect();
            if f.len() < 8 {
                continue;
            }
            let (days, start, end) = (f[f.len() - 2], hm(f[2]), hm(f[3]));
            if days.is_empty() || start == 0 || end <= start {
                continue;
            }
            let mut location = f[4..f.len() - 2].join(" ").trim().to_owned();
            if location.is_empty() && online {
                location = "Online".into();
            }
            if f[f.len() - 1] == "EXAM" {
                let date = month_day(year, f[0]);
                if !date.is_empty() && date == month_day(year, f[1]) {
                    s.exams.push(Exam { date, start, end, location });
                }
                continue;
            }
            s.meetings.push(Meeting { days: days.to_owned(), start, end, location });
        }
        s
    }

    async fn get(&self, url: &str, query: Option<Vec<(&'static str, String)>>) -> Result<String, AdapterError> {
        let mut request = self.http.get(url).header(ACCEPT, "application/json, text/html");
        if let Some(q) = query {
            request = request.query(&q);
        }
        let (status, body) = self.http.send_any(request, TIMEOUT).await?;
        if status != StatusCode::OK {
            let path = url::Url::parse(url).map(|u| u.path().to_owned()).unwrap_or_default();
            return Err(AdapterError::new(format!("ou: {path} -> {}", status_text(status))));
        }
        Ok(String::from_utf8_lossy(&body).into_owned())
    }
}

#[derive(Deserialize)]
struct AjaxResponse {
    #[serde(rename = "iTotalRecords", default, deserialize_with = "nd")]
    total: i64,
    #[serde(rename = "aaData", default, deserialize_with = "nd")]
    data: Vec<Vec<Value>>,
}

/// A trimmed string cell; empty for `null` and non-string cells.
fn cell(row: &[Value], i: usize) -> String {
    row[i].as_str().unwrap_or("").trim().to_owned()
}

/// The calendar year a term's dates fall in: `202610` is Fall 2026, while
/// `202620` and `202630` are Spring and Summer 2027.
fn term_year(term: &str) -> i32 {
    let mut year = term.get(..4).and_then(|y| y.parse().ok()).unwrap_or(0);
    if term.len() == 6 && term.as_bytes()[4] != b'1' {
        year += 1;
    }
    year
}

/// Reads `Dec 17` in the term's year.
fn month_day(year: i32, s: &str) -> String {
    match s.split_once(' ') {
        Some((mon, day)) => date(year, mon, day.trim()),
        None => String::new(),
    }
}

fn mode(delivery: &str) -> String {
    let d = delivery.to_lowercase();
    if d.contains("online") || d.contains("web") {
        "Online".into()
    } else if d.contains("blend") || d.contains("hybrid") {
        "Hybrid".into()
    } else if d.is_empty() {
        String::new()
    } else {
        "In person".into()
    }
}

/// Separates the `Prerequisite: ...` note from a course narrative, after
/// turning `<br />` into newlines and removing other tags.
fn split_prereqs(text: &str) -> (String, String) {
    let text = RE_TAG.replace_all(&text.replace("<br />", "\n"), "").trim().to_owned();
    match text.find("Prerequisite") {
        Some(i) => (text[..i].trim().to_owned(), text[i..].trim().to_owned()),
        None => (text, String::new()),
    }
}

const MONTHS: [&str; 12] = ["jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec"];

/// Builds `YYYY-MM-DD` from an abbreviated month name (any case) and a one-
/// or two-digit day; empty when the date does not exist.
fn date(year: i32, mon: &str, day: &str) -> String {
    // Only four-digit years produce a date.
    if !(1000..=9999).contains(&year) {
        return String::new();
    }
    let Some(month) = MONTHS.iter().position(|m| m.eq_ignore_ascii_case(mon)) else { return String::new() };
    if day.is_empty() || day.len() > 2 || !day.bytes().all(|b| b.is_ascii_digit()) {
        return String::new();
    }
    let day: u32 = day.parse().unwrap_or(0);
    NaiveDate::from_ymd_opt(year, month as u32 + 1, day).map(iso).unwrap_or_default()
}

/// Parses `2:00 pm` into minutes after midnight; zero otherwise.
fn hm(s: &str) -> i64 {
    let s = s.to_lowercase();
    let b = s.as_bytes();
    let hour_len = match b {
        [h1, h2, b':', ..] if h1.is_ascii_digit() && h2.is_ascii_digit() => 2,
        [h, b':', ..] if h.is_ascii_digit() => 1,
        _ => return 0,
    };
    let rest = &b[hour_len + 1..];
    let [m1, m2, rest @ ..] = rest else { return 0 };
    if !m1.is_ascii_digit() || !m2.is_ascii_digit() {
        return 0;
    }
    let mut hour: i64 = std::str::from_utf8(&b[..hour_len]).ok().and_then(|h| h.parse().ok()).unwrap_or(99);
    let minute = i64::from((m1 - b'0') * 10 + (m2 - b'0'));
    if hour > 12 || minute > 59 {
        return 0;
    }
    // One or more spaces separate the time from the meridiem.
    let spaces = rest.iter().take_while(|&&c| c == b' ').count();
    if spaces == 0 {
        return 0;
    }
    match &rest[spaces..] {
        b"pm" if hour < 12 => hour += 12,
        b"pm" => {}
        b"am" if hour == 12 => hour = 0,
        b"am" => {}
        _ => return 0,
    }
    hour * 60 + minute
}

fn type_code(kind: &str) -> String {
    let k = kind.to_lowercase();
    let code = if k.starts_with("lecture/lab") || k.starts_with("lec") {
        "LEC"
    } else if k.starts_with("lab") {
        "LAB"
    } else if k.starts_with("dis") {
        "DIS"
    } else if k.starts_with("sem") {
        "SEM"
    } else if k.starts_with("independent") {
        "IND"
    } else if k.starts_with("thesis") {
        "THS"
    } else if k.is_empty() {
        "LEC"
    } else {
        return upper_prefix3(kind);
    };
    code.into()
}

#[async_trait]
impl Adapter for Ou {
    /// Returns the three most recent semesters, newest first.
    async fn terms(&self) -> Result<Vec<Term>, AdapterError> {
        let form = self.form().await?;
        let mut codes: Vec<&String> = form.terms.keys().collect();
        codes.sort_by(|a, b| b.cmp(a));
        Ok(codes
            .into_iter()
            .take(3)
            .map(|c| Term { id: c.clone(), name: form.terms[c].clone(), ..Default::default() })
            .collect())
    }

    /// Fetches every subject one request at a time. A term with no rows
    /// returns no sections.
    async fn catalog(&self, term: &str) -> Result<Vec<Section>, AdapterError> {
        let form = self.form().await?;
        if self.count(term).await? == 0 {
            return Ok(Vec::new());
        }
        let mut out = Vec::new();
        for (i, subject) in form.subjects.iter().enumerate() {
            if i > 0 {
                tokio::time::sleep(PAUSE).await;
            }
            let rows = self
                .rows(term, subject, 2000)
                .await
                .map_err(|err| AdapterError::new(format!("ou: {term} {subject}: {err}")))?;
            out.extend(rows);
        }
        self.remember(term, &out);
        Ok(out)
    }

    /// Re-queries only the subjects that contain watched CRNs.
    async fn availability(&self, term: &str, ids: &[String]) -> Result<Vec<Seats>, AdapterError> {
        let want: HashSet<&str> = ids.iter().map(String::as_str).collect();
        let subjects: BTreeSet<String> = {
            let routing = self.subject_of.lock().expect("ou routing lock");
            ids.iter().filter_map(|id| routing.get(term).and_then(|m| m.get(id)).cloned()).collect()
        };
        let mut out = Vec::new();
        for (i, subject) in subjects.iter().enumerate() {
            if i > 0 {
                tokio::time::sleep(PAUSE).await;
            }
            for s in self.rows(term, subject, 2000).await? {
                if want.contains(s.id.as_str()) {
                    out.push(Seats { id: s.id, capacity: s.capacity, available: s.available, waitlist: s.waitlist });
                }
            }
        }
        Ok(out)
    }

    fn version(&self) -> &str {
        "ou-classnav-1"
    }

    fn state(&self) -> Option<&dyn AdapterState> {
        Some(self)
    }
}

/// The persisted state is the section-to-subject routing, as JSON
/// `{term: {crn: subject}}`.
impl AdapterState for Ou {
    fn save_state(&self) -> Result<Vec<u8>, AdapterError> {
        let routing = self.subject_of.lock().expect("ou routing lock");
        serde_json::to_vec(&*routing).map_err(|err| AdapterError::new(err.to_string()))
    }

    fn load_state(&self, data: &[u8]) -> Result<(), AdapterError> {
        let state: Option<Routing> = serde_json::from_slice(data).map_err(|err| AdapterError::new(err.to_string()))?;
        *self.subject_of.lock().expect("ou routing lock") = state.unwrap_or_default();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::testserver::{Response, serve};

    const INDEX: &str = include_str!("fixture_index.html");
    const CS: &str = include_str!("fixture_cs.json");

    /// Serves recorded ClassNav responses (Fall 2026, subject `C S`).
    async fn fixture_server() -> String {
        serve(|r| {
            if r.path != "/index_ajax.php" {
                return Response::ok(format!("<title>ClassNav</title>{INDEX}"));
            }
            // available=true hides full sections.
            if r.q("semester") != "202610" || r.q("available") != "false" {
                return Response::error(400, "bad params");
            }
            match r.q("subject") {
                "C S" | "all" => Response::ok(CS),
                _ => Response::ok(r#"{"sEcho":1,"iTotalRecords":0,"iTotalDisplayRecords":"0","aaData":[]}"#),
            }
        })
        .await
    }

    #[tokio::test]
    async fn contract() {
        let base = fixture_server().await;
        let mut o = Ou::new(Http::unpaced());
        o.base = base;

        let terms = o.terms().await.unwrap();
        assert!(terms.len() == 3 && terms[0].id == "202630" && terms[2].name == "Fall 2026", "terms: {terms:?}");
        let secs = o.catalog("202610").await.unwrap();
        assert_eq!(secs.len(), 4, "catalog");
        let lec = &secs[0];
        assert!(
            lec.id == "38728"
                && lec.course == "C S 1213"
                && lec.number == "010"
                && lec.kind == "LEC"
                && lec.title == "Programming with Python",
            "bad lecture: {lec:?}"
        );
        assert!(
            lec.available == 27 && lec.capacity == 100 && lec.waitlist == 0 && lec.credits == 3.0 && lec.link == "010",
            "bad lecture seats/credits/link: {lec:?}"
        );
        assert!(lec.start == "2026-08-24" && lec.end == "2026-12-11", "bad dates: {} {}", lec.start, lec.end);
        assert!(
            lec.meetings.len() == 1
                && lec.meetings[0].days == "MWF"
                && lec.meetings[0].start == 840
                && lec.meetings[0].end == 890
                && lec.meetings[0].location == "Felgar Hall 304",
            "bad meeting: {:?}",
            lec.meetings
        );
        let ex = &lec.exams;
        assert!(
            ex.len() == 1
                && ex[0].date == "2026-12-17"
                && ex[0].start == 630
                && ex[0].end == 750
                && ex[0].location == "Felgar Hall 304",
            "bad exam: {ex:?}"
        );
        assert_eq!(lec.instructors[0], "Sanjana Mudduluru", "bad instructor");
        let lab = &secs[1];
        assert!(
            lab.kind == "LAB"
                && lab.link == "010"
                && lab.credits == 0.0
                && lab.available == 2
                && lab.meetings[0].days == "F"
                && lab.exams.is_empty(),
            "bad lab: {lab:?}"
        );
        let multi = &secs[2];
        assert!(
            multi.meetings.len() == 2
                && multi.exams.len() == 1
                && multi.exams[0].date == "2026-12-16"
                && multi.exams[0].start == 810
                && multi.exams[0].location == "Physical Science Ctr 108",
            "expected two meeting patterns and one exam: {multi:?}"
        );
        let ind = &secs[3];
        assert!(ind.kind == "IND" && ind.meetings.is_empty(), "independent study should have no meetings: {ind:?}");
        let seats = o.availability("202610", &["38729".into(), "nope".into()]).await.unwrap();
        assert!(seats.len() == 1 && seats[0].available == 2 && seats[0].capacity == 50, "availability: {seats:?}");
        assert_eq!(term_year("202610"), 2026);
        assert_eq!(term_year("202620"), 2027);
        assert_eq!(term_year("202530"), 2026);
    }

    #[test]
    fn clock_and_date_parsing() {
        assert_eq!(hm("2:00 pm"), 840);
        assert_eq!(hm("10:30 AM"), 630);
        assert_eq!(hm("12:15 am"), 15);
        assert_eq!(hm("12:30 pm"), 750);
        assert_eq!(hm("2:00pm"), 0);
        assert_eq!(hm("13:00 pm"), 0);
        assert_eq!(hm("TBA"), 0);
        assert_eq!(date(2026, "Aug", "24"), "2026-08-24");
        assert_eq!(date(2026, "feb", "30"), "");
        assert_eq!(month_day(2026, "Dec 17"), "2026-12-17");
    }
}
