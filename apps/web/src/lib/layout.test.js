import { test } from 'node:test';
import assert from 'node:assert/strict';
import { layout, columns } from './layout.js';

test('layout widens for weekend and late meetings', () => {
  assert.deepEqual(layout([{ meetings: [{ days: 'MW', start: 540, end: 600 }] }]), { days: 7, startH: 8, endH: 17, hours: 9 });
  assert.deepEqual(layout([{ meetings: [{ days: 'S', start: 420, end: 1230 }] }]), { days: 7, startH: 7, endH: 21, hours: 14 });
});

test('columns places overlapping entries side by side', () => {
  const e = (id, start, end) => ({ it: { id }, m: { start, end } });
  const out = columns([e('a', 540, 600), e('b', 560, 620), e('c', 700, 760)]);
  assert.deepEqual(out.map((x) => [x.it.id, x.col, x.n]), [['a', 0, 2], ['b', 1, 2], ['c', 0, 1]]);
  assert.deepEqual(columns([e('x', 540, 600)]).map((x) => [x.col, x.n]), [[0, 1]]);
});

