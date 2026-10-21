// Calendar dates are civil dates in the institution's timezone, never elapsed milliseconds.
const LETTERS = 'MTWRFSU';
export const civilDay = (date) => Math.floor(Date.parse(date + 'T00:00:00Z') / 86400000);
export const civilDate = (day) => new Date(day * 86400000).toISOString().slice(0, 10);
export function rangeOf(item, term = {}) {
  const section = item.section || item;
  const once = item.block?.date || item.date;
  return [once || section.start && typeof section.start === 'string' && section.start || term.start || '', once || section.end && typeof section.end === 'string' && section.end || term.end || ''];
}
export function meetingsOn(item, date, term = {}, closures = item.closures || {}) {
  const [from, to] = rangeOf(item, term);
  if ((from && date < from) || (to && date > to) || item.removed?.includes(date)) return [];
  const closure = (item.section || item.course) && !item.block ? closures[date] : null;
  if (closure && !closure.follows && !closure.after) return [];
  const day = closure?.follows || LETTERS[(new Date(date + 'T00:00:00Z').getUTCDay() + 6) % 7];
  return (item.meetings || []).filter((m) => m.days?.includes(day) && m.end > m.start && !(closure?.after && m.start >= closure.after));
}
// Whether a closure cancels a class meeting starting at minute `start`: all
// of them, unless it begins partway through the day (after, in minutes).
export const closureCancels = (closure, start) => !closure.after || start >= closure.after;
export function occurrencesConflict(a, b) {
  if (!(a.meetings || []).some((x) => (b.meetings || []).some((y) => x.start < y.end && y.start < x.end))) return false;
  const ar = rangeOf(a), br = rangeOf(b);
  const from = [ar[0], br[0]].filter(Boolean).sort().at(-1);
  const to = [ar[1], br[1]].filter(Boolean).sort()[0];
  if (from && to && from > to) return false;
  // Outside exceptional dates both patterns repeat every seven days. Testing
  // one week per exceptional day plus one is sufficient for unbounded events.
  const exceptions = (a.removed?.length || 0) + (b.removed?.length || 0) + Object.keys(a.closures || {}).length + Object.keys(b.closures || {}).length;
  const window = 7 * (exceptions + 1);
  const first = from ? civilDay(from) : to ? civilDay(to) - window : civilDay('2000-01-03');
  const last = to ? Math.min(civilDay(to), first + window) : first + window;
  for (let day = first; day <= last; day++) {
    const date = civilDate(day);
    const am = meetingsOn(a, date), bm = meetingsOn(b, date);
    if (am.some((x) => bm.some((y) => x.start < y.end && y.start < x.end))) return true;
  }
  return false;
}
// Reuse timezone formatters; constructing one consults the locale/timezone
// database and was a substantial part of the calendar's initial render.
const formatters = new Map();
export function campusNow(timezone, now = new Date()) {
  if (!timezone) return now;
  let formatter = formatters.get(timezone);
  if (!formatter) formatters.set(timezone, formatter = new Intl.DateTimeFormat('en-US', { timeZone: timezone, year: 'numeric', month: '2-digit', day: '2-digit', hour: '2-digit', minute: '2-digit', second: '2-digit', hourCycle: 'h23' }));
  const fields = Object.fromEntries(formatter.formatToParts(now).map((x) => [x.type, x.value]));
  return new Date(+fields.year, +fields.month - 1, +fields.day, +fields.hour, +fields.minute, +fields.second);
}

// Convert an institution-local meeting time to an instant using the platform's
// timezone database. Iterate the offset so DST is resolved on the meeting date.
export function meetingInstant(date, minute, timezone) {
  const target = civilDay(date) * 86400000 + minute * 60000;
  if (!timezone) { const [y, m, d] = date.split('-'); return new Date(+y, m - 1, +d, 0, minute).getTime(); }
  let instant = target;
  for (let i = 0; i < 4; i++) {
    const local = campusNow(timezone, new Date(instant));
    const represented = Date.UTC(local.getFullYear(), local.getMonth(), local.getDate(), local.getHours(), local.getMinutes(), local.getSeconds());
    const correction = target - represented;
    if (!correction) return instant;
    instant += correction;
  }
  throw new Error('This meeting time does not exist in the school timezone');
}
