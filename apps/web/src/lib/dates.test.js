import { test } from 'node:test';
import assert from 'node:assert/strict';
import { startOfWeek, defaultWeek, activeInWeek, ymd, orderTerms, seasonDates, defaultTerm } from './dates.js';

test('startOfWeek and defaultWeek', () => {
  assert.equal(ymd(startOfWeek(new Date(2026, 8, 3))), '2026-08-31'); // Thursday -> Monday
  assert.equal(ymd(defaultWeek({ start: '2026-08-24', end: '2026-12-11' }, new Date(2026, 8, 3))), '2026-08-31');
  assert.equal(ymd(defaultWeek({ start: '2027-01-19', end: '2027-05-07' }, new Date(2026, 8, 3))), '2027-01-18');
});

test('activeInWeek respects section and term dates', () => {
  const term = { start: '2026-08-24', end: '2026-12-11' };
  const week = startOfWeek(new Date(2026, 8, 7));
  assert.equal(activeInWeek({ section: { start: '2026-10-19', end: '2026-12-11' } }, week, term), false);
  assert.equal(activeInWeek({ section: {} }, week, term), true);
  assert.equal(activeInWeek({ block: {} }, startOfWeek(new Date(2026, 11, 28)), term), false);
  // one-time events show only in their own week
  assert.equal(activeInWeek({ block: { date: '2026-09-10' } }, week, term), true);
  assert.equal(activeInWeek({ block: { date: '2026-09-17' } }, week, term), false);
  // an exam sitting is dated like a one-time event
  assert.equal(activeInWeek({ date: '2026-09-10', exam: {} }, week, term), true);
  assert.equal(activeInWeek({ date: '2026-12-09', exam: {} }, week, term), false);
});

test('weeks can start on Sunday', () => {
  assert.equal(ymd(startOfWeek(new Date(2026, 8, 3), 'sun')), '2026-08-30');
  assert.equal(ymd(startOfWeek(new Date(2026, 8, 6), 'sun')), '2026-09-06'); // a Sunday starts its own week
  assert.equal(ymd(startOfWeek(new Date(2026, 8, 6), 'mon')), '2026-08-31');
  assert.equal(ymd(defaultWeek({ start: '2027-01-19', end: '2027-05-07' }, new Date(2026, 8, 3), 'sun')), '2027-01-17');
});

test('orderTerms puts the current term first', () => {
  const terms = [{ id: 'su27', start: '2027-05-24', end: '2027-08-06' }, { id: 'sp27', start: '2027-01-19', end: '2027-05-07' }, { id: 'fa26', start: '2026-08-24', end: '2026-12-11' }];
  assert.deepEqual(orderTerms(terms, new Date(2026, 8, 3)).map((x) => x.id), ['fa26', 'sp27', 'su27']);
  assert.deepEqual(orderTerms([{ id: 'a' }, { id: 'b' }], new Date()).map((x) => x.id), ['a', 'b']);
});

test('undated terms are ordered by dates guessed from their names', () => {
  // Rutgers: newest id first would open a 120-section Winter intersession.
  const rutgers = [{ id: '2027-0', name: 'Winter 2027' }, { id: '2026-9', name: 'Fall 2026' }, { id: '2026-7', name: 'Summer 2026' }];
  assert.deepEqual(orderTerms(rutgers, new Date(2026, 8, 3)).map((x) => x.id), ['2026-9', '2027-0', '2026-7']);
  const uci = [{ id: '2026-92', name: '2026 Fall Quarter' }, { id: '2026-14', name: '2026 Spring Quarter' }, { id: '2026-03', name: '2026 Winter Quarter' }];
  assert.deepEqual(orderTerms(uci, new Date(2026, 8, 3)).map((x) => x.id), ['2026-92', '2026-14', '2026-03']);
  assert.deepEqual(seasonDates('Fall Semester 2026'), { start: '2026-08-15', end: '2026-12-20' });
  assert.deepEqual(seasonDates('Winter 2026-2027'), { start: '2026-12-15', end: '2027-01-20' });
  assert.deepEqual(seasonDates('Winter 2027'), { start: '2026-12-15', end: '2027-01-20' });
  assert.equal(seasonDates('202640'), null);
});

test('defaultTerm skips terms that are unfetched or empty', () => {
  const terms = [{ id: 'su27', updated: 1, sections: 0 }, { id: 'sp27', updated: 0, sections: 0 }, { id: 'fa26', updated: 1, sections: 12000 }];
  assert.equal(defaultTerm(terms).id, 'fa26');
  assert.equal(defaultTerm([{ id: 'x', updated: 0, sections: 0 }]).id, 'x');
  assert.equal(defaultTerm([]), null);
});
