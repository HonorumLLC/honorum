// The shared contract fixtures, checked against the web implementation that
// generated them. The Rust server and the iOS app read these fixtures.
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
// A device zone far from the fixture's: results must not depend on it.
process.env.TZ = 'Asia/Tokyo';
const { mergeBody } = await import('./sync.js');
const { parseICS, mergeTasks, recentTasks } = await import('./tasks.js');
const { meetingsOn } = await import('./occurrences.js');

const fixture = (name) => JSON.parse(readFileSync(new URL('../../../../contracts/fixtures/' + name, import.meta.url), 'utf8'));

test('merge fixtures', () => {
  for (const c of fixture('merge.json').cases) {
    const result = mergeBody(c.base, c.local, c.remote);
    assert.deepEqual(result.body, c.body, c.name);
    assert.equal(result.conflicted, c.conflicted, c.name);
  }
});

test('calendar feed fixtures', () => {
  const { timezone, cases } = fixture('icalendar.json');
  for (const c of cases) {
    const got = recentTasks(parseICS(c.text, 'feed', c.label, timezone), Date.parse(c.now));
    assert.equal(got.length, c.expected.length, c.name);
    got.forEach((t, i) => {
      const want = c.expected[i];
      assert.deepEqual({ uid: t.uid, title: t.title, course: t.course, done: t.done, url: t.url }, { uid: want.uid, title: want.title, course: want.course, done: want.done, url: want.url }, c.name);
      assert.equal(Date.parse(t.due), Date.parse(want.due), `${c.name}: due of ${t.uid}`);
    });
  }
});

test('task import fixtures', () => {
  for (const c of fixture('tasks-merge.json').cases) {
    // Compare as JSON: the fixture cannot hold undefined fields.
    const got = JSON.parse(JSON.stringify(mergeTasks(c.existing, c.incoming, c.source, { complete: c.complete }).map(({ id, ...t }) => t)));
    assert.deepEqual(got, c.expected, c.name);
  }
});

test('occurrence fixtures', () => {
  for (const c of fixture('occurrences.json').cases) {
    assert.deepEqual(meetingsOn(c.item, c.date, c.term, c.closures).map((m) => [m.days, m.start, m.end]), c.expected, c.name);
  }
});
