import { test } from 'node:test';
import assert from 'node:assert/strict';
import { emptyFilters, applyFilters, attributeList } from './filters.js';

const course = { code: 'CS 1213', sections: [
  { id: 'a', available: 0, mode: 'In person', attributes: ['Gen Ed: Natural Science'], meetings: [{ days: 'MWF', start: 540, end: 590 }] },
  { id: 'b', available: 5, mode: 'Online', attributes: [], meetings: [] },
  { id: 'c', available: 2, mode: 'In person', attributes: [], meetings: [{ days: 'TR', start: 1080, end: 1155 }] },
] };
const grad = { code: 'CS 5013', sections: [{ id: 'g', available: 1, mode: 'In person', meetings: [{ days: 'M', start: 600, end: 700 }] }] };
const ids = (f) => applyFilters([course, grad], f).flatMap((c) => c.sections.map((s) => s.id));

test('filters narrow sections and drop empty courses', () => {
  assert.deepEqual(ids({ ...emptyFilters(), open: true }), ['b', 'c', 'g']);
  assert.deepEqual(ids({ ...emptyFilters(), days: 'MWF' }), ['a', 'b', 'g']);
  assert.deepEqual(ids({ ...emptyFilters(), time: 'evening' }), ['c']);
  assert.deepEqual(ids({ ...emptyFilters(), mode: 'Online' }), ['b']);
  assert.deepEqual(ids({ ...emptyFilters(), level: '1' }), ['a', 'b', 'c']);
  assert.deepEqual(ids({ ...emptyFilters(), level: 'grad' }), ['g']);
  assert.deepEqual(ids({ ...emptyFilters(), attr: 'Gen Ed: Natural Science' }), ['a']);
  assert.equal(applyFilters([course], emptyFilters()).length, 1);
});

test('attributeList ranks tags by frequency', () => {
  assert.deepEqual(attributeList({ courses: [course, grad] }), ['Gen Ed: Natural Science']);
});
