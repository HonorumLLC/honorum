import test from 'node:test';
import assert from 'node:assert/strict';
import { googleItems, googleId } from './googleEvents.js';

const calendars = [{ id: 'me', name: 'Me', color: '#9fe1e7' }];
const week = new Date(2026, 8, 7); // Monday, September 7, 2026

test('a timed Google event is drawn on its campus day', () => {
  const [item, ...rest] = googleItems([{ id: 'a', calendar: 'me', title: 'Dentist', start: '2026-09-09T15:00:00-05:00', end: '2026-09-09T16:30:00-05:00', location: 'Main St' }], calendars, week, 'America/Chicago');
  assert.equal(rest.length, 0);
  assert.equal(item.id, 'g:me/a');
  assert.equal(item.date, '2026-09-09');
  assert.deepEqual(item.meetings, [{ days: 'W', start: 900, end: 990, location: 'Main St' }]);
  assert.equal(item.sub, 'Me');
  assert.equal(item.google.calendar.color, '#9fe1e7');
});

test('times follow the campus time zone, not the event’s', () => {
  // 20:00 UTC is 3pm in Chicago and 4pm in New York on this date.
  const event = { id: 'b', calendar: 'me', title: 'Call', start: '2026-09-08T20:00:00Z', end: '2026-09-08T21:00:00Z' };
  assert.equal(googleItems([event], calendars, week, 'America/Chicago')[0].meetings[0].start, 15 * 60);
  assert.equal(googleItems([event], calendars, week, 'America/New_York')[0].meetings[0].start, 16 * 60);
});

test('an event past midnight draws on both days', () => {
  const items = googleItems([{ id: 'c', calendar: 'me', title: 'Shift', start: '2026-09-11T22:00:00-05:00', end: '2026-09-12T02:00:00-05:00' }], calendars, week, 'America/Chicago');
  assert.deepEqual(items.map((it) => [it.date, it.meetings[0].days, it.meetings[0].start, it.meetings[0].end]), [['2026-09-11', 'F', 1320, 1440], ['2026-09-12', 'S', 0, 120]]);
  assert.ok(items.every((it) => it.id === googleId({ calendar: 'me', id: 'c' })));
});

test('all-day, empty, and other weeks’ events are left out', () => {
  const events = [
    { id: 'd', calendar: 'me', title: 'Trip', start: '2026-09-08', end: '2026-09-10', allDay: true },
    { id: 'e', calendar: 'me', title: 'Zero', start: '2026-09-08T10:00:00Z', end: '2026-09-08T10:00:00Z' },
    { id: 'f', calendar: 'me', title: 'Later', start: '2026-10-08T10:00:00Z', end: '2026-10-08T11:00:00Z' },
    { id: 'g', calendar: 'gone', title: '', start: '2026-09-10T10:00:00Z', end: '2026-09-10T11:00:00Z' },
  ];
  const items = googleItems(events, calendars, week, 'UTC');
  assert.deepEqual(items.map((it) => [it.id, it.label, it.sub]), [['g:gone/g', 'Busy', '']]);
  assert.deepEqual(googleItems(events, calendars, undefined, 'UTC'), []);
});
