import { occurrencesConflict } from './occurrences.js';
// Pure schedule logic: conflicts, linked-section options and the generator.
export const DAYS = 'MTWRFSU';
export const DAY_NAMES = ['Mon', 'Tue', 'Wed', 'Thu', 'Fri', 'Sat', 'Sun'];

export const itemsConflict = occurrencesConflict;

// Ids of every class that overlaps another class. Personal events (items
// with a block) never count: they sit on the calendar beside classes, and
// only the rearranger treats them as time to keep clear. Exam sittings are
// dated facts about a class rather than a choice, so they never count either.
export function conflictIds(items) {
  const bad = new Set();
  const classes = items.filter((it) => !it.block && !it.exam);
  for (let i = 0; i < classes.length; i++)
    for (let j = i + 1; j < classes.length; j++)
      if (itemsConflict(classes[i], classes[j])) bad.add(classes[i].id).add(classes[j].id);
  return bad;
}

// Required groups are established before seat filtering. Generate options lazily.
function* combinations(lists, index = 0, chosen = []) {
  if (index === lists.length) { yield [...chosen]; return; }
  for (const section of lists[index]) {
    chosen.push(section); yield* combinations(lists, index + 1, chosen); chosen.pop();
  }
}
export function* courseOptions(course, openOnly = false) {
  const groups = new Map();
  for (const section of course.sections) {
    if (!section.link) { if (!openOnly || section.available > 0) yield [section]; continue; }
    if (!groups.has(section.link)) groups.set(section.link, new Map());
    const types = groups.get(section.link);
    if (!types.has(section.type)) types.set(section.type, []);
    if (!openOnly || section.available > 0) types.get(section.type).push(section);
  }
  for (const types of groups.values()) { const lists = [...types.values()]; if (lists.every((list) => list.length)) yield* combinations(lists); }
}

export function generate(courses, fixed = [], { limit = 50, openOnly = false, maxSteps = 100000, timeoutMs = 1500 } = {}) {
  const results = [], taken = [...fixed], chosen = [];
  const order = courses.map((course, index) => ({ course, index })).sort((a, b) => a.course.sections.length - b.course.sections.length);
  let steps = 0;
  const deadline = performance.now() + timeoutMs;
  function go(index) {
    if (results.length >= limit) return;
    if (index === order.length) { results.push([...chosen].sort((a,b) => a.index - b.index).flatMap((x) => x.option)); return; }
    for (const option of courseOptions(order[index].course, openOnly)) {
      if (++steps > maxSteps || performance.now() > deadline) throw new Error('This search is too large. Pin fewer courses or narrow the sections and try again.');
      if (conflictIds(option).size || option.some((s) => taken.some((t) => itemsConflict(s, t)))) continue;
      chosen.push({ index: order[index].index, option }); taken.push(...option);
      go(index + 1); chosen.pop(); taken.length -= option.length;
      if (results.length >= limit) return;
    }
  }
  go(0);
  return results;
}

export function fmtTime(m) {
  const h = Math.floor(m / 60), mm = String(m % 60).padStart(2, '0');
  return `${((h + 11) % 12) + 1}:${mm}${h < 12 ? 'am' : 'pm'}`;
}

export const fmtHour = (h) => `${((h + 11) % 12) + 1}${h < 12 ? 'am' : 'pm'}`;

export const fmtMeeting = (m) => `${m.days} ${fmtTime(m.start)} – ${fmtTime(m.end)}`;

export const rid = () => crypto.randomUUID();

// Compact copy of a section kept inside a schedule, so the calendar, credits
// and exports render without the term's catalog. Seat counts are a snapshot
// from when it was added; live ones come from /api/seats.
export function compact(s) {
  return {
    id: s.id, course: s.course, subject: s.subject || '', title: s.title || '', num: s.num, type: s.type || '', link: s.link || '',
    meetings: (s.meetings || []).map((m) => ({ days: m.days, start: m.start, end: m.end, location: m.location || '' })),
    exams: (s.exams || []).map((e) => ({ date: e.date, start: e.start, end: e.end, location: e.location || '' })),
    credits: s.credits || 0, start: s.start || '', end: s.end || '', instructors: s.instructors || [], mode: s.mode || '',
    capacity: s.capacity || 0, available: s.available ?? -1, waitlist: s.waitlist || 0, url: s.url || '',
  };
}
