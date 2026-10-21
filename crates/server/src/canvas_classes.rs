//! Finding a student's classes from Canvas: the courses they are enrolled
//! in, matched to the school's catalog, so setting up a schedule takes one
//! step instead of a search for each class.
//!
//! Schools name Canvas courses however they like ("CS 1213-010",
//! "2026FA-CS-1213-010", "01:198:111:03", "COMPSCI 161 LEC A (34250)"), so
//! the match reads course codes, section numbers, and registration numbers
//! from every name Canvas gives, trusting the student's own sections over
//! the course, and falls back to the instructor. What it cannot decide it
//! offers as a choice.

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, LazyLock};
use std::time::Duration;

use axum::body::Body;
use axum::extract::State;
use chrono::NaiveDate;
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::value::RawValue;

use crate::Server;
use crate::api::{ApiError, ApiResult, fail, null_default, ok, read_json};
use crate::canvas_account::ImportError;
use crate::db::Sql;
use crate::lms::{CanvasClient, NumberText};
use crate::session::Auth;

/// One Canvas course as clients see it.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct FoundCourse {
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) code: String,
}

/// One Canvas course found in the catalog: the sections it is sure or
/// fairly sure of, and groups the student still picks one from, such as the
/// lab a lecture requires.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct FoundClass {
    pub(crate) course: FoundCourse,
    pub(crate) code: String,
    pub(crate) title: String,
    pub(crate) sections: Vec<String>,
    pub(crate) options: Vec<Vec<String>>,
    pub(crate) sure: bool,
}

/// The result of a class search: the chosen term, matched classes, courses
/// not found, and the stored catalog JSON of every section mentioned, keyed
/// by section ID in sorted order.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub(crate) struct FoundClasses {
    pub(crate) term: String,
    pub(crate) classes: Vec<FoundClass>,
    pub(crate) unmatched: Vec<FoundCourse>,
    pub(crate) sections: BTreeMap<String, Box<RawValue>>,
}

/// A catalog term as the matcher reads it.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct CatalogTerm {
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) start: String,
    pub(crate) end: String,
}

/// A course from Canvas's enrollment list.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
struct Enrolled {
    id: NumberText,
    #[serde(deserialize_with = "null_default")]
    name: String,
    #[serde(rename = "course_code", deserialize_with = "null_default")]
    code: String,
    #[serde(rename = "sis_course_id", deserialize_with = "null_default")]
    sis: String,
    term: Option<EnrolledTerm>,
    #[serde(deserialize_with = "null_default")]
    sections: Vec<EnrolledSection>,
    #[serde(deserialize_with = "null_default")]
    teachers: Vec<Teacher>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
struct EnrolledTerm {
    #[serde(deserialize_with = "null_default")]
    name: String,
    #[serde(rename = "start_at", deserialize_with = "null_default")]
    start: String,
    #[serde(rename = "end_at", deserialize_with = "null_default")]
    end: String,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
struct EnrolledSection {
    #[serde(deserialize_with = "null_default")]
    name: String,
    #[serde(rename = "sis_section_id", deserialize_with = "null_default")]
    sis: String,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
struct Teacher {
    #[serde(rename = "display_name", deserialize_with = "null_default")]
    name: String,
}

/// The fields of a stored catalog section the matcher reads.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
struct CatalogSection {
    #[serde(deserialize_with = "null_default")]
    id: String,
    #[serde(deserialize_with = "null_default")]
    course: String,
    #[serde(deserialize_with = "null_default")]
    title: String,
    #[serde(deserialize_with = "null_default")]
    num: String,
    #[serde(rename = "type", deserialize_with = "null_default")]
    kind: String,
    #[serde(deserialize_with = "null_default")]
    link: String,
    #[serde(deserialize_with = "null_default")]
    instructors: Vec<String>,
    #[serde(skip)]
    raw: String,
}

/// A term's sections, indexed by registration number and by course code
/// with the separators removed. Sections are referred to by index.
#[derive(Default)]
struct TermCatalog {
    sections: Vec<CatalogSection>,
    by_id: HashMap<String, usize>,
    by_course: HashMap<String, Vec<usize>>,
}

fn course_key(code: &str) -> String {
    code_tokens(code).concat()
}

/// The runs of `A-Z0-9` in an upper-cased name.
fn code_tokens(s: &str) -> Vec<String> {
    s.to_uppercase()
        .split(|c: char| !(c.is_ascii_uppercase() || c.is_ascii_digit()))
        .filter(|t| !t.is_empty())
        .map(str::to_owned)
        .collect()
}

/// What one or more Canvas names say: courses by code, with the tokens
/// written after each (candidate section numbers), in first-seen order, and
/// registration numbers.
#[derive(Default)]
struct Mentions {
    courses: HashMap<String, Vec<String>>,
    order: Vec<String>,
    crns: Vec<String>,
}

impl TermCatalog {
    fn section(&self, i: usize) -> &CatalogSection {
        &self.sections[i]
    }

    fn id_of(&self, id: &str) -> Option<usize> {
        self.by_id.get(id).copied()
    }

    fn course(&self, key: &str) -> &[usize] {
        self.by_course.get(key).map(Vec::as_slice).unwrap_or_default()
    }

    fn read(&self, m: &mut Mentions, name: &str) {
        let tokens = code_tokens(name);
        let mut i = 0;
        while i < tokens.len() {
            // Registration numbers are five digits or more, so a year or a
            // section number is never taken for one.
            let token = &tokens[i];
            if token.len() >= 5
                && token.bytes().all(|b| b.is_ascii_digit())
                && self.by_id.contains_key(token)
                && !m.crns.contains(token)
            {
                m.crns.push(token.clone());
            }
            for span in (1..=3).rev() {
                if i + span > tokens.len() {
                    continue;
                }
                let key = tokens[i..i + span].concat();
                if self.course(&key).is_empty() {
                    continue;
                }
                if !m.courses.contains_key(&key) {
                    m.order.push(key.clone());
                }
                // A course may list several sections ("500, 501, 502").
                let after = &tokens[i + span..tokens.len().min(i + span + 4)];
                m.courses.entry(key).or_default().extend(after.iter().cloned());
                i += span - 1;
                break;
            }
            i += 1;
        }
    }

    /// Matches one Canvas course. The student's own section names come
    /// first: they name the one section a combined course was split into.
    fn resolve(&self, c: &Enrolled) -> Option<FoundClass> {
        let mut out = FoundClass {
            course: FoundCourse { id: c.id.0.clone(), name: c.name.clone(), code: c.code.clone() },
            ..FoundClass::default()
        };
        let mut own = Mentions::default();
        for sec in &c.sections {
            self.read(&mut own, &sec.name);
            self.read(&mut own, &sec.sis);
        }
        let mut course = Mentions::default();
        for name in [&c.code, &c.name, &c.sis] {
            self.read(&mut course, name);
        }
        let mut chosen: Vec<usize> = Vec::new();
        let mut sure = false;
        // 1. Registration numbers.
        for m in [&own, &course] {
            chosen.extend(m.crns.iter().filter_map(|crn| self.id_of(crn)));
            if !chosen.is_empty() {
                sure = true;
                break;
            }
        }
        // 2. A course code with its section number.
        let mut key = String::new();
        if chosen.is_empty() {
            'mentions: for m in [&own, &course] {
                for k in &m.order {
                    if key.is_empty() {
                        key = k.clone();
                    }
                    let numbers = m.courses.get(k).map(Vec::as_slice).unwrap_or_default();
                    for &sec in self.course(k) {
                        if numbers.iter().any(|n| same_number(n, &self.section(sec).num)) {
                            chosen.push(sec);
                        }
                    }
                    if !chosen.is_empty() {
                        key = k.clone();
                        sure = true;
                        break 'mentions;
                    }
                }
            }
        }
        if chosen.is_empty() && key.is_empty() {
            return None;
        }
        if chosen.is_empty() {
            // 3. The instructor, then a course with a single section.
            let sections = self.course(&key);
            let names = surnames(c);
            if !names.is_empty() {
                chosen.extend(sections.iter().copied().filter(|&sec| taught_by(self.section(sec), &names)));
            }
            if chosen.is_empty() && sections.len() == 1 {
                chosen = sections.to_vec();
            }
            if chosen.is_empty() {
                // Offer the lectures, or every section when none is marked one.
                let mut offered: Vec<usize> = sections
                    .iter()
                    .copied()
                    .filter(|&sec| self.section(sec).kind.eq_ignore_ascii_case("LEC"))
                    .collect();
                if offered.is_empty() {
                    offered = sections.to_vec();
                }
                out.options.push(self.ids(&offered));
            }
        }
        // Several sections of one kind for a course is a choice, unless the
        // student's own sections named them.
        let mut by_kind: HashMap<String, Vec<usize>> = HashMap::new();
        let mut kinds: Vec<String> = Vec::new();
        for &sec in &chosen {
            let s = self.section(sec);
            let k = format!("{}\0{}", s.course, s.kind);
            let list = by_kind.entry(k.clone()).or_default();
            if list.is_empty() {
                kinds.push(k);
            }
            if !list.contains(&sec) {
                list.push(sec);
            }
        }
        for k in &kinds {
            let list = &by_kind[k];
            if list.len() == 1 {
                out.sections.push(self.section(list[0]).id.clone());
            } else {
                out.options.push(self.ids(list));
                sure = false;
            }
        }
        // Certainty is about the sections found; a lab left to choose below
        // does not make the lecture any less certain.
        out.sure = sure && !out.sections.is_empty();
        // A linked section needs one of each kind in its group, such as a lab.
        // Only the sections found above are examined; sections this loop
        // adds do not pull in further linked sections.
        for i in 0..out.sections.len() {
            let Some(sec) = self.id_of(&out.sections[i]).map(|s| self.section(s)) else { continue };
            if sec.link.is_empty() {
                continue;
            }
            let mut missing: HashMap<&str, Vec<usize>> = HashMap::new();
            let mut types: Vec<&str> = Vec::new();
            for &other in self.course(&course_key(&sec.course)) {
                let o = self.section(other);
                let covered = out.sections.iter().any(|id| {
                    self.id_of(id).map(|s| self.section(s)).is_some_and(|s| s.link == sec.link && s.kind == o.kind)
                });
                if o.link != sec.link || o.kind == sec.kind || covered {
                    continue;
                }
                let list = missing.entry(o.kind.as_str()).or_default();
                if list.is_empty() {
                    types.push(o.kind.as_str());
                }
                list.push(other);
            }
            for t in types {
                let list = &missing[t];
                if list.len() == 1 {
                    out.sections.push(self.section(list[0]).id.clone());
                } else {
                    out.options.push(self.ids(list));
                }
            }
        }
        let first = match chosen.first() {
            Some(&sec) => sec,
            None => self.id_of(&out.options[0][0]).expect("offered sections are in the catalog"),
        };
        out.code = self.section(first).course.clone();
        out.title = self.section(first).title.clone();
        Some(out)
    }

    fn ids(&self, list: &[usize]) -> Vec<String> {
        list.iter().map(|&i| self.section(i).id.clone()).collect()
    }
}

/// Reports whether two section numbers are the same, ignoring case and
/// leading zeros.
fn same_number(a: &str, b: &str) -> bool {
    if a.to_lowercase() == b.to_lowercase() {
        return true;
    }
    matches!((a.parse::<i64>(), b.parse::<i64>()), (Ok(x), Ok(y)) if x == y)
}

/// The words of a name, lower case, with commas and periods as spaces.
fn name_words(name: &str) -> Vec<String> {
    name.replace([',', '.'], " ").to_lowercase().split_whitespace().map(str::to_owned).collect()
}

/// The last names of Canvas teachers, lower case, of three bytes or more.
fn surnames(c: &Enrolled) -> Vec<String> {
    c.teachers.iter().filter_map(|t| name_words(&t.name).pop()).filter(|w| w.len() >= 3).collect()
}

fn taught_by(sec: &CatalogSection, names: &[String]) -> bool {
    sec.instructors.iter().any(|instructor| name_words(instructor).iter().any(|w| names.contains(w)))
}

// Term names Canvas and the catalog both use, like "Fall 2026", "2026 Fall
// Semester", or "FA26". Word boundaries are ASCII: a letter outside ASCII
// does not count as part of a word.
static SEASON_WORD: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)(?-u:\b)(fall|autumn|spring|summer|winter)(?-u:\b)").expect("valid pattern"));
static SEASON_SHORT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)(?:^|[^a-z])(fa|sp|su|wi)[\t\n\f\r '-]?([0-9]{2}|20[0-9]{2})(?:[^0-9]|$)|(?:^|[^0-9])(20[0-9]{2})[\t\n\f\r -]?(fa|sp|su|wi)(?:[^a-z]|$)")
        .expect("valid pattern")
});
static YEAR: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?-u:\b)(20[0-9]{2})(?-u:\b)").expect("valid pattern"));

/// The season and year a term name mentions, normalized as "fall 2026";
/// empty when the name says neither or only a season. Reads "Fall 2026",
/// "2026 Fall Semester", "Autumn 2026", "FA26", "2026FA", and similar.
pub(crate) fn season_of(name: &str) -> String {
    if let Some(m) = SEASON_WORD.captures(name) {
        let mut season = m[1].to_lowercase();
        if season == "autumn" {
            season = "fall".into();
        }
        return match YEAR.captures(name) {
            Some(y) => format!("{season} {}", &y[1]),
            None => String::new(),
        };
    }
    if let Some(m) = SEASON_SHORT.captures(name) {
        let group = |i: usize| m.get(i).map(|g| g.as_str()).unwrap_or_default();
        let (mut season, mut year) = (group(1), group(2).to_owned());
        if season.is_empty() {
            (season, year) = (group(4), group(3).to_owned());
        }
        if year.len() == 2 {
            year = format!("20{year}");
        }
        let full = match season.to_lowercase().as_str() {
            "fa" => "fall",
            "sp" => "spring",
            "su" => "summer",
            "wi" => "winter",
            _ => "",
        };
        return format!("{full} {year}");
    }
    String::new()
}

/// The date at the start of a value such as `2026-08-24T05:00:00Z`.
fn parse_day(value: &str) -> Option<NaiveDate> {
    NaiveDate::parse_from_str(value.get(..10)?, "%Y-%m-%d").ok()
}

/// Whether a Canvas course's term is a catalog term: 1 yes, -1 no, 0 when
/// Canvas says too little, as with "Default Term".
fn same_term(c: &Enrolled, t: &CatalogTerm) -> i32 {
    let Some(term) = &c.term else { return 0 };
    let (a, b) = (season_of(&term.name), season_of(&t.name));
    if !a.is_empty() && !b.is_empty() {
        return if a == b { 1 } else { -1 };
    }
    let (Some(cs), Some(ce), Some(ts), Some(te)) =
        (parse_day(&term.start), parse_day(&term.end), parse_day(&t.start), parse_day(&t.end))
    else {
        return 0;
    };
    // Terms overlap by more than a month.
    let (start, end) = (cs.max(ts), ce.min(te));
    if end - start > chrono::Duration::days(30) { 1 } else { -1 }
}

impl Server {
    async fn term_catalog(&self, inst: &str, term: &str) -> crate::db::Result<TermCatalog> {
        let rows = self.db.fetch_all("SELECT data FROM sections WHERE inst=$1 AND term=$2", &[&inst, &term]).await?;
        let mut tc = TermCatalog::default();
        for row in rows {
            let raw: String = row.try_get::<_, Option<String>>(0)?.unwrap_or_default();
            let Ok(mut sec) = serde_json::from_str::<CatalogSection>(&raw) else { continue };
            if sec.id.is_empty() {
                continue;
            }
            sec.raw = raw;
            let index = tc.sections.len();
            tc.by_id.insert(sec.id.clone(), index);
            tc.by_course.entry(course_key(&sec.course)).or_default().push(index);
            tc.sections.push(sec);
        }
        let sections = &tc.sections;
        for list in tc.by_course.values_mut() {
            // A duplicate section ID resolves to the later stored row in
            // `by_id`, while both rows stay in the course list.
            list.sort_by(|&i, &j| (&sections[i].num, &sections[i].id).cmp(&(&sections[j].num, &sections[j].id)));
        }
        Ok(tc)
    }

    async fn catalog_terms(&self, inst: &str) -> crate::db::Result<Vec<CatalogTerm>> {
        let rows = self
            .db
            .fetch_all(
                r#"SELECT id,COALESCE(name,''),COALESCE(start,''),COALESCE("end",'') FROM terms WHERE inst=$1 ORDER BY id DESC"#,
                &[&inst],
            )
            .await?;
        rows.iter()
            .map(|row| {
                Ok(CatalogTerm {
                    id: row.try_get(0)?,
                    name: row.try_get(1)?,
                    start: row.try_get(2)?,
                    end: row.try_get(3)?,
                })
            })
            .collect()
    }

    /// Resolves every Canvas course that could belong to the term.
    async fn match_term(
        &self,
        inst: &str,
        term: &CatalogTerm,
        courses: &[Enrolled],
    ) -> crate::db::Result<FoundClasses> {
        let mut out = FoundClasses { term: term.id.clone(), ..FoundClasses::default() };
        let tc = self.term_catalog(inst, &term.id).await?;
        let raw = |id: &str| {
            let sec = tc.section(tc.id_of(id).expect("matched sections are in the catalog"));
            RawValue::from_string(sec.raw.clone()).expect("stored sections are JSON")
        };
        for c in courses {
            if same_term(c, term) < 0 {
                continue;
            }
            let Some(found) = tc.resolve(c) else {
                out.unmatched.push(FoundCourse { id: c.id.0.clone(), name: c.name.clone(), code: c.code.clone() });
                continue;
            };
            for id in found.sections.iter().chain(found.options.iter().flatten()) {
                out.sections.insert(id.clone(), raw(id));
            }
            out.classes.push(found);
        }
        Ok(out)
    }

    /// Reads the student's Canvas courses and matches them to the school's
    /// catalog within 30 seconds. The candidate terms are those Canvas
    /// names, the client's hint, and the one in session; up to three are
    /// tried and the one finding the most classes wins. Fails with 400
    /// "unknown school", 502 with Canvas's error, or 500 for catalog reads.
    pub(crate) async fn find_canvas_classes(
        &self,
        base: &url::Url,
        token: &str,
        inst: &str,
        hint: &str,
    ) -> Result<FoundClasses, ImportError> {
        if self.inst(inst).is_none() {
            return Err(ImportError::new(400, "unknown school"));
        }
        tokio::time::timeout(Duration::from_secs(30), self.match_canvas_classes(base, token, inst, hint))
            .await
            .unwrap_or_else(|_| Err(ImportError::new(502, "context deadline exceeded")))
    }

    async fn match_canvas_classes(
        &self,
        base: &url::Url,
        token: &str,
        inst: &str,
        hint: &str,
    ) -> Result<FoundClasses, ImportError> {
        let client = CanvasClient::new(self.outbound.clone(), base.clone(), token.to_owned());
        let courses: Vec<Enrolled> = client
            .list("/api/v1/courses?enrollment_type=student&enrollment_state=active&include[]=sections&include[]=term&include[]=teachers&per_page=100")
            .await
            .map_err(|e| ImportError::new(502, e.to_string()))?;
        let terms =
            self.catalog_terms(inst).await.map_err(|_| ImportError::new(500, "could not read the school's terms"))?;
        let today = self.now().format("%Y-%m-%d").to_string();
        let mut score: HashMap<String, usize> = HashMap::new();
        let mut candidates: Vec<CatalogTerm> = Vec::new();
        for t in terms {
            let n = courses.iter().filter(|c| same_term(c, &t) > 0).count();
            score.insert(t.id.clone(), n);
            let current = !t.start.is_empty() && t.start <= today && today <= t.end;
            if n > 0 || t.id == hint || current {
                candidates.push(t);
            }
        }
        candidates.sort_by_key(|t| std::cmp::Reverse(score[&t.id]));
        candidates.truncate(3);
        let mut best = FoundClasses { term: hint.to_owned(), ..FoundClasses::default() };
        for (i, t) in candidates.iter().enumerate() {
            let found = self
                .match_term(inst, t, &courses)
                .await
                .map_err(|_| ImportError::new(500, "could not read the catalog"))?;
            if i == 0
                || found.classes.len() > best.classes.len()
                || (found.classes.len() == best.classes.len() && t.id == hint)
            {
                best = found;
            }
        }
        Ok(best)
    }
}

#[derive(Default, Deserialize)]
#[serde(default)]
struct GuestClasses {
    #[serde(alias = "Base", deserialize_with = "null_default")]
    base: String,
    #[serde(alias = "Token", deserialize_with = "null_default")]
    token: String,
    #[serde(alias = "Inst", deserialize_with = "null_default")]
    inst: String,
    #[serde(alias = "Term", deserialize_with = "null_default")]
    term: String,
}

/// `POST /api/lms/canvas/classes`: finds a guest's classes with the token in
/// the body (4 KiB), which is never stored.
pub(crate) async fn handle_canvas_classes(State(s): State<Arc<Server>>, body: Body) -> ApiResult {
    let input = read_json::<GuestClasses>(body, 4 << 10).await.filter(|i| !i.token.is_empty());
    let Some(input) = input else { return fail(400, "base url and token are required") };
    let base = s.outbound.safe_url(&input.base).map_err(|e| ApiError::new(400, e))?;
    let found = s.find_canvas_classes(&base, &input.token, &input.inst, &input.term).await?;
    ok(&found)
}

#[derive(Default, Deserialize)]
#[serde(default)]
struct AccountClasses {
    #[serde(alias = "Inst", deserialize_with = "null_default")]
    inst: String,
    #[serde(alias = "Term", deserialize_with = "null_default")]
    term: String,
}

/// `POST /api/me/canvas/classes`: finds an account's classes with its saved
/// Canvas connection (1 KiB body).
pub(crate) async fn handle_account_canvas_classes(
    State(s): State<Arc<Server>>,
    Auth(se): Auth,
    body: Body,
) -> ApiResult {
    let Some(input) = read_json::<AccountClasses>(body, 1024).await else { return fail(400, "bad json") };
    let c =
        s.canvas_connection(&se.account).await.map_err(|_| ApiError::new(500, "could not read Canvas connection"))?;
    if !c.connected {
        return fail(404, "Canvas is not connected");
    }
    let token = s
        .decrypt_canvas(&se.account, &c)
        .map_err(|_| ApiError::new(503, "the Canvas token is unavailable; connect Canvas again"))?;
    let base = s.outbound.safe_url(&c.base).map_err(|e| ApiError::new(400, e))?;
    let found = s.find_canvas_classes(&base, &token, &input.inst, &input.term).await.map_err(|err| match err {
        ImportError::Other(err) => ImportError::new(502, err.to_string()),
        status => status,
    })?;
    ok(&found)
}

#[cfg(test)]
#[path = "canvas_classes_tests.rs"]
mod tests;
