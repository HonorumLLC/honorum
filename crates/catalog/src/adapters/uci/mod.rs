//! UC Irvine's WebSoc schedule of classes (`www.reg.uci.edu/perl/WebSoc`),
//! whose search form has a public XML output mode with enrollment counts,
//! capacity, waitlist, and status per section.
//!
//! The search form supplies the terms (the three newest quarters) and the
//! department list, read once and cached. A catalog queries each department
//! with `Submit=XML`, `FullCourses=ANY`, and `CancelledCourses=Exclude`,
//! pausing 500 ms between departments. Departments with no offerings answer
//! with a short HTML note, which reads as no sections. A failed connection
//! is retried once after two seconds; each request times out after 90
//! seconds.
//!
//! The adapter persists section-to-department routing, so established
//! watches query only the relevant departments. Watched sections without
//! routing get a recovery scan of at most eight departments per poll, which
//! resumes where the previous poll stopped; the next catalog collection
//! refreshes the routing.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::{LazyLock, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use regex::Regex;
use reqwest::StatusCode;
use serde::{Deserialize, Serialize};
use tokio::sync::OnceCell;

use super::{status_text, unescape_html, xml};
use crate::{Adapter, AdapterError, AdapterState, Http, HttpError, Meeting, Seats, Section, Term, title_case};

const TIMEOUT: Duration = Duration::from_secs(90);
const RETRY_AFTER: Duration = Duration::from_secs(2);
/// The most departments one availability poll scans for unrouted sections.
const RECOVERY_LIMIT: usize = 8;

static RE_TERM: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"<option value="([0-9]{4}-[0-9]{2})"[^>]*>[\t\n\f\r ]*([^<]+?)[\t\n\f\r ]*<"#).expect("valid regex")
});
static RE_DEPT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"<option value="([^"]+)""#).expect("valid regex"));
static RE_WS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[\t\n\f\r ]+").expect("valid regex"));

/// The search form's term and department lists.
struct Form {
    terms: Vec<Term>,
    /// Department codes in form order.
    depts: Vec<String>,
}

/// Section routing and recovery progress.
#[derive(Default)]
struct Routing {
    /// Term, then section code, to department.
    dept_of: BTreeMap<String, BTreeMap<String, String>>,
    /// Term to the next department index a recovery scan visits.
    scan_at: BTreeMap<String, i64>,
    /// Whether routing changed since it was last saved.
    dirty: bool,
}

#[derive(Serialize, Deserialize)]
struct SavedRouting {
    #[serde(default)]
    departments: Option<BTreeMap<String, BTreeMap<String, String>>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    scan_at: Option<BTreeMap<String, i64>>,
}

/// UC Irvine WebSoc.
pub(super) struct Uci {
    /// The WebSoc URL. Tests point it at a fixture server.
    pub base: String,
    /// The pause between department requests. Tests set it to zero.
    pub pause: Duration,
    http: Http,
    form: OnceCell<Form>,
    routing: Mutex<Routing>,
}

impl Uci {
    pub(super) fn new(http: Http) -> Self {
        Self {
            base: "https://www.reg.uci.edu/perl/WebSoc".into(),
            pause: Duration::from_millis(500),
            http,
            form: OnceCell::new(),
            routing: Mutex::new(Routing::default()),
        }
    }

    /// Loads the term and department lists once. A failed load is retried
    /// on the next call.
    async fn form(&self) -> Result<&Form, AdapterError> {
        self.form
            .get_or_try_init(|| async {
                let s = self.get(&self.base).await?;
                let mut terms = Vec::new();
                for m in RE_TERM.captures_iter(&s) {
                    let name = RE_WS.replace_all(&m[2], " ").into_owned();
                    if name.contains("Quarter") && !name.contains("COM") {
                        terms.push(Term { id: m[1].to_owned(), name, ..Default::default() });
                    }
                    if terms.len() == 3 {
                        break;
                    }
                }
                let mut depts = Vec::new();
                if let Some(i) = s.find(r#"name="Dept""#) {
                    let mut block = &s[i..];
                    if let Some(j) = block.find("</select>").filter(|&j| j > 0) {
                        block = &block[..j];
                    }
                    for m in RE_DEPT.captures_iter(block) {
                        let d = unescape_html(&m[1]).trim().to_owned();
                        if !d.is_empty() && d != "ALL" {
                            depts.push(d);
                        }
                    }
                }
                if terms.is_empty() || depts.is_empty() {
                    return Err(AdapterError::new("uci: could not parse WebSoc form"));
                }
                Ok(Form { terms, depts })
            })
            .await
    }

    /// Fetches one department's sections.
    async fn dept(&self, term: &str, dept: &str) -> Result<Vec<Section>, AdapterError> {
        let query = url::form_urlencoded::Serializer::new(String::new())
            .append_pair("CancelledCourses", "Exclude")
            .append_pair("Dept", dept)
            .append_pair("FullCourses", "ANY")
            .append_pair("Submit", "XML")
            .append_pair("YearTerm", term)
            .finish();
        let body = self
            .get(&format!("{}?{query}", self.base))
            .await
            .map_err(|err| AdapterError::new(format!("uci: {dept}: {err}")))?;
        match self.parse_results(&body) {
            Ok(secs) => Ok(secs),
            // Departments with no offerings answer with a short HTML note.
            Err(_) if !body.trim().starts_with("<?xml") => Ok(Vec::new()),
            Err(err) => Err(AdapterError::new(format!("uci: {dept}: {err}"))),
        }
    }

    fn parse_results(&self, body: &str) -> Result<Vec<Section>, String> {
        let doc = xml::parse(body).map_err(|err| err.to_string())?;
        let mut out = Vec::new();
        for school in xml::all(doc.root_element(), &["course_list", "school"]) {
            for d in xml::all(school, &["department"]) {
                let dept_code = xml::attr(d, "dept_code");
                for c in xml::all(d, &["course"]) {
                    let number = xml::attr(c, "course_number");
                    let title = title_case(&xml::attr(c, "course_title"));
                    for sec in xml::all(c, &["section"]) {
                        let max = xml::int(sec, &["sec_enrollment", "sec_max_enroll"])?;
                        let enrolled = xml::int(sec, &["sec_enrollment", "sec_enrolled"])?;
                        let mut s = Section {
                            id: xml::text(sec, &["course_code"]),
                            course: format!("{dept_code} {number}"),
                            subject: dept_code.clone(),
                            title: title.clone(),
                            number: xml::text(sec, &["sec_num"]),
                            kind: xml::text(sec, &["sec_type"]).to_uppercase(),
                            capacity: max,
                            available: (max - enrolled).max(0),
                            credits: units(&xml::text(sec, &["sec_units"])),
                            source_url: self.base.clone(),
                            ..Default::default()
                        };
                        let modality = xml::text(sec, &["sec_modality"]).to_lowercase();
                        if modality.contains("online") || modality.contains("remote") {
                            s.mode = "Online".into();
                        } else if modality.contains("hybrid") {
                            s.mode = "Hybrid".into();
                        } else if !modality.is_empty() {
                            s.mode = "In person".into();
                        }
                        if let Ok(n) = xml::text(sec, &["sec_enrollment", "sec_waitlist"]).trim().parse() {
                            s.waitlist = n;
                        }
                        let status = xml::text(sec, &["sec_status"]).to_uppercase();
                        if status == "FULL" || status.starts_with("WAIT") {
                            s.available = 0;
                        }
                        for instructor in xml::all(sec, &["sec_instructors", "instructor"]) {
                            let name = xml::own_text(instructor).trim().to_owned();
                            if !name.is_empty() && name != "STAFF" {
                                s.instructors.push(name);
                            }
                        }
                        for m in xml::all(sec, &["sec_meetings", "sec_meet"]) {
                            let days = parse_days(&xml::text(m, &["sec_days"]));
                            let (start, end) = parse_time(&xml::text(m, &["sec_time"]));
                            if days.is_empty() || end <= start {
                                continue;
                            }
                            let location = format!("{} {}", xml::text(m, &["sec_bldg"]), xml::text(m, &["sec_room"]))
                                .trim()
                                .to_owned();
                            s.meetings.push(Meeting { days, start, end, location });
                        }
                        out.push(s);
                    }
                }
            }
        }
        Ok(out)
    }

    /// Fetches a page, retrying once after a short wait when the connection
    /// fails; WebSoc occasionally resets connections.
    async fn get(&self, url: &str) -> Result<String, AdapterError> {
        let (status, body) = match self.http.send_any(self.http.get(url), TIMEOUT).await {
            Ok(response) => response,
            Err(HttpError::Transport(_)) => {
                tokio::time::sleep(RETRY_AFTER).await;
                self.http.send_any(self.http.get(url), TIMEOUT).await?
            }
            Err(err) => return Err(err.into()),
        };
        if status != StatusCode::OK {
            return Err(AdapterError::new(format!("uci: {}", status_text(status))));
        }
        Ok(String::from_utf8_lossy(&body).into_owned())
    }

    fn encode_routing(routing: &Routing) -> Result<Vec<u8>, AdapterError> {
        let saved = SavedRouting {
            departments: Some(routing.dept_of.clone()),
            scan_at: (!routing.scan_at.is_empty()).then(|| routing.scan_at.clone()),
        };
        serde_json::to_vec(&saved).map_err(|err| AdapterError::new(err.to_string()))
    }
}

/// Converts WebSoc day names (`TuTh`, `MWF`, `Sa`) to `MTWRFSU` letters.
fn parse_days(s: &str) -> String {
    let b = s.trim().as_bytes();
    let mut out = String::new();
    let mut i = 0;
    while i < b.len() {
        match (b[i], b.get(i + 1)) {
            (b'T', Some(b'u')) => {
                out.push('T');
                i += 1;
            }
            (b'T', Some(b'h')) => {
                out.push('R');
                i += 1;
            }
            (b'S', Some(b'a')) => {
                out.push('S');
                i += 1;
            }
            (b'S', Some(b'u')) => {
                out.push('U');
                i += 1;
            }
            (c @ (b'M' | b'W' | b'F'), _) => out.push(char::from(c)),
            _ => {}
        }
        i += 1;
    }
    out
}

/// Reads WebSoc's ` 3:30- 4:50p` form: the suffix applies to the end time
/// and the start is the closest earlier reading of the same clock.
fn parse_time(s: &str) -> (i64, i64) {
    let compact = s.replace(' ', "");
    let parts: Vec<&str> = compact.split('-').collect();
    if parts.len() != 2 {
        return (0, 0);
    }
    let pm = parts[1].ends_with('p');
    let mut end = clock(parts[1].strip_suffix('p').unwrap_or(parts[1]));
    if pm && end < 12 * 60 {
        end += 12 * 60;
    }
    let mut start = clock(parts[0]);
    if start >= 12 * 60 {
        start -= 12 * 60;
    }
    if start + 12 * 60 < end {
        start += 12 * 60;
    }
    (start, end)
}

/// Reads `H:MM` as minutes; unparsable parts count as zero.
fn clock(s: &str) -> i64 {
    let Some((h, m)) = s.split_once(':') else { return 0 };
    h.parse::<i64>().unwrap_or(0) * 60 + m.parse::<i64>().unwrap_or(0)
}

/// Reads the low end of a units field such as `4` or `1-4`.
fn units(s: &str) -> f64 {
    s.trim().split('-').next().and_then(|u| u.parse().ok()).unwrap_or(0.0)
}

#[async_trait]
impl Adapter for Uci {
    async fn terms(&self) -> Result<Vec<Term>, AdapterError> {
        Ok(self.form().await?.terms.clone())
    }

    /// Queries every department and replaces the term's routing.
    async fn catalog(&self, term: &str) -> Result<Vec<Section>, AdapterError> {
        let form = self.form().await?;
        let mut out = Vec::new();
        let mut routing = BTreeMap::new();
        for (i, dept) in form.depts.iter().enumerate() {
            if i > 0 {
                tokio::time::sleep(self.pause).await;
            }
            let secs = self.dept(term, dept).await?;
            for sec in &secs {
                routing.insert(sec.id.clone(), dept.clone());
            }
            out.extend(secs);
        }
        let mut state = self.routing.lock().expect("uci routing lock");
        state.dept_of.insert(term.to_owned(), routing);
        state.dirty = true;
        state.scan_at.remove(term);
        Ok(out)
    }

    /// Uses persisted routing for known sections. Unknown or moved IDs get a
    /// bounded, rotating recovery scan instead of a full catalog every poll.
    async fn availability(&self, term: &str, ids: &[String]) -> Result<Vec<Seats>, AdapterError> {
        let form = self.form().await?;
        let mut want: HashSet<String> = ids.iter().cloned().collect();
        let (known, mut cursor) = {
            let state = self.routing.lock().expect("uci routing lock");
            let routes = state.dept_of.get(term);
            let known: HashSet<String> =
                ids.iter().filter_map(|id| routes.and_then(|r| r.get(id)).filter(|d| !d.is_empty()).cloned()).collect();
            (known, state.scan_at.get(term).copied().unwrap_or(0))
        };
        let mut out = Vec::new();
        let mut queried: HashSet<&str> = HashSet::new();
        let mut learned: HashMap<String, String> = HashMap::new();
        // Visits in provider order, so results are deterministic even when
        // several watches share a department.
        let routed = form.depts.iter().filter(|d| known.contains(*d));
        let n = form.depts.len() as i64;
        let mut scanned = 0;
        let mut visited = 0;
        for dept in routed {
            self.fetch(term, dept, &mut queried, &mut learned, &mut want, &mut out).await?;
        }
        while !want.is_empty() && visited < n && scanned < RECOVERY_LIMIT {
            visited += 1;
            let dept = &form.depts[cursor.rem_euclid(n) as usize];
            cursor = (cursor + 1) % n;
            if queried.contains(dept.as_str()) {
                continue;
            }
            self.fetch(term, dept, &mut queried, &mut learned, &mut want, &mut out).await?;
            scanned += 1;
        }
        let mut state = self.routing.lock().expect("uci routing lock");
        let Routing { dept_of, scan_at, dirty } = &mut *state;
        let routes = dept_of.entry(term.to_owned()).or_default();
        for id in &want {
            if routes.remove(id).is_some() {
                *dirty = true;
            }
        }
        for (id, dept) in learned {
            if routes.get(&id) != Some(&dept) {
                routes.insert(id, dept);
                *dirty = true;
            }
        }
        if want.is_empty() {
            if scan_at.remove(term).is_some() {
                *dirty = true;
            }
        } else if scan_at.get(term).copied().unwrap_or(0) != cursor {
            scan_at.insert(term.to_owned(), cursor);
            *dirty = true;
        }
        Ok(out)
    }

    fn version(&self) -> &str {
        "uci-websoc-1"
    }

    fn state(&self) -> Option<&dyn AdapterState> {
        Some(self)
    }
}

impl Uci {
    /// Queries one department for an availability poll, pausing first unless
    /// it is the poll's first request, and records what it learned.
    async fn fetch<'a>(
        &self,
        term: &str,
        dept: &'a str,
        queried: &mut HashSet<&'a str>,
        learned: &mut HashMap<String, String>,
        want: &mut HashSet<String>,
        out: &mut Vec<Seats>,
    ) -> Result<(), AdapterError> {
        if !queried.is_empty() {
            tokio::time::sleep(self.pause).await;
        }
        let secs = self.dept(term, dept).await?;
        queried.insert(dept);
        for s in secs {
            learned.insert(s.id.clone(), dept.to_owned());
            if want.remove(&s.id) {
                out.push(Seats { id: s.id, capacity: s.capacity, available: s.available, waitlist: s.waitlist });
            }
        }
        Ok(())
    }
}

/// The persisted state is
/// `{"departments": {term: {section: dept}}, "scan_at": {term: index}}`,
/// with `scan_at` omitted when empty.
impl AdapterState for Uci {
    fn save_state(&self) -> Result<Vec<u8>, AdapterError> {
        let mut state = self.routing.lock().expect("uci routing lock");
        let data = Self::encode_routing(&state)?;
        state.dirty = false;
        Ok(data)
    }

    /// Negative scan positions reset to zero.
    fn load_state(&self, data: &[u8]) -> Result<(), AdapterError> {
        let saved: SavedRouting = serde_json::from_slice(data).map_err(|err| AdapterError::new(err.to_string()))?;
        let mut scan_at = saved.scan_at.unwrap_or_default();
        for cursor in scan_at.values_mut() {
            *cursor = (*cursor).max(0);
        }
        let mut state = self.routing.lock().expect("uci routing lock");
        state.dept_of = saved.departments.unwrap_or_default();
        state.scan_at = scan_at;
        Ok(())
    }

    /// Returns nothing unless routing changed since it was last saved.
    fn save_polling_state(&self) -> Result<Vec<u8>, AdapterError> {
        let mut state = self.routing.lock().expect("uci routing lock");
        if !state.dirty {
            return Ok(Vec::new());
        }
        let data = Self::encode_routing(&state)?;
        state.dirty = false;
        Ok(data)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;
    use crate::adapters::testserver::{Response, serve};

    const FORM: &[u8] = include_bytes!("fixture_form.html");
    const DEPT: &[u8] = include_bytes!("fixture_dept.xml");

    fn uci(base: &str) -> Uci {
        let mut u = Uci::new(Http::unpaced());
        u.base = base.to_owned();
        u.pause = Duration::ZERO;
        u
    }

    #[tokio::test]
    async fn contract() {
        let base = serve(|r| {
            if r.q("Submit") != "XML" {
                return Response::ok(FORM);
            }
            if r.q("FullCourses") != "ANY" {
                return Response::error(400, "full sections must be requested");
            }
            if r.q("Dept") == "COMPSCI" && r.q("YearTerm") == "2026-92" {
                return Response::ok(DEPT);
            }
            Response::ok("<html>No courses matched your search criteria for this term.</html>")
        })
        .await;
        let u = uci(&format!("{base}/perl/WebSoc"));
        let terms = u.terms().await.unwrap();
        assert!(
            !terms.is_empty() && terms[0].id == "2026-92" && terms[0].name == "2026 Fall Quarter",
            "terms: {terms:?}"
        );
        let depts = &u.form.get().unwrap().depts;
        assert!(depts.len() >= 5 && depts[0] == "AC ENG", "depts: {:?}", &depts[..depts.len().min(5)]);
        let secs = u.catalog("2026-92").await.unwrap();
        assert!(!secs.is_empty(), "catalog");
        let s = &secs[0];
        assert!(
            s.id == "34020"
                && s.course == "COMPSCI 116"
                && s.kind == "LEC"
                && s.credits == 4.0
                && s.capacity == 246
                && s.available == 72
                && s.instructors[0] == "BERG, A.",
            "bad section: {s:?}"
        );
        assert!(
            s.meetings.len() == 1
                && s.meetings[0].days == "TR"
                && s.meetings[0].start == 930
                && s.meetings[0].end == 1010
                && s.meetings[0].location == "ELH 100",
            "bad meeting: {:?}",
            s.meetings
        );
        let seats = u.availability("2026-92", &["34020".into()]).await.unwrap();
        assert!(seats.len() == 1 && seats[0].available == 72, "availability: {seats:?}");
    }

    #[test]
    fn parsers() {
        for (input, want) in [
            (" 3:30- 4:50p", (930, 1010)),
            ("11:00-12:20p", (660, 740)),
            (" 8:00- 9:20", (480, 560)),
            ("12:00- 1:20p", (720, 800)),
            ("TBA", (0, 0)),
        ] {
            assert_eq!(parse_time(input), want, "{input:?}");
        }
        assert_eq!(parse_days("TuTh"), "TR");
        assert_eq!(parse_days("MWF"), "MWF");
        assert_eq!(parse_days("Sa"), "S");
        assert_eq!(parse_days("TBA"), "");
    }

    #[tokio::test]
    async fn persisted_department_routing_and_bounded_recovery() {
        let requests = Arc::new(AtomicUsize::new(0));
        let counter = requests.clone();
        let base = serve(move |r| {
            if r.q("Submit") != "XML" {
                return Response::ok(FORM);
            }
            counter.fetch_add(1, Ordering::SeqCst);
            if r.q("Dept") == "COMPSCI" {
                return Response::ok(DEPT);
            }
            Response::ok("<html>No courses</html>")
        })
        .await;
        let u = uci(&base);
        u.catalog("2026-92").await.unwrap();
        let state = u.save_state().unwrap();
        let restarted = uci(&base);
        restarted.load_state(&state).unwrap();
        requests.store(0, Ordering::SeqCst);
        let seats = restarted.availability("2026-92", &["34020".into()]).await.unwrap();
        let n = requests.load(Ordering::SeqCst);
        assert!(seats.len() == 1 && n == 1, "persisted watch: {n} requests, {} seats", seats.len());
        let data = restarted.save_polling_state().unwrap();
        assert!(data.is_empty(), "unchanged poll rewrote routing: {}", data.len());

        let mut cold = uci(&base);
        let mut found = false;
        for _ in 0..8 {
            if found {
                break;
            }
            requests.store(0, Ordering::SeqCst);
            let seats = cold.availability("2026-92", &["34020".into()]).await.unwrap();
            let n = requests.load(Ordering::SeqCst);
            assert!(n <= 8, "unbounded recovery scan: {n}");
            found = seats.len() == 1;
            // Persisting and restoring must retain scan progress across process restarts.
            let data = cold.save_state().unwrap();
            let next = uci(&base);
            next.load_state(&data).unwrap();
            cold = next;
        }
        assert!(found, "rotating recovery never found department");
        requests.store(0, Ordering::SeqCst);
        cold.availability("2026-92", &["34020".into()]).await.unwrap();
        let n = requests.load(Ordering::SeqCst);
        assert_eq!(n, 1, "recovered routing not reused");
    }

    #[test]
    fn load_state_resets_negative_cursors() {
        let u = uci("http://x");
        u.load_state(br#"{"departments":null,"scan_at":{"2026-92":-3}}"#).unwrap();
        assert_eq!(u.save_state().unwrap(), br#"{"departments":{},"scan_at":{"2026-92":0}}"#);
        u.load_state(br#"{"departments":{"t":{"1":"CS"}}}"#).unwrap();
        assert_eq!(u.save_state().unwrap(), br#"{"departments":{"t":{"1":"CS"}}}"#);
    }
}
