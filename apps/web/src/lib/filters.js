// Section filters for the class search panel.
export const emptyFilters = () => ({ open: false, days: '', time: 'any', mode: 'any', level: 'any', attr: '' });

export const isDefault = (f) => !f.open && !f.days && f.time === 'any' && f.mode === 'any' && f.level === 'any' && !f.attr;

const window = { morning: [0, 12 * 60], afternoon: [12 * 60, 17 * 60], evening: [17 * 60, 24 * 60] };

export function sectionPasses(s, f, course) {
  if (f.open && !(s.available > 0)) return false;
  if (f.days && !s.meetings.every((m) => [...m.days].every((d) => f.days.includes(d)))) return false;
  if (f.time !== 'any') {
    const [lo, hi] = window[f.time];
    if (!s.meetings.length || !s.meetings.every((m) => m.start >= lo && m.start < hi)) return false;
  }
  if (f.mode !== 'any' && s.mode !== f.mode) return false;
  if (f.level !== 'any') {
    const digit = (course.code.match(/\b(\d)\d*[A-Z]*$/) || [])[1];
    if (f.level === 'grad' ? !(digit >= 5) : digit !== f.level) return false;
  }
  if (f.attr && !(s.attributes || []).includes(f.attr)) return false;
  return true;
}

// Courses with at least one passing section, each trimmed to those sections.
export function applyFilters(courses, f) {
  if (isDefault(f)) return courses;
  const out = [];
  for (const c of courses) {
    const sections = c.sections.filter((s) => sectionPasses(s, f, c));
    if (sections.length) out.push({ ...c, sections });
  }
  return out;
}

export function attributeList(catalog) {
  const count = new Map();
  for (const c of catalog?.courses || []) for (const s of c.sections) for (const a of s.attributes || []) count.set(a, (count.get(a) || 0) + 1);
  return [...count.entries()].sort((a, b) => b[1] - a[1]).slice(0, 40).map(([a]) => a);
}
