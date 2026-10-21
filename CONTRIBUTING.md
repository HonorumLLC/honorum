# Contributing to Honorum

Thank you for your help. You can report a bug, improve a document, add a school,
or propose a code change.

## Before you start

Read the [README](README.md) and the [application reference](docs/application.md).
Read the [architecture reference](docs/architecture.md) for backend or API changes.
Use the [school adapter guide](docs/adapters.md) to add a school.

Search existing issues before you open an issue. Describe the problem and the
expected result. Use synthetic data in examples and screenshots.
Report security defects through the [security policy](SECURITY.md).

For a large change, open an issue first. Explain its scope and effect on students.

## Set up the repository

Install Rust 1.90 or later, Node.js 24, Make, and Docker or Podman.
Native iOS work also needs macOS and Xcode.

1. Fork the repository if you do not have write access.
2. Clone your fork.
3. Add the upstream remote.
4. Create a task branch and worktree from the latest beta.

```sh
git remote add upstream https://github.com/HonorumLLC/honorum.git
git fetch upstream
git worktree add -b fix/my-change ../honorum-my-change upstream/beta
cd ../honorum-my-change
make setup
npm --prefix apps/web exec -- playwright install chromium
make demo
```

Open `http://localhost:18080`. The demo uses recorded school data and a local database.
It needs no account or provider credentials.
See [development](docs/development.md) for hot reload and focused checks.

## Make the change

- Keep each change small enough to review.
- Preserve offline plans, atomic local saves, pending sync edits, and conflict recovery.
- Use campus time for class dates and deadlines.
- Keep the native clients separate. Share API contracts, fixtures, and brand assets.
- Keep guest credentials on the device. Never put credentials in sync documents.
- Keep generated files, local data, and private keys out of Git.
- Commit lockfiles when dependencies change.
- Update the existing reference document when behavior changes.
- Use short sentences and consistent terms in documents and comments.
- Explain a code constraint when it needs a comment. Remove comments that only repeat the code.

Follow [AGENTS.md](AGENTS.md) when you use a coding agent.

## Check the change

Run the checks for the changed code. Run the complete suite for structural or
shared API changes:

```sh
make check
```

Run the native suite for iOS changes:

```sh
make test-ios
```

If you change a shared planner rule, update the [contract fixtures](contracts/README.md).
Run the web, Rust, and iOS suites that check those fixtures.
State any check that you could not run and its reason.

## Open a pull request

1. Review the diff, including new files.
2. Review the staged changes before you commit.
3. Push the task branch to your fork or the upstream repository.
4. Open a pull request against `beta`.

Describe the problem, the new behavior, and your test results.
Include screenshots for interface changes. Remove personal data from each screenshot.
Note any API, storage, or release steps.

The repository uses the [GNU AGPL version 3](LICENSE).
Contributions use the same license unless a file states another license.
