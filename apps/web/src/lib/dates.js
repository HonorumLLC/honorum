// Week arithmetic for the dated calendar.

// First day of the week containing d; weeks start on Monday or Sunday.
export function startOfWeek(d, weekStart = 'mon') {
  const m = new Date(d.getFullYear(), d.getMonth(), d.getDate());
  m.setDate(m.getDate() - (weekStart === 'sun' ? m.getDay() : (m.getDay() + 6) % 7));
  return m;
}

export const addDays = (d, n) => new Date(d.getFullYear(), d.getMonth(), d.getDate() + n);
export const ymd = (d) => `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, '0')}-${String(d.getDate()).padStart(2, '0')}`;
export const parse = (s) => (s ? new Date(s + 'T00:00:00') : null);

// The week to open on: this week during the term, otherwise the term's first week.
export function defaultWeek(term, today = new Date(), weekStart = 'mon') {
  const start = parse(term?.start), end = parse(term?.end);
  if (start && end && (today < start || today > end)) return startOfWeek(start, weekStart);
  return startOfWeek(today, weekStart);
}

// Does an item meet during the week starting at first? Sections and the term
// carry date ranges; a one-time event or an exam sitting carries a single date.
export function activeInWeek(item, first, term) {
  const once = item.block?.date || item.date;
  const from = parse(once || item.section?.start || term?.start), to = parse(once || item.section?.end || term?.end);
  const last = addDays(first, 6);
  if (from && from > last) return false;
  if (to && to < first) return false;
  return true;
}

// Several schools (Banner, UC Irvine, Rutgers) publish no term dates, so for
// ordering we guess a window from the name: "Fall 2026", "2026 Fall Quarter",
// "Fall Semester 2026", "Winter 2026-2027". Rough but enough to tell the
// current term from a distant one. Never shown as real dates.
const SEASONS = { spring: ['01-05', '05-20'], summer: ['05-15', '08-15'], fall: ['08-15', '12-20'], autumn: ['08-15', '12-20'], winter: ['12-15', '01-20'] };
export function seasonDates(name = '') {
  const season = (name.toLowerCase().match(/spring|summer|fall|autumn|winter/) || [])[0];
  const year = (name.match(/\b(20\d\d)\b/) || [])[1];
  if (!season || !year) return null;
  const [a, b] = SEASONS[season];
  // A winter term named by one year ("Winter 2027") is the intersession that starts the December before.
  const startYear = season === 'winter' && !/20\d\d\s*[-–]\s*20\d\d/.test(name) ? String(+year - 1) : year;
  return { start: `${startYear}-${a}`, end: `${season === 'winter' ? +startYear + 1 : year}-${b}` };
}

const dated = (x) => (x.start ? x : { ...x, ...(seasonDates(x.name) || {}), guessed: true });

// Current term first, then upcoming by start, then past newest first. Terms
// without dates are ordered by dates guessed from their names.
export function orderTerms(terms, today = new Date()) {
  const t = ymd(today);
  const rank = (x) => (!x.start ? 1 : x.start <= t && (!x.end || t <= x.end) ? 0 : x.start > t ? 1 : 2);
  const key = new Map(terms.map((x) => [x, dated(x)]));
  const r = (x) => rank(key.get(x)), st = (x) => key.get(x).start || '';
  return [...terms].sort((a, b) => r(a) - r(b) || (r(a) === 1 ? st(a).localeCompare(st(b)) : st(b).localeCompare(st(a))));
}

// The term to open when none is saved: the first in order that has been
// fetched and has sections, else the first fetched, else the first listed.
export function defaultTerm(terms) {
  return terms.find((x) => x.updated && x.sections) || terms.find((x) => x.updated) || terms[0] || null;
}
