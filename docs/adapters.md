# School adapters

This reference defines school adapters, provider settings, available data, and request limits.
It also explains how to add a school. An adapter
translates one provider's public class search into Honorum's catalog types
(`Term`, `Section`, `Seats` in `crates/catalog/src/types.rs`). Adapters live
in `crates/catalog/src/adapters/<name>/` with their recorded fixtures;
`config/schools.json` lists the configured schools
([configuration](deployment.md#11-configuration-file)).

## 1. Shared behavior

- Every adapter sends requests through the shared client in
  `crates/catalog/src/http.rs`. Requests to one host go one at a time,
  spaced at least 200 ms apart, so all adapters reaching a host share one
  pacing budget. Each request carries the timeout its adapter documents.
- Collection scheduling, slots, and backoff are in
  [catalog collection](architecture.md#43-catalog-collection). Catalog
  collection releases its shared slot between terms.
- Known ended terms refresh every `archive_catalog_hours` (default 168 h)
  once a 14-day grace period has passed; terms without dates keep the active
  cadence.
- Exam sittings (`exams` on a section: a date, start and end times, and a
  location) come only from providers that publish them.

## 2. Supported adapters

| Adapter | Source | Settings | Seats | Exams |
| --- | --- | --- | --- | --- |
| `demo` | Embedded fixtures | — | Random counts | — |
| `banner` | Ellucian Banner 9 Self-Service JSON | `url`: the `StudentRegistrationSsb/ssb` base | Exact | Exam-typed meeting rows with a single date |
| `ou` | University of Oklahoma ClassNav | — | Exact, checked by subject | The exam column |
| `tamu` | Texas A&M Howdy | — | Open or closed only (counts are `NA`); minimum seat interval 300 s | Dated "Examination" rows |
| `civitas` | Civitas College Scheduler GraphQL | `env`: the school's subdomain, such as `ivytech`, `uta`, or `uc` | Exact | — |
| `fose` | CourseLeaf CLSS | `url`: the class-search site | Exact, one detail request per watched section | — |
| `illinois` | Illinois Course Explorer XML | — | None; seat alerts unavailable | — |
| `uci` | UC Irvine WebSoc XML | — | Enrollment, capacity, and waitlist per department | — (WebSoc's final field is a lead for a later change) |
| `rutgers` | Rutgers Schedule of Classes JSON | `env`: `NB` (default), `NK`, or `CM` | Open or closed only | Each section's final exam note |

`demo` is absent from the default configuration. Recorded fixtures cover
`banner`; verify a school's data before enabling its alerts. Virginia Tech's
CourseLeaf list lacks meeting times, so those sections cannot appear on the
calendar.

## 3. Provider details

### 3.1 Ellucian Banner

The adapter uses public JSON endpoints under its configured base URL, such as
`https://reg.example.edu/StudentRegistrationSsb/ssb`.

| Endpoint | Purpose |
| --- | --- |
| `classSearch/getTerms` | List terms |
| `term/search` | Select a term |
| `classSearch/resetDataForm` | Reset the search before each query |
| `searchResults/searchResults` | Read pages of 500 sections |

 Banner keeps the
selected term and search state in a cookie session, so every catalog or
availability call starts its own session. Each request times out after
60 seconds. A meeting row typed as an exam whose start and end dates match
becomes an exam sitting.

### 3.2 University of Oklahoma

The adapter reads `https://classnav.ou.edu/index_ajax.php`. ClassNav returns
21-column rows with course information, meeting patterns, dates, seat data,
and exam sittings (column 17, in the meeting pattern's format with kind
`EXAM`). Term codes and subjects come from the search page. The endpoint has
no CRN filter, so availability is checked by subject, with a 200 ms pause
between subjects. Credits derive from the last digit of the course number.

### 3.3 Texas A&M

The adapter reads terms from `https://howdy.tamu.edu/api/all-terms` and posts
to `https://howdy.tamu.edu/api/course-sections`:

```json
{"startRow":0,"endRow":0,"termCode":"202631","publicSearch":"Y"}
```

The public response contains the whole term. The adapter reuses a response
for 60 seconds, because catalog refreshes and availability checks need the
same data. `STUSEAT_OPEN` reports `Y` or `N`; counts are `NA`. The seat
interval is at least 300 seconds; a school `poll_seconds` override can only
raise it.

### 3.4 Civitas College Scheduler

The adapter sends GraphQL requests to
`https://api.collegescheduler.com/graphql` with the school's site as
`Origin`. It pages through `findCourses` at 500 courses per request, then
`getCourseSections` at 200 sections per request, pausing 100 ms between
requests. Availability checks query only courses with watched sections. To
configure another school on this system, set `env` to the subdomain of its
`<env>.search.collegescheduler.com` address.

### 3.5 CourseLeaf CLSS

Set `url` to the class-search site. The search lists sections; seat counts
for watched sections need one `details` request each.

### 3.6 UC Irvine

The adapter queries each department for enrollment, capacity, and waitlist
data. It saves section-to-department routing so established watches query
only the relevant departments. The fallback for missing routing is paced and
bounded, and the next catalog collection refreshes the mapping.

## 4. Adding a school

Adapter code and fixture tests do not establish that a school's endpoint
works today or carries usable meetings and availability. A failed endpoint
probe does not prove that a school has no public data source. Historical
discovery surveys remain in Git history as research leads.

1. Verify current public data, term coverage, time zone, and seat semantics.
2. For a new provider, add `crates/catalog/src/adapters/<name>/mod.rs`
   implementing `Adapter`, with recorded fixtures (`fixture*.json`, `.html`,
   or `.xml`) beside it, loaded by its tests with `include_str!` or
   `include_bytes!`.
3. Register the name in `build` in `crates/catalog/src/adapters/mod.rs`, the
   match the server binary calls for each configured institution.
4. Configure the school in `config/schools.json`.
5. Add its academic closures, with source URLs, to
   `crates/server/src/closures.json` (embedded at build time), or set the
   institution's `closures`.
6. Run `cargo test -p honorum-catalog` and enable alerts only after checking
   live seat data.
