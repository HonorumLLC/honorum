# Deployment and configuration

This reference defines server settings, environment variables, and production procedures.
It covers releases, credentials, health checks, and recovery. DigitalOcean runs the Rust API, background worker, and managed PostgreSQL.
Cloudflare serves the web app and caches public catalogs in R2.
A private tunnel connects the edge to the backend. Backend and
edge releases are independent: release compatible API changes before
publishing the frontend, because installed mobile clients and older web tabs
must keep working.

## 1. Configuration

### 1.1 Configuration file

Settings live in `config/schools.json`, selected with `-config`. The images
copy it to `/app/config.json`. Startup fails on an out-of-range value.

| Key | Default | Meaning |
| --- | --- | --- |
| `base_url` | `http://localhost:8080` | Public URL for cookies, alert links, write-origin checks, and Google callbacks |
| `contact` | `mailto:support@honorum.app` | `mailto:` address used as the push (VAPID) contact and in the provider user agent |
| `poll_seconds` | 120 | Interval between checks of watched sections; at least 10 s |
| `catalog_hours` | 6 | Interval between active-term catalog refreshes; at least 1 h |
| `archive_catalog_hours` | 168 | Refresh interval once a known term ended more than 14 days ago; at least `catalog_hours` |
| `debounce_minutes` | 30 | Minimum gap between seat alerts for the same subscriber and section; nonnegative |
| `vapid_public`, `vapid_private` | generated | Optional fixed key pair for push notifications ([§1.4](#14-push-keys)) |
| `trust_proxy` | `false` | Trust `cf-connecting-ip`/`X-Forwarded-For` for client IPs. Enable only behind a trusted proxy |
| `institutions` | `[]` | Schools to load |

Institution entries:

| Key | Meaning |
| --- | --- |
| `id`, `name` | Identifier and display name |
| `adapter` | Adapter name ([adapters](adapters.md)); an unknown name stops startup |
| `url`, `env` | Provider settings the adapter requires |
| `timezone` | IANA campus time zone; an unknown zone stops startup |
| `poll_seconds` | Overrides the school's seat interval; at least 10 s. TAMU enforces at least 300 s because each seat check downloads a whole term |
| `lms` | `{kind, url}` prefilling the assignment connection dialog; `kind` is `canvas`, `blackboard`, `brightspace`, or `moodle` |
| `closures` | Replaces the school's built-in academic calendar |

Unknown term dates keep the active-term collection interval.

### 1.2 Command-line flags

`-addr`, `-config`, `-data`, and `-role` are documented in
[development](development.md#32-standalone-server).

### 1.3 Environment variables

Environment variables supply secrets and override deployment settings.
Settings marked "together" are refused at startup when only some are set.

| Variable | Setting |
| --- | --- |
| `HONORUM_DATABASE_URL` | Required PostgreSQL URL. Use direct TLS connections in production |
| `HONORUM_ADDR` | Listen address, default `:8080` (`-addr` overrides) |
| `HONORUM_ROLE` | `api`, `worker`, or `all`, default `all` ([process roles](architecture.md#2-process-roles)) |
| `HONORUM_DB_CONNECTIONS` | Query pool size per process, default 16, range 2–100. Lock and `LISTEN` connections are additional |
| `HONORUM_DB_LOCK_CONNECTIONS` | Advisory-lock pool per process, default 8, range 1–8. A worker needs at least 2: the coordinator holds one for as long as it runs, and imports need another |
| `HONORUM_BASE_URL` | Overrides `base_url` |
| `HONORUM_TRUST_PROXY` | `1` enables `trust_proxy` |
| `HONORUM_CATALOG_OBJECTS` | `1` publishes immutable catalog objects to R2 |
| `HONORUM_R2_BUCKET`, `HONORUM_R2_ENDPOINT` | Catalog publication bucket and S3-compatible R2 endpoint |
| `HONORUM_R2_ACCESS_KEY_ID`, `HONORUM_R2_SECRET_ACCESS_KEY` | R2 publication credentials |
| `HONORUM_TWILIO_ACCOUNT_SID`, `HONORUM_TWILIO_AUTH_TOKEN`, `HONORUM_TWILIO_VERIFY_SERVICE_SID` | Phone sign-in, together ([§8.1](#81-phone-sign-in)) |
| `HONORUM_CANVAS_KEY` | Base64 32-byte AES-256 key sealing account Canvas tokens, calendar feed URLs, and Google refresh tokens ([§8.2](#82-credential-encryption)) |
| `HONORUM_GOOGLE_CLIENT_ID`, `HONORUM_GOOGLE_CLIENT_SECRET` | Google Calendar OAuth web client, together ([§8.3](#83-google-calendar)) |
| `HONORUM_VAPID_PUBLIC`, `HONORUM_VAPID_PRIVATE` | Push keys; used only when both are set |
| `HONORUM_APNS_KEY` or `HONORUM_APNS_KEY_FILE` | APNs signing key: `.p8` contents, or a path read when `HONORUM_APNS_KEY` is empty |
| `HONORUM_APNS_KEY_ID`, `HONORUM_APNS_TEAM_ID`, `HONORUM_APNS_TOPIC` | Key identifier, Apple team, and the app's bundle identifier. The four APNs settings go together; the key must be PEM |
| `HONORUM_APNS_SANDBOX` | `1` uses Apple's sandbox gateway, for development builds |
| `HONORUM_ADMIN_TOKEN` | At least 32 characters; enables the [analytics dashboard](#72-analytics-dashboard) |
| `RUST_LOG` | Log filter, default `info`. Logs go to standard error |
| `MAINTENANCE` | Edge Worker variable, not a server variable: `1` pauses new API and catalog requests ([§5](#5-cloudflare)) |

Build-time only: `HONORUM_VERSION` stamps the version into the binary
(default `dev`); the Dockerfiles set it from the `VERSION` build argument.
Test and development variables (`HONORUM_TEST_POSTGRES`,
`HONORUM_LOCAL_POSTGRES_PORT`, `HONORUM_TEST_PORT`, and others) are in
[development](development.md).

### 1.4 Push keys

Without configured VAPID keys, the server generates a pair on first start and
stores it in the database (`kv`). Browser subscriptions are bound to that
pair: keep the keys when replacing the database, or set them explicitly.
`/api/config` exposes the public key.

### 1.5 Provider user agent

University requests use:

```text
Mozilla/5.0 (compatible; Honorum/<version>; +<base_url>; <contact>)
```

## 2. Topology

| Component | Location | Role |
| --- | --- | --- |
| Edge Worker and web assets | Cloudflare | Static files, public config and catalog caching, private routing |
| Catalog objects | Cloudflare R2, private bucket | Public catalog files under `honorum-public/catalog/` |
| API process | DigitalOcean Droplet | `HONORUM_ROLE=api` |
| Worker process | DigitalOcean Droplet | `HONORUM_ROLE=worker`: collection, imports, streaks, reminders, delivery |
| Cloudflare Tunnel | DigitalOcean Droplet | Private path from Workers VPC services to the processes |
| PostgreSQL | DigitalOcean managed database, same VPC | All durable data |

## 3. DigitalOcean production

### 3.1 Layout

The `honorum-production` Droplet runs `honorum.service`, which starts the API,
worker, and tunnel from [`compose.yaml`](../deploy/digitalocean/compose.yaml).
Both processes share managed PostgreSQL in the production VPC.
Separate volumes, `api-data` and `worker-data`, hold catalog files that the server can rebuild.
Account data persists only in PostgreSQL.

| Resource | Size |
| --- | --- |
| Droplet | NYC3, Ubuntu 24.04, 1 shared CPU, 2 GiB RAM |
| Managed PostgreSQL | PostgreSQL 17, one node, 1 GiB RAM |
| `api` container | 0.75 CPU, 650 MiB memory, loopback port 8080 |
| `worker` container | 0.5 CPU, 650 MiB memory, loopback port 8081 |
| `tunnel` container | 0.25 CPU, 128 MiB memory |

- Containers drop all capabilities and use `no-new-privileges`.
  They retain three rotated 10 MB log files.
  The stop grace period is 45 seconds; the server drains for up to 40 seconds.
- Health checks request `/readyz` every 30 s (5 s timeout, 3 retries, 60 s
  start period).
- Both processes share one server failure domain. Add capacity and a
  database standby based on measured load and availability needs.

### 3.2 Network

- Two Workers VPC services reach the tunnel's loopback API port 8080 and
  worker port 8081.
- The firewall admits SSH only from the administrator's single IPv4 address.
  HTTP and PostgreSQL ports are private.
- The edge replaces forwarded client addresses and keeps private responses
  out of its public cache. Both processes set `HONORUM_TRUST_PROXY=1` and
  `HONORUM_BASE_URL=https://honorum.app`.

### 3.3 Database connections

The managed database allows 22 connections.

| Process | Query | Advisory lock | `LISTEN` | Total |
| --- | --- | --- | --- | --- |
| `api` | 3 | 1 | 1 | 5 |
| `worker` | 3 | 3 (one held by the coordinator) | 0 | 6 |
| Configured maximum | | | | 11 |

Reserve headroom for administration and replacement processes; raise the
budget before adding replicas. Connect to the direct private hostname with
`sslmode=verify-full` and the provider CA mounted at
`/etc/honorum/postgres-ca.crt`. Transaction pooling cannot preserve session
advisory locks or the `LISTEN` connection.

### 3.4 Provisioning

Review the plan, then apply it:

```sh
python3 deploy/digitalocean/provision.py --ssh-cidr YOUR_ADDRESS/32
python3 deploy/digitalocean/provision.py --ssh-cidr YOUR_ADDRESS/32 --apply
```

- The script uses the `honorum` doctl context and checks the account, VPS
  price, resource names, and recorded resource IDs. Check provider pricing
  before provisioning.
- Rerun it after database creation finishes to add the production Droplet
  tag to the database's trusted sources.
- Resource records belong in ignored `artifacts/production/`.

### 3.5 Host credentials

Install these files before the first backend release, each mode 0600:

| File | Contents |
| --- | --- |
| `/etc/honorum/app.env` | PostgreSQL URL, credential key and provider credentials, and R2 publication settings |
| `/etc/honorum/postgres-ca.crt` | Managed PostgreSQL CA certificate |
| `/etc/honorum/tunnel-token` | Token for the `honorum-production` Cloudflare Tunnel |

- Compose reads `app.env` with `format: raw`, preserving dollar signs and
  quotes in credentials.
- Preserve `HONORUM_CANVAS_KEY`, Twilio and Google credentials, and the
  database-stored VAPID keys across releases and restores.
- Backend secrets live only on DigitalOcean; the edge holds no database or
  authentication secrets.
- Catalog publication uses the `HONORUM_R2_*` variables.
  The worker enables it with `HONORUM_CATALOG_OBJECTS=1`.

## 4. Release procedures

Run `make check` before merging (checks run locally; see
[pre-push hook](development.md#5-pre-push-hook)). Release from a clean,
tested commit on `main`.

### 4.1 Backend

```sh
deploy/digitalocean/build-release.sh
```

1. The script builds [`deploy/digitalocean/Dockerfile`](../deploy/digitalocean/Dockerfile)
   with Podman (`CONTAINER_TOOL=docker` selects Docker). The image compiles
   the API-only binary on `rust:1-alpine` with `VERSION` set to the commit,
   and runs it on Alpine as an unprivileged user.
2. It writes an ignored bundle, `artifacts/production/releases/COMMIT.tar.gz`,
   containing the image, `compose.yaml`, `honorum.service`,
   `install-release.sh`, the version, and SHA-256 checksums.
3. Transfer the bundle over verified SSH and unpack it under
   `/opt/honorum/releases/COMMIT`.
4. Run its `install-release.sh` as root. The installer verifies the
   checksums and the host credential files, loads the image, records the
   previous release path in `/opt/honorum/previous`, and restarts
   `honorum.service`. Systemd waits up to 180 seconds for Compose readiness.
5. Check API and worker health, logs, and R2 publication.

The image cannot start without `HONORUM_DATABASE_URL`. The installer does not
roll back a failed release or reverse schema changes. To return to a previous
compatible bundle, run that bundle's installer. Keep the current database and
named volumes: restoring an earlier database discards later writes.

### 4.2 Edge and frontend

The manual GitHub `Deploy` workflow (`.github/workflows/deploy.yml`) publishes
the edge Worker and frontend from `main`. It runs no tests and does not
release the backend. It generates ignored `wrangler.release.jsonc` stamped
with the edge commit, deploys it, and verifies `/readyz`, the frontend entry
assets, and the backend release version. Backend and edge commits may
differ.

Its `production` environment needs:

| Secret | Value |
| --- | --- |
| `CLOUDFLARE_ACCOUNT_ID` | Cloudflare account ID |
| `CLOUDFLARE_API_TOKEN` | Token with Workers, R2 binding, and Workers VPC deployment permissions |

A manual release, from `deploy/cloudflare/`:

```sh
npm ci
npx wrangler login
node scripts/release-config.mjs "$(git rev-parse HEAD)"
npx wrangler deploy --config wrangler.release.jsonc
node scripts/verify-release.mjs "$(git rev-parse HEAD)"
```

### 4.3 Beta frontend

The `Deploy beta` workflow (`.github/workflows/deploy-beta.yml`) publishes the
`beta` branch's frontend to `beta.honorum.app` every day at 09:00 UTC, and can
also be run by hand. It deploys the `beta` environment in
`deploy/cloudflare/wrangler.jsonc`: a separate `honorum-beta` Worker that
shares production's backend, catalog bucket, and tunnels, so only the
frontend differs. The backend deploys from `main` alone (§4.1). GitHub reads
the schedule from `main` and the Worker configuration from `beta`, so a
change to either belongs on both branches.

It runs no tests and uses no GitHub environment, so it reads
`CLOUDFLARE_ACCOUNT_ID` and `CLOUDFLARE_API_TOKEN` from repository secrets.
Sign-in cookies belong to one host, so beta needs its own sign-in. Google
Calendar's callback returns to `HONORUM_BASE_URL`, `honorum.app`.

A manual beta release, from `deploy/cloudflare/` on `beta`:

```sh
node scripts/release-config.mjs "$(git rev-parse HEAD)"
npx wrangler deploy --config wrangler.release.jsonc --env beta
node scripts/verify-release.mjs "$(git rev-parse HEAD)" https://beta.honorum.app
```

## 5. Cloudflare

[`wrangler.jsonc`](../deploy/cloudflare/wrangler.jsonc) configures the
Worker's static assets, the `CATALOG` R2 binding, and the private API and
worker services.

- Both private bindings are required; a missing binding returns 503.
- `MAINTENANCE=1` returns 503 with `Retry-After` for API, catalog, status,
  and admin requests while the app shell and health endpoints stay
  available. It does not stop backend jobs or existing event streams.
- The Worker serves precompressed Brotli web assets to browsers that accept
  them; the standalone server serves the same files.
- `npx wrangler tail` shows edge logs; `<BASE_URL>/status` shows application
  status; `<BASE_URL>/admin` is the [analytics dashboard](#72-analytics-dashboard).
- The `CATALOG` binding names the private R2 bucket. Only
  `honorum-public/catalog/` is public through the Worker.
  Keep immutable files because existing catalog URLs refer to them.
- A new installation needs its own bucket, tunnel, and private services, with
  matching names and service IDs in `wrangler.jsonc`. Preserve existing
  production IDs.
- Keep the applied Durable Object migration history in `wrangler.jsonc`.
  Cloudflare uses these identifiers for existing deployments.
  The retired class and bucket names are deployment identities.
  A source cleanup must not rename them.

### 5.1 Caching

| Content | Browser | Edge |
| --- | --- | --- |
| `/api/config` | 30 s fresh; explicit reloads revalidate | 60 s |
| Catalog publications | 1 year, immutable | 1 year, a separate cache key per encoding |
| Private API responses | — | never stored |

- Public data fetches omit credentials.
- Catalog files are content addressed. The Worker uses Cloudflare's original
  client encoding, preserves the origin's browser freshness after cache
  retrieval, and marks prepared catalog bodies `no-transform`.
- Prepared catalog bytes enter the cache as opaque data.
  This prevents the Cache API from decoding them.
  The Worker restores the JSON type and encoding for each visitor. Normalized request headers, the
  zone's Browser Cache TTL, and recompression therefore cannot change these
  delivery contracts.
- The Worker Cache API is local to each data center. Catalog misses read the
  `CATALOG` binding; a missing object is rebuilt by the worker. See
  [Cloudflare cache behavior](https://developers.cloudflare.com/workers/runtime-apis/cache/).

## 6. Data and recovery

### 6.1 Backups

- Managed PostgreSQL backups and point-in-time recovery protect account data.
  VPS backups protect configuration.
- Test restoration. Keep the credential and push keys separately from the
  database.
- Restoring an earlier recovery point discards later writes. When diagnosing
  a release, prefer a forward fix that keeps current account data.

### 6.2 Schema and migrations

The `CREATE ... IF NOT EXISTS` statements in
[`crates/server/src/schema.rs`](../crates/server/src/schema.rs) define the
schema. Every start creates missing schemas, tables, indexes, and functions.

- A change to an existing table or to stored rows needs an explicit
  migration in `schema.rs`.
- Remove a migration once every deployment has run it, and record here the
  last release that carries it.
- The current schema carries no migrations.

### 6.3 Migration history

Commit `926242f` is the last release source with the PostgreSQL import migration.
The current schema has no pending migrations.

### 6.4 Client storage

Browser storage upgrades remain necessary for existing clients. IndexedDB
upgrades records when the app opens and stops other open tabs from writing
while an upgrade runs.

## 7. Health, status, and analytics

### 7.1 Health and status

| Endpoint | Behavior |
| --- | --- |
| `GET /healthz` | 200 while the process runs |
| `GET /readyz` | 200 once the database answers, else 503. Catalog files rebuild on request and do not affect readiness |
| `GET /status`, `GET /api/status` | Collection and delivery status, cached for 30 s |
| `GET /api/config` | Reports `phone_auth`, `apns`, and `google_calendar` as booleans: whether each is configured, not whether the provider is healthy |

The `performance` field of `/api/status` holds process-local counters that
reset with the process and contain no task contents or credentials:

- Cumulative API duration buckets with upper bounds 10, 50, 100, 500, and
  1,000 ms, then an overflow bucket.
- Mean API duration.
- Snapshot publications and skips.
- Cumulative collection queue wait.

### 7.2 Analytics dashboard

`/admin` charts account totals, active accounts, sign-ins, new accounts, and accounts by school.
It also charts edits, seat watches, shares, requests, API time, errors, connections, imports, import failures, and notifications.
Active-account periods are one day, one week, and thirty days. A 7-, 30-, or 90-day range is charted
by day and a year by week; tiles compare the range with the previous one.
`GET /api/admin/metrics?days=N` returns every metric by day as JSON
(`days` is 7, 30, 90, or 365; anything else means 30).

- Set `HONORUM_ADMIN_TOKEN` to a random value of at least 32 characters, for
  example from `openssl rand -base64 32`. Without it both paths return 404.
- Browsers send the token as the HTTP Basic password with any user name;
  scripts send `Authorization: Bearer <token>`. Requests are limited to 60 per
  minute per address.

Storage (`daily_metrics`):

- One number per metric per UTC day, kept for 2 years.
- Counters flush every minute and on shutdown; totals are read every
  10 minutes, and a day's last reading stands for the day.
- Rows hold counts only, never an account, device, phone number, or address.
  Guests have no identity and appear only in request counts.
- On a database without history, the first start backfills accounts and new
  accounts from account creation dates; deleted accounts are missing from
  that history.

## 8. Integrations

### 8.1 Phone sign-in

1. Create a Twilio Verify service named Honorum with six-digit SMS codes and
   the public hostname's origin-bound SMS template. See
   [Twilio Verify setup](https://www.twilio.com/docs/verify/quickstarts) and
   [origin-bound SMS guidance](https://www.twilio.com/docs/verify/developer-best-practices).
2. Check the provider account's trial restrictions, geographic permissions,
   Fraud Guard, and send limits.
3. Set `HONORUM_TWILIO_ACCOUNT_SID`, `HONORUM_TWILIO_AUTH_TOKEN`, and
   `HONORUM_TWILIO_VERIFY_SERVICE_SID` together.
4. Verify delivery and autofill on the supported devices with the configured
   service. Fixture tests replace Twilio and send no SMS.

Requirements:

- Clients show the current SMS consent notice beside Send Code and Resend
  Code and link to `/legal.html`. The notice text, client request versions,
  `SMS_CONSENT_VERSION` in `crates/server/src/auth.rs`, and the legal page
  change together ([authentication contract](architecture.md#63-authentication-and-account)).
- Verification failures log a status and provider error code, without phone
  numbers or raw responses.
- The web app uses `autocomplete="one-time-code"` and does not call WebOTP,
  because some browsers expose the API without a working implementation.
- The Privacy Policy and Terms are served at `/legal.html`, linked beside
  each SMS request action, and included in the offline shell. Support goes to
  `support@honorum.app`.
- New visitors get the landing page at `/` ([site root](architecture.md#22-site-root));
  it is also served at `/about` and stays available offline.

### 8.2 Credential encryption

`HONORUM_CANVAS_KEY` seals account Canvas tokens, account calendar feed URLs,
and Google refresh tokens.

1. Generate a key: `openssl rand -base64 32`.
2. Store it in the backend environment (`/etc/honorum/app.env` in
   production) and in a secret manager, separately from the database and its
   backups. Keep it out of source files, logs, and Git.
3. Retain it for database restores.

Failure behavior:

| Condition | Effect |
| --- | --- |
| Key missing | Account Canvas, account feeds, and Google Calendar are unavailable; connecting returns 503 |
| Key changed or lost | Saved credentials are unreadable. Restore the old key, or users reconnect to seal with the new key |

Automatic key rotation and re-encryption are not implemented. Guest relays
and account streaks do not need the key.

After deployment, use a test account.
Connect Canvas. Sign in on a second device without a token.
Refresh assignments.

Complete an assignment.
Disconnect Canvas. Check the result on both devices. Check the streak on both devices.

Today must count while nothing is overdue.
Let a task pass its deadline. Check that the streak drops.
Complete the task before midnight.

Check that the streak returns.

### 8.3 Google Calendar

`/api/config` reports `google_calendar: true`, and clients show the feature,
when both `HONORUM_GOOGLE_CLIENT_*` variables and `HONORUM_CANVAS_KEY` are
set. Change notifications additionally need an HTTPS `base_url`; without one
the server polls shown calendars every 15 minutes.

1. In the [Google Cloud console](https://console.cloud.google.com/), create
   a project and enable the Google Calendar API.
2. Configure the OAuth consent screen as an external app.
   Set its name to Honorum. Add the support address and logo.
   Use `honorum.app` as the authorized domain.
   Use `https://honorum.app/about` as the home page.
   Use `https://honorum.app/legal.html` as the privacy policy. Add the scopes
   `openid`, `email`, `calendar.app.created`,
   `calendar.calendarlist.readonly`, and `calendar.events.readonly`.
3. Create an OAuth client ID of type Web application with redirect URI
   `https://honorum.app/api/google/callback`. For a local server, add
   `http://localhost:8080/api/google/callback` and set `HONORUM_BASE_URL` to
   match.
4. Store `HONORUM_GOOGLE_CLIENT_ID` and `HONORUM_GOOGLE_CLIENT_SECRET` in the
   backend environment and restart the API and worker.

Google's rules:

- In Testing mode, only listed test users can connect.
  Google expires their refresh tokens after seven days, so these connections need weekly renewal.
- Before opening to everyone, submit the app for verification. The calendar scopes are sensitive.
  Google reviews the privacy policy, verified domain, a consent video, and the reason for each scope.
  The video must show each scope in use. Unverified apps show a warning and are limited to a small
  number of users. See
  [sensitive scope verification](https://developers.google.com/identity/protocols/oauth2/production-readiness/sensitive-scope-verification).
- A school's Google Workspace administrator can block Honorum for managed
  accounts until they allow it.

Change notifications arrive at `https://honorum.app/api/google/notify`
through the Worker; Google calls only HTTPS addresses with valid
certificates. After deployment, use a test user.
Turn on Add Classes. Check the Honorum calendar in Google Calendar.

Edit a class. Check that the change appears in Google Calendar.
Turn on Show Google Events. Add an event in Google Calendar.

Check that the event appears in Honorum. Disconnect Google Calendar.
Check that the Honorum calendar is absent.
Check that the Google account no longer lists Honorum under third-party access.

### 8.4 Apple Push Notification service

Set the four `HONORUM_APNS_*` settings ([§1.3](#13-environment-variables)) to
enable iPhone delivery; `/api/config` then reports `apns`. Use
`HONORUM_APNS_SANDBOX=1` for development builds.

## 9. Standalone hosting

Standalone hosting runs one binary with the web client embedded, next to
PostgreSQL, from the root [`Dockerfile`](../Dockerfile) and
[`docker-compose.yml`](../docker-compose.yml). Generate a URL-safe password and
keep it in an ignored environment file for later starts:

```sh
export HONORUM_POSTGRES_PASSWORD="$(openssl rand -hex 32)"
docker compose up --build
```

- Compose stores the database in the `postgres-data` volume, mounts `./data`
  for catalog files and `./config/schools.json` read-only, and publishes
  port 8080. PostgreSQL is reachable only inside the Compose network.
- It passes through the Twilio variables and `HONORUM_CANVAS_KEY`.
- Set `base_url` and `contact` for the installation.
- Keep the same password and volume across restarts. Changing the
  environment password does not rotate the database role's password.

Local demos and tests use `scripts/postgres.sh`; see
[local PostgreSQL](development.md#4-local-postgresql).

## 10. iOS releases

The `iOS` workflow (`.github/workflows/ios.yml`) uploads app changes on
`beta` to TestFlight for internal testers. Promoting them to `main` submits
the same TestFlight build for App Store review, and the version releases once
Apple approves it. Promote after beta's iOS run has finished; main fails if no
TestFlight build matches its source. See
[releasing the iOS app](../apps/ios/README.md#release) for versions, secrets,
and releasing from a Mac.
