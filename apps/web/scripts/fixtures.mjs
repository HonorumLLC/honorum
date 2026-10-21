// Regenerates contracts/fixtures/*.json from the web implementation, the
// reference for the shared planner rules. Run from apps/web:
//   node scripts/fixtures.mjs
import { writeFileSync, readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { dirname, join } from 'node:path';
process.env.TZ = 'Asia/Tokyo'; // any zone but the fixtures': results must not depend on it
const { mergeBody } = await import('../src/lib/sync.js');
const { parseICS, mergeTasks, recentTasks } = await import('../src/lib/tasks.js');
const { meetingsOn } = await import('../src/lib/occurrences.js');

const dir = join(dirname(fileURLToPath(import.meta.url)), '../../../contracts/fixtures');
const load = (name) => JSON.parse(readFileSync(join(dir, name), 'utf8'));
const save = (name, value) => writeFileSync(join(dir, name), JSON.stringify(value, null, 2) + '\n');

const merge = load('merge.json');
for (const c of merge.cases) Object.assign(c, mergeBody(c.base, c.local, c.remote));
save('merge.json', merge);

const ical = load('icalendar.json');
for (const c of ical.cases) c.expected = recentTasks(parseICS(c.text, 'feed', c.label, ical.timezone), Date.parse(c.now)).map(({ uid, title, course, due, done, url }) => ({ uid, title, course, due, done, url }));
save('icalendar.json', ical);

const tasks = load('tasks-merge.json');
for (const c of tasks.cases) c.expected = mergeTasks(c.existing, c.incoming, c.source, { complete: c.complete }).map(({ id, ...t }) => t);
save('tasks-merge.json', tasks);

const occ = load('occurrences.json');
for (const c of occ.cases) c.expected = meetingsOn(c.item, c.date, c.term, c.closures).map((m) => [m.days, m.start, m.end]);
save('occurrences.json', occ);
console.log('fixtures written to', dir);
