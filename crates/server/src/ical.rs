//! A minimal iCalendar reader for assignment feeds: VEVENT and VTODO items
//! with a date. It mirrors the browser's parser so a feed imported on the
//! server produces the task identities a guest import produced locally.
//!
//! This module also holds the campus-time helpers the planner modules share:
//! [`load_location`] resolves zone names, and [`civil_instant`] turns a
//! campus wall-clock time into an instant, normalizing out-of-range fields
//! and resolving DST gaps and repeats.

use std::collections::HashMap;
use std::sync::LazyLock;

use chrono::{DateTime, NaiveDate, Offset, TimeDelta, TimeZone, Utc};
use chrono_tz::Tz;
use regex::Regex;

use crate::lms::Task;

static SUMMARY_COURSE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(.*?)[\t\n\f\r ]*\[([^\]]+)\][\t\n\f\r ]*$").expect("valid pattern"));
static DATE_PATTERN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^([0-9]{4})([0-9]{2})([0-9]{2})(?:T([0-9]{2})([0-9]{2})([0-9]{2})?(Z)?)?$").expect("valid pattern")
});

/// Resolves an IANA zone name: an empty name and "UTC" are UTC, and "Local" is
/// UTC because servers run in UTC. `None` for an unknown name.
pub(crate) fn load_location(name: &str) -> Option<Tz> {
    match name {
        "" | "UTC" | "Local" => Some(Tz::UTC),
        _ => name.parse().ok(),
    }
}

/// The UTC offset in seconds that `zone` applies at Unix second `at`.
fn offset_at(zone: Tz, at: i64) -> i64 {
    let Some(instant) = DateTime::from_timestamp(at, 0) else { return 0 };
    i64::from(zone.offset_from_utc_datetime(&instant.naive_utc()).fix().local_minus_utc())
}

/// The instant of a wall-clock time in `zone`. Out-of-range fields are
/// normalized (month 13 is January of the next year, minute 60 is the next
/// hour). A time skipped by a DST change takes the offset in effect after it,
/// and a repeated time resolves to its first occurrence. `None` only for years
/// outside chrono's range.
pub(crate) fn civil_instant(
    zone: Tz,
    year: i64,
    month: i64,
    day: i64,
    hour: i64,
    minute: i64,
    second: i64,
) -> Option<DateTime<Utc>> {
    let months = month - 1;
    let year = i32::try_from(year + months.div_euclid(12)).ok()?;
    let first = NaiveDate::from_ymd_opt(year, u32::try_from(months.rem_euclid(12) + 1).ok()?, 1)?;
    let wall =
        first.and_hms_opt(0, 0, 0)?.and_utc().timestamp() + (day - 1) * 86_400 + hour * 3600 + minute * 60 + second;
    // Look up the offset at the wall time read as UTC, then use the offset
    // in effect at the instant that offset implies.
    let offset = offset_at(zone, wall - offset_at(zone, wall));
    DateTime::from_timestamp(wall - offset, 0)
}

/// Midnight starting `date` in `zone`, resolved as [`civil_instant`] does.
pub(crate) fn local_midnight(zone: Tz, date: NaiveDate) -> Option<DateTime<Utc>> {
    use chrono::Datelike;
    civil_instant(zone, i64::from(date.year()), i64::from(date.month()), i64::from(date.day()), 0, 0, 0)
}

/// Formats an instant as RFC 3339 in UTC with whole seconds:
/// `2026-09-10T23:59:00Z`.
pub(crate) fn rfc3339_utc(at: DateTime<Utc>) -> String {
    at.format("%Y-%m-%dT%H:%M:%SZ").to_string()
}

/// Parses a `YYYY-MM-DD` date strictly: four-digit year, two-digit month
/// and day, nothing else.
pub(crate) fn parse_civil_date(value: &str) -> Option<NaiveDate> {
    let b = value.as_bytes();
    if b.len() != 10 || b[4] != b'-' || b[7] != b'-' {
        return None;
    }
    let digits = |r: std::ops::Range<usize>| -> Option<u32> {
        b[r].iter().try_fold(0u32, |n, c| c.is_ascii_digit().then(|| n * 10 + u32::from(c - b'0')))
    };
    NaiveDate::from_ymd_opt(i32::try_from(digits(0..4)?).ok()?, digits(5..7)?, digits(8..10)?)
}

/// Replaces iCalendar text escapes (`\n`, `\N`, `\,`, `\;`, `\\`) and trims
/// surrounding white space.
fn unescape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.peek() {
                Some('n' | 'N') => {
                    chars.next();
                    out.push(' ');
                    continue;
                }
                Some(&next @ (',' | ';' | '\\')) => {
                    chars.next();
                    out.push(next);
                    continue;
                }
                _ => {}
            }
        }
        out.push(c);
    }
    out.trim().to_owned()
}

/// Converts "20260910T235900Z", a floating "20260910T235900", or a bare
/// "20260910" to RFC 3339 UTC (`2026-09-10T23:59:00Z`). Floating times are
/// read in `zone`; a whole-day item is due at 23:59 that day. Anything else
/// yields an empty string.
pub(crate) fn ics_date(value: &str, zone: Tz) -> String {
    let Some(m) = DATE_PATTERN.captures(value.trim()) else { return String::new() };
    let field = |i: usize, default: i64| -> i64 {
        m.get(i).map_or(default, |g| g.as_str().bytes().fold(0, |n, c| n * 10 + i64::from(c - b'0')))
    };
    let zone = if m.get(7).is_some() { Tz::UTC } else { zone };
    civil_instant(zone, field(1, 0), field(2, 1), field(3, 1), field(4, 23), field(5, 59), field(6, 0))
        .map(rfc3339_utc)
        .unwrap_or_default()
}

/// Turns a feed into tasks with UIDs `source:UID` (or `source:title+due`
/// without a UID). Titles like "Homework 3 [CS 1213-010]" put the
/// bracketed part in the course; otherwise `label` is the course. An item's
/// `TZID` overrides `zone` for its date. The first occurrence of a property
/// wins; folded lines (leading space or tab) are joined.
pub(crate) fn parse_ics(text: &str, source: &str, label: &str, zone: Tz) -> Vec<Task> {
    let normalized = text.replace("\r\n", "\n");
    let mut lines: Vec<String> = Vec::new();
    for raw in normalized.split('\n') {
        match lines.last_mut() {
            Some(last) if raw.starts_with(' ') || raw.starts_with('\t') => last.push_str(&raw[1..]),
            _ => lines.push(raw.to_owned()),
        }
    }
    let mut tasks = Vec::new();
    let mut current: Option<HashMap<String, String>> = None;
    for line in &lines {
        match line.as_str() {
            "BEGIN:VEVENT" | "BEGIN:VTODO" => {
                current = Some(HashMap::new());
                continue;
            }
            "END:VEVENT" | "END:VTODO" => {
                if let Some(item) = current.take()
                    && let Some(task) = item_task(&item, source, label, zone)
                {
                    tasks.push(task);
                }
                continue;
            }
            _ => {}
        }
        let Some(item) = current.as_mut() else { continue };
        let Some(i) = line.find(':') else { continue };
        let mut params = line[..i].split(';');
        let key = params.next().unwrap_or_default().to_owned();
        if item.contains_key(&key) {
            continue;
        }
        item.insert(key.clone(), line[i + 1..].to_owned());
        for p in params {
            if let Some(tzid) = p.strip_prefix("TZID=") {
                item.insert(format!("{key};TZID"), tzid.trim_matches('"').to_owned());
            }
        }
    }
    tasks
}

/// One finished VEVENT or VTODO as a task; `None` without a summary.
fn item_task(item: &HashMap<String, String>, source: &str, label: &str, zone: Tz) -> Option<Task> {
    let get = |k: &str| item.get(k).map(String::as_str).unwrap_or_default();
    if get("SUMMARY").is_empty() {
        return None;
    }
    let mut due = "";
    let mut where_ = zone;
    for key in ["DUE", "DTSTART", "DTEND"] {
        due = get(key);
        if !due.is_empty() {
            let tzid = get(&format!("{key};TZID"));
            if !tzid.is_empty()
                && let Some(z) = load_location(tzid)
            {
                where_ = z;
            }
            break;
        }
    }
    let summary = unescape(get("SUMMARY"));
    let (title, course) = match SUMMARY_COURSE.captures(&summary) {
        Some(m) => (m[1].trim().to_owned(), m[2].trim().to_owned()),
        None => (summary.clone(), label.to_owned()),
    };
    let uid = match get("UID") {
        "" => format!("{title}{due}"),
        uid => uid.to_owned(),
    };
    Some(Task {
        uid: format!("{source}:{uid}"),
        title,
        course,
        due: ics_date(due, where_),
        done: get("STATUS") == "COMPLETED",
        url: get("URL").to_owned(),
        ..Task::default()
    })
}

/// Drops tasks due more than fourteen days before `now`; undated tasks stay.
pub(crate) fn recent_tasks(mut tasks: Vec<Task>, now: DateTime<Utc>) -> Vec<Task> {
    let cutoff = rfc3339_utc(now - TimeDelta::days(14));
    tasks.retain(|t| t.due.is_empty() || t.due >= cutoff);
    tasks
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) const FIXTURE_ICS: &str = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nBEGIN:VEVENT\r\nUID:hw-1\r\nDTSTART:20991115T170000Z\r\nSUMMARY:Homework 3 [CS 1213-010]\r\nURL:https://canvas.example/hw1\r\nEND:VEVENT\r\nBEGIN:VEVENT\r\nUID:folded-1\r\nDTSTART;TZID=America/Chicago:20991201T235900\r\nSUMMARY:A very long assignment title that the\r\n  feed wrapped onto a second line\r\nSTATUS:COMPLETED\r\nEND:VEVENT\r\nBEGIN:VEVENT\r\nUID:old-1\r\nDTSTART:20200101T120000Z\r\nSUMMARY:Long past\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";

    fn instant(v: &str) -> Option<DateTime<Utc>> {
        DateTime::parse_from_rfc3339(v).ok().map(|t| t.with_timezone(&Utc))
    }

    // The shared fixture in contracts/fixtures is generated by the web
    // client; the server's parser must produce the same tasks for the same
    // feed.
    #[test]
    fn icalendar_contract_fixture() {
        #[derive(serde::Deserialize)]
        struct Expected {
            uid: String,
            title: String,
            course: String,
            due: String,
            url: String,
            done: bool,
        }
        #[derive(serde::Deserialize)]
        struct Case {
            name: String,
            label: String,
            now: String,
            text: String,
            expected: Vec<Expected>,
        }
        #[derive(serde::Deserialize)]
        struct Fixture {
            timezone: String,
            cases: Vec<Case>,
        }
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../contracts/fixtures/icalendar.json");
        let fixture: Fixture = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
        let zone = load_location(&fixture.timezone).unwrap();
        for c in fixture.cases {
            let now = instant(&c.now).unwrap();
            let got = recent_tasks(parse_ics(&c.text, "feed", &c.label, zone), now);
            assert_eq!(got.len(), c.expected.len(), "{}: {got:?}", c.name);
            for (i, (task, want)) in got.iter().zip(&c.expected).enumerate() {
                assert!(
                    task.uid == want.uid
                        && task.title == want.title
                        && task.course == want.course
                        && task.url == want.url
                        && task.done == want.done
                        && instant(&task.due) == instant(&want.due),
                    "{}: task {i} = {task:?}, expected uid {} title {} course {} due {} url {} done {}",
                    c.name,
                    want.uid,
                    want.title,
                    want.course,
                    want.due,
                    want.url,
                    want.done
                );
            }
        }
    }

    #[test]
    fn icalendar_parser_matches_the_browser() {
        let chicago: Tz = "America/Chicago".parse().unwrap();
        let now = Utc.with_ymd_and_hms(2026, 9, 1, 0, 0, 0).unwrap();
        let tasks = recent_tasks(parse_ics(FIXTURE_ICS, "feed", "ENGL 1213", chicago), now);
        assert_eq!(tasks.len(), 2, "tasks: {tasks:?}");
        let t = &tasks[0];
        assert!(
            t.uid == "feed:hw-1"
                && t.title == "Homework 3"
                && t.course == "CS 1213-010"
                && t.due == "2099-11-15T17:00:00Z"
                && t.url == "https://canvas.example/hw1"
                && !t.done,
            "first task: {t:?}"
        );
        // A floating time is read in the feed's campus time zone (CST is UTC-6).
        let t = &tasks[1];
        assert!(
            t.due == "2099-12-02T05:59:00Z"
                && t.title.contains("wrapped onto a second line")
                && t.course == "ENGL 1213"
                && t.done,
            "second task: {t:?}"
        );
        assert_eq!(ics_date("20260910", Tz::UTC), "2026-09-10T23:59:00Z");
        assert_eq!(ics_date("nope", Tz::UTC), "");
    }

    // Normalization, the DST gap, and the repeated hour.
    #[test]
    fn civil_instants_normalize_and_resolve_dst() {
        let chicago: Tz = "America/Chicago".parse().unwrap();
        let at = |y, mo, d, h, mi| civil_instant(chicago, y, mo, d, h, mi, 0).map(rfc3339_utc).unwrap();
        // Spring forward: 02:30 does not exist and takes the later offset.
        assert_eq!(at(2026, 3, 8, 2, 30), "2026-03-08T07:30:00Z");
        // Fall back: 01:30 happens twice; the first (CDT) is chosen.
        assert_eq!(at(2026, 11, 1, 1, 30), "2026-11-01T06:30:00Z");
        assert_eq!(at(2026, 13, 1, 0, 0), "2027-01-01T06:00:00Z");
        assert_eq!(at(2026, 1, 32, 24, 60), "2026-02-02T07:00:00Z");
        assert_eq!(parse_civil_date("2026-02-30"), None);
        assert_eq!(parse_civil_date("2026-2-03"), None);
        assert!(parse_civil_date("2026-02-03").is_some());
        assert_eq!(unescape(r" a\, b\; c\\d\ne\x "), r"a, b; c\d e\x");
    }
}
