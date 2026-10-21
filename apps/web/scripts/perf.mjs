// End-user performance suite. Builds nothing: run `npm run build` first.
//
//   node scripts/perf.mjs [--runs=3] [--only=load,sync] [--profiles=desktop,mobile]
//                         [--out=DIR] [--compare=DIR/results.json] [--fixture=DIR]
//
// It serves the build through scripts/perf-lab.mjs, with production-like
// compression and cache headers, recorded public catalog data, and the real
// API from `honorum-lab perf` (crates/server/src/lab/perf.rs: a temporary
// database and a simulated Canvas). Chromium drives what students do: a first visit, a
// returning visit to a saved plan, week navigation, class search, a new
// device signing in, edits reaching the server and other devices, and Canvas
// imports. Each measure is the median of --runs repetitions.
//
// Profiles: desktop is a 40 ms, 20 Mbit/s link; mobile is Lighthouse's slow
// 4G (150 ms, 1.6 Mbit/s) with the main thread slowed fourfold. Results are
// lab measurements for comparing builds, not production percentiles.
// The fixture (public config and one term's publications) is captured from
// --capture (default https://honorum.app) for --school (default ou) when absent.
import { chromium } from '@playwright/test';
import { spawn, execFileSync } from 'node:child_process';
import fs from 'node:fs';
import path from 'node:path';
import zlib from 'node:zlib';
import { fileURLToPath } from 'node:url';
import { startLab } from './perf-lab.mjs';
import { compact } from '../src/lib/schedule.js';
import { orderTerms, defaultTerm } from '../src/lib/dates.js';

const here = path.dirname(fileURLToPath(import.meta.url));
const web = path.resolve(here, '..'), root = path.resolve(web, '../..');
const args = Object.fromEntries(process.argv.slice(2).map((a) => { const [k, ...v] = a.replace(/^--/, '').split('='); return [k, v.length ? v.join('=') : 'true']; }));
const runs = Math.max(1, +(args.runs || 3));
const dist = path.resolve(args.dist || path.join(web, 'dist'));
const fixtureDir = path.resolve(args.fixture || path.join(root, 'artifacts/perf/fixture'));
const out = path.resolve(args.out || path.join(root, 'artifacts/perf', new Date().toISOString().replace(/[:.]/g, '-')));
const only = args.only ? args.only.split(',') : null;
const PROFILES = {
  desktop: { viewport: { width: 1440, height: 900 }, latency: 40, rate: 2_500_000, cpu: 1 },
  mobile: { viewport: { width: 390, height: 844 }, deviceScaleFactor: 2, isMobile: true, hasTouch: true, latency: 150, rate: 200_000, cpu: 4 },
};
const profiles = (args.profiles || 'desktop,mobile').split(',');
const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
const median = (xs) => { const v = xs.filter((x) => Number.isFinite(x)).sort((a, b) => a - b); return v.length ? (v.length % 2 ? v[(v.length - 1) / 2] : (v[v.length / 2 - 1] + v[v.length / 2]) / 2) : null; };
const brotli = (body, quality) => zlib.brotliCompressSync(body, { params: { [zlib.constants.BROTLI_PARAM_QUALITY]: quality } }).length;

// Fixture ------------------------------------------------------------------

async function ensureFixture() {
  if (fs.existsSync(path.join(fixtureDir, 'config.json'))) return;
  const base = args.capture && args.capture !== 'true' ? args.capture : 'https://honorum.app';
  const school = args.school || 'ou';
  fs.mkdirSync(fixtureDir, { recursive: true });
  const config = await (await fetch(base + '/api/config')).json();
  const inst = config.institutions.find((i) => i.id === school);
  const term = defaultTerm(orderTerms(inst.terms));
  for (const [kind, hash] of [['structure', term.structure_snapshot], ['seats', term.seats_snapshot], ['desc', term.descriptions_snapshot]]) {
    if (!hash) continue;
    const response = await fetch(`${base}/catalog/${inst.id}/${term.id}.${kind}.${hash}.json`);
    if (!response.ok) throw new Error(`Could not capture ${kind} for ${inst.id} ${term.id}`);
    fs.writeFileSync(path.join(fixtureDir, `${inst.id}-${term.id}.${kind}.json`), Buffer.from(await response.arrayBuffer()));
  }
  fs.writeFileSync(path.join(fixtureDir, 'config.json'), JSON.stringify(config));
  fs.writeFileSync(path.join(fixtureDir, 'source.json'), JSON.stringify({ base, inst: inst.id, term: term.id, captured: new Date().toISOString() }));
  console.log(`Captured ${inst.name} ${term.name} from ${base} into ${fixtureDir}`);
}

function readFixture() {
  const config = JSON.parse(fs.readFileSync(path.join(fixtureDir, 'config.json'), 'utf8'));
  const source = JSON.parse(fs.readFileSync(path.join(fixtureDir, 'source.json'), 'utf8'));
  const inst = config.institutions.find((i) => i.id === source.inst);
  const term = inst.terms.find((t) => t.id === source.term);
  const file = (kind) => path.join(fixtureDir, `${inst.id}-${term.id}.${kind}.json`);
  const catalog = JSON.parse(fs.readFileSync(file('structure'), 'utf8'));
  return { config, inst, term, catalog, file };
}

// A student's plan: six classes with meetings and forty tasks over five weeks.
function studentPlan({ inst, term, catalog }) {
  const sections = [];
  for (const course of catalog.courses) {
    const s = course.sections.find((x) => x.meetings?.length && x.meetings.every((m) => /^[MTWRF]+$/.test(m.days)));
    if (!s || sections.some((x) => x.meetings.some((m) => s.meetings.some((n) => m.days.split('').some((d) => n.days.includes(d)) && m.start < n.end && n.start < m.end)))) continue;
    sections.push({ ...s, course: course.code, title: course.title, subject: course.subject, start: s.start || term.start, end: s.end || term.end });
    if (sections.length === 6) break;
  }
  const day = (n) => { const d = new Date(Date.now() + n * 86400e3); d.setHours(10 + (n % 8), 0, 0, 0); return d.toISOString(); };
  const schedule = { id: 'perf-schedule', name: 'Fall plan', inst: inst.id, term: term.id, primary: true, sections: sections.map((s) => s.id), pinned: Object.fromEntries(sections.map((s) => [s.id, compact(s)])), blocks: [], removed: {}, colors: {} };
  const tasks = Array.from({ length: 40 }, (_, i) => ({ id: `perf-task-${i}`, source: 'manual', title: `Problem set ${i + 1}`, course: sections[i % sections.length].course, done: i < 6, due: day(i - 5), ...(i < 6 ? { completedAt: day(i - 6) } : {}) }));
  return { schedule, tasks, sections };
}

function guestState(fx, plan, extra = {}) {
  return {
    config: fx.config, inst: fx.inst.id, term: fx.term.id, onboarded: true, current: plan.schedule.id, account: null, dataOwner: '',
    watches: [], reminders: { id: '', on: false }, schedules: [plan.schedule], tasks: plan.tasks, prefExtras: {},
    lms: { canvas: { base: '', token: '', synced: 0 }, feeds: [], map: {} }, ...extra,
  };
}

// The lab API (honorum-lab perf) -------------------------------------------

async function startApi() {
  execFileSync('cargo', ['build', '--release', '--locked', '-p', 'honorum', '--features', 'lab', '--bin', 'honorum-lab'], { cwd: root, stdio: 'inherit' });
  const binary = path.resolve(root, process.env.CARGO_TARGET_DIR || 'target', 'release/honorum-lab');
  const port = 18400 + Math.floor(Math.random() * 500);
  const child = spawn(path.join(root, 'scripts/postgres.sh'), ['test', binary, 'perf'], {
    cwd: root, stdio: ['ignore', 'pipe', 'inherit'],
    env: { ...process.env, HONORUM_PERF_ADDR: `127.0.0.1:${port}`, HONORUM_PERF_CONFIG: path.join(fixtureDir, 'config.json'), HONORUM_PERF_CANVAS_LATENCY: args.canvas || '300ms' },
  });
  await new Promise((resolve, reject) => {
    child.stdout.on('data', (d) => { if (String(d).includes('performance lab API')) resolve(); });
    child.on('exit', (code) => reject(new Error(`performance lab API exited with ${code}`)));
  });
  const url = `http://127.0.0.1:${port}`;
  const call = async (p, body) => {
    const r = await fetch(url + p, body ? { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify(body) } : {});
    if (!r.ok) throw new Error(`${p}: ${r.status} ${await r.text()}`);
    return r.json();
  };
  return { url, call, close: () => child.kill('SIGTERM') };
}

// Browser helpers -----------------------------------------------------------

let browser, lab, api, fx, contextCount = 0, current = 'setup', currentProfile = 'desktop';
const contexts = new Set();

// Collected in the page from navigation start: paints, long tasks, layout
// shifts, and when the welcome page or the planner became usable.
function instrument() {
  const p = window.__perf = { lcp: 0, cls: 0, longTasks: [], marks: {} };
  const observe = (type, fn, extra = {}) => { try { new PerformanceObserver((list) => list.getEntries().forEach(fn)).observe({ type, buffered: true, ...extra }); } catch {} };
  observe('largest-contentful-paint', (e) => { p.lcp = e.startTime; });
  observe('layout-shift', (e) => { if (!e.hadRecentInput) p.cls += e.value; });
  observe('longtask', (e) => p.longTasks.push([e.startTime, e.duration]));
  const mark = (name, test) => { if (!p.marks[name] && test()) p.marks[name] = performance.now(); };
  const check = () => {
    const usable = !document.querySelector('.app-content')?.inert;
    mark('welcome', () => usable && document.querySelector('.welcome .actions .suggested'));
    mark('planner', () => usable && document.querySelector('.cal .times, .day .times'));
  };
  new MutationObserver(check).observe(document, { subtree: true, childList: true, attributes: true });
}

async function newPage(profileName, { cookies = [] } = {}) {
  const profile = PROFILES[profileName];
  const id = `c${++contextCount}`;
  const context = await browser.newContext({ viewport: profile.viewport, deviceScaleFactor: profile.deviceScaleFactor, isMobile: profile.isMobile, hasTouch: profile.hasTouch, serviceWorkers: 'allow' });
  await context.addCookies([{ name: 'lab', value: `${id}:${profile.latency}:${profile.rate}`, url: lab.url }, ...cookies.map((c) => ({ ...c, url: lab.url }))]);
  await context.addInitScript(instrument);
  const page = await context.newPage();
  const errors = [];
  page.on('pageerror', (e) => errors.push(e.message));
  const cdp = await context.newCDPSession(page);
  await cdp.send('Performance.enable');
  if (profile.cpu > 1) await cdp.send('Emulation.setCPUThrottlingRate', { rate: profile.cpu });
  const netlog = () => lab.log(id);
  // Each context's requests are kept beside the results for inspection.
  const close = context.close.bind(context);
  context.close = async () => { contexts.delete(context); fs.mkdirSync(path.join(out, 'network'), { recursive: true }); fs.writeFileSync(path.join(out, 'network', `${current}-${id}.json`), JSON.stringify(netlog(), null, 1)); await close(); };
  contexts.add(context);
  return { context, page, cdp, id, errors, netlog, profile };
}

// Seed IndexedDB from a page on the origin before the app first runs.
async function seed(page, records) {
  await page.goto(lab.url + '/__seed');
  await page.evaluate(async (records) => {
    const db = await new Promise((resolve, reject) => { const r = indexedDB.open('honorum', 2); r.onupgradeneeded = () => r.result.createObjectStore('kv'); r.onsuccess = () => resolve(r.result); r.onerror = () => reject(r.error); });
    await new Promise((resolve, reject) => { const t = db.transaction('kv', 'readwrite'); for (const [k, v] of records) t.objectStore('kv').put(v, k); t.oncomplete = resolve; t.onabort = () => reject(t.error); });
    db.close();
  }, records);
}

// The service worker installs its shell after the first visit; later visits are controlled by it.
async function installShell(page) {
  await page.evaluate(() => navigator.serviceWorker.ready.then(() => new Promise((resolve) => (navigator.serviceWorker.controller ? resolve() : navigator.serviceWorker.addEventListener('controllerchange', resolve, { once: true })))));
}

// Settled: nothing but the event stream has been in flight for quietMs.
async function settle(s, since, { quietMs = 1000, timeout = 30000 } = {}) {
  const deadline = Date.now() + timeout;
  let quietFrom = Date.now();
  while (Date.now() < deadline) {
    const open = s.netlog().slice(since).some((e) => e.end === undefined && !e.path.startsWith('/api/me/events'));
    if (open) quietFrom = Date.now();
    else if (Date.now() - quietFrom >= quietMs) break;
    await sleep(100);
  }
}

function traffic(entries) {
  const list = entries.filter((e) => !e.path.startsWith('/api/me/events'));
  const api = list.filter((e) => e.path.startsWith('/api/'));
  return {
    requests: list.length, bytes: list.reduce((n, e) => n + e.bytes, 0),
    apiRequests: api.length, apiBytes: api.reduce((n, e) => n + e.bytes, 0),
    paths: list.map((e) => `${e.method} ${e.path.replace(/\?.*/, '')} ${e.status}`),
  };
}

async function pageMetrics(s) {
  const perf = await s.page.evaluate(() => {
    const nav = performance.getEntriesByType('navigation')[0];
    const fcp = performance.getEntriesByName('first-contentful-paint')[0]?.startTime;
    return { ...window.__perf, ttfb: nav?.responseStart, fcp, nodes: document.getElementsByTagName('*').length };
  });
  // Memory is what the page keeps, after collecting garbage.
  await s.cdp.send('HeapProfiler.collectGarbage');
  const metrics = Object.fromEntries((await s.cdp.send('Performance.getMetrics')).metrics.map((m) => [m.name, m.value]));
  return { ...perf, heapMB: metrics.JSHeapUsedSize / 1048576 };
}
// Blocking time: the part of each long task beyond 50 ms.
const blocking = (longTasks) => longTasks.reduce((n, [, d]) => n + Math.max(0, d - 50), 0);

// Frames after an action: resolves once two animation frames have passed.
const frames = (page, action) => page.evaluate(async (action) => {
  const at = performance.now();
  eval(action);
  await new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(resolve)));
  return performance.now() - at;
}, action);
// Time from an action until a selector matches, then two frames.
const until = (page, action, selector, timeout = 60000) => page.evaluate(async ([action, selector, timeout]) => {
  const at = performance.now();
  eval(action);
  await new Promise((resolve, reject) => { const check = () => document.querySelector(selector) ? resolve() : performance.now() - at > timeout ? reject(new Error('Timed out waiting for ' + selector)) : setTimeout(check, 4); check(); });
  await new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(resolve)));
  return performance.now() - at;
}, [action, selector, timeout]);

// CPU time of every renderer process, the page's workers and service worker
// included: what a visit costs a phone's battery, whatever thread it ran on.
let browserSession;
async function cpuTime() {
  browserSession ??= await browser.newBrowserCDPSession();
  const { processInfo } = await browserSession.send('SystemInfo.getProcessInfo');
  return Object.fromEntries(processInfo.filter((p) => p.type === 'renderer').map((p) => [p.id, p.cpuTime * 1000]));
}
// Chromium's CPU throttling itself costs CPU time, so throttled runs report none.
const cpuSince = (before, after) => PROFILES[currentProfile].cpu > 1 ? null : Object.entries(after).reduce((n, [id, ms]) => n + ms - (before[id] || 0), 0);
// When the last request (by default the last API request) of a stretch ended,
// from its first request. The service worker's own update check is not the app's.
const lastEnd = (entries, filter = (e) => e.path.startsWith('/api/')) => Math.max(0, ...entries.filter((e) => filter(e) && !e.path.startsWith('/api/me/events')).map((e) => e.end || 0)) - (entries[0]?.start ?? 0);
const planner = (page) => page.waitForFunction(() => window.__perf.marks.planner, null, { timeout: 60000 });
const tasksShown = async (s) => { if (s.profile.isMobile) await s.page.getByRole('tab', { name: /^Tasks/ }).click(); };

// Scenarios -----------------------------------------------------------------
// Each returns a flat object of numbers for one run: Ms are milliseconds, KB
// kilobytes as transferred (compressed), and the rest are counts.

const scenarios = {};

// A new visitor: nothing cached, no service worker yet. Critical bytes are
// those requested before the welcome page was usable; total includes the
// offline shell the service worker installs afterwards.
scenarios['load.first'] = async (profile) => {
  const s = await newPage(profile);
  const cpu = await cpuTime();
  await s.page.goto(lab.url + '/');
  await s.page.waitForFunction(() => window.__perf.marks.welcome, null, { timeout: 60000 });
  const readyAt = s.netlog().length;
  await installShell(s.page);
  await settle(s, 0);
  const m = await pageMetrics(s);
  const all = traffic(s.netlog()), critical = traffic(s.netlog().slice(0, readyAt));
  return { ttfbMs: m.ttfb, fcpMs: m.fcp, lcpMs: m.lcp, readyMs: m.marks.welcome, cls: m.cls, blockingMs: blocking(m.longTasks), cpuMs: cpuSince(cpu, await cpuTime()), heapMB: m.heapMB, criticalKB: critical.bytes / 1000, criticalRequests: critical.requests, totalKB: all.bytes / 1000, requests: all.requests, errors: s.errors.length };
};

// The term's catalog as the worker saves it when a student picks the school
// on this device (store.js keys, catalogService.js records).
const savedCatalog = () => [
  [`structure:${fx.inst.id}:${fx.term.id}`, { hash: fx.term.structure_snapshot, value: fx.catalog }],
  [`seats:${fx.inst.id}:${fx.term.id}`, { hash: fx.term.seats_snapshot, value: JSON.parse(fs.readFileSync(fx.file('seats'), 'utf8')) }],
];

// A returning guest: a saved plan, and the shell and catalog saved by earlier visits.
async function returningGuest(profile, extra = {}) {
  const s = await newPage(profile);
  const plan = studentPlan(fx);
  await seed(s.page, [['activeOwner', 'guest'], ['state:guest', guestState(fx, plan, extra)], ...savedCatalog()]);
  await s.page.goto(lab.url + '/');
  await planner(s.page);
  await installShell(s.page);
  await settle(s, 0, { quietMs: 1500 });
  return { s, plan };
}
// Background work of the earlier visit (the shell's installation, an open
// worker) finishes first, so the CPU time is this visit's own.
const idle = () => sleep(3000);
async function revisit(s, { nextDay = false } = {}) {
  if (nextDay) { lab.publish(['seats', 'structure']); await s.cdp.send('Network.clearBrowserCache'); }
  await idle();
  const since = s.netlog().length, cpu = await cpuTime();
  await s.page.goto(lab.url + '/');
  await planner(s.page);
  await settle(s, since, { quietMs: 1500 });
  const m = await pageMetrics(s), entries = s.netlog().slice(since), t = traffic(entries);
  return { fcpMs: m.fcp, lcpMs: m.lcp, readyMs: m.marks.planner, cls: m.cls, blockingMs: blocking(m.longTasks), cpuMs: cpuSince(cpu, await cpuTime()), heapMB: m.heapMB, nodes: m.nodes, requests: t.requests, KB: t.bytes / 1000, settledMs: lastEnd(entries, (e) => e.path !== '/sw.js'), errors: s.errors.length };
}

// Reopening the planner the same day, then the next day once the school's
// seats and classes have been republished.
scenarios['load.repeat'] = async (profile) => (await revisit((await returningGuest(profile)).s));
scenarios['load.nextday'] = async (profile) => (await revisit((await returningGuest(profile)).s, { nextDay: true }));

scenarios['interact.week'] = async (profile) => {
  const { s } = await returningGuest(profile);
  const next = [];
  for (let i = 0; i < 5; i++) next.push(await frames(s.page, `document.querySelector('[aria-label="Next Week"]').click()`));
  const today = await frames(s.page, `document.querySelector('[title^="This Week"]').click()`);
  await tasksShown(s);
  // Ticking tasks done: the local save path, including IndexedDB.
  const tick = [];
  for (let i = 0; i < 3; i++) tick.push(await frames(s.page, `document.querySelector('.tasks .check:not(:checked)').click()`));
  return { nextWeekMs: median(next), todayMs: today, tickTaskMs: median(tick) };
};

scenarios['interact.dialogs'] = async (profile) => {
  const { s } = await returningGuest(profile);
  const settings = await until(s.page, `location.hash = '#settings'`, 'dialog[open] nav, dialog[open] .pages');
  await s.page.keyboard.press('Escape');
  await s.page.waitForFunction(() => !document.querySelector('dialog[open]'));
  await tasksShown(s);
  const task = await until(s.page, `document.querySelector('.tasks .row-btn').click()`, 'dialog[open] input');
  return { settingsMs: settings, taskDialogMs: task };
};

// Class search. First: a new visitor picks a school and goes from the
// welcome pages to the finder; the catalog downloads, parses and
// indexes meanwhile. Later: a returning student looks at the week, opens Edit
// Classes (the catalog saved on this device), then types a search.
scenarios['classes.first'] = async (profile) => {
  const s = await newPage(profile);
  await s.page.goto(lab.url + '/');
  await s.page.waitForFunction(() => window.__perf.marks.welcome, null, { timeout: 60000 });
  const cpu = await cpuTime(), since = s.netlog().length;
  const welcome = s.page.getByRole('region', { name: 'Welcome', exact: true });
  await welcome.getByRole('button', { name: 'Get Started', exact: true }).click();
  const at = await s.page.evaluate(() => performance.now());
  await welcome.getByRole('button', { name: fx.inst.name, exact: true }).click();
  // The last page leads to the class search, whether it offers Canvas or not.
  for (let step = 0; step < 4; step++) {
    const search = welcome.getByRole('button', { name: /^(Search Instead|Search Classes)$/ });
    if (await search.isVisible()) { await search.click(); break; }
    await welcome.getByRole('button', { name: /^(Skip|Continue Without an Account)$/ }).click();
  }
  await s.page.waitForSelector('.results .course', { timeout: 60000 });
  const shown = await s.page.evaluate(() => performance.now());
  await settle(s, since);
  const t = traffic(s.netlog().slice(since));
  return { searchReadyMs: shown - at, KB: t.bytes / 1000, cpuMs: cpuSince(cpu, await cpuTime()) };
};

scenarios['classes.search'] = async (profile) => {
  const { s } = await returningGuest(profile);
  await s.page.goto(lab.url + '/');
  await planner(s.page);
  // A student looks at the week first, then edits classes.
  await idle();
  const open = await s.page.evaluate(async () => {
    const at = performance.now();
    (document.querySelector('[aria-label="Edit Classes"]') || [...document.querySelectorAll('[role="tab"]')].find((b) => /Classes/.test(b.textContent))).click();
    await new Promise((resolve) => { const check = () => document.querySelector('.results .course') ? resolve() : setTimeout(check, 4); check(); });
    await new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(resolve)));
    return performance.now() - at;
  });
  // A course search typed one key at a time: each key until its results are drawn.
  const keys = [];
  for (const ch of 'calculus') {
    keys.push(await s.page.evaluate(async (ch) => {
      const input = document.querySelector('input[aria-label="Search classes"]'), list = document.querySelector('.results');
      const at = performance.now();
      input.value += ch; input.dispatchEvent(new Event('input', { bubbles: true }));
      // The list is busy from the keystroke until the worker's page is drawn.
      let busy = false;
      await new Promise((resolve) => { const check = () => { busy ||= list.getAttribute('aria-busy') === 'true'; if (busy && list.getAttribute('aria-busy') !== 'true') resolve(); else if (performance.now() - at > 5000) resolve(); else setTimeout(check, 0); }; check(); });
      await new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(resolve)));
      return performance.now() - at;
    }, ch));
    await sleep(150);
  }
  const m = await pageMetrics(s);
  return { openMs: open, keystrokeMs: median(keys), keystrokeMaxMs: Math.max(...keys), heapMB: m.heapMB };
};

// Accounts ------------------------------------------------------------------

function accountDocs(plan) {
  return [
    { id: 's_' + plan.schedule.id, type: 'schedule', body: plan.schedule },
    ...plan.tasks.map((t) => ({ id: 't_' + t.id, type: 'task', body: t })),
    { id: 'prefs', type: 'prefs', body: { inst: fx.inst.id, term: fx.term.id, current: plan.schedule.id, onboarded: true, reminders: false, map: {} } },
  ];
}

// A new device signs in to an account that holds a plan, forty tasks and a
// Canvas connection with 150 assignments. The code check is answered by the
// lab; everything after it is the real sign-in path against the real API.
async function signIn(profile) {
  const plan = studentPlan(fx);
  const account = await api.call('/__perf/account', { docs: accountDocs(plan), canvas: true });
  const s = await newPage(profile, { cookies: [{ name: 'hn_session', value: account.token }] });
  const me = { id: account.id, phone: account.phone, device: account.device };
  // A guest who finished the welcome pages without planning anything yet.
  await seed(s.page, [['activeOwner', 'guest'], ['state:guest', guestState(fx, { schedule: { ...plan.schedule, id: 'guest-schedule', sections: [], pinned: {} }, tasks: [] })]]);
  await s.page.route('**/api/auth/code', (route) => route.fulfill({ status: 202, json: { retry_after: 30 } }));
  await s.page.route('**/api/auth/verify', (route) => route.fulfill({ json: me }));
  await s.page.goto(lab.url + '/');
  await s.page.waitForFunction(() => !document.querySelector('.app-content')?.inert && document.querySelector('.headerbar'));
  await installShell(s.page);
  await settle(s, 0);
  await s.page.evaluate(() => { location.hash = '#settings/account'; });
  await s.page.getByRole('dialog', { name: 'Settings' }).getByRole('button', { name: 'Sign In', exact: true }).click();
  const dialog = s.page.getByRole('dialog', { name: 'Sign In', exact: true });
  await dialog.getByLabel('Mobile number', { exact: true }).fill('2015550123');
  await dialog.getByRole('button', { name: 'Send Code', exact: true }).click();
  const since = s.netlog().length, cpu = await cpuTime();
  const verified = await s.page.evaluate(() => performance.now());
  await s.page.getByRole('dialog', { name: 'Enter the Code', exact: true }).getByLabel('Code', { exact: true }).fill('123456');
  // Signed in once the account's schedule is on the week.
  await s.page.waitForFunction(() => document.querySelectorAll('.cal .ev, .day .ev').length > 0, null, { timeout: 60000 });
  const shown = await s.page.evaluate(() => performance.now());
  await settle(s, since);
  const entries = s.netlog().slice(since), t = traffic(entries);
  return { s, plan, account, me, signin: { scheduleMs: shown - verified, settledMs: lastEnd(entries), syncRequests: t.paths.filter((x) => x.includes('/api/me/sync')).length, apiRequests: t.apiRequests, apiKB: t.apiBytes / 1000, cpuMs: cpuSince(cpu, await cpuTime()) } };
}

scenarios['sync.signin'] = async (profile) => (await signIn(profile)).signin;

// A returning signed-in student opens the planner.
async function returningAccount(profile) {
  const signed = await signIn(profile);
  await signed.s.page.goto(lab.url + '/');
  await planner(signed.s.page);
  await settle(signed.s, 0, { quietMs: 1500 });
  return signed;
}

scenarios['load.account'] = async (profile) => {
  const { s } = await returningAccount(profile);
  const since = s.netlog().length, cpu = await cpuTime();
  await s.page.goto(lab.url + '/');
  await planner(s.page);
  await settle(s, since, { quietMs: 1500 });
  const m = await pageMetrics(s), entries = s.netlog().slice(since), t = traffic(entries);
  return { readyMs: m.marks.planner, blockingMs: blocking(m.longTasks), cpuMs: cpuSince(cpu, await cpuTime()), requests: t.requests, apiRequests: t.apiRequests, apiKB: t.apiBytes / 1000, settledMs: lastEnd(entries), heapMB: m.heapMB };
};

// Coming back to the tab after another window had focus.
scenarios['sync.resume'] = async (profile) => {
  const { s } = await returningAccount(profile);
  const since = s.netlog().length, cpu = await cpuTime();
  await s.page.evaluate(() => { window.dispatchEvent(new Event('blur')); setTimeout(() => window.dispatchEvent(new Event('focus')), 50); });
  await sleep(300);
  await s.page.waitForFunction(() => !document.querySelector('.app-content')?.inert);
  await settle(s, since, { quietMs: 1500 });
  const entries = s.netlog().slice(since), t = traffic(entries);
  return { apiRequests: t.apiRequests, apiKB: t.apiBytes / 1000, settledMs: lastEnd(entries), cpuMs: cpuSince(cpu, await cpuTime()) };
};

// A ticked task reaches the server; another device's edit reaches this page.
scenarios['sync.edits'] = async (profile) => {
  const { s, account, plan } = await returningAccount(profile);
  await tasksShown(s);
  const since = s.netlog().length;
  const clicked = Date.now();
  // A task of the student's own; Canvas assignments also report to Canvas.
  await s.page.locator('.tasks .row', { hasText: 'Problem set 7' }).locator('.check').check();
  let uploaded;
  for (const deadline = Date.now() + 15000; !uploaded && Date.now() < deadline; await sleep(10)) {
    uploaded = s.netlog().slice(since).find((e) => e.method === 'POST' && e.path === '/api/me/sync' && e.end);
  }
  const tickToServerMs = uploaded ? Date.now() - clicked : null;
  // A tick just before switching to another app, which hides the page.
  const away = s.netlog().length, left = Date.now();
  await s.page.locator('.tasks .row', { hasText: 'Problem set 8' }).locator('.check').check();
  await s.page.evaluate(() => setTimeout(() => window.dispatchEvent(new Event('blur')), 50));
  let sentAway;
  for (const deadline = Date.now() + 5000; !sentAway && Date.now() < deadline; await sleep(10)) {
    sentAway = s.netlog().slice(away).find((e) => e.method === 'POST' && e.path === '/api/me/sync' && e.end);
  }
  const awayToServerMs = sentAway ? Date.now() - left : null;
  await s.page.evaluate(() => window.dispatchEvent(new Event('focus')));
  await s.page.waitForFunction(() => !document.querySelector('.app-content')?.inert);
  const bearer = { Authorization: `Bearer ${account.token}`, 'Content-Type': 'application/json' };
  const pulled = await (await fetch(api.url + '/api/me/sync', { method: 'POST', headers: bearer, body: JSON.stringify({ since: 0, docs: [] }) })).json();
  // A task due in a few days, so it is listed without expanding Later.
  const doc = pulled.docs.find((d) => d.id === 't_' + plan.tasks[9].id);
  const title = `Renamed elsewhere ${Date.now()}`;
  const shown = s.page.waitForFunction((title) => document.body.innerText.includes(title), title, { timeout: 20000, polling: 10 }).then(() => Date.now());
  const sent = Date.now();
  await fetch(api.url + '/api/me/sync', { method: 'POST', headers: bearer, body: JSON.stringify({ since: pulled.seq, docs: [{ id: doc.id, type: 'task', base: doc.rev, updated: Date.now(), body: { ...doc.body, title } }] }) });
  return { tickToServerMs, awayUploaded: sentAway ? 1 : 0, awayToServerMs, remoteEditMs: (await shown) - sent };
};

// Canvas ----------------------------------------------------------------------

const canvasLab = () => api.call('/__perf/lab');
const toast = (page, pattern, timeout = 60000) => page.waitForFunction((source) => [...document.querySelectorAll('.toast .tt')].some((t) => new RegExp(source).test(t.textContent)), pattern.source, { timeout, polling: 10 });

// A guest's Sync Now through the relay: assignments first shown, then complete.
scenarios['canvas.guest'] = async (profile) => {
  const { canvas } = await canvasLab();
  const { s } = await returningGuest(profile, { tasks: [], lms: { canvas: { base: canvas, token: 'perf-lab-token-long-enough', synced: Date.now() }, feeds: [], map: {} } });
  await tasksShown(s);
  const before = (await canvasLab()).canvas_requests;
  const result = await s.page.evaluate(async () => {
    const at = performance.now(), button = () => document.querySelector('[aria-label="Sync assignments"]');
    button().click();
    let first = 0;
    await new Promise((resolve) => { const check = () => { if (!first && document.querySelector('.tasks .row')) first = performance.now() - at; if (performance.now() - at > 50 && button()?.getAttribute('aria-busy') !== 'true') resolve(); else setTimeout(check, 4); }; check(); });
    return { first, complete: performance.now() - at };
  });
  const after = await canvasLab();
  return { firstTasksMs: result.first || result.complete, completeMs: result.complete, canvasRequests: after.canvas_requests - before, _paths: after.canvas_paths.slice(-(after.canvas_requests - before)) };
};

// An account's Sync Now: the server imports from Canvas, the page downloads the result.
scenarios['canvas.account'] = async (profile) => {
  const { s } = await returningAccount(profile);
  await tasksShown(s);
  await s.page.waitForSelector('[aria-label="Sync assignments"]:not([aria-busy="true"])');
  const before = (await canvasLab()).canvas_requests, since = s.netlog().length;
  const at = await s.page.evaluate(() => { document.querySelector('[aria-label="Sync assignments"]').click(); return performance.now(); });
  await toast(s.page, /^Synced \d+ assignments/);
  const done = await s.page.evaluate(() => performance.now());
  await settle(s, since);
  const t = traffic(s.netlog().slice(since));
  return { syncNowMs: done - at, apiRequests: t.apiRequests, apiKB: t.apiBytes / 1000, canvasRequests: (await canvasLab()).canvas_requests - before };
};

// Static weight ---------------------------------------------------------------

function pageWeight() {
  const served = (file) => { const p = path.join(dist, file); return fs.existsSync(p + '.br') ? fs.statSync(p + '.br').size : brotli(fs.readFileSync(p), 4); };
  const html = fs.readFileSync(path.join(dist, 'index.html'), 'utf8');
  const entry = [...new Set([...html.matchAll(/(?:src|href)="\/(assets\/[^"]+\.(?:js|css))"/g)].map((m) => m[1]))];
  const fonts = [...html.matchAll(/href="\/(fonts\/[^"]+)"/g)].map((m) => m[1]);
  const sw = fs.readFileSync(path.join(dist, 'sw.js'), 'utf8');
  const shell = JSON.parse(sw.match(/^const PRECACHE = (\[.*\]);$/m)[1]).map((u) => (u === '/' ? 'index.html' : u.slice(1)));
  const js = entry.filter((f) => f.endsWith('.js')), css = entry.filter((f) => f.endsWith('.css'));
  const sum = (files) => files.reduce((n, f) => n + served(f), 0) / 1000;
  const raw = (files) => files.reduce((n, f) => n + fs.statSync(path.join(dist, f)).size, 0) / 1000;
  const lazy = shell.filter((f) => f.startsWith('assets/') && !entry.includes(f));
  const publication = (kind) => { const f = fx.file(kind); return fs.existsSync(f) ? brotli(fs.readFileSync(f), 6) / 1000 : 0; };
  return {
    htmlKB: served('index.html') / 1000, entryJsKB: sum(js), entryJsRawKB: raw(js), entryCssKB: sum(css), fontKB: sum(fonts),
    criticalKB: served('index.html') / 1000 + sum(entry) + sum(fonts) + brotli(Buffer.from(JSON.stringify(fx.config)), 4) / 1000,
    lazyJsKB: sum(lazy), shellKB: sum([...shell, 'sw.js']), shellFiles: shell.length + 1,
    configKB: brotli(Buffer.from(JSON.stringify(fx.config)), 4) / 1000,
    catalogKB: publication('structure'), catalogRawKB: fs.statSync(fx.file('structure')).size / 1000, seatsKB: publication('seats'), descriptionsKB: publication('desc'),
  };
}

// Report ------------------------------------------------------------------------

function summarize(results) {
  const rows = {};
  for (const [key, list] of Object.entries(results)) {
    const names = [...new Set(list.flatMap((r) => Object.keys(r)))].filter((n) => !n.startsWith('_'));
    rows[key] = Object.fromEntries(names.map((n) => [n, median(list.map((r) => r[n]))]));
  }
  return rows;
}

function report(summary, previous) {
  const fmt = (v) => (v === null || v === undefined ? '–' : Math.abs(v) >= 100 ? Math.round(v).toString() : (+v.toFixed(2)).toString());
  const lines = [`# Honorum performance`, '', `${new Date().toISOString()} · ${runs} run(s) · git ${gitHead()} · fixture ${fx.inst.id} ${fx.term.id} (${fx.catalog.courses.length} courses)`, ''];
  lines.push(previous ? '| Measure | Before | After | Change |' : '| Measure | Value |', previous ? '| --- | ---: | ---: | ---: |' : '| --- | ---: |');
  for (const [key, row] of Object.entries(summary)) {
    for (const [name, value] of Object.entries(row)) {
      if (!previous) { lines.push(`| ${key} · ${name} | ${fmt(value)} |`); continue; }
      const before = previous[key]?.[name];
      const change = before ? `${value - before > 0 ? '+' : ''}${(((value - before) / before) * 100).toFixed(0)}%` : '';
      lines.push(`| ${key} · ${name} | ${fmt(before)} | ${fmt(value)} | ${change} |`);
    }
  }
  return lines.join('\n') + '\n';
}
function gitHead() { try { return execFileSync('git', ['rev-parse', '--short', 'HEAD'], { cwd: root }).toString().trim() + (execFileSync('git', ['status', '--porcelain'], { cwd: root }).toString().trim() ? '+dirty' : ''); } catch { return 'unknown'; } }

// Main ---------------------------------------------------------------------------

fs.mkdirSync(out, { recursive: true });
await ensureFixture();
fx = readFixture();
api = await startApi();
lab = await startLab({ dist, fixture: fixtureDir, api: api.url, phoneAuth: true });
browser = await chromium.launch({ args: [lab.flag], executablePath: process.env.HONORUM_TEST_BROWSER || undefined });
// The browser's own first navigation is slower than any later one; spend it here.
{ const s = await newPage('desktop'); await s.page.goto(lab.url + '/'); await s.page.waitForFunction(() => window.__perf.marks.welcome); await s.context.close(); }
const results = { weight: [pageWeight()] };
try {
  for (const [name, run] of Object.entries(scenarios)) {
    if (only && !only.some((o) => name === o || name.startsWith(o + '.'))) continue;
    for (const profile of profiles) {
      const key = `${name} (${profile})`;
      results[key] = [];
      for (let i = 0; i < runs; i++) {
        try {
          current = `${name}-${profile}-${i + 1}`; currentProfile = profile;
          const r = await run(profile);
          results[key].push(r);
          console.log(key, JSON.stringify(Object.fromEntries(Object.entries(r).filter(([k]) => !k.startsWith('_')).map(([k, v]) => [k, typeof v === 'number' ? Math.round(v * 100) / 100 : v]))));
        } catch (error) {
          console.error(`${key} run ${i + 1} failed: ${error.message.split('\n')[0]}`);
        } finally {
          for (const context of [...contexts]) await context.close().catch(() => {});
        }
      }
    }
  }
} finally {
  await browser.close(); await lab.close(); api.close();
}
const summary = summarize(results);
const previous = args.compare ? JSON.parse(fs.readFileSync(path.resolve(args.compare), 'utf8')).summary : null;
fs.writeFileSync(path.join(out, 'results.json'), JSON.stringify({ git: gitHead(), runs, profiles, summary, results }, null, 2));
fs.writeFileSync(path.join(out, 'report.md'), report(summary, previous));
console.log(`\n${report(summary, previous)}\nWrote ${path.join(out, 'report.md')}`);
