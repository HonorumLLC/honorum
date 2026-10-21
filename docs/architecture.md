# Architecture and API contracts

This document defines the backend's structure, storage, and background work,
and is the single reference for every HTTP endpoint contract the clients
depend on. It also states the rules that the web, iOS, and Android clients
must follow for local state and sync. Product behavior is in
[application behavior](application.md); settings and environment variables
are in [deployment](deployment.md#1-configuration); commands are in
[development](development.md).

## 1. Components

### 1.1 Repository boundaries

| Location | Responsibility |
| --- | --- |
| `crates/honorum/` | The `honorum` binary: flags, configuration loading, adapter construction, process roles, shutdown, embedded web client |
| `crates/server/` | `honorum-server`: HTTP API, PostgreSQL storage, catalog collection and publication, accounts, sync, imports, Google Calendar, and notifications |
| `crates/catalog/` | `honorum-catalog`: catalog types, the paced provider HTTP client, and school adapters (`src/adapters/<name>/`) |
| `apps/web/` | Svelte UI, browser persistence, workers, and offline shell |
| `apps/ios/` | Native SwiftUI app, with UIKit where needed |
| `apps/android/` | Planned Kotlin app using Jetpack Compose, with Android Views where needed |
| `contracts/` | Planner fixtures every client checks ([contracts](../contracts/README.md)) |
| `config/schools.json` | Configured schools and service defaults |
| `assets/brand/` | Shared brand source artwork |
| `deploy/cloudflare/` | Edge Worker, web assets, public R2 objects, and private backend routing |
| `deploy/digitalocean/` | API/worker image, production Compose, provisioning, and release bundles |

Native apps consume HTTP and JSON only; their UI and platform code stay in
their own projects.

### 1.2 Server modules

`crates/server/src/lib.rs` groups the modules by concern; each module's `//!`
comment states its contract.

| Concern | Modules |
| --- | --- |
| Foundation | `api` (handler conventions, middleware), `clock`, `closures`, `config`, `db`, `hub`, `jobs`, `metrics`, `outbound`, `rate`, `schema`, `session`, `state` |
| HTTP surface | `router`, `admin`, `assets`, `catalog_api`, `landing`, `public`, `shares`, `status`, `watches` |
| Accounts and documents | `accounts`, `auth`, `docs`, `events`, `phone`, `streaks` |
| Integrations and imports | `canvas_account`, `canvas_classes`, `feeds`, `google` (`mod`, `publish`, `events`), `ical`, `imports`, `lms`, `seal` |
| Catalog collection and publication | `monitor`, `object_store`, `snapshot`, `snapshot_cleanup` |
| Notifications | `deliveries`, `push` (`mod`, `webpush`, `apns`), `reminder_schedule`, `reminders` |
| Background work | `analytics`, `worker` |

### 1.3 Shared planner rules

Three-way document merge, calendar feed parsing, import reconciliation, and
civil-date occurrences are implemented by every client and checked against
[`contracts/`](../contracts/README.md). Current JSON bodies have no formal
schema version.

## 2. Process roles

One binary runs in one of three roles, selected by `-role` or `HONORUM_ROLE`.
Every role serves HTTP on its listen address.

| Task | `api` | `worker` | `all` |
| --- | --- | --- | --- |
| HTTP API (and the embedded web client, when built with it) | yes | yes | yes |
| `LISTEN honorum_accounts` relay for live event streams | yes | no | yes |
| Snapshot restoration at start (retried every 30 s until it succeeds) | no | yes | yes |
| Coordinator: catalog collection, Google worker, hourly sweep | no | yes | yes |
| Queues: reminders, deliveries, imports, streak maintenance | no | yes | yes |
| Analytics flush | yes | yes | yes |

- Production runs one `api` and one `worker` process; local and standalone
  hosting runs `all`. See [production](deployment.md#3-digitalocean-production).
- The coordinator is elected through the `coordinator` session advisory lock.
  The holder runs collection, the Google worker, and the hourly sweep.
  Loss of the database session releases leadership. A lost lease retries after one second. Every worker consumes the durable queues.
- On SIGINT or SIGTERM the process stops accepting connections and drains
  requests and background work for up to 40 seconds. Shutdown cancels
  provider waits.
- The layout supports adding API replicas without moving account state.
  It establishes no user-count guarantee: database CPU, latency, connection
  budget, event streams, and provider quotas need measurement.

### 2.1 Hosting

- Standalone builds embed the web client (`web` Cargo feature). Builds
  without default features serve the API only.
- In production, Cloudflare serves static files and caches public
  configuration and catalogs; private requests reach the DigitalOcean API
  through a tunnel. Cache misses for public data omit credentials. See
  [Cloudflare](deployment.md#5-cloudflare).

### 2.2 Site root

Both hostings choose what `/` shows by one rule
([`landing.rs`](../crates/server/src/landing.rs) and
[`landing.ts`](../deploy/cloudflare/src/landing.ts)):

- A document navigation to the bare `/` without the `hn_app` cookie or a
  session cookie receives the landing page (`/about.html`).
- Every other request receives the app and renews `hn_app` for 400 days.
- `/app` sets the cookie and redirects to `/`.
- Root responses carry `Vary: Cookie`.
- The offline shell is fetched, not navigated to, so a service worker always
  installs the app, and an installed app never requests the root.

## 3. Storage

### 3.1 Database

- PostgreSQL 17 or later. The schema is the `CREATE ... IF NOT EXISTS`
  statements in [`schema.rs`](../crates/server/src/schema.rs).
  Each start applies them under a transaction-scoped advisory lock.
  Concurrent API and worker starts therefore cannot race. Revision and time columns are `BIGINT`;
  credentials are `BYTEA`. Migration rules are in
  [data and recovery](deployment.md#6-data-and-recovery).
- Account tables live in the `public` schema and catalog tables in
  `catalog`. Both share one database, so catalog updates and the seat alerts
  they queue commit together.
- Account records are durable data. Public catalog files are reconstructible
  caches.

### 3.2 Connections

[`db.rs`](../crates/server/src/db.rs) keeps two bounded pools per process,
sized by environment variables
([configuration](deployment.md#13-environment-variables)):

| Pool | Purpose |
| --- | --- |
| Query pool | Requests and background work |
| Lock pool | Session advisory locks held across provider network calls (imports, Google, the coordinator lease), so waiting imports cannot starve ordinary queries |
| `LISTEN` connection | One dedicated connection per `api`/`all` process, outside both pools |

- The API locks the account row before accepting document revisions.
- Download transactions use a consistent snapshot, preserving the sync
  cursor while another replica accepts edits.
- Queries use native PostgreSQL parameters (`$1`, `$2`, and later positions).
  Each connection prepares and caches its statements.
- Transaction pooling cannot provide session advisory locks or `LISTEN`;
  production uses direct connections.

### 3.3 Advisory locks

| Lock | Scope | Holder |
| --- | --- | --- |
| `coordinator` | Service | The elected worker; held for as long as it coordinates |
| `imports` | Account | Canvas and feed imports for one account, together with an in-process per-account lock |
| Google | Account | Google Calendar operations for one account |

A cancelled request stops waiting for its lock.

### 3.4 Rate limit storage

Request limits are counted in the `rate_limits` table, so they survive
restarts and apply across replicas. Keys are SHA-256 hashes; the hourly sweep
deletes expired rows. The limits themselves are in [§5.3](#53-rate-limits).

- Sign-in and admin limits call the `rate_allow` database function on every
  request.
- Other API writes call `rate_record` only when the process's view of the
  key is more than 5 seconds old or past half the budget. Requests admitted
  in between are recorded with that call. Each replica admits at most half
  a budget on its own, so two replicas stay within it.

## 4. Background work

### 4.1 Durable queue

[`jobs.rs`](../crates/server/src/jobs.rs) implements the `work_jobs` queue
used by imports, reminder rebuilds, and streak maintenance.

- A row is keyed by kind and key. A worker claims one due row with
  `FOR UPDATE SKIP LOCKED`, holds a 2-minute lease renewed every 30 seconds,
  and acknowledges only the owner and generation it claimed.
- A crash leaves the row claimable once its lease lapses.
- A newer edit bumps the generation, so an older acknowledgement cannot erase
  it.
- Failed jobs back off up to 1 hour.
- Streak jobs are drained by two workers, up to 1,000 per pass, every
  second.

### 4.2 Hourly sweep

The coordinator's sweep deletes:

| Record | Age |
| --- | --- |
| Completed delivery records | 30 days |
| Sessions | 1 year since last use |
| SMS consent records | 1 year |
| Share blobs | 180 days since upload |
| Replaced document versions | 30 days |
| `daily_metrics` rows | 2 years |
| Expired rate-limit rows | at expiry |

It also settles streak days and removes seat alerts for ended terms (see
[collection and alerts](application.md#2-collection-and-alerts)).

### 4.3 Catalog collection

[`monitor.rs`](../crates/server/src/monitor.rs) runs a catalog job and a seat
job per school with independent due times and failure backoff (up to
1 hour).

- At most two catalog jobs and four seat jobs run at once. Catalog jobs
  release their slot between terms, so seat checks keep capacity while large
  catalogs collect.
- Provider pacing, ended-term cadence, and per-school limits apply across
  clients. Requests to one host are serialized and spaced by at least 200 ms
  through the shared client in `crates/catalog/src/http.rs`. Provider limits
  are in [adapters](adapters.md).
- The `catalog` schema stores structural JSON in `sections` and changing
  counts in `section_seats`. Revision tracking skips unchanged publication
  work.

### 4.4 Catalog publication

[`snapshot.rs`](../crates/server/src/snapshot.rs) publishes each term as
three independent, immutable, content-addressed files: structure, seats, and
descriptions.

- The server writes each JSON file with gzip and Brotli copies before it advances the manifest pointer in `kv`.
  Both encodings use compression level 6. The Brotli window fits the file size. Nothing is written when no input
  changed.
- Snapshots omit section fields that repeat course, term, or institution data.
  Descriptions and prerequisites use a separate file that loads when a class opens.
- Files live under `<data>/catalog/`. A file missing from disk, as after a
  container replacement, is rebuilt by the first request for it; readiness
  needs only the database.
- With R2 publication enabled, [`object_store.rs`](../crates/server/src/object_store.rs)
  uploads every encoding to the private bucket under
  `honorum-public/catalog/<inst>/<file>` before switching the manifest. Only
  those paths are readable through the Worker binding, and public cache
  misses read them without using an API database connection. The worker
  handles a missing object or a manifest rebuild.
- `snapshot_cleanup.rs` removes superseded objects.

### 4.5 Notifications

- Seat changes and the delivery jobs they cause commit in one transaction.
- [`deliveries.rs`](../crates/server/src/deliveries.rs) keeps one row per
  destination, with its own lease, retries with backoff, expiry, and
  cancellation checks. A destination is a Web Push subscription or an APNs
  device token.
- Delivery is at least once: a provider accepting a send immediately before
  a crash, before success is recorded, produces a duplicate.
- Web Push uses VAPID (RFC 8292) and `aes128gcm` encryption (RFC 8291). APNs
  uses a `.p8` key with ES256 provider tokens
  ([`apns.rs`](../crates/server/src/push/apns.rs)).

### 4.6 Reminders

- The server derives account reminders from documents.
  It uses the primary schedule for the preferred school and term, closures, substitute weekdays, removed occurrences, and open tasks with deadlines. The derivation is cached by the account's document sequence.
- Guests register their own patterns (`PUT /api/reminders/{id}`) because
  their planner never leaves the device.
- [`reminder_schedule.rs`](../crates/server/src/reminder_schedule.rs) stores
  each registration's next occurrence in an indexed `reminder_schedule`
  table instead of scanning every account each minute. Edits, push
  registrations, and catalog publications enqueue a rebuild; a daily rebuild
  refreshes other metadata.
- Each occurrence enters the queue once with a stable identity and a five-minute catch-up window.
  Class reminders precede meetings by 15 minutes. Task reminders precede deadlines by one hour. Times are campus time.

### 4.7 Account imports and streaks

- Account Canvas connections and feeds refresh in the background through
  `work_jobs`; see [§8](#8-account-imports).
- Streaks keep frozen days in `account_streak_days` and summarize older
  history in `account_streak_rollups`. PostgreSQL caches the current result
  until an edit, midnight, or the next open task's due time. See
  [§9](#9-streaks).

## 5. HTTP conventions

### 5.1 Requests and responses

- Failures render as `{"error": "<message>"}` with an HTTP status.
- Each handler bounds its request body (listed per endpoint below); the
  framework's default body limit is off.
- Every response carries security headers and a cache policy. Private API
  responses are never stored by the public cache.
- Writes (any method but `GET`/`HEAD`, except `POST /api/google/notify`) with
  an `Origin` whose host matches neither the request host nor `base_url` are
  refused with 403.

### 5.2 Authentication

- Browsers use an HttpOnly, `SameSite=Lax` cookie, `Secure` over HTTPS. The
  cookie lasts 1 year from sign-in.
- Cookie-authenticated requests must name the immutable account ID in
  `X-Honorum-Account`; a missing or mismatched value returns 409. An explicit
  empty header declares guest use and detects a conflicting shared cookie.
  This stops one tab from acting on an account another tab selected.
- Native clients send `Authorization: Bearer <token>` and may omit the
  header; if they send it, it must match. Native tokens belong in
  OS-protected credential storage. Browser code does not persist the bearer
  token.
- `GET /api/me/events` also accepts the account as `?account=`, because
  EventSource cannot set headers; the check is the same.
- The server stores token hashes and keeps at most 20 sessions per account across replicas.
  A new sign-in retires the least recently used session. Sessions expire after one year without use. A request more than 5 minutes
  after a session's recorded last use refreshes it. Each process writes
  refreshes together once a minute and at shutdown, outside the request.

### 5.3 Rate limits

| Requests | Limit | Key |
| --- | --- | --- |
| `POST /api/auth/code` | 100 per 10 min, checked first | IP |
| `POST /api/auth/code` | 1 per 30 s and 5 per 10 min | Phone number |
| `POST /api/auth/verify` | 10 per 10 min | Phone number |
| `/api/lms/*` writes | 30 per min | IP |
| `/api/sync/*` writes (shares) | 100 per hour | IP |
| Other writes with a valid session | 120 per min | Account (students on campus Wi-Fi share an address) |
| Other writes | 120 per min | IP |
| `/admin`, `/api/admin/metrics` | 60 per min | IP |

- Exceeding a limit returns 429 with `Retry-After`. If the limiter's database
  is unavailable, the request fails with 503, unless the process admitted it
  without reading the database ([§3.4](#34-rate-limit-storage)).
- The client IP is the connection's address; `cf-connecting-ip` and
  `X-Forwarded-For` count only when `trust_proxy` is set.
- Provider limits (Twilio, Canvas, Google) apply in addition.

## 6. Endpoint reference

### 6.1 Service and public data

| Endpoint | Contract |
| --- | --- |
| `GET /healthz` | `ok` while the process runs |
| `GET /readyz` | `ready` once the database answers, else 503 `starting` |
| `GET /api/config` | Schools, terms with snapshot hashes and section counts, closures, and capabilities (`phone_auth`, `apns`, `google_calendar`, the VAPID public key). `Cache-Control: public, max-age=30, s-maxage=60, must-revalidate`; `ETag`, 304 on match |
| `GET /api/status` | Collection, delivery, and process status as JSON, cached 30 s; fields in [health and status](deployment.md#7-health-status-and-analytics) |
| `GET /status` | The same status as an HTML page |
| `GET /admin`, `GET /api/admin/metrics?days=N` | Operator dashboard and metrics; 404 unless `HONORUM_ADMIN_TOKEN` is set ([analytics](deployment.md#72-analytics-dashboard)) |
| `GET /catalog/{inst}/{file}` | A snapshot file `<term>.<structure\|seats\|desc>.<sha256>.json`, rebuilt on demand, served in the best prepared encoding, immutable, `ETag: "HASH-ENCODING"`. With `X-Honorum-Store-Encoded: 1` (the Worker) the encoded bytes are returned as `application/octet-stream` with `X-Honorum-Stored-Encoding`. Other names are 404 |
| `GET /api/seats?inst=&term=&ids=a,b,…` | Current counts for up to 100 sections: `{id, available, capacity, waitlist, fetched_at}`, plus `exams` when the section is in the catalog. Repeated parameters use the first value |

### 6.2 Guest endpoints

| Endpoint | Contract |
| --- | --- |
| `POST /api/watches` | `{inst, term, section, channel, target}`, 8 KiB. Creates a seat alert; 201 `{id}`. A signed-in request always uses the account channel and returns 200 with the existing ID when the alert exists. At most 200 alerts per subscriber (account or push destination) |
| `DELETE /api/watches/{id}` | Removes a seat alert |
| `POST /api/push/retarget` | `{from, to}`, 8 KiB. Moves every watch and reminder from one push subscription to another after the browser replaces it; returns `{moved}` |
| `PUT /api/reminders/{id}` | `{target, tz, items}`, 64 KiB, at most 100 items. Registers a guest's meeting patterns and due times for its push subscription; 204. Invalid ID, subscription, or time zone is 400 |
| `DELETE /api/reminders/{id}` | Removes a guest registration; 204 |
| `PUT /api/sync/{id}` | Stores a client-encrypted share blob under a 32-hex-digit ID; 204. At most 256 KiB (413). An existing ID is 409, so a link holder can read a share but not replace it |
| `GET /api/sync/{id}` | The blob as `application/octet-stream` with `X-Updated-At` (RFC 3339 UTC); 404 when absent |
| `POST /api/lms/canvas` | `{base, token}`, 4 KiB. Reads a guest's Canvas planner and courses. With `Accept: application/x-ndjson` it streams a `tasks` event (assignments usable before module checks), then `complete` with the final tasks, or `error`; otherwise one JSON array |
| `POST /api/lms/canvas/complete` | `{base, token, type, id, overrideId?, courseId?, moduleItem?}`, 4 KiB. Writes a planner item's completion to Canvas. Not retried automatically |
| `POST /api/lms/canvas/classes` | `{base, token, inst, term?}`, 4 KiB. The guest's Canvas courses matched to the catalog ([§6.5.1](#651-canvas-class-matching)) |
| `POST /api/lms/feed` | `{url}`, 4 KiB. Fetches an iCalendar feed and returns it unchanged as `text/calendar`; the client parses it |

Relay rules (guest and account Canvas, feeds):

- Destinations must be public HTTPS addresses. The outbound client resolves
  each host itself and refuses any non-public address, ignores proxy
  settings, and follows redirects only after checking each hop. Canvas
  pagination and redirects carrying tokens stay on the original origin.
- A relayed response body is limited to 4 MiB.
- One Canvas operation permits at most five concurrent requests: the course list and four planner windows.
  Module checks use four concurrent requests.
  Each operation permits at most 10 pages per collection, 100 requests, 16 MiB of response bodies, and 30 seconds. Throttled GETs retry up to twice within the same
  budget.
- Guest tokens and feed URLs arrive with each request and are never stored.

### 6.3 Authentication and account

Twilio Verify sends and checks six-digit SMS codes; Honorum owns accounts
and device sessions. One flow creates an account or signs into an existing
one ([`auth.rs`](../crates/server/src/auth.rs),
[`phone.rs`](../crates/server/src/phone.rs)).

| Endpoint | Contract |
| --- | --- |
| `POST /api/auth/code` | `{phone, consent}`, 4 KiB: an E.164 number and the current notice version. 202 `{retry_after}` |
| `POST /api/auth/verify` | `{phone, code, device}`, 4 KiB. On approval returns the account details with a bearer `token` and sets the browser cookie |
| `POST /api/auth/logout` | Revokes the current session and clears the cookie; 204 |
| `GET /api/me` | Account and device details |
| `PATCH /api/me` | `{device}`, 4 KiB. Renames the current device; returns the details |
| `DELETE /api/me/devices/{id}` | Revokes a device session; 204 |
| `DELETE /api/me` | Disconnects Google (deleting the Honorum calendar and revoking access), then deletes the account and all its server-side data; 204. 503 while a Google operation holds the account |
| `PUT /api/me/push` | `{subscription}` registers the device's Web Push subscription, `{apns}` an iPhone device token; an empty body clears it. 8 KiB; 204 |
| `GET /api/me/watches` | The account's seat alerts with class labels |

Guarantees and failure behavior:

- Provider approval happens before the transaction that creates the account
  and session. If that write fails, the student requests a new code.
- Provider requests time out after 10 seconds and are never retried
  automatically: a timeout can follow an accepted send or a consumed code.
- 202 means the provider accepted the send, not that a handset received it.
  There is no local OTP table, delivery queue, or delivery-status endpoint.
- A send records the number, timestamp, and notice version before contacting
  Twilio; a failed consent write prevents the send. Consent records last
  1 year, independently of account deletion.
- Changing the SMS notice means changing `SMS_CONSENT_VERSION` in `auth.rs`,
  each client's request and notice text, and the legal page together.
- Logs exclude phone numbers, codes, credentials, and raw provider responses.
  `phone_auth` reports configuration, not delivery health.

SMS is the only sign-in method. Phone-number changes, passkeys, and other recovery credentials are unavailable.
Loss of a number can mean loss of its account.
Interception or reassignment of a number affects account security.
See [privacy and retention](application.md#13-privacy-and-retention) and
[phone sign-in](deployment.md#81-phone-sign-in).

### 6.4 Document sync and live events

[`docs.rs`](../crates/server/src/docs.rs):

```text
POST /api/me/sync
  request:  { since, watches_version?, docs: [{ id, type, base, updated, deleted?, body }] }
  response: { seq, docs, more, applied, conflicts, watches_version?, watches? }
```

| Rule | Value |
| --- | --- |
| Documents per upload | 200 |
| Body size per document | 64 KiB |
| Upload size | 4 MiB |
| Download page | 500 documents or 4 MiB of bodies |
| Version history retention | 30 days |
| Versions per history response | 30 |

- Downloads contain `{id, type, rev, updated, deleted?, body}`. Uploads name
  the revision they edited in `base`.
- Uploads and downloads share one transaction, so an upload never hides a
  remote change behind its own cursor. Stale writes return as `conflicts`;
  accepted versions are acknowledged in `applied`.
- Clients follow `seq` and `more` until caught up, acknowledge only the exact
  versions sent, and never skip unseen remote changes. Watch changes have an
  independent version.
- A document's ID prefix names its type (`s_<id>` schedule, `t_<id>` task,
  `prefs` preferences). A body carrying an `id` must match its document.
  Anything else is rejected before storage.
- Bodies are stored and written compacted, with `<`, `>`, `&`, U+2028, and
  U+2029 escaped, so downloads copy them unchanged. Bodies earlier releases
  stored verbatim are converted as they are read. Page byte budgets count
  that encoding.
- Deletion records persist until account deletion.

| Endpoint | Contract |
| --- | --- |
| `GET /api/me/history/{id}` | Replaced versions of one document, newest first. Restoring is an ordinary upload of that body |
| `GET /api/me/events?account=<id>` | Server-sent events. A `seq` frame carries the document sequence after every commit, and `google` the Google Calendar version once the account has connected. Heartbeat every 25 s |

Live event guarantees ([`events.rs`](../crates/server/src/events.rs)):

- `pg_notify('honorum_accounts', <account id>)` wakes other API replicas after
  commits; the payload is only the account ID.
- Each heartbeat rereads the durable cursors, recovering from a lost
  notification without holding a database connection per stream. An API process groups heartbeat reads across streams.
  It spreads streams across the 25-second period.
  Each second uses queries of up to 1,000 streams. A stream that receives none for
  50 seconds reads its own.
- The stream is a wakeup only. Clients run their ordinary exchange when the
  sequence moves; a missed frame costs only the next poll.

### 6.5 Account Canvas, feeds, and streaks

These endpoints use the authentication and ownership checks of document
sync. Guests use the relay endpoints in [§6.2](#62-guest-endpoints).

| Endpoint | Contract |
| --- | --- |
| `GET /api/me/canvas` | `{id, base, connected, available, synced, error}`; `synced` is Unix seconds, 0 before the first refresh. Credentials are never returned |
| `PUT /api/me/canvas` | `{base, token}`, 4 KiB. Validates a complete import, seals the token, and publishes tasks atomically; returns status. 503 without `HONORUM_CANVAS_KEY` |
| `DELETE /api/me/canvas` | Disconnects every device and erases the sealed credential; assignments stay. Returns status |
| `POST /api/me/canvas/refresh` | `{force, rescan_modules?}`, 1 KiB. Publishes a complete import and returns status. `force` bypasses the 5-minute freshness window while reusing recent module routing; `rescan_modules` bypasses freshness and rescans modules |
| `POST /api/me/canvas/complete` | `{document: "t_<id>", intent: "<pendingCanvasDone>"}`, 1 KiB. Reads the uploaded task, sends its completion to Canvas, acknowledges the exact revision; returns `{applied, module?, overrideId?, done?}` |
| `POST /api/me/canvas/classes` | `{inst, term?}`, 1 KiB. Class matching with the account's token |
| `GET /api/me/feeds` | `{feeds: [{id, label, tz, synced, error}], available}`; the feed address is never returned |
| `PUT /api/me/feeds` | `{url, label}`, 4 KiB. Fetches and validates the feed, seals the address, publishes its tasks, returns the list. At most 20 feeds |
| `DELETE /api/me/feeds/{id}` | Removes the feed and the tasks it imported, on every device |
| `POST /api/me/feeds/refresh` | `{force}`, 1 KiB. Imports every feed; automatic refreshes share the 5-minute window |
| `POST /api/me/activity` | `{inst}`, 8 KiB. Foreground check-in; the first fixes the account time zone. `times` from older clients (at most 200, none more than 5 minutes in the future, else 400) are otherwise ignored. Returns the streak |
| `GET /api/me/streak` | `{current, longest, today, last_day, timezone, overdue, days}`. `overdue` counts tasks open past their due time now. `days` lists up to 28 days ending today as `{day, state}`: `clean` or `missed` for a finished day, `clean` or `overdue` for today. Dates are `YYYY-MM-DD`; an empty `last_day` means no clean day yet |

#### 6.5.1 Canvas class matching

`POST /api/lms/canvas/classes` and `POST /api/me/canvas/classes` answer:

```text
{ term, classes: [{ course: {id, name, code}, code, title, sections, options, sure }],
  unmatched: [{id, name, code}], sections }
```

- `sections` (per class): section IDs the match settled on.
- `options`: groups of IDs the student picks one from.
- `sure`: whether Canvas named the sections, rather than the instructor or a
  lone section implying them.
- `sections` (top level): every mentioned ID mapped to its catalog section,
  so a client can save them without the catalog.
- `term`: the catalog term matched, chosen from the terms Canvas names, the
  one requested, and the one in session by the most classes found.

[`canvas_classes.rs`](../crates/server/src/canvas_classes.rs) holds the
matching and its reasons; the product rules are in
[classes from Canvas](application.md#5-classes-from-canvas).

#### 6.5.2 Credential sealing

[`seal.rs`](../crates/server/src/seal.rs) encrypts Canvas tokens, feed URLs, and Google refresh tokens with AES-256-GCM.
`HONORUM_CANVAS_KEY` supplies a base64-encoded 32-byte key.
Keep the key separate from the database and its backups.

- A sealed value is `nonce || ciphertext` with a 12-byte nonce. The
  associated data is `account \0 connection \0 scope`, so a row cannot move
  to another account, connection, or purpose. The scope is the Canvas origin
  for Canvas tokens, `feed` for feed URLs, and `google` for Google refresh
  tokens.
- Without the key, status reads report `available: false` and connecting
  returns 503. Existing ciphertext is unusable with a missing or different
  key. Key handling is in [credential encryption](deployment.md#82-credential-encryption).

### 6.6 Google Calendar

[`google/mod.rs`](../crates/server/src/google/mod.rs) holds the OAuth flow and
the connection, [`google/publish.rs`](../crates/server/src/google/publish.rs)
the calendar Honorum writes, and
[`google/events.rs`](../crates/server/src/google/events.rs) the calendars it
reads, their change notifications, and the worker. Endpoints use the
document-sync checks, except the callback and notifications, which are
public.

| Endpoint | Contract |
| --- | --- |
| `GET /api/me/google` | `{available, connected, email, error, publish: {on, granted, tasks, synced, error}, show: {on, granted, calendars: [{id, name, color, primary, on, synced, error}]}}`; never a credential |
| `POST /api/me/google/start` | `{want: "publish" \| "show" \| "reconnect", back?: "web" \| "ios"}`. Returns `{url}`: Google's consent for that direction's scopes, or for every direction that is on, plus `openid email`, with PKCE and `include_granted_scopes` |
| `GET /api/google/callback` | Exchanges the code, keeps the sealed refresh token behind a one-time handoff, and redirects to `/?google_handoff=…#settings/schedule`, or `honorum://google?handoff=…` for a native client. A refusal returns `google_error` |
| `POST /api/me/google/finish` | `{handoff}`. Only the session that started may finish, within 10 minutes. Connects, turns on the requested direction when granted, clears a lapsed connection's error, and returns the status with `granted` (false when the student cleared the permission on Google's screen) |
| `PATCH /api/me/google` | `{publish?, tasks?, show?, calendars?: {id: on}}`, 16 KiB. Turning a direction on without its scope is 403, so clients start consent instead |
| `DELETE /api/me/google` | Deletes the Honorum calendar, stops notifications, revokes the token at Google, deletes every record; returns status |
| `GET /api/me/google/events` | `{version, etag, calendars: [{id, name, color, synced}], events: [{id, calendar, title, start, end, allDay?, location?, link?, free?}]}` for shown calendars. `If-None-Match` with the etag is 304. Times are RFC 3339; all-day events carry dates |
| `POST /api/google/notify` | Google's change notifications; the channel's secret token identifies it, and anything else is ignored |

Connection guarantees:

- The handoff prevents another person's consent link from attaching a Google account.
  The callback connects nothing. The device that started consent must finish through its own session.
- The refresh token is sealed (scope `google`) and never leaves the server;
  access tokens live only in process memory.
- A refresh refused with `invalid_grant` records an error and stops all work
  for the connection until the student connects again.
- A permission removed since connecting is detected from the scopes a
  refresh reports; that direction pauses with a note.
- Connecting another Google account replaces the connection and its
  calendars.

| Table | Contents |
| --- | --- |
| `google_connections` | One connection per account |
| `google_published` | Google ID, body hash, and returned etag per event key |
| `google_calendars` | Calendar list, copied events, and notification channels |
| `google_states` | Consent in progress |

Publishing:

- Occurrences are derived as for reminders (`plan_items` in
  [`reminders.rs`](../crates/server/src/reminders.rs)) for each term's
  primary schedule.
- An event key names the term and item. The Google ID uses base32hex of a hash of the connection and event key.
  A repeated insert returns 409 and becomes an update. This also restores events deleted in Google.
- Only keys whose hash changed are written or deleted, at most 100 per run,
  so a first publication spans several runs 30 seconds apart.
- A 404 or 410 from the calendar turns publishing off. Rate limits and
  Google failures back off exponentially up to 1 hour and report a problem
  after three failures; a non-throttling 403 waits 24 hours.

The worker (`Server::run_google`, run by the coordinator):

- Wakes every 30 seconds and when an endpoint or notification asks.
- Publishes accounts whose document sequence moved or whose daily check is
  due.
- Lists calendars every 12 hours.
- Fetches changed or due calendars and clears calendars no longer shown.
  Notification channels use a six-hour poll. Without a channel, visible calendars use a fifteen-minute poll.
- Renews notification channels 1 day before they lapse. Channels need an
  HTTPS `base_url`; a local server polls instead.
- A fetch or listing that changes anything moves the connection's version,
  which the live stream reports as `google`.
- There is one bounded Google worker per deployment (on the coordinator), so
  it can become a bottleneck at larger scale.

## 7. Local state and sync

Rules every client follows:

- Planning works without an account and without waiting for the network.
  Saved sections include enough meeting data to render a schedule offline.
- A saved edit and its pending sync version commit in one local transaction.
- Account IDs own local namespaces; phone numbers do not.
- Same-field conflicts keep the local edit and archive the remote copy for
  recovery. Independent edits merge, including lists of objects with ids
  (such as personal events), item by item. Device clocks never choose a
  winner.
- Clients preserve document fields they do not recognize, so one client can
  edit a document another created.
- Empty polls do not rewrite saved state or identical watches.
- Pending account edits survive sign-out in the account's local namespace.
- Guest LMS tokens and calendar feed URLs stay device-local. Account Canvas
  tokens and account feed URLs are sealed on the server and never enter sync
  documents. Account documents are ordinary JSON on the server; share links
  upload client-encrypted copies.

Web implementation:

- [`store.js`](../apps/web/src/lib/store.js) keeps one IndexedDB state/outbox
  pair per owner. Storage upgrades stop older open connections from writing
  the previous layout.
- Web Locks coordinate editing between tabs. Handover flushes local writes and allows pending uploads up to two seconds.
  It cancels obsolete sync and LMS work before the next tab reloads state.
  This helps pending edits reach the account before a phone suspends its tab. Without Web Locks the app falls
  back to a single tab.
- Edits upload 300 ms after the last one.
- [`sync.js`](../apps/web/src/lib/sync.js) defines the document bodies,
  three-way merge, and durable outbox.

Search and scheduling:

- The web client searches cached catalogs in
  [`catalog.worker.ts`](../apps/web/src/lib/catalog.worker.ts), at most 40
  section rows per page. A separate worker performs bounded schedule
  rearrangement; only accepting the preview changes a plan.
- Native clients need equivalent background search and scheduling with
  bounded work and cancellation.
- Calendar dates use the school's IANA time zone, including closures and
  replacement class days; [`occurrences.js`](../apps/web/src/lib/occurrences.js)
  and its tests define the civil-date behavior.

## 8. Account imports

Canvas connections and calendar feeds share one import pipeline
([`canvas_account.rs`](../crates/server/src/canvas_account.rs),
[`feeds.rs`](../crates/server/src/feeds.rs),
[`imports.rs`](../crates/server/src/imports.rs)).

Serialization and atomicity:

- Imports for one account are serialized by the `imports` advisory lock.
  Complete imports publish task revisions in one transaction. Failed or
  incomplete imports cannot delete saved assignments.
- Import requests share four slots per provider host and honor upstream 429
  backoff. This bounds provider traffic; large queues delay automatic
  refreshes.

Background refresh:

| Parameter | Value |
| --- | --- |
| Concurrent background imports per worker | 8 |
| Connection age that triggers a refresh | 1 hour |
| Required account activity | A device seen in the last 30 days |
| Retry after a failed attempt | 15 minutes |
| Freshness window shared by automatic refreshes | 5 minutes |

Students therefore receive new assignments, and reminders for them, without
opening a client.

Canvas reads:

- The planner is bookmark-paginated at 100 items per page. The date range is
  split into four contiguous windows walked concurrently and merged by item
  identity, about half the wall time of one sequential walk.
- Module completion checks need one or more requests per course.
  They run on connect, after an explicit `rescan_modules` request, and once a day. A planner-only refresh keeps each
  task's last known module item and does not postpone the daily scan. A
  rescan shares a concurrent import only if that import also scanned
  modules.
- Manual refresh sends `force: true`: fresh assignments without repeating a
  recent module scan. Both paths fetch the planner and course list when an
  import is needed.

Identity and reconciliation:

- New task IDs derive from the connection and upstream identity. Existing
  imports keep their task IDs, notes, course overrides, changed due dates,
  and pending completions. `canvas_items` remembers identities and
  deletions.
- Every import, guest or account, records the source's due date as
  `importedDue`. A `due` that differs is the student's and stays until the
  source reports a new date, which replaces it. A task without `importedDue`
  counts as unchanged.
- Duplicates, such as guest imports synced from several devices, become
  labeled manual recovery copies with their contents.
- Changing the Canvas origin requires disconnecting first; the old
  connection's tasks become manual recovery copies. Token renewal for the
  same origin keeps identities.
- A feed task's `canvasConnection` names the feed connection, despite the
  field name, and its `uid` is `feed:` followed by the item's UID. Floating
  and whole-day feed times are read in the item's `TZID`, else the campus
  time zone of the school in the account's preferences. Guest feeds on web
  and iOS use the same rule.

Completion:

- Clients upload local changes before requesting an account import or
  completion, then download through ordinary sync.
- Clients mark an account completion with a durable random
  `pendingCanvasDone` intent on the task, with `done` as the desired value.
  An iOS guest completion waiting for Canvas records only `pendingDone`; when
  such a task reaches an account, the next import turns it into an intent.
- Revision conflicts preserve newer edits. A partial module write leaves the
  intent pending for retry.
- Clients never receive the account token or create imported task IDs.

## 9. Streaks

[`streaks.rs`](../crates/server/src/streaks.rs) derives the streak from the
account's task documents, not from visits.

- A finished account-local day is clean when no task was overdue at midnight.
  A task was open if incomplete or if its `completedAt` was after that midnight. A task done without `completedAt` counts as on time.
- Days run from the account's first streak read through yesterday. After two more days, the server writes each day permanently to `account_streak_days`.
  Late sync can repair recent days. Later task omissions cannot change frozen history. The hourly sweep settles days
  for every account with a streak.
- Today counts while no task is overdue.
- Account imports that change a task's done state remove its `completedAt`.
- The first check-in fixes the account time zone to its configured school,
  or UTC when no school is known. Later check-ins cannot change it.
- `account_activity` holds visit days recorded by earlier releases; nothing
  reads or writes it.
- Account deletion removes the streak record, time zone, visit days,
  credentials, and Canvas identity records with the documents.

API tests cover these contracts; frontend activity tests cover check-ins and
account switching.

## 10. Native clients

The [iOS app](../apps/ios/README.md):

- Targets iOS 18 with Swift 6 and complete concurrency checking; no package
  dependencies.
- Keeps its plan in one atomic JSON envelope per owner (the guest and every
  account that has signed in on the device). Session tokens, guest Canvas
  credentials, and feed URLs live in the Keychain, outside the envelope and
  outside sync. Account Canvas connections use the authenticated endpoints;
  their tokens never reach a device.
- Runs catalog download, indexing, and search on their own actor. Calendar
  dates are campus civil dates, not instants.
- Exchanges the web client's documents (`s_<id>`, `t_<id>`, `prefs`) through
  `POST /api/me/sync`.
- Registers its APNs device token with `PUT /api/me/push` when `/api/config`
  reports `apns`, and stops scheduling local reminders once the server
  accepts it.
- `apps/ios/scripts/release.sh` builds, verifies, and uploads device
  archives; signing is passed on the command line and never committed.

### 10.1 Work remaining

1. Server-side seat alerts from iOS. Account alerts created on the web reach
   an iPhone through APNs; the app's own watchlist, kept in the preferences
   document, is checked on the device.
2. Encrypted share links, printing, and the bounded rearrangement search on
   iOS, all present on the web.
3. The Android app: its Kotlin/Gradle project, native persistence, FCM
   registration and server-side FCM delivery, background work, and emulator
   checks, following the [application contract](application.md).
4. Versioned API and document schemas. Routes are unversioned and document
   shapes are agreed by convention; the fixtures in `contracts/` check the
   rules each client implements, not the wire shapes.

Measure launch, calendar scrolling, search, memory, and battery use on real
devices; neither native UI nor a simulator establishes performance. Use
[Apple's SwiftUI profiling tools](https://developer.apple.com/documentation/xcode/understanding-and-improving-swiftui-performance)
and [Android Macrobenchmark with Baseline Profiles](https://developer.android.com/develop/ui/compose/performance/baseline-profiles).
