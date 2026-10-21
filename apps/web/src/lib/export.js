import { meetingsOn, civilDay, civilDate, rangeOf, meetingInstant } from './occurrences.js';

const escapeText = (s = '') => String(s).replace(/\\/g, '\\\\').replace(/\r?\n/g, '\\n').replace(/,/g, '\\,').replace(/;/g, '\\;');
// An event's notes travel as the description; classes and exams have none.
const description = (it) => (it.block?.note ? `DESCRIPTION:${escapeText(it.block.note)}` : '');

// Every occurrence is its own event at an explicit UTC instant, which encodes
// school closures, substitute weekdays and daylight-saving changes the same
// way in every calendar client, without a hand-maintained VTIMEZONE. Times
// are campus times, or the device's own when the school has no time zone.
export function toICS(items, term = {}) {
  const lines = ['BEGIN:VCALENDAR', 'VERSION:2.0', 'PRODID:-//Honorum//EN'];
  const instant = (date, minute) => new Date(meetingInstant(date, minute, term.timezone)).toISOString().replace(/[-:]/g, '').replace('.000', '');
  for (const item of items) {
    const [from, to] = rangeOf(item, term);
    const first = civilDay(from || new Date().toISOString().slice(0, 10));
    const last = to ? civilDay(to) : first + 105;
    if (last - first > 730) throw new Error('Calendar exports are limited to two years');
    for (let day = first; day <= last; day++) {
      const date = civilDate(day);
      for (const meeting of meetingsOn(item, date, term)) {
        lines.push('BEGIN:VEVENT', `UID:${item.id}-${date}-${meeting.start}@honorum`,
          `DTSTART:${instant(date, meeting.start)}`, `DTEND:${instant(date, meeting.end)}`,
          `SUMMARY:${escapeText(item.label + (item.sub ? ' ' + item.sub : ''))}`,
          `LOCATION:${escapeText(meeting.location || '')}`, ...[description(item)].filter(Boolean), 'END:VEVENT');
      }
    }
  }
  lines.push('END:VCALENDAR');
  return lines.join('\r\n');
}

export function download(name, href) {
  const a = document.createElement('a');
  a.href = href; a.download = name;
  a.click();
}
