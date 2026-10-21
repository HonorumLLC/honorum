// Progress rings: how much of today, this week and this term is done.
// Today and This Week follow Coursicle's rule: an overdue task stays in them
// until it is done, and finishing one counts in the period it was finished.
// This Term counts from a record kept on the device, because imports drop
// tasks a week or two after they are due.
import { startOfWeek, addDays, parse } from './dates.js';
import { campusNow } from './occurrences.js';

// Outer to inner: key, label, color, and what closing it says.
export const RINGS = [['today', 'Today', 'day', 'Day complete'], ['week', 'This Week', 'week', 'Week complete'], ['term', 'This Term', 'term', 'All done']];

// Periods are campus days; zone shifts instants onto the campus clock.
const due = (t, zone) => (t.due ? campusNow(zone, new Date(t.due)) : null);
const finished = (t, zone) => (t.done && t.completedAt ? campusNow(zone, new Date(t.completedAt)) : null);
const count = (list) => ({ done: list.filter((t) => t.done).length, total: list.length });

// A task is in a period when it is due within it, or was due earlier and is
// still open or was finished during the period.
function inPeriod(t, from, to, zone) {
  const d = due(t, zone);
  if (!d || d >= to) return false;
  if (d >= from) return true;
  if (!t.done) return true;
  const f = finished(t, zone);
  return !!f && f >= from;
}

// The record keeps one entry per task due in the term: [done, class, due ms].
const keyOf = (t) => t.uid || t.id;
const WEEK = 7 * 86400e3;

// The record brought up to date with the current tasks. Tasks in the list are
// authoritative; a task that left the list stays only when it was due more
// than a week ago, since that is when imports stop carrying it. Returns the
// same object when nothing changed.
export function recordTerm(record = {}, tasks, { start, end, now = new Date(), codeOf = (t) => t.course || '' }) {
  const from = parse(start), to = end ? addDays(parse(end), 1) : null;
  const next = {}, seen = new Set();
  for (const t of tasks) {
    const key = keyOf(t);
    seen.add(key);
    const d = due(t);
    if (!key || !d || (from && d < from) || (to && d >= to)) continue;
    next[key] = [t.done ? 1 : 0, codeOf(t), d.getTime()];
  }
  for (const [key, entry] of Object.entries(record)) {
    if (!seen.has(key) && entry[2] < now.getTime() - WEEK) next[key] = entry;
  }
  const same = Object.keys(next).length === Object.keys(record).length
    && Object.entries(next).every(([k, v]) => record[k] && record[k][0] === v[0] && record[k][1] === v[1] && record[k][2] === v[2]);
  return same ? record : next;
}

// Counts for the three rings. `only` limits them to one class, as the task
// filter does; `codeOf` names a task's class the same way the record does.
export function ringCounts(tasks, { now: instant = new Date(), weekStart = 'mon', record = {}, only = '', codeOf = (t) => t.course || '', zone = '' } = {}) {
  const now = campusNow(zone, instant);
  const mine = only ? tasks.filter((t) => codeOf(t) === only) : tasks;
  const today = new Date(now.getFullYear(), now.getMonth(), now.getDate());
  const week = startOfWeek(now, weekStart);
  const term = Object.values(record).filter((e) => !only || e[1] === only);
  return {
    today: count(mine.filter((t) => inPeriod(t, today, addDays(today, 1), zone))),
    week: count(mine.filter((t) => inPeriod(t, week, addDays(week, 7), zone))),
    term: { done: term.filter((e) => e[0]).length, total: term.length },
  };
}

// A period with nothing in it is complete: there is nothing left to do.
export const percent = ({ done, total }) => (total ? Math.round((done / total) * 100) : 100);
