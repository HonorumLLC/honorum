# Shared contracts

This directory holds the fixtures that define the planner rules every client
implements in its own language. Each file holds inputs and the expected
result; every implementation runs the same cases. The web implementation is
the reference.

## Fixtures

| Fixture | Rule | Checked by |
| --- | --- | --- |
| `fixtures/merge.json` | Three-way document merge: independent fields both survive, string lists merge as sets, lists of objects with ids merge item by item, same-field edits keep the local value and report a conflict | `apps/web/src/lib/contracts.test.js`, `apps/ios/HonorumTests/FixtureTests.swift` |
| `fixtures/icalendar.json` | Calendar feed parsing: unfolding, escaped separators, bracketed course names, floating and whole-day times in the campus zone, `TZID`, the fortnight cutoff | web, `crates/server/src/ical.rs`, iOS |
| `fixtures/tasks-merge.json` | Import reconciliation: local edits survive, a complete import's completion wins over an unsent tick, absent open items are removed only after a complete import | web, `crates/server/src/canvas_account_tests.rs` (complete imports only) |
| `fixtures/occurrences.json` | Which meetings happen on a civil date given term bounds, removed dates, and closures with substitute weekdays or a starting time | web |

## Comparison rules

- Due times in `icalendar.json` are compared as instants, not strings,
  because platforms format RFC 3339 differently.
- Floating and whole-day times are read in the fixture's time zone, which
  every implementation is told explicitly. The web test runs in a device zone
  far from it, so a result cannot depend on the machine.

## Changing a rule

1. Change the web implementation.
2. Regenerate the expected results: `node scripts/fixtures.mjs` in `apps/web`
   (the script is `apps/web/scripts/fixtures.mjs`).
3. Review the fixture diff.
4. Update the other implementations and run every suite that checks the
   fixtures: web (`npm --prefix apps/web test`), Rust
   (`scripts/postgres.sh test cargo test --workspace`), and iOS (`make test-ios`).
