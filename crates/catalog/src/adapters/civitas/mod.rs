//! Civitas Learning's College Scheduler course search, which many
//! institutions host at `https://<env>.search.collegescheduler.com`. The site
//! is a thin client over an open GraphQL endpoint that reports exact open and
//! total seats.
//!
//! The adapter posts GraphQL queries to `https://api.collegescheduler.com/graphql`
//! with the school's site as the `Origin`. A catalog pages through
//! `findCourses` at 500 courses per request, then `getCourseSections` at 200
//! sections per request for each course, pausing 100 ms between courses.
//! Full courses and sections are requested explicitly. Availability checks
//! query only the courses that contain watched sections, using the
//! section-to-course routing the last catalog recorded; that routing is the
//! adapter's persisted state. Each request times out after 60 seconds.

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::sync::Mutex;
use std::time::Duration;

use async_trait::async_trait;
use reqwest::header::{ACCEPT, CONTENT_TYPE, ORIGIN};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

use super::{null_default as nd, status_text, upper_prefix3};
use crate::{Adapter, AdapterError, AdapterState, Http, Meeting, Seats, Section, Term};

const PAUSE: Duration = Duration::from_millis(100);
const TIMEOUT: Duration = Duration::from_secs(60);

const SECTION_FIELDS: &str = "id registrationNumber sectionNumber instructors component creditsMin openSeats totalSeats instructionMode meetings{room building days startDate endDate startTime endTime}";

/// Section routing by term, then CRN, to the GraphQL course node ID.
type Routing = BTreeMap<String, BTreeMap<String, String>>;

/// A College Scheduler environment.
pub(super) struct Civitas {
    /// The school's subdomain, such as `ivytech`.
    pub env: String,
    /// The GraphQL endpoint. Tests point it at a fixture server.
    pub api: String,
    http: Http,
    course_of: Mutex<Routing>,
}

impl Civitas {
    pub(super) fn new(http: Http, env: &str) -> Self {
        Self {
            env: env.to_owned(),
            api: "https://api.collegescheduler.com/graphql".into(),
            http,
            course_of: Mutex::new(Routing::new()),
        }
    }

    fn site(&self) -> String {
        format!("https://{}.search.collegescheduler.com", self.env)
    }
}

#[derive(Deserialize, Default, Clone)]
#[serde(default)]
struct Course {
    #[serde(deserialize_with = "nd")]
    id: String,
    #[serde(deserialize_with = "nd")]
    subject: Subject,
    #[serde(rename = "courseNumber", deserialize_with = "nd")]
    number: String,
    #[serde(deserialize_with = "nd")]
    title: String,
    #[serde(deserialize_with = "nd")]
    description: String,
}

#[derive(Deserialize, Default, Clone)]
#[serde(default)]
struct Subject {
    #[serde(deserialize_with = "nd")]
    id: String,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct RawSection {
    #[serde(rename = "registrationNumber", deserialize_with = "nd")]
    crn: String,
    #[serde(rename = "sectionNumber", deserialize_with = "nd")]
    number: String,
    #[serde(deserialize_with = "nd")]
    instructors: Vec<String>,
    #[serde(deserialize_with = "nd")]
    component: String,
    #[serde(rename = "instructionMode", deserialize_with = "nd")]
    mode: String,
    #[serde(rename = "creditsMin", deserialize_with = "nd")]
    credits: f64,
    #[serde(rename = "openSeats", deserialize_with = "nd")]
    open: i64,
    #[serde(rename = "totalSeats", deserialize_with = "nd")]
    total: i64,
    #[serde(deserialize_with = "nd")]
    meetings: Vec<RawMeeting>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct RawMeeting {
    #[serde(deserialize_with = "nd")]
    room: String,
    #[serde(deserialize_with = "nd")]
    building: String,
    #[serde(deserialize_with = "nd")]
    days: String,
    #[serde(rename = "startDate", deserialize_with = "nd")]
    start_date: String,
    #[serde(rename = "endDate", deserialize_with = "nd")]
    end_date: String,
    /// `HHMM` as an integer, such as `1800`.
    #[serde(rename = "startTime", deserialize_with = "nd")]
    start_time: i64,
    #[serde(rename = "endTime", deserialize_with = "nd")]
    end_time: i64,
}

#[derive(Deserialize, Default)]
#[serde(default, bound(deserialize = "T: Deserialize<'de> + Default"))]
struct Page<T> {
    #[serde(rename = "pageInfo", deserialize_with = "nd")]
    page_info: PageInfo,
    #[serde(deserialize_with = "nd")]
    edges: Vec<Edge<T>>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct PageInfo {
    #[serde(rename = "hasNextPage", deserialize_with = "nd")]
    has_next_page: bool,
    #[serde(rename = "endCursor", deserialize_with = "nd")]
    end_cursor: String,
}

#[derive(Deserialize)]
struct Edge<T> {
    node: T,
}

#[derive(Deserialize, Default)]
#[serde(default, bound(deserialize = "T: Deserialize<'de> + Default"))]
struct EnvironmentOf<T> {
    #[serde(deserialize_with = "nd")]
    environment: T,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct TermsData {
    #[serde(rename = "courseSearchTerms", deserialize_with = "nd")]
    terms: Vec<RawTerm>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct RawTerm {
    #[serde(deserialize_with = "nd")]
    code: String,
    #[serde(deserialize_with = "nd")]
    name: String,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct CoursesData {
    #[serde(rename = "findCourses", deserialize_with = "nd")]
    found: Page<Course>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct SectionsData {
    #[serde(rename = "getCourseSections", deserialize_with = "nd")]
    found: Page<RawSection>,
}

#[derive(Deserialize)]
struct Envelope {
    data: Option<Value>,
    #[serde(default, deserialize_with = "nd")]
    errors: Vec<GqlError>,
}

#[derive(Deserialize)]
struct GqlError {
    #[serde(default, deserialize_with = "nd")]
    message: String,
}

impl Civitas {
    async fn sections(&self, co: &Course) -> Result<Vec<Section>, AdapterError> {
        let q = format!(
            "query($e:String!,$id:ID!,$c:String){{environment(name:$e){{getCourseSections(courseId:$id,first:200,after:$c,includeFullSections:true){{pageInfo{{hasNextPage endCursor}} edges{{node{{{SECTION_FIELDS}}}}}}}}}}}"
        );
        let mut out = Vec::new();
        let mut cursor = String::new();
        loop {
            let res: EnvironmentOf<SectionsData> =
                self.gql(&q, json!({"e": self.env, "id": co.id, "c": non_empty(&cursor)})).await?;
            let found = res.environment.found;
            out.extend(found.edges.into_iter().map(|e| self.convert(co, e.node)));
            if !found.page_info.has_next_page {
                return Ok(out);
            }
            cursor = found.page_info.end_cursor;
        }
    }

    fn convert(&self, co: &Course, s: RawSection) -> Section {
        let mut out = Section {
            id: s.crn,
            course: format!("{} {}", co.subject.id, co.number).trim().to_owned(),
            subject: co.subject.id.clone(),
            title: co.title.clone(),
            number: s.number,
            kind: type_code(&s.component),
            instructors: s.instructors,
            available: s.open.max(0),
            capacity: s.total,
            credits: s.credits,
            source_url: format!("{}/", self.site()),
            description: co.description.trim().to_owned(),
            mode: mode(&s.mode),
            ..Default::default()
        };
        for m in s.meetings {
            let start = m.start_time / 100 * 60 + m.start_time % 100;
            let end = m.end_time / 100 * 60 + m.end_time % 100;
            if m.days.is_empty() || end <= start {
                continue;
            }
            let location = if m.room == "VIRTUAL" {
                "Online".to_owned()
            } else {
                format!("{} {}", m.building, m.room).trim().to_owned()
            };
            out.meetings.push(Meeting { days: m.days, start, end, location });
            if out.start.is_empty() {
                out.start = m.start_date;
                out.end = m.end_date;
            }
        }
        out
    }

    /// Posts one GraphQL query. The response status is not checked: an
    /// undecodable body reports the status, and the first GraphQL error
    /// becomes the adapter error. A `null` `data` decodes as empty.
    async fn gql<T: DeserializeOwned + Default>(&self, query: &str, vars: Value) -> Result<T, AdapterError> {
        let body = json!({"query": query, "variables": vars});
        let request = self
            .http
            .post(&self.api)
            .header(CONTENT_TYPE, "application/json")
            .header(ACCEPT, "application/json")
            .header(ORIGIN, self.site())
            .body(body.to_string());
        let (status, body) = self.http.send_any(request, TIMEOUT).await?;
        let env: Envelope = serde_json::from_slice(&body)
            .map_err(|err| AdapterError::new(format!("civitas: {}: {err}", status_text(status))))?;
        if let Some(first) = env.errors.first() {
            return Err(AdapterError::new(format!("civitas: {}", first.message)));
        }
        match env.data {
            None => Err(AdapterError::new("unexpected end of JSON input")),
            Some(Value::Null) => Ok(T::default()),
            Some(data) => serde_json::from_value(data).map_err(|err| AdapterError::new(err.to_string())),
        }
    }
}

fn non_empty(s: &str) -> Value {
    if s.is_empty() { Value::Null } else { Value::String(s.to_owned()) }
}

fn mode(instruction: &str) -> String {
    let m = instruction.to_lowercase();
    if m.is_empty() {
        String::new()
    } else if m.contains("virtual") || m.contains("online") || m.contains("distance") {
        "Online".into()
    } else if m.contains("hybrid") || m.contains("blend") {
        "Hybrid".into()
    } else {
        "In person".into()
    }
}

fn type_code(component: &str) -> String {
    let k = component.to_lowercase();
    if k.starts_with("lec") {
        "LEC".into()
    } else if k.starts_with("lab") {
        "LAB".into()
    } else if k.starts_with("dis") || k.starts_with("rec") {
        "DIS".into()
    } else if k.starts_with("sem") {
        "SEM".into()
    } else if k.is_empty() {
        "LEC".into()
    } else {
        upper_prefix3(component)
    }
}

#[async_trait]
impl Adapter for Civitas {
    async fn terms(&self) -> Result<Vec<Term>, AdapterError> {
        let out: EnvironmentOf<TermsData> = self
            .gql("query($e:String!){environment(name:$e){courseSearchTerms{code name}}}", json!({"e": self.env}))
            .await?;
        Ok(out.environment.terms.into_iter().map(|t| Term { id: t.code, name: t.name, ..Default::default() }).collect())
    }

    /// Lists every course, then each course's sections, and records which
    /// course holds each section.
    async fn catalog(&self, term: &str) -> Result<Vec<Section>, AdapterError> {
        let q = "query($e:String!,$t:String!,$c:String){environment(name:$e){findCourses(termCode:$t,first:500,after:$c,includeFullCourses:true){pageInfo{hasNextPage endCursor} edges{node{id subject{id} courseNumber title description creditsMin}}}}}";
        let mut courses = Vec::new();
        let mut cursor = String::new();
        loop {
            let out: EnvironmentOf<CoursesData> =
                self.gql(q, json!({"e": self.env, "t": term, "c": non_empty(&cursor)})).await?;
            let found = out.environment.found;
            courses.extend(found.edges.into_iter().map(|e| e.node));
            if !found.page_info.has_next_page {
                break;
            }
            cursor = found.page_info.end_cursor;
        }
        let mut secs = Vec::new();
        let mut ids = BTreeMap::new();
        for (i, co) in courses.iter().enumerate() {
            if i > 0 {
                tokio::time::sleep(PAUSE).await;
            }
            let ss = self
                .sections(co)
                .await
                .map_err(|err| AdapterError::new(format!("civitas: {} {}: {err}", co.subject.id, co.number)))?;
            for s in &ss {
                ids.insert(s.id.clone(), co.id.clone());
            }
            secs.extend(ss);
        }
        self.course_of.lock().expect("civitas routing lock").insert(term.to_owned(), ids);
        Ok(secs)
    }

    /// Refreshes only the courses that contain watched sections.
    async fn availability(&self, term: &str, ids: &[String]) -> Result<Vec<Seats>, AdapterError> {
        let want: HashSet<&str> = ids.iter().map(String::as_str).collect();
        let courses: BTreeSet<String> = {
            let routing = self.course_of.lock().expect("civitas routing lock");
            ids.iter().filter_map(|id| routing.get(term).and_then(|m| m.get(id)).cloned()).collect()
        };
        let mut out = Vec::new();
        for (i, cid) in courses.into_iter().enumerate() {
            if i > 0 {
                tokio::time::sleep(PAUSE).await;
            }
            let ss = self.sections(&Course { id: cid, ..Default::default() }).await?;
            for s in ss {
                if want.contains(s.id.as_str()) {
                    out.push(Seats { id: s.id, available: s.available, capacity: s.capacity, waitlist: 0 });
                }
            }
        }
        Ok(out)
    }

    fn version(&self) -> &str {
        "civitas-1"
    }

    fn state(&self) -> Option<&dyn AdapterState> {
        Some(self)
    }
}

/// The persisted state is the section-to-course routing, as JSON
/// `{term: {crn: courseNodeId}}`.
impl AdapterState for Civitas {
    fn save_state(&self) -> Result<Vec<u8>, AdapterError> {
        let routing = self.course_of.lock().expect("civitas routing lock");
        serde_json::to_vec(&*routing).map_err(|err| AdapterError::new(err.to_string()))
    }

    fn load_state(&self, data: &[u8]) -> Result<(), AdapterError> {
        let state: Option<Routing> = serde_json::from_slice(data).map_err(|err| AdapterError::new(err.to_string()))?;
        *self.course_of.lock().expect("civitas routing lock") = state.unwrap_or_default();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::testserver::{Response, serve};

    /// Recorded GraphQL responses for the `ivytech` environment.
    const FIXTURE: &str = include_str!("fixture.json");

    #[tokio::test]
    async fn contract() {
        let fx: BTreeMap<String, Value> = serde_json::from_str(FIXTURE).unwrap();
        let base = serve(move |r| {
            let req: Value = serde_json::from_slice(&r.body).unwrap_or_default();
            let query = req["query"].as_str().unwrap_or("");
            if r.header("origin") != "https://ivytech.search.collegescheduler.com" {
                Response::error(400, "bad origin")
            } else if query.contains("courseSearchTerms") {
                Response::ok(fx["terms"].to_string())
            } else if (query.contains("findCourses") && !query.contains("includeFullCourses:true"))
                || (query.contains("getCourseSections") && !query.contains("includeFullSections:true"))
            {
                Response::error(400, "full sections must be requested")
            } else if query.contains("findCourses") {
                Response::ok(fx["courses"].to_string())
            } else if query.contains("getCourseSections") {
                Response::ok(fx["sections"].to_string())
            } else {
                Response::error(400, "unknown query")
            }
        })
        .await;
        let mut c = Civitas::new(Http::unpaced(), "ivytech");
        c.api = base;

        let terms = c.terms().await.unwrap();
        assert!(terms.len() == 2 && terms[0].id == "202620" && terms[0].name == "Fall 2026", "terms: {terms:?}");
        let secs = c.catalog("202620").await.unwrap();
        assert_eq!(secs.len(), 6, "fixture: 2 courses x 3 recorded sections");
        let s = &secs[0];
        assert!(
            s.id == "21395"
                && s.course == "ACCT 101"
                && s.title == "Financial Accounting"
                && s.number == "00A"
                && s.kind == "LEC"
                && s.credits == 3.0,
            "bad section: {s:?}"
        );
        assert!(
            s.available == 12 && s.capacity == 30 && s.instructors[0] == "Pirtle, Gina A" && s.start == "2026-08-24",
            "bad seats/instructor/dates: {s:?}"
        );
        assert!(
            s.meetings.len() == 1
                && s.meetings[0].days == "M"
                && s.meetings[0].start == 1080
                && s.meetings[0].end == 1245
                && s.meetings[0].location == "Online",
            "bad meeting: {:?}",
            s.meetings
        );
        assert_eq!(secs[2].meetings[0].location, "IVY 2370", "bad location");
        let seats = c.availability("202620", &["31213".into(), "nope".into()]).await.unwrap();
        assert!(seats.len() == 1 && seats[0].available == 8 && seats[0].capacity == 28, "availability: {seats:?}");
    }

    #[test]
    fn state_round_trips() {
        let c = Civitas::new(Http::unpaced(), "ivytech");
        c.load_state(br#"{"202620":{"21395":"node"}}"#).unwrap();
        assert_eq!(c.save_state().unwrap(), br#"{"202620":{"21395":"node"}}"#);
        c.load_state(b"null").unwrap();
        assert_eq!(c.save_state().unwrap(), b"{}");
        assert!(c.load_state(b"[").is_err());
    }
}
