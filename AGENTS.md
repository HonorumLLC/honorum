# Working in Honorum

Read [README.md](README.md) for the layout and [docs/development.md](docs/development.md)
for commands. [docs/architecture.md](docs/architecture.md) defines the backend structure
and API contracts. [docs/application.md](docs/application.md) is the product behavior reference.

- Keep the iOS and Android apps separate and native. Use Swift and SwiftUI for iOS,
  Kotlin and Jetpack Compose for Android. Share API contracts, fixtures, and brand assets.
- Preserve optional accounts, offline planning, atomic local edits and sync outboxes,
  revision conflict recovery, and campus time semantics across clients.
- Account Canvas credentials use encrypted backend storage, separate from document sync.
  Keep guest LMS credentials and calendar feed URLs device-local. Never return account
  Canvas tokens to clients. Streaks count days that ended with nothing overdue,
  derived on the server from account task documents; record `completedAt` when a
  task is completed.
- Backend code is Rust and belongs in `crates/`: `catalog` for catalog types and
  school adapters, `server` for the service, `honorum` for the binary. Keep it
  `cargo fmt` clean and free of `cargo clippy -- -D warnings` findings (`make lint`).
  Native apps keep their own platform build systems.
- Keep generated output, local data, design experiments, and signing credentials out
  of Git. Commit lockfiles (including `Cargo.lock`) and future Gradle wrappers and
  Xcode project files.
- Use a separate branch and worktree for each task. Preserve unrelated uncommitted
  edits. Review staged changes before committing; see [docs/git.md](docs/git.md).
- For structural or shared API changes, run `make check`. Install Chromium once with
  `npm --prefix apps/web exec -- playwright install chromium`. For a focused change,
  run the relevant checks. `make test-ios` covers the iOS app. There is no hosted CI:
  the pre-push hook from `make setup` runs the suites a change touches.
- The database schema is the `CREATE ... IF NOT EXISTS` statements in
  `crates/server/src/schema.rs`. A change that alters an existing table or rewrites
  rows needs a migration there; remove it once every deployment has run it, and note
  the last release that carries it in [docs/deployment.md](docs/deployment.md).
- The planner rules clients share are checked by `contracts/fixtures`. After an
  intentional rule change, regenerate them with `node scripts/fixtures.mjs` in
  `apps/web` and run the web (`npm test`), Rust (`cargo test`), and iOS suites.
- Update the existing reference document when behavior changes. Keep historical
  audits, temporary plans, and screenshots in Git history or ignored `artifacts/`.
