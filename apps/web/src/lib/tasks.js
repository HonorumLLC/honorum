// Tasks: manual to-dos plus assignments imported from an LMS.
import { rid } from './schedule.js';
import { campusNow, meetingInstant } from './occurrences.js';

// Minimal iCalendar reader: VEVENT and VTODO items with a date. Floating and
// whole-day times are read in the item's TZID, else the campus zone.
export function parseICS(text, source = 'feed', label = '', zone = '') {
  const lines = [];
  for (const raw of text.split(/\r?\n/)) {
    if ((raw.startsWith(' ') || raw.startsWith('\t')) && lines.length) lines[lines.length - 1] += raw.slice(1);
    else lines.push(raw);
  }
  const tasks = [];
  let cur = null;
  for (const line of lines) {
    if (line === 'BEGIN:VEVENT' || line === 'BEGIN:VTODO') { cur = {}; continue; }
    if (line === 'END:VEVENT' || line === 'END:VTODO') {
      if (cur && cur.SUMMARY) {
        const key = ['DUE', 'DTSTART', 'DTEND'].find((k) => cur[k]), due = cur[key] || '';
        const { title, course } = splitCourse(unescape(cur.SUMMARY), label);
        tasks.push({ uid: `${source}:${cur.UID || title + due}`, title, course, due: icsDate(due, cur[key + ';TZID'] || zone, zone), done: cur.STATUS === 'COMPLETED', url: cur.URL || '', source });
      }
      cur = null;
      continue;
    }
    if (!cur) continue;
    const i = line.indexOf(':');
    if (i < 0) continue;
    const [key, ...params] = line.slice(0, i).split(';');
    if (key in cur) continue;
    cur[key] = line.slice(i + 1);
    const tzid = params.find((p) => p.startsWith('TZID='));
    if (tzid) cur[key + ';TZID'] = tzid.slice(5).replace(/"/g, '');
  }
  return tasks;
}

const unescape = (s) => s.replace(/\\([\\;,nN])/g, (_, c) => (c === 'n' || c === 'N' ? ' ' : c)).trim();

// Canvas feeds title events "Homework 3 [CS 1213-010]"; strip that into the course.
function splitCourse(summary, label) {
  const m = summary.match(/^(.*?)\s*\[([^\]]+)\]\s*$/);
  return m ? { title: m[1].trim(), course: m[2].trim() } : { title: summary, course: label };
}

// "20260910T235900Z" | "20260910T235900" (floating) | "20260910" -> ISO string.
// Floating times are read in zone; one the platform does not know falls back
// to the campus zone, then the device's.
export function icsDate(v, zone = '', campus = '') {
  const m = v.match(/^(\d{4})(\d{2})(\d{2})(?:T(\d{2})(\d{2})(\d{2})?(Z)?)?$/);
  if (!m) return '';
  const [, y, mo, d, h = '23', mi = '59', s = '00', z] = m;
  const at = z ? Date.UTC(+y, mo - 1, +d, +h, +mi) : wallInstant(`${y}-${mo}-${d}`, +h * 60 + +mi, zone, campus);
  return new Date(at + s * 1000).toISOString();
}

// The instant a campus wall-clock time names. A time skipped by a daylight
// saving change moves an hour later, and an unknown zone falls back.
export function wallInstant(date, minute, ...zones) {
  for (const zone of [...zones.filter(Boolean), '']) {
    try { return meetingInstant(date, minute, zone); } catch {
      try { return meetingInstant(date, minute + 60, zone); } catch { /* unknown zone */ }
    }
  }
}

const sameTime = (a = '', b = '') => a === b || (!!a && !!b && Date.parse(a) === Date.parse(b));

// A due date the student changed stays until the source moves its own.
// importedDue is what the source last said; a task imported before it was
// recorded counts as unchanged.
function keptDue(old, due = '') {
  const base = old.importedDue ?? old.due;
  return !sameTime(old.due, base) && sameTime(due, base) ? old.due : due;
}

// Merge imported tasks into the existing list: keep local edits, adopt a
// complete import's completion unless a tick is still on its way to Canvas
// (a feed or a preview only adds completion, so a personal tick stays), and
// drop open imported items that disappeared upstream. Finished ones stay: a source looks back
// only a week or two, so its silence about an old task says nothing.
export function mergeTasks(existing, incoming, source, { complete = true } = {}) {
  const byUid = new Map(existing.filter((t) => t.uid).map((t) => [t.uid, t]));
  const incomingIDs = new Set(incoming.map(t => t.uid));
  const out = existing.filter((t) => t.source !== source || (!incomingIDs.has(t.uid) && (!complete || t.done)));
  for (const t of incoming) {
    const old = byUid.get(t.uid);
    if (!old) { out.push({ id: rid(), ...t, importedDue: t.due || '' }); continue; }
    const done = old.pendingCanvasDone ? old.done : source === 'feed' || !complete ? old.done || t.done : t.done;
    const task = { ...old, title: t.title, due: keptDue(old, t.due), importedDue: t.due || '', course: old.courseEdited ? old.course : t.course, courseId: t.courseId, courseCode: t.courseCode, overrideId: t.overrideId || old.overrideId, moduleItem: complete ? t.moduleItem : t.moduleItem || old.moduleItem, reason: old.pendingCanvasDone ? old.reason : t.reason || (!complete ? old.reason : '') || '', url: t.url, done };
    // A completion the import reports carries no time of its own.
    if (!!done !== !!old.done) delete task.completedAt;
    out.push(task);
  }
  return out;
}

// Feed items due more than a fortnight ago are dropped, on every platform.
export function recentTasks(tasks, now = Date.now()) {
  const cutoff = new Date(now - 14 * 86400e3).toISOString();
  return tasks.filter((t) => !t.due || t.due >= cutoff);
}

const dayKey = (d) => `${d.getFullYear()}-${d.getMonth()}-${d.getDate()}`;

// Due times belong to campus, like classes: the Date whose local fields are
// the campus wall clock, or the instant itself when the school has no zone.
export const campusDue = (iso, zone) => campusNow(zone, new Date(iso));

// Buckets: overdue, today, tomorrow, the rest of this week, next week (weeks
// start on Monday or Sunday, like the calendar), later, and undated. Work
// done that was due before today is past rather than overdue.
export function groupTasks(tasks, instant = new Date(), weekStart = 'mon', zone = '') {
  const now = campusNow(zone, instant);
  const today = dayKey(now), tomorrow = dayKey(new Date(now.getFullYear(), now.getMonth(), now.getDate() + 1));
  const into = weekStart === 'sun' ? now.getDay() : (now.getDay() + 6) % 7; // days since the week began
  const sunday = new Date(now.getFullYear(), now.getMonth(), now.getDate() + (7 - into)); // first day of next week
  const following = new Date(sunday.getFullYear(), sunday.getMonth(), sunday.getDate() + 7); // first day of the week after next
  const g = { past: [], overdue: [], today: [], tomorrow: [], week: [], next: [], later: [], undated: [] };
  for (const t of [...tasks].sort((a, b) => (a.due || '9').localeCompare(b.due || '9'))) {
    if (!t.due) { g.undated.push(t); continue; }
    const d = campusDue(t.due, zone), k = dayKey(d);
    if (k === today) g.today.push(t);
    else if (k === tomorrow) g.tomorrow.push(t);
    else if (d < now) (t.done ? g.past : g.overdue).push(t);
    else if (d < sunday) g.week.push(t);
    else if (d < following) g.next.push(t);
    else g.later.push(t);
  }
  return g;
}

let dueTime, dueDate;
const formatDate = (formatter, date) => Number.isNaN(date.getTime()) ? 'Invalid Date' : formatter.format(date);
const shortDate = (date) => formatDate(dueDate ??= new Intl.DateTimeFormat(undefined, { month: 'short', day: 'numeric' }), date);

// Up Next: overdue work and the rest of this week are listed in full; next
// week, Later and No Due Date each fold whole, so nothing past this week's
// end shows in the list. Completed work due before today is past, listed
// before the rest when completed tasks are shown.
export function upNext(tasks, instant = new Date(), weekStart = 'mon', zone = '') {
  const g = groupTasks(tasks, instant, weekStart, zone);
  const folds = ['next', 'later', 'undated'].map((key) => ({ key, items: g[key] })).filter((f) => f.items.length);
  return { past: g.past, near: [...g.overdue, ...g.today, ...g.tomorrow, ...g.week], folds };
}

// Headings for a list in due order: completed work from before today, a
// day at a time back to the start of last week and a week at a time before
// that; then Overdue, which is only work still open; one a day through next
// week, one a week after that, then No Due Date. Each holds its tasks; rows
// then say only what the heading does not (taskWhen).
export function taskHeadings(tasks, instant = new Date(), weekStart = 'mon', zone = '') {
  const now = campusNow(zone, instant), midnight = new Date(now.getFullYear(), now.getMonth(), now.getDate());
  const into = weekStart === 'sun' ? now.getDay() : (now.getDay() + 6) % 7;
  const following = new Date(now.getFullYear(), now.getMonth(), now.getDate() + 14 - into);
  const previous = new Date(now.getFullYear(), now.getMonth(), now.getDate() - 7 - into);
  const rank = (t) => (!t.due || campusDue(t.due, zone) >= midnight ? 2 : t.done ? 0 : 1);
  const out = [];
  for (const t of [...tasks].sort((a, b) => rank(a) - rank(b) || (a.due || '9').localeCompare(b.due || '9'))) {
    let kind = 'undated', start = null;
    if (t.due) {
      const d = campusDue(t.due, zone);
      if (d < midnight && !t.done) kind = 'overdue';
      else if (d < following && d >= (d < midnight ? previous : midnight)) { kind = 'day'; start = new Date(d.getFullYear(), d.getMonth(), d.getDate()); }
      else {
        kind = 'week';
        const back = weekStart === 'sun' ? d.getDay() : (d.getDay() + 6) % 7;
        start = new Date(d.getFullYear(), d.getMonth(), d.getDate() - back);
      }
    }
    const key = kind + (start ? dayKey(start) : '');
    const last = out[out.length - 1];
    if (last?.key === key) last.items.push(t);
    else out.push({ key, kind, start, items: [t] });
  }
  return out;
}

let headWeekday, rowWeekday;
// A heading's name and the date beside it: ["Today", "Oct 2"], ["Yesterday", "Oct 1"], ["Monday", "Oct 5"],
// ["Week of Oct 12", ""], ["Overdue", ""].
export function headingLabel(h, instant = new Date(), zone = '') {
  if (h.kind === 'overdue') return ['Overdue', ''];
  if (h.kind === 'undated') return ['No Due Date', ''];
  const date = shortDate(h.start);
  if (h.kind === 'week') return [`Week of ${date}`, ''];
  const now = campusNow(zone, instant);
  const days = Math.round((h.start - new Date(now.getFullYear(), now.getMonth(), now.getDate())) / 86400e3);
  return [days === 0 ? 'Today' : days === 1 ? 'Tomorrow' : days === -1 ? 'Yesterday' : formatDate(headWeekday ??= new Intl.DateTimeFormat(undefined, { weekday: 'long' }), h.start), date];
}

// What a row adds to its heading: the date under Overdue, the weekday under a
// week, and the time unless it is 11:59pm, the usual end of the day.
export function taskWhen(iso, kind, zone = '') {
  if (!iso) return '';
  const d = campusDue(iso, zone);
  if (kind === 'overdue') return shortDate(d);
  const time = d.getHours() === 23 && d.getMinutes() === 59 ? '' : formatDate(dueTime ??= new Intl.DateTimeFormat(undefined, { hour: 'numeric', minute: '2-digit' }), d).toLowerCase().replace(/\s/, '');
  if (kind !== 'week') return time;
  return [formatDate(rowWeekday ??= new Intl.DateTimeFormat(undefined, { weekday: 'short' }), d), time].filter(Boolean).join(' ');
}

// Map an LMS course name or code ("CS 1213-010 Fall 2026", "2026FA-CS-1213-010",
// "Intro to Programming") to a schedule course code. The code may be written
// with any separators, but must sit on its own: "CS 101" is not inside
// "PHYSICS 1010". Failing that, a course title from the schedule that appears
// in the LMS name counts.
function codePattern(code) {
  const m = code.toUpperCase().match(/^([A-Z][A-Z ]*?)\s*(\d[A-Z0-9-]*)$/);
  if (!m) return null;
  const subject = m[1].replace(/\s+/g, '').split('').join('[^A-Z0-9]?');
  const number = m[2].replace(/[^A-Z0-9]/g, '').split('').join('[^A-Z0-9]?');
  return new RegExp(`(^|[^A-Z])${subject}[^A-Z0-9]{0,3}${number}(?![0-9])`);
}
// Title words, minus filler; used so "Intro to Business Ethics" and
// "Introduction to Business Ethics (Honors)" count as the same title.
const STOP = new Set(['THE', 'OF', 'TO', 'AND', 'IN', 'FOR', 'A', 'AN', 'ON', 'WITH', 'I', 'II', 'III', 'IV', 'HONORS', 'HONOR', 'SECTION', 'SEC', 'LECTURE', 'LEC', 'DISCUSSION', 'DISC', 'LAB', 'ONLINE', 'FALL', 'SPRING', 'SUMMER', 'WINTER']);
const words = (s) => (s || '').toUpperCase().replace(/&/g, ' AND ').split(/[^A-Z0-9]+/).filter((w) => w && !STOP.has(w) && !/^\d+$/.test(w));
const sameWord = (a, b) => a === b || (a.length >= 4 && b.length >= 4 && (a.startsWith(b) || b.startsWith(a))); // INTRO ~ INTRODUCTION
// Every word of the shorter title appears in the other, with enough substance to trust.
function titlesAlike(a, b) {
  const [short, long] = a.length <= b.length ? [a, b] : [b, a];
  if (!short.length || (short.length < 2 && short[0].length < 8)) return false; // one word only counts when it is distinctive enough
  return short.every((w) => long.some((v) => sameWord(w, v)));
}

// Compiled once per schedule change, rather than for each task and each render.
export function createCourseMatcher(codes, titles = {}) {
  const patterns = [...codes].sort((a, b) => b.length - a.length).map(code => [code, codePattern(code)]);
  const names = codes.map(code => [code, [].concat(titles[code] || []).map(words)]);
  return (name) => {
    const N = (name || '').toUpperCase();
    if (!N.trim()) return '';
    const byCode = patterns.find(([, pattern]) => pattern?.test(N));
    if (byCode) return byCode[0];
    const nameWords = words(N);
    return names.find(([, titles]) => titles.some(title => titlesAlike(title, nameWords)))?.[0] || '';
  };
}
