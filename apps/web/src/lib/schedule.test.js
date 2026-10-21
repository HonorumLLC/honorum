import { test } from 'node:test';
import assert from 'node:assert/strict';
import { conflictIds, courseOptions, generate, fmtTime, compact } from './schedule.js';

const sec = (id, days, start, end, extra = {}) => ({ id, meetings: [{ days, start, end }], available: 5, ...extra });

test('conflictIds flags every overlapping item', () => {
  const bad = conflictIds([sec('a', 'M', 540, 600), sec('b', 'M', 550, 560), sec('c', 'T', 540, 600)]);
  assert.deepEqual([...bad].sort(), ['a', 'b']);
});

test('conflictIds leaves personal events and exam sittings out', () => {
  const exam = { id: 'a:exam:0', meetings: [{ days: 'M', start: 540, end: 600 }], date: '2026-09-14', exam: {} };
  const event = { id: 'ev', meetings: [{ days: 'M', start: 540, end: 600 }], block: {} };
  assert.deepEqual([...conflictIds([sec('a', 'M', 540, 600), exam, event])], []);
});

test('compact keeps a section’s exam sittings beside its meetings', () => {
  const c = compact({ id: 'x', course: 'CS 101', num: '001', meetings: [], exams: [{ date: '2026-10-08', start: 1140, end: 1260 }] });
  assert.deepEqual(c.exams, [{ date: '2026-10-08', start: 1140, end: 1260, location: '' }]);
  assert.deepEqual(compact({ id: 'y', course: 'CS 101', num: '002' }).exams, []);
});

test('options pairs linked lecture and lab sections', () => {
  const course = { sections: [
    sec('lec1', 'MW', 540, 590, { type: 'LEC', link: 'A' }),
    sec('lab1', 'T', 600, 700, { type: 'LAB', link: 'A' }),
    sec('lab2', 'R', 600, 700, { type: 'LAB', link: 'A', available: 0 }),
    sec('solo', 'F', 540, 590, { type: 'LEC' }),
  ] };
  const ids = (o) => o.map((s) => s.id).join('+');
  assert.deepEqual([...courseOptions(course)].map(ids).sort(), ['lec1+lab1', 'lec1+lab2', 'solo']);
  assert.deepEqual([...courseOptions(course, true)].map(ids).sort(), ['lec1+lab1', 'solo']);
});

test('generate finds only conflict-free combinations and respects blocks', () => {
  const cs = { sections: [sec('cs1', 'MWF', 540, 590), sec('cs2', 'TR', 600, 675)] };
  const math = { sections: [sec('m1', 'MWF', 540, 590), sec('m2', 'MWF', 720, 770)] };
  let r = generate([cs, math]);
  assert.deepEqual(r.map((c) => c.map((s) => s.id).join('+')).sort(), ['cs1+m2', 'cs2+m1', 'cs2+m2']);
  r = generate([cs, math], [{ id: 'work', meetings: [{ days: 'TR', start: 600, end: 800 }] }]);
  assert.deepEqual(r.map((c) => c.map((s) => s.id).join('+')), ['cs1+m2']);
  assert.equal(generate([cs, { sections: [sec('x', 'MWF', 540, 590), sec('y', 'TR', 600, 675)] }], [], { limit: 1 }).length, 1);
});

test('fmtTime', () => {
  assert.equal(fmtTime(540), '9:00am');
  assert.equal(fmtTime(780), '1:00pm');
  assert.equal(fmtTime(0), '12:00am');
});

test('compact keeps what the planner renders and nothing else', () => {
  const c = compact({ id: '1', course: 'CS 101', num: '001', meetings: [{ days: 'MW', start: 540, end: 590, location: 'Hall 1', extra: 'x' }], credits: 3, available: 0, fetched_at: 5, description: 'long' });
  assert.deepEqual(c, { id: '1', course: 'CS 101', subject: '', title: '', num: '001', type: '', link: '', meetings: [{ days: 'MW', start: 540, end: 590, location: 'Hall 1' }], exams: [], credits: 3, start: '', end: '', instructors: [], mode: '', capacity: 0, available: 0, waitlist: 0, url: '' });
  assert.equal(compact({ id: '2', course: 'X', num: '1' }).available, -1);
});

test('open-only cannot omit a required closed or unknown lab', () => {
  for (const available of [0, -1]) {
    const course = { sections: [sec('lecture', 'M', 540, 600, { type: 'LEC', link: 'A' }), sec('lab', 'T', 540, 600, { type: 'LAB', link: 'A', available })] };
    assert.deepEqual(generate([course], [], { openOnly: true }), []);
  }
});
test('disjoint dates, one-time dates, and exclusions affect conflicts', () => {
  const a = sec('a', 'M', 540, 600, { start: '2026-08-01', end: '2026-09-01' });
  const b = sec('b', 'M', 540, 600, { start: '2026-10-01', end: '2026-11-01' });
  assert.equal(conflictIds([a,b]).size, 0);
  const once = sec('once', 'M', 540, 600, { start: '2026-08-03', end: '2026-08-03' });
  assert.equal(conflictIds([a,once]).size, 2);
  assert.equal(conflictIds([{ ...a, removed: ['2026-08-03'] }, once]).size, 0);
});
test('events never count as overlapping classes or each other', () => {
  const a = sec('a', 'M', 540, 600);
  const work = { id: 'work', block: { date: '' }, meetings: [{ days: 'M', start: 540, end: 600 }] };
  const lunch = { id: 'lunch', block: { date: '2026-08-03' }, meetings: [{ days: 'M', start: 550, end: 610 }] };
  assert.equal(conflictIds([a, work, lunch]).size, 0);
  assert.deepEqual([...conflictIds([a, sec('b', 'M', 550, 560), work])].sort(), ['a', 'b']);
});
test('unsatisfiable searches stop at their work budget', () => {
  const course = { sections: Array.from({length:10},(_,i)=>sec(String(i),'M',540,600)) };
  assert.throws(() => generate([course, course, course], [], { maxSteps: 5 }), /too large/);
});
