import { test } from 'node:test';
import assert from 'node:assert/strict';
import { toICS } from './export.js';

test('toICS emits every meeting from the first matching day through the end of its dates', () => {
  const campus = { start: '2026-08-26', end: '2026-12-11', timezone: 'America/Chicago' }; // 2026-08-26 is a Wednesday
  const ics = toICS([{ id: 'x', label: 'CS 101', sub: '001', meetings: [{ days: 'MWF', start: 540, end: 590, location: 'Eng 110' }] }], campus);
  assert.match(ics, /UID:x-2026-08-26-540@honorum\r\nDTSTART:20260826T140000Z\r\nDTEND:20260826T145000Z\r\nSUMMARY:CS 101 001\r\nLOCATION:Eng 110/);
  assert.equal((ics.match(/BEGIN:VEVENT/g) || []).length, 47);
  assert.doesNotMatch(ics, /RRULE|DESCRIPTION/);
  const half = toICS([{ id: 'y', label: 'X', meetings: [{ days: 'M', start: 540, end: 600 }], section: { start: '2026-10-19', end: '2026-12-11' } }], { ...campus, start: '2026-08-24' });
  assert.match(half, /DTSTART:20261019T140000Z/);
  assert.doesNotMatch(half, /DTSTART:20261012/);
  // Without a campus time zone, times are the device's own.
  const once = toICS([{ id: 'z', label: 'Career fair', meetings: [{ days: 'W', start: 600, end: 720, location: 'Union' }], block: { date: '2026-09-16' } }], { start: '2026-08-24', end: '2026-12-11' });
  const local = new Date('2026-09-16T10:00:00').toISOString().replace(/[-:]/g, '').replace('.000', '');
  assert.match(once, new RegExp(`DTSTART:${local}`));
  assert.equal((once.match(/BEGIN:VEVENT/g) || []).length, 1);
});

test('toICS carries event notes as the description and exam sittings as single events', () => {
  const term = { start: '2026-08-24', end: '2026-12-11', timezone: 'America/Chicago' };
  const items = [
    { id: 'w', label: 'Work', meetings: [{ days: 'TR', start: 1020, end: 1200 }], block: { days: 'TR', note: 'Bring badge; ask for Sam' } },
    { id: 'e', label: 'CS 101 Exam', meetings: [{ days: 'R', start: 1140, end: 1260, location: 'Eng 110' }], date: '2026-10-08', exam: { date: '2026-10-08' } },
  ];
  const ics = toICS(items, term);
  assert.match(ics, /DESCRIPTION:Bring badge\\; ask for Sam/);
  assert.match(ics, /UID:e-2026-10-08-1140@honorum\r\nDTSTART:20261009T000000Z\r\nDTEND:20261009T020000Z\r\nSUMMARY:CS 101 Exam\r\nLOCATION:Eng 110/);
  assert.equal((ics.match(/CS 101 Exam/g) || []).length, 1);
});

test('campus exports preserve DST and omit closures with makeup occurrences', () => {
  const item = {id:'math',label:'Math, honors',section:{start:'2026-10-26',end:'2026-11-09'},meetings:[{days:'M',start:540,end:600}],closures:{'2026-11-02':{name:'Closed'},'2026-11-03':{name:'Monday classes',follows:'M'}}};
  const text=toICS([item],{timezone:'America/Chicago'});
  assert.ok(text.includes('DTSTART:20261026T140000Z'));
  assert.ok(text.includes('DTSTART:20261103T150000Z'));
  assert.ok(text.includes('DTSTART:20261109T150000Z'));
  assert.ok(!text.includes('DTSTART:20261102'));
  assert.ok(text.includes('SUMMARY:Math\\, honors'));
});
