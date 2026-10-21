import { get, set } from './store.js';

// Descriptions and prerequisites live in their own publication, fetched only
// when a class is opened, so they never weigh on the first load.
const descs = new Map();
export async function loadDescriptions(inst, term, hash) {
  if (!hash) return {};
  const key = `desc:${inst}:${term}`;
  const memoryKey = `${key}:${hash}`;
  if (descs.has(memoryKey)) return descs.get(memoryKey);
  const cached = await get(key).catch(() => null);
  if (cached?.hash === hash) { descs.set(memoryKey, cached.data); return cached.data; }
  try {
    const r = await fetch(`/catalog/${encodeURIComponent(inst)}/${encodeURIComponent(term)}.desc.${encodeURIComponent(hash)}.json`);
    if (r.ok) {
      const d = await r.json();
      set(key, { hash, data: d }).catch(() => {});
      descs.set(memoryKey, d);
      return d;
    }
  } catch {}
  // A failed request is not memoized; an earlier saved version still helps.
  return cached?.data || {};
}

// The search index is a plain array kept outside Svelte's reactive proxies,
// so scanning a few thousand courses per keystroke costs well under a
// millisecond.
const words = (s) => s.toLowerCase().split(/[^a-z0-9]+/).filter(Boolean);

export function indexCatalog(c) {
  // Derive the in-memory index without expanding the raw IndexedDB record.
  c = { ...c, courses: c.courses.map(course => ({ ...course, sections: course.sections.map(s => ({ ...s })) })) };
  c.byId = new Map();
  c.index = c.courses.map((course) => {
    for (const s of course.sections) {
      // The snapshot omits what repeats the course, the term or the institution.
      s.course ??= course.code;
      s.subject ??= course.subject;
      s.title ??= course.title;
      s.instructors ??= [];
      s.meetings ??= [];
      s.capacity ??= 0;
      s.available ??= -1;
      s.waitlist ??= 0;
      s.start ??= c.term?.start || '';
      s.end ??= c.term?.end || '';
      s.url ??= c.url || '';
    }
    const people = [...new Set(course.sections.flatMap((s) => s.instructors))];
    for (const s of course.sections) c.byId.set(s.id, { ...s, course: course.code, title: course.title });
    const code = course.code.toLowerCase();
    return {
      course, code, compact: code.replace(/\s+/g, ''), subject: course.subject.toLowerCase(),
      title: words(course.title), people: words(people.join(' ')),
      hay: `${code} ${course.title} ${people.join(' ')}`.toLowerCase(),
    };
  });
  return c;
}

// Score one course against the query. 0 means no match. Course code and
// subject outrank title words, which outrank instructors, which outrank
// substrings buried inside other words (so "mis" is MIS, not chemistry).
function score(e, q, compact, tokens) {
  let s = 0;
  if (e.subject === q) s += 1000;
  else if (e.code.startsWith(q) || e.compact.startsWith(compact)) s += 500 - e.compact.length;
  for (const t of tokens) {
    if (e.subject === t) s += 100;
    else if (e.compact.startsWith(t)) s += 80;
    else if (e.title.some((w) => w.startsWith(t))) s += 40;
    else if (e.people.some((w) => w.startsWith(t))) s += 20;
    else if (e.hay.includes(t)) s += 1;
    else return 0; // every token must match somewhere
  }
  return s;
}

export function search(catalog, query, limit = 40) {
  // Separators between code and section number ("B AD 1001-006", "cs1213/010") become spaces.
  const q = query.trim().toLowerCase().replace(/[-_/.:]+/g, ' ').replace(/\s+/g, ' ');
  if (!q) return catalog.index.slice(0, limit).map((e) => e.course);
  return narrowToSection(catalog, q) || rank(catalog.index, q, limit);
}

function rank(index, q, limit) {
  const compact = q.replace(/\s+/g, ''), tokens = q.split(' ');
  const hits = [];
  for (const e of index) {
    const s = score(e, q, compact, tokens);
    if (s) hits.push([s, e]);
  }
  hits.sort((a, b) => b[0] - a[0] || (a[1].code < b[1].code ? -1 : 1));
  return hits.slice(0, limit).map((h) => h[1].course);
}

// Section numbers differ by school ("010", "01B", "501", "10A", "1", "40003"),
// so match exactly, by prefix, or ("6" for "006") ignoring the number's
// leading zeros when the person typed none.
const numMatches = (num, tok) => {
  const n = String(num).toLowerCase();
  return n === tok || n.startsWith(tok) || (!tok.startsWith('0') && n.replace(/^0+/, '') === tok);
};

// "B AD 1001 006": when everything but the last token names one course
// exactly, show that course trimmed to the sections whose number matches.
// A bare section id (CRN) finds its section too.
function narrowToSection(catalog, q) {
  const tokens = q.split(' ');
  if (tokens.length >= 2) {
    const tok = tokens[tokens.length - 1];
    const base = tokens.slice(0, -1).join(' ');
    const baseCompact = base.replace(/\s+/g, '');
    const out = [];
    for (const e of catalog.index) {
      if (e.compact !== baseCompact && e.code !== base) continue;
      const sections = e.course.sections.filter((s) => numMatches(s.num, tok));
      if (sections.length) out.push({ ...e.course, sections });
    }
    if (out.length) return out;
  }
  if (tokens.length === 1 && /^\d{4,}$/.test(q) && catalog?.byId) {
    const sec = catalog.byId.get(q);
    const course = sec && catalog.courses.find((c) => c.code === sec.course);
    if (course) return [{ ...course, sections: course.sections.filter((s) => s.id === q) }];
  }
  return null;
}
