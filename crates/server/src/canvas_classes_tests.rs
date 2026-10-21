//! Finding a student's classes from Canvas: name parsing, catalog matching,
//! linked sections, and the account endpoint.

use std::sync::{Arc, Mutex};

use axum::extract::Request;
use serde_json::{Value, json};

use super::*;
use crate::lms::tests::{Upstream, error, reply, upstream};
use crate::test_support::{Client, signed_in_client};
use crate::test_support::{TEST_CANVAS_KEY, TestServer, test_server_with};

/// The demo catalog's Fall 2026 term: CS 101 lectures 001 and 002 with
/// linked labs, CS 240 with two lectures by one instructor, MATH 151 with
/// three lectures, and PHYS 201 with a lecture and two labs linked to it.
const CANVAS_ENROLLMENTS: &str = r#"[
 {"id":1,"name":"CS-101-001: Intro to Computing","course_code":"CS-101-001","term":{"name":"Fall 2026"},
  "sections":[{"name":"CS-101-001"},{"name":"CS-101-L02"}],"teachers":[{"display_name":"Ada Lovelace"}]},
 {"id":2,"name":"Calculus I","course_code":"MATH151","term":{"name":"2026 Fall"},"teachers":[{"display_name":"Carl Friedrich Gauss"}]},
 {"id":3,"name":"Physics I (10030)","course_code":"PHYS I","term":{"name":"FA26"}},
 {"id":4,"name":"Data Structures","course_code":"CS 240","term":{"name":"Fall Semester 2026"}},
 {"id":5,"name":"Campus Engagement","course_code":"ENGAGE","term":{"name":"Default Term"}},
 {"id":6,"name":"CS 101-002 last spring","course_code":"CS 101-002","term":{"name":"Spring 2026"}}
]"#;

struct Fixture {
    t: TestServer,
    up: Upstream,
    asked: Arc<Mutex<Vec<String>>>,
}

async fn fixture(courses: &'static str, keyed: bool) -> Fixture {
    let asked = Arc::new(Mutex::new(Vec::new()));
    let up = upstream({
        let asked = asked.clone();
        move |req: Request| {
            let asked = asked.clone();
            async move {
                let authorized = req.headers().get("authorization").and_then(|v| v.to_str().ok())
                    == Some("Bearer canvas-token-for-classes");
                if !authorized {
                    return error(401, "unauthorized");
                }
                match req.uri().path() {
                    "/api/v1/courses" => {
                        asked.lock().unwrap().push(req.uri().query().unwrap_or_default().to_owned());
                        reply(200, courses)
                    }
                    "/api/v1/planner/items" => reply(200, "[]"),
                    _ => error(404, "404 page not found"),
                }
            }
        }
    })
    .await;
    let t = test_server_with(|cfg| {
        if keyed {
            cfg.canvas_key = TEST_CANVAS_KEY.into();
        }
    })
    .await;
    t.server.outbound.set_allow_local(true);
    t.server.sync_catalog(&t.server.insts[0], None, true).await.unwrap();
    Fixture { t, up, asked }
}

async fn find_classes(f: &Fixture, term: &str) -> FoundClasses {
    let mut c = Client::new(&f.t);
    let body = json!({"base": f.up.url, "token": "canvas-token-for-classes", "inst": "demo", "term": term}).to_string();
    let (res, _) = c.send("POST", "/api/lms/canvas/classes", &body).await;
    assert_eq!(res.status, 200, "classes: {}", res.text());
    serde_json::from_slice(&res.body).unwrap()
}

#[tokio::test]
async fn canvas_classes_match_the_catalog() {
    let f = fixture(CANVAS_ENROLLMENTS, false).await;
    let out = find_classes(&f, "").await;
    let asked = f.asked.lock().unwrap().clone();
    assert!(
        asked.len() == 1 && asked[0].contains("enrollment_type=student") && asked[0].contains("include[]=sections"),
        "courses were asked for with {asked:?}"
    );
    assert_eq!(out.term, "2026FA");
    let by_course: HashMap<&str, &FoundClass> = out.classes.iter().map(|c| (c.course.id.as_str(), c)).collect();
    let check = |id: &str, sections: &[&str], options: &[&[&str]], sure: bool| {
        let c = by_course.get(id).unwrap_or_else(|| panic!("course {id} was not found: {out:?}"));
        assert!(c.sections == sections && c.options.len() == options.len() && c.sure == sure, "course {id}: {c:?}");
        for (got, want) in c.options.iter().zip(options) {
            assert_eq!(got, want, "course {id} options: {:?}", c.options);
        }
        for sec in c.sections.iter().chain(c.options.iter().flatten()) {
            assert!(
                out.sections.get(sec).is_some_and(|raw| !raw.get().is_empty()),
                "section {sec} is not in the reply"
            );
        }
    };
    // The student's own sections name the lecture and the lab.
    check("1", &["10001", "10003"], &[], true);
    assert!(by_course["1"].code == "CS 101" && !by_course["1"].title.is_empty(), "course: {:?}", by_course["1"]);
    // The instructor picks one of three lectures.
    check("2", &["10022"], &[], false);
    // A registration number in the name; the lab linked to it is a choice.
    check("3", &["10030"], &[&["10031", "10032"]], true);
    // Two lectures and nothing to choose between them.
    check("4", &[], &[&["10010", "10011"]], false);
    assert!(out.unmatched.len() == 1 && out.unmatched[0].id == "5", "unmatched: {:?}", out.unmatched);
    assert!(!by_course.contains_key("6"), "a course from another term was matched");
    let section: Value = serde_json::from_str(out.sections["10001"].get()).unwrap();
    assert!(section["course"] == "CS 101" && !section["meetings"].is_null(), "section copy: {section}");
}

/// A lecture with two labs linked to it leaves the lab to the student; a
/// course code listing several sections without the student's own is a
/// choice too.
#[tokio::test]
async fn canvas_classes_choose_linked_sections_and_sections() {
    let f = fixture(
        r#"[
	 {"id":7,"name":"Intro","course_code":"CS 101-001","term":{"name":"Fall 2026"}},
	 {"id":8,"name":"Calculus","course_code":"MATH-151-001,002","term":{"name":"Fall 2026"}}]"#,
        false,
    )
    .await;
    let out = find_classes(&f, "2026FA").await;
    assert_eq!(out.classes.len(), 2, "classes: {out:?}");
    let c = &out.classes[0];
    assert!(c.sections == ["10001"] && c.options == [vec!["10002", "10003"]] && c.sure, "linked labs: {c:?}");
    let c = &out.classes[1];
    assert!(c.sections.is_empty() && c.options == [vec!["10020", "10021"]] && !c.sure, "listed sections: {c:?}");
}

#[tokio::test]
async fn canvas_classes_for_accounts_and_failures() {
    let f = fixture(CANVAS_ENROLLMENTS, true).await;
    let mut c = signed_in_client(&f.t).await;
    c.must("POST", "/api/me/canvas/classes", r#"{"inst":"demo"}"#, 404).await;
    let connect = json!({"base": f.up.url, "token": "canvas-token-for-classes"}).to_string();
    c.must("PUT", "/api/me/canvas", &connect, 200).await;
    let out = c.must("POST", "/api/me/canvas/classes", r#"{"inst":"demo","term":"2026FA"}"#, 200).await;
    assert!(out["term"] == "2026FA" && out["classes"].as_array().map(Vec::len) == Some(4), "account classes: {out}");
    let mut guest = Client::new(&f.t);
    let body = json!({"base": f.up.url, "token": "wrong", "inst": "demo"}).to_string();
    let (res, _) = guest.send("POST", "/api/lms/canvas/classes", &body).await;
    assert!(res.status == 502 && res.text().contains("rejected"), "a rejected token: {} {}", res.status, res.text());
    let body = json!({"base": f.up.url, "token": "canvas-token-for-classes", "inst": "nowhere"}).to_string();
    let (res, _) = guest.send("POST", "/api/lms/canvas/classes", &body).await;
    assert_eq!(res.status, 400, "an unknown school");
}

#[test]
fn season_names() {
    for (name, want) in [
        ("Fall 2026", "fall 2026"),
        ("2026 Fall Semester", "fall 2026"),
        ("FA26", "fall 2026"),
        ("2027SP", "spring 2027"),
        ("Summer I 2026", "summer 2026"),
        ("Autumn Quarter 2026", "fall 2026"),
        ("Default Term", ""),
        ("202610", ""),
        ("Supplemental 26", ""),
    ] {
        assert_eq!(season_of(name), want, "season_of({name:?})");
    }
}
