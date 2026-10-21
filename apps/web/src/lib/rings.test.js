import test from 'node:test';
import assert from 'node:assert/strict';
import { ringCounts, recordTerm, percent } from './rings.js';

// Tuesday, September 22, 2026, 11:20 local time.
const now = new Date(2026, 8, 22, 11, 20);
const at = (day, h = 23, m = 59) => new Date(2026, 8, day, h, m).toISOString();

test('today and this week keep overdue work until it is done', () => {
  const tasks = [
    { id: 'late', due: at(18), done: false, course: 'HIST' },          // overdue from last week
    { id: 'lateDone', due: at(17), done: true, completedAt: at(22, 9), course: 'HIST' }, // finished today
    { id: 'oldDone', due: at(16), done: true, completedAt: at(19, 9) }, // finished last week
    { id: 'undated', done: false },
    { id: 'mon', due: at(21), done: true, course: 'CS' },
    { id: 'tue', due: at(22, 14), done: true, course: 'PHYS' },
    { id: 'tue2', due: at(22, 17), done: false },
    { id: 'fri', due: at(25), done: false, course: 'CS' },
    { id: 'sun', due: at(27), done: false },
    { id: 'next', due: at(28, 9), done: false },
  ];
  const c = ringCounts(tasks, { now });
  assert.deepEqual(c.today, { done: 2, total: 4 });  // late, lateDone, tue, tue2
  assert.deepEqual(c.week, { done: 3, total: 7 });   // + mon, fri, sun
  // A week starting on Sunday ends on Saturday the 26th.
  assert.deepEqual(ringCounts(tasks, { now, weekStart: 'sun' }).week, { done: 3, total: 6 });
  // Picking a class narrows every ring.
  assert.deepEqual(ringCounts(tasks, { now, only: 'CS' }).week, { done: 1, total: 2 });
  assert.deepEqual(ringCounts(tasks, { now, only: 'CS' }).today, { done: 0, total: 0 });
});

test('a completion without a time counts only while its task is due in the period', () => {
  const tasks = [{ id: 'a', due: at(18), done: true }];
  assert.deepEqual(ringCounts(tasks, { now }).today, { done: 0, total: 0 });
  assert.equal(percent({ done: 0, total: 0 }), 100);
  assert.equal(percent({ done: 1, total: 3 }), 33);
});

test('the term record keeps work that imports have dropped', () => {
  const term = { start: '2026-08-24', end: '2026-12-11' };
  const early = { uid: 'canvas:a', id: 'x', due: at(1), done: true, course: 'CS' };
  const recent = { uid: 'canvas:b', id: 'y', due: at(20), done: false, course: 'MATH' };
  const manual = { id: 'z', due: at(23), done: false };
  const before = { id: 'summer', due: new Date(2026, 7, 10).toISOString(), done: true };
  let record = recordTerm({}, [early, recent, manual, before], { ...term, now });
  assert.deepEqual(Object.keys(record).sort(), ['canvas:a', 'canvas:b', 'z']);
  assert.equal(recordTerm(record, [early, recent, manual], { ...term, now }), record); // unchanged
  // Canvas stops carrying the September 1 assignment; it stays in the record.
  // One due two days ago that disappears was removed, so it goes.
  record = recordTerm(record, [manual], { ...term, now });
  assert.deepEqual(Object.keys(record).sort(), ['canvas:a', 'z']);
  assert.deepEqual(ringCounts([manual], { now, record }).term, { done: 1, total: 2 });
  assert.deepEqual(ringCounts([manual], { now, record, only: 'CS' }).term, { done: 1, total: 1 });
  // A task moved out of the term leaves it.
  record = recordTerm(record, [{ ...manual, due: new Date(2027, 0, 5).toISOString() }], { ...term, now });
  assert.deepEqual(Object.keys(record), ['canvas:a']);
});
