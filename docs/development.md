# Development and validation

Run commands from the repository root unless a step gives another directory.
Read [deployment](deployment.md#1-configuration) for settings and environment variables.
Read [architecture](architecture.md) for backend structure and API contracts.

## 1. Toolchain

| Tool | Version | Purpose |
| --- | --- | --- |
| Rust | Stable, 1.90 or later; include `rustfmt` and `clippy` | Backend build and checks |
| Node.js | 24 | Web app, edge Worker, and browser tests |
| Make | Any | Repository commands |
| Docker or Podman | Any | Local PostgreSQL 17 container |
| Xcode | Current; macOS only | iOS build and tests |

Make sure `cargo`, `rustc`, `node`, and `npm` are on `PATH`.

```sh
make setup
```

This command installs npm dependencies from both lockfiles.
It also installs the local pre-push hook through `make hooks`.
The repository has no hosted test workflow.

## 2. Builds

| Command | Output |
| --- | --- |
| `make build` | Web assets, then `bin/honorum` with the web app embedded |
| `make build-api` | `bin/honorum-api`, without web assets |
| `make build-web` | Web assets in `apps/web/dist` |

| Crate | Package | Purpose |
| --- | --- | --- |
| `crates/honorum` | `honorum` | Server binary |
| `crates/server` | `honorum-server` | Service library |
| `crates/catalog` | `honorum-catalog` | Catalog types, provider client, and school adapters |

The `honorum` package has these features:

| Feature | Default | Effect |
| --- | --- | --- |
| `web` | On | Embeds the web build in release binaries. Debug builds read the files from disk and permit an absent directory |
| `lab` | Off | Adds `honorum-lab` and server test support. Production builds omit it |

Build the web app before a release build with `web`.
Use `--no-default-features` for an API-only binary.

```sh
npm --prefix apps/web run build
cargo build --release --locked -p honorum
cargo build --release --locked -p honorum --no-default-features
HONORUM_VERSION=1.2.3 cargo build --release --locked -p honorum
```

`HONORUM_VERSION` sets the version at compile time. Its default is `dev`.
The provider user agent and `/api/config` include this value.
Container builds set it from the `VERSION` build argument.

### 2.1 Development labs

Test and performance scripts start `honorum-lab` automatically.
The code is in [`crates/server/src/lab/`](../crates/server/src/lab).

```sh
cargo build --release --locked -p honorum --features lab --bin honorum-lab
```

| Mode | Script | Purpose |
| --- | --- | --- |
| `browser` | `apps/web/e2e/server.sh` | Disposable database and release server for browser tests |
| `perf` | `apps/web/scripts/perf.mjs` | Account API and simulated Canvas for performance tests |
| `capacity` | `scripts/load-test.py` | Separate seed, server, and traffic processes for capacity tests |

## 3. Run locally

### 3.1 Demo and hot reload

```sh
make demo
```

Open `http://localhost:18080`. The demo uses recorded fixtures and random seat counts.
It uses `apps/web/e2e/config.json` and saves catalog files in `data/demo/`.
It needs no provider credentials.

For hot reload, run these commands in separate terminals:

```sh
make dev-api
```

```sh
make dev
```

The API uses fixtures on port 8080. Vite prints the web URL.
Vite proxies API, catalog, status, and admin requests to the backend.

### 3.2 Standalone server

Build the server first. This command uses the configured university adapters:

```sh
scripts/postgres.sh dev ./bin/honorum
```

The server needs `HONORUM_DATABASE_URL`.
The local database wrapper sets it when you use a container.
Flags accept one or two dashes and a separate value or an equals sign.

| Flag | Default | Purpose |
| --- | --- | --- |
| `-addr` | `HONORUM_ADDR`, else `:8080` | Listen address; `:port` binds every interface |
| `-config` | `config/schools.json` | School configuration |
| `-data` | `data` | Catalog snapshot directory |
| `-role` | `HONORUM_ROLE`, else `all` | `api`, `worker`, or `all`; see [process roles](architecture.md#2-process-roles) |

`-h` prints help and exits with code 0.
An unknown flag or absent value exits with code 2.
Logs use standard error. `RUST_LOG` sets the filter; its default is `info`.
SIGINT or SIGTERM starts shutdown. The server drains work for up to 40 seconds.

## 4. Local PostgreSQL

The backend needs PostgreSQL 17 or later.
The wrapper starts a loopback-only PostgreSQL 17 container.
It uses Podman when available, else Docker.

| Command | Effect |
| --- | --- |
| `scripts/postgres.sh start` or `make postgres` | Starts the container; creates its volume on first use |
| `scripts/postgres.sh stop` or `make postgres-stop` | Stops the container and keeps its volume |
| `scripts/postgres.sh demo COMMAND…` | Runs a command with the persistent demo database for this worktree |
| `scripts/postgres.sh dev COMMAND…` | Runs a command with a separate development database for this worktree |
| `scripts/postgres.sh test COMMAND…` | Runs a command with a cluster URL for disposable test databases |

| Variable | Effect |
| --- | --- |
| `CONTAINER_TOOL` | Selects `docker` or `podman` |
| `HONORUM_LOCAL_POSTGRES_PORT` | Sets the host port; default 15432 |
| `HONORUM_TEST_POSTGRES` | With `test`, uses an existing disposable cluster and skips the container |
| `HONORUM_DATABASE_URL` | With `demo` or `dev`, uses an existing database and skips the container |

Worktrees share the container `honorum-postgres-local-<port>`.
Each worktree has separate `honorum_demo_<id>` and `honorum_dev_<id>` databases.
The worktree path determines `<id>`.
The local container uses disposable credentials.

For an existing local PostgreSQL installation, use an admin URL such as:

```sh
export HONORUM_TEST_POSTGRES='postgres://localhost/postgres?sslmode=disable'
make test-server
```

Use a disposable cluster. The role must have permission to create and drop databases.
Each server test creates a separate database and drops it after the test.
Keep production URLs out of test commands, logs, and Git.
Production needs direct TLS connections, session advisory locks, and `LISTEN`.
Transaction pooling cannot supply the last two features.

## 5. Pre-push hook

`make setup` sets `core.hooksPath` to `scripts/githooks`.
The hook compares the branch with its merge base at `origin/main`.
It selects checks from the changed paths.

| Changed area | Checks |
| --- | --- |
| Rust, contracts, configuration, web, or deployment | `make lint` and Rust tests with all features |
| Web | Unit tests and Svelte type checks |
| Deployment | Worker tests and type checks |
| iOS | `make test-ios` |

A failure blocks the push and prints the last 30 output lines.
A missing toolchain produces a skip message. A skipped check has not passed.
Use `HONORUM_SKIP_CHECKS=1 git push` to skip the hook once.
Run `make check` before merge even when the hook passes.

## 6. Checks and tests

### 6.1 Full check

Install Chromium once:

```sh
npm --prefix apps/web exec -- playwright install chromium
```

Run the backend, web, edge, and browser suites:

```sh
make check
```

The command builds the web app, then runs these checks:

1. Rust format and Clippy checks, including development labs and the API-only build.
2. Web unit tests, type checks, and bundle budgets.
3. Edge Worker tests and type checks.
4. Rust tests with all features against local PostgreSQL.
5. The API-only release build.
6. Chromium end-to-end tests against a release server and disposable database.

Browser tests use port 18080. Select another port if it is busy:

```sh
HONORUM_TEST_PORT=18148 make check
```

### 6.2 Focused checks

```sh
make lint
make test-server
scripts/postgres.sh test cargo test -p honorum-server docs::
cargo test -p honorum-catalog
npm --prefix apps/web test
npm --prefix apps/web run check
npm --prefix apps/web run test:e2e
npm --prefix deploy/cloudflare test
npm --prefix deploy/cloudflare run check
make build-api
```

Server tests fail without `HONORUM_TEST_POSTGRES`. Use the wrapper to set it.
Catalog tests use recorded fixtures and need no database.
`HONORUM_TEST_BROWSER` selects an installed Chromium executable for browser tests.

### 6.3 iOS

```sh
make test-ios
make verify-ios-release
```

These checks need macOS and Xcode.
The first runs unit and UI tests on an iPhone 17 Pro simulator.
The second checks an unsigned device archive.
If the active developer directory points to Command Line Tools, set `DEVELOPER_DIR`:

```sh
DEVELOPER_DIR=/Applications/Xcode.app/Contents/Developer make test-ios
```

See the [iOS guide](../apps/ios/README.md) for build and release details.
The Android project does not exist yet.

### 6.4 Coverage limits

Fixture tests do not establish live university endpoint health or provider delivery.
They do not test a database restore.
Test those operations separately when the change depends on them.
Run the native suite separately from `make check`.

## 7. Shared contracts

[`contracts/fixtures`](../contracts/README.md) defines the shared planner rules.
After an intentional rule change, regenerate the fixtures:

```sh
cd apps/web
node scripts/fixtures.mjs
```

Review the fixture diff. Update each affected implementation.
Run the web, Rust, and iOS suites that check the fixtures.

## 8. Performance

### 8.1 Web budgets

The size check enforces gzip budgets:

| Budget | Limit |
| --- | --- |
| Entry JavaScript and CSS | 100 KB |
| Complete offline shell, including lazy chunks, fonts, and icons | 200 KB |

The phone-number chunk that SMS sign-in loads is excluded from the shell,
because sign-in needs the network; it is cached on first use. API and catalog
data are measured separately.

```sh
npm --prefix apps/web run build
npm --prefix apps/web run check:size
```

Maintained behavior constraints, not production latency claims:

- Saved plans stay usable while network requests run. A planner visit does
  not load the term's catalog.
- Search returns at most 40 section rows per page; schedule rearrangement has
  time and result limits.
- Empty account polls perform no saved-state writes.
- The service worker installs the complete shell before activation and keeps
  the prior shell for open tabs.

### 8.2 Performance suite

The suite measures what students wait for on a production build. `make perf`
builds the web client and runs it against local PostgreSQL; to pass options,
run it directly:

```sh
npm --prefix apps/web run build
npm --prefix apps/web run perf -- --runs=3 --only=load
```

[`perf.mjs`](../apps/web/scripts/perf.mjs) builds `honorum-lab` with the `lab` feature and starts its `perf` mode through the database wrapper.
It uses the real API with a temporary database and simulated Canvas.
Canvas has six courses and 150 assignments. `--canvas` sets its response delay; the default is `300ms`. The suite itself builds no web
assets.

Chromium tests first visits and returns on the same day or next day.
The next-day case republishes classes and seats.
Other cases cover week changes, dialogs, first and repeated searches, sign-in, signed-in returns, and tab focus.
The suite also measures edits across devices and guest and account Canvas imports.

Each measure is the
median of `--runs`. Results include page weight, paint and readiness times, blocked time, and renderer CPU time, including workers.
They also include memory after garbage collection, request counts, transferred bytes, and Canvas import requests.

[`perf-lab.mjs`](../apps/web/scripts/perf-lab.mjs) simulates production with HTTP/2, Worker cache headers, recorded public data, and a shaped network per browser context.
It serves prepared Brotli files when available. Otherwise, it compresses responses at quality 4.

| Profile | Network | CPU |
| --- | --- | --- |
| `desktop` | 40 ms latency, 20 Mbit/s | Unthrottled |
| `mobile` | Lighthouse slow 4G: 150 ms, 1.6 Mbit/s | Main thread slowed 4x; CPU time not reported |

| Option | Default | Meaning |
| --- | --- | --- |
| `--runs=N` | 3 | Repetitions per measure (median reported) |
| `--only=a,b` | all | Scenarios to run, for example `load,sync` |
| `--profiles=a,b` | `desktop,mobile` | Profiles to run |
| `--dist=DIR` | `apps/web/dist` | Build to measure |
| `--fixture=DIR` | `artifacts/perf/fixture` | Recorded public data |
| `--school=ID`, `--capture=URL` | `ou`, `https://honorum.app` | Source of the fixture, captured on the first run when absent |
| `--canvas=DURATION` | `300ms` | Simulated Canvas latency |
| `--out=DIR` | `artifacts/perf/<time>` | Results, a Markdown report, and every browser context's requests |
| `--compare=FILE` | — | Earlier `results.json` to compare against |

Compare two builds with the same fixture:

```sh
node apps/web/scripts/perf.mjs --dist=/path/to/old/dist --out=artifacts/perf/before
node apps/web/scripts/perf.mjs --out=artifacts/perf/after --compare=artifacts/perf/before/results.json
```

These are lab numbers for comparing builds, not production percentiles.
Record the commit, fixture, and machine with results you keep.

### 8.3 Runtime counters

`/api/status` exposes process-local API duration buckets, snapshot counts,
and collector queue wait totals; see
[health and status](deployment.md#7-health-status-and-analytics). Use them
alongside client measurements; local browser traces are not production
percentiles.

## 9. Capacity lab

[`scripts/load-test.py`](../scripts/load-test.py) runs synthetic loopback
traffic against the real API. It requires Rust, Python 3, and, for resource
limits, Linux with user systemd. It builds `honorum-lab`
(`cargo build --release --locked -p honorum --features lab --bin honorum-lab`)
and runs its `capacity` mode
([`crates/server/src/lab/capacity.rs`](../crates/server/src/lab/capacity.rs))
as separate seeding, server, and open-loop traffic processes.

Safeguards:

- The database must be a disposable `honorum_load*` database on IPv4
  loopback (`127.0.0.1`), and every lab address must be IPv4 loopback. No
  production URL, credential, or upstream is accepted; Canvas is a local
  simulation with 300 ms latency.
- Stages must increase and cannot exceed 100,000 accounts. A run stops at
  the first failing stage and exits with a nonzero status.

```sh
# A disposable local honorum_load* PostgreSQL database must already exist:
python3 scripts/load-test.py --postgres "$HONORUM_LOAD_DATABASE" \
  --replicas 2 --separate-worker --stages 1000,5000,10000
# Current small deployment's per-process limits and connection budgets:
python3 scripts/load-test.py --postgres "$HONORUM_LOAD_DATABASE" \
  --replicas 1 --separate-worker --query-connections 3 \
  --api-lock-connections 1 --worker-lock-connections 3 \
  --cpu-quota 75% --worker-cpu-quota 50% \
  --memory 650M --worker-memory 650M --seconds 180 \
  --stages 1000,3500,5000,10000,25000,50000,100000
```

| Option | Default | Meaning |
| --- | --- | --- |
| `--postgres URL` | required | The disposable load database |
| `--stages a,b,…` | `100,500,1000,2500,5000,10000,25000,50000,100000` | Active accounts per stage |
| `--replicas N` | 1 | API replicas (1 or 2; 2 requires `--separate-worker`) |
| `--separate-worker` | off | Run background jobs in a separate, resource-limited process; it does not run the API event listener |
| `--seconds`, `--warmup` | 90, 15 | Measured and warm-up seconds per stage |
| `--tasks`, `--days` | 100, 90 | Manual tasks per account (2–199) and frozen streak days (at least 3) |
| `--cpu-quota`, `--memory` | `25%`, `1G` | Per-process systemd limits; no swap |
| `--worker-cpu-quota`, `--worker-memory` | the API limits | Worker limits |
| `--query-connections` | 16 | Query pool per process (1–64) |
| `--api-lock-connections`, `--worker-lock-connections` | 8, 8 | Lock pools (1–8) |
| `--no-streams` | off | Skip event streams |
| `--no-limits` | off | Run unconfined; limits are recorded as `none (no systemd)` |
| `--lab-binary`, `--traffic-binary` | the Cargo build | Other `honorum-lab` builds to compare |
| `--out DIR` | `artifacts/load/<time>` | Results and exact copies of the harness sources |

Every account is active and holds one event stream.
It sends one sync and one streak read per minute, plus one edit every five minutes. Accounts have the configured manual tasks, frozen streak history,
and reminders; half have Canvas connections. Each stage includes one forced
hourly sweep.

Each endpoint must have p95 at most one second and errors at most 1%.
Generator lag p95 must stay below 100 ms.
At least 99% of streams must open with no drops.
The forced streak maintenance pass must complete. Queue depth and
database size are reported too.

Limits of the lab:

- With systemd, only the server processes are confined; the traffic driver
  and PostgreSQL run outside those scopes, so constrain and record database
  resources separately. Without systemd (macOS) resources come from each
  server's `/__load/stats`.
- On macOS, loopback clients have 16,384 ephemeral ports (49152–65535). Each
  event stream holds one, so stages above about 16,000 accounts cannot open
  every stream and fail the stream criterion regardless of the server. Use
  `--no-streams` above that size, or run on Linux.
- Reminder deliveries due during the window run in the same process as the
  API. A server that keeps pace with deliveries does more work than one that falls behind.
  Compare `delivered` in the `after` statistics as well as latency.
- The limits apply independently on the lab host. They do not reproduce the
  production VPS's single shared CPU, its CPU steal time, or managed database
  latency. Compare resource budgets as well as timings across runs.
- Runs exclude Cloudflare routing, real network and database latency,
  catalog collection, Google, calendar feeds, SMS and push, and client
  rendering. They measure active connected accounts, not registered
  accounts, and prove neither sustained provider refresh throughput nor a
  production capacity ceiling. Run a longer soak against a representative
  staging database before setting a production limit.
