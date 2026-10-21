import { test } from 'node:test';
import assert from 'node:assert/strict';
import { indexCatalog, search } from './catalog.js';

let nextId = 10000;
const course = (code, title, instructors = [], nums = ['001']) => ({
  code, subject: code.split(' ').slice(0, -1).join(' '), title,
  sections: nums.map((num) => ({ id: String(nextId++), num, instructors, meetings: [] })),
});
const cat = indexCatalog({ courses: [
  course('CHEM 1315', 'General Chemistry'),
  course('MIS 3013', 'Management Information Systems'),
  course('MIS 4223', 'Business Intelligence', ['Amy Misner']),
  course('C S 1213', 'Programming with Python', ['Sanjana Mudduluru']),
  course('HIST 1483', 'US History to 1865', ['Chris Mission']),
  course('ENGL 1113', 'Principles of English Composition'),
  course('PSY 1113', 'Elements of Psychology', ['Pat Chemist']),
  course('B AD 1001', 'Business Foundations', [], ['001', '006', '010', '01B']),
  course('BIO 100', 'Biology', [], ['1', '10', '10A', '501']),
] });
const codes = (q) => search(cat, q).map((c) => c.code);

test('subject code outranks substring matches', () => {
  assert.deepEqual(codes('mis'), ['MIS 3013', 'MIS 4223', 'HIST 1483', 'CHEM 1315', 'PSY 1113']);
});

test('course number narrows within a subject, with or without a space', () => {
  assert.deepEqual(codes('mis 30'), ['MIS 3013']);
  assert.deepEqual(codes('mis3013'), ['MIS 3013']);
  assert.deepEqual(codes('cs 1213'), ['C S 1213']);
  assert.deepEqual(codes('c s 1213'), ['C S 1213']);
});

test('title words beat instructor names, every token must match', () => {
  assert.deepEqual(codes('chem')[0], 'CHEM 1315');
  assert.deepEqual(codes('chemist'), ['CHEM 1315', 'PSY 1113']);
  assert.deepEqual(codes('python mudduluru'), ['C S 1213']);
  assert.deepEqual(codes('python nobody'), []);
});

test('empty query lists the catalog in order; results are capped', () => {
  assert.equal(search(cat, '   ').length, 9);
  assert.equal(search(cat, 'e', 2).length, 2);
});

test('a term the size of OU searches in under a millisecond per query', () => {
  const big = { courses: [] };
  for (let i = 0; i < 3200; i++) big.courses.push(course(`SUBJ${i % 190} ${1000 + i}`, `Course title number ${i} about topic ${i % 37}`, [`Person ${i % 500} Surname${i % 91}`]));
  const indexed = indexCatalog(big);
  const t0 = performance.now();
  for (let i = 0; i < 200; i++) search(indexed, 'subj12 10');
  const per = (performance.now() - t0) / 200;
  assert.ok(per < 1, `took ${per.toFixed(2)} ms per query`);
});

test('a trailing section number narrows to that section, across number styles', () => {
  const only = (q) => search(cat, q).map((c) => [c.code, c.sections.map((s) => s.num)]);
  assert.deepEqual(only('B AD 1001-006'), [['B AD 1001', ['006']]]);
  assert.deepEqual(only('b ad 1001 006'), [['B AD 1001', ['006']]]);
  assert.deepEqual(only('bad1001-6'), [['B AD 1001', ['006']]]);
  assert.deepEqual(only('B AD 1001 01'), [['B AD 1001', ['010', '01B']]]);
  assert.deepEqual(only('bio 100-10'), [['BIO 100', ['10', '10A']]]);
  assert.deepEqual(only('bio 100 501'), [['BIO 100', ['501']]]);
  assert.deepEqual(only('bio 100 999'), []); // no such section: nothing, rather than every BIO course
});

test('a bare section id finds its section', () => {
  const sec = cat.courses[0].sections[0];
  const hit = search(cat, sec.id);
  assert.deepEqual(hit.map((c) => [c.code, c.sections.map((s) => s.id)]), [['CHEM 1315', [sec.id]]]);
});
