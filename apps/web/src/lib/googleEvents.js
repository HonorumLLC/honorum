import { campusNow } from './occurrences.js';
import { addDays, ymd } from './dates.js';
import { DAYS } from './schedule.js';

// Google Calendar events on the week. Each timed event becomes one calendar
// item per campus day it touches, so an evening that runs past midnight
// draws on both days; all-day events stay in Google Calendar. Items carry
// the event, which the details pane shows read-only.
export const GOOGLE_PREFIX = 'g:';
export const googleId = (event) => `${GOOGLE_PREFIX}${event.calendar}/${event.id}`;

const dayLetter = (date) => DAYS[(new Date(date + 'T00:00:00Z').getUTCDay() + 6) % 7];

// A campus-local wall clock for an instant: the date and minutes since midnight.
function wall(instant, timezone) {
  const d = campusNow(timezone, new Date(instant));
  return { date: ymd(d), minute: d.getHours() * 60 + d.getMinutes(), day: new Date(d.getFullYear(), d.getMonth(), d.getDate()) };
}

export function googleItems(events = [], calendars = [], first, timezone = '') {
  if (!first) return [];
  const from = ymd(addDays(first, -1)), to = ymd(addDays(first, 7));
  const names = Object.fromEntries(calendars.map((c) => [c.id, c]));
  const out = [];
  for (const event of events) {
    if (event.allDay || !event.start || !event.end) continue;
    const start = wall(event.start, timezone), end = wall(event.end, timezone);
    if (end.date < from || start.date > to || !(Date.parse(event.end) > Date.parse(event.start))) continue;
    const calendar = names[event.calendar] || {};
    // Split at campus midnights; seven days is as much as one week can show.
    for (let day = start.day, n = 0; ymd(day) <= end.date && n < 8; day = addDays(day, 1), n++) {
      const date = ymd(day);
      const s = date === start.date ? start.minute : 0;
      const e = date === end.date ? end.minute : 24 * 60;
      if (e <= s || date < from || date > to) continue;
      out.push({
        id: googleId(event), label: event.title || 'Busy', sub: calendar.name || '', date,
        meetings: [{ days: dayLetter(date), start: s, end: e, location: event.location || '' }],
        google: { event, calendar: { id: event.calendar, name: calendar.name || '', color: calendar.color || '' } },
      });
    }
  }
  return out;
}
