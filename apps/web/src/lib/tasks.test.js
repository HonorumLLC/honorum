import { test } from 'node:test';
import assert from 'node:assert/strict';
import { parseICS, icsDate, wallInstant, mergeTasks, groupTasks, upNext, taskHeadings, headingLabel, taskWhen, createCourseMatcher } from './tasks.js';

const matchCourse = (name, codes, titles) => createCourseMatcher(codes, titles)(name);

const ics = `BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nUID:a1\r\nSUMMARY:Homework 3 [CS 1213-010]\r\nDTSTART:20260910T235900Z\r\nURL:https://canvas/x\r\nEND:VEVENT\r\nBEGIN:VTODO\r\nUID:t1\r\nSUMMARY:Read chapter \r\n 2\r\nDUE;VALUE=DATE:20260912\r\nSTATUS:COMPLETED\r\nEND:VTODO\r\nBEGIN:VEVENT\r\nSUMMARY:Office hours\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n`;

test('parseICS reads events and todos, unfolds lines, splits Canvas course tags', () => {
  const t = parseICS(ics, 'feed', 'MATH 151');
  assert.equal(t.length, 3);
  assert.deepEqual([t[0].title, t[0].course, t[0].due, t[0].url, t[0].uid], ['Homework 3', 'CS 1213-010', '2026-09-10T23:59:00.000Z', 'https://canvas/x', 'feed:a1']);
  assert.deepEqual([t[1].title, t[1].course, t[1].done], ['Read chapter 2', 'MATH 151', true]);
  assert.equal(t[2].due, '');
  assert.equal(icsDate('20260912'), new Date(2026, 8, 12, 23, 59).toISOString());
});

test('mergeTasks keeps local done state and manual tasks, drops vanished imports', () => {
  const existing = [{ id: 'm', title: 'Buy book', source: 'manual', done: false }, { id: 'x', uid: 'feed:a1', title: 'HW 3', source: 'feed', done: true, course: 'CS 101', courseEdited: true }, { id: 'y', uid: 'feed:gone', title: 'Old', source: 'feed', done: false }];
  const out = mergeTasks(existing, [{ uid: 'feed:a1', title: 'HW 3 (v2)', course: 'CS 1213', due: '', done: false, source: 'feed' }, { uid: 'feed:new', title: 'New', course: '', due: '', done: false, source: 'feed' }], 'feed');
  assert.deepEqual(out.map((t) => t.title), ['Buy book', 'HW 3 (v2)', 'New']);
  assert.equal(out[1].done, true);
  assert.equal(out[1].course, 'CS 101');
  assert.ok(out[2].id);
});

test('a saved Canvas checkbox intent survives refresh until Canvas acknowledges it', () => {
  const existing = [{ id: '1', uid: 'canvas:assignment:1', source: 'canvas', done: false, pendingCanvasDone: 'intent', reason: '' }];
  const incoming = [{ uid: 'canvas:assignment:1', source: 'canvas', done: true, reason: 'marked', moduleItem: '3:4' }];
  const [merged] = mergeTasks(existing, incoming, 'canvas');
  assert.equal(merged.done, false);
  assert.equal(merged.reason, '');
  assert.equal(merged.pendingCanvasDone, 'intent');
  assert.equal(merged.moduleItem, '3:4');
});

test('groupTasks buckets', () => {
  const now = new Date(2026, 8, 3, 9, 0);
  const at = (d, h = 23, m = 59) => new Date(2026, 8, d, h, m).toISOString();
  // Sep 3 2026 is a Thursday: the 5th and 6th are this week, the 7th to 13th next week, the 14th later.
  const g = groupTasks([{ due: at(3, 8) }, { due: at(3) }, { due: at(4) }, { due: at(5) }, { due: at(6) }, { due: at(7) }, { due: at(13) }, { due: at(14) }, { due: at(1) }, { due: '' }], now);
  assert.deepEqual([g.overdue.length, g.today.length, g.tomorrow.length, g.week.length, g.next.length, g.later.length, g.undated.length], [1, 2, 1, 2, 2, 1, 1]);
  // With Sunday-start weeks the 6th (a Sunday) belongs to next week.
  const gs = groupTasks([{ due: at(5) }, { due: at(6) }], now, 'sun');
  assert.deepEqual([gs.week.length, gs.next.length], [1, 1]);
});

test('Up Next lists only this week and folds the rest whole', () => {
  const now = new Date(2026, 9, 2, 14, 15); // a Friday
  const at = (m, d, h = 23, min = 59) => ({ id: `${m}-${d}-${h}`, due: new Date(2026, m - 1, d, h, min).toISOString() });
  const late = at(9, 30), sun = at(10, 4), mon = at(10, 5, 15, 0), tue = at(10, 6), later = at(10, 14), undated = { id: 'x', due: '' };
  const { near, folds } = upNext([undated, later, tue, mon, sun, late], now);
  assert.deepEqual(near.map((t) => t.id), [late.id, sun.id]);
  // Monday is next week, so it waits in the Next Week fold, however close.
  assert.deepEqual(folds.map((f) => [f.key, f.items.map((t) => t.id)]), [['next', [mon.id, tue.id]], ['later', [later.id]], ['undated', ['x']]]);
  // Nothing due this week leaves the list empty rather than borrowing later work.
  assert.deepEqual(upNext([later, undated, at(10, 20)], now).near, []);
  assert.deepEqual(upNext([], now), { past: [], near: [], folds: [] });
});

test('completed work from before today is past, never overdue', () => {
  const now = new Date(2026, 9, 2, 14, 15); // a Friday
  const at = (m, d, done = false) => ({ id: `${m}-${d}${done ? '-done' : ''}`, due: new Date(2026, m - 1, d, 23, 59).toISOString(), done });
  const tasks = [at(9, 10, true), at(9, 23, true), at(9, 30), at(9, 30, true), at(10, 1, true), at(10, 2, true), at(10, 2)];
  const { past, near } = upNext(tasks, now);
  assert.deepEqual(past.map((t) => t.id), ['9-10-done', '9-23-done', '9-30-done', '10-1-done']);
  assert.deepEqual(near.map((t) => t.id), ['9-30', '10-2-done', '10-2']);
  // Past days head a day at a time back to last week's Monday, then a week at a time; open work stays under Overdue after them.
  const heads = taskHeadings(tasks, now);
  assert.deepEqual(heads.map((h) => [h.kind, h.items.map((t) => t.id).join()]), [
    ['week', '9-10-done'], ['day', '9-23-done'], ['day', '9-30-done'], ['day', '10-1-done'], ['overdue', '9-30'], ['day', '10-2-done,10-2'],
  ]);
  assert.deepEqual(heads.slice(0, 4).map((h) => headingLabel(h, now)), [['Week of Sep 7', ''], ['Wednesday', 'Sep 23'], ['Wednesday', 'Sep 30'], ['Yesterday', 'Oct 1']]);
  assert.equal(taskWhen(at(9, 10).due, 'week'), 'Thu');
});

test('Up Next lists the whole campus week and folds next week at the configured boundary', () => {
  // Monday night in Chicago is already Tuesday on a UTC device.
  const now = new Date('2026-10-06T04:30:00Z'), zone = 'America/Chicago';
  const tasks = Array.from({ length: 10 }, (_, i) => ({
    id: String(i), due: new Date(wallInstant(`2026-10-${String(5 + i).padStart(2, '0')}`, 23 * 60 + 59, zone)).toISOString(),
  }));
  for (const [weekStart, count] of [['mon', 7], ['sun', 6]]) {
    const { near, folds } = upNext([...tasks].reverse(), now, weekStart, zone);
    assert.deepEqual(near.map((t) => t.id), tasks.slice(0, count).map((t) => t.id));
    assert.deepEqual(folds.map((f) => f.key), ['next']);
    assert.deepEqual(folds[0].items.map((t) => t.id), tasks.slice(count).map((t) => t.id));
  }
});

test('task headings say the day once; rows add only what differs', () => {
  const now = new Date(2026, 9, 2, 14, 15);
  const at = (m, d, h = 23, min = 59) => new Date(2026, m - 1, d, h, min).toISOString();
  const tasks = [at(10, 2, 9), at(10, 4), at(10, 5, 15), at(10, 5), at(10, 11), at(10, 12), at(10, 16, 10), at(9, 30), ''].map((due, i) => ({ id: String(i), due }));
  const heads = taskHeadings(tasks, now);
  assert.deepEqual(heads.map((h) => [h.kind, h.items.map((t) => t.id).join()]), [
    ['overdue', '7'], ['day', '0'], ['day', '1'], ['day', '2,3'], ['day', '4'], ['week', '5,6'], ['undated', '8'],
  ]);
  const labels = heads.map((h) => headingLabel(h, now));
  assert.deepEqual(labels.slice(0, 3), [['Overdue', ''], ['Today', 'Oct 2'], ['Sunday', 'Oct 4']]);
  assert.deepEqual(labels.slice(5), [['Week of Oct 12', ''], ['No Due Date', '']]);
  // Earlier today stays under Today; the week after next starts on Sunday for Sunday weeks.
  assert.deepEqual(taskHeadings([{ due: at(10, 11) }], now, 'sun').map((h) => h.kind), ['week']);
  assert.equal(taskWhen(at(10, 4), 'day'), '');
  assert.match(taskWhen(at(10, 5, 15, 0), 'day'), /^3:00\s?pm$/i);
  assert.equal(taskWhen(at(9, 30), 'overdue'), 'Sep 30');
  assert.equal(taskWhen(at(10, 12), 'week'), 'Mon');
  assert.match(taskWhen(at(10, 16, 10, 0), 'week'), /^Fri 10:00\s?am$/i);
  assert.equal(taskWhen('', 'undated'), '');
});

test('due times are read on the campus clock, whatever the device zone', () => {
  // 23:59 in Chicago on September 3 is 04:59 UTC on the 4th.
  const due = '2026-09-04T04:59:00.000Z', now = new Date('2026-09-03T15:00:00Z');
  assert.equal(groupTasks([{ due }], now, 'mon', 'America/Chicago').today.length, 1);
  assert.equal(taskWhen(due, 'day', 'America/Chicago'), '');
  assert.equal(wallInstant('2026-09-03', 23 * 60 + 59, 'America/Chicago'), Date.parse(due));
  // A time the spring change skips moves an hour later; an unknown zone falls back.
  assert.equal(wallInstant('2026-03-08', 150, 'America/Chicago'), Date.parse('2026-03-08T08:30:00Z'));
  assert.equal(wallInstant('2026-09-03', 0, 'Central Standard Time', 'UTC'), Date.parse('2026-09-03T00:00:00Z'));
});

test('matchCourse maps LMS names to schedule codes', () => {
  assert.equal(matchCourse('CS 1213-010 Fall 2026', ['MATH 1914', 'CS 1213']), 'CS 1213');
  assert.equal(matchCourse('CS1213 (Sec 010)', ['CS 1213']), 'CS 1213');
  assert.equal(matchCourse('Intro Chem', ['CS 1213']), '');
  assert.equal(matchCourse('2026FA-CS-1213-010-12345', ['CS 1213']), 'CS 1213'); // Canvas course_code style
  assert.equal(matchCourse('B-AD-1001-006', ['B AD 1001']), 'B AD 1001'); // subject with a space
  assert.equal(matchCourse('PHYSICS 1010', ['CS 101']), ''); // "CS 101" hides inside PHYSICS 1010
  assert.equal(matchCourse('CS 1213 Honors', ['CS 121']), ''); // no prefix matches on the number
  assert.equal(matchCourse('Introduction to Programming', ['CS 1213', 'MATH 1914'], { 'CS 1213': 'Introduction to Programming' }), 'CS 1213');
  // Titles: abbreviations, honors suffixes and several titles per code (lecture vs discussion labels).
  const titles = { 'PHIL 1273': ['Disc-PHIL 1273-030', 'Intro to Business Ethics'], 'CS 1213': ['Programming with Python'] };
  assert.equal(matchCourse('Intro to Business Ethics', ['CS 1213', 'PHIL 1273'], titles), 'PHIL 1273');
  assert.equal(matchCourse('Introduction to Business Ethics (Honors) - Fall 2026', ['CS 1213', 'PHIL 1273'], titles), 'PHIL 1273');
  assert.equal(matchCourse('Business Ethics', ['CS 1213', 'PHIL 1273'], titles), 'PHIL 1273');
  assert.equal(matchCourse('Ethics', ['CS 1213', 'PHIL 1273'], titles), ''); // one short word is not enough
  assert.equal(matchCourse('Python Programming', ['CS 1213', 'PHIL 1273'], titles), 'CS 1213');
});

test('a reusable course matcher preserves precedence and independent schedule indexes', () => {
  const codes = ['CS 121', 'CS 1213', 'PHIL 1273'];
  const match = createCourseMatcher(codes, { 'PHIL 1273': ['Introduction to Business Ethics'] });
  for (let i = 0; i < 3; i++) {
    assert.equal(match('CS 121 and CS 1213'), 'CS 1213');
    assert.equal(match('Intro to Business Ethics (Honors)'), 'PHIL 1273');
    assert.equal(match('PHYSICS 12130'), '');
    assert.equal(match(''), '');
  }
  const other = createCourseMatcher(['PHIL 101'], { 'PHIL 101': 'Business Ethics' });
  assert.equal(other('Business Ethics'), 'PHIL 101');
  assert.equal(match('Business Ethics'), 'PHIL 1273');
  assert.deepEqual(codes, ['CS 121', 'CS 1213', 'PHIL 1273']);
});
