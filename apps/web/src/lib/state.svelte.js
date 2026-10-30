// Application state (Svelte 5 runes) and the actions that change it. Views
// read `app` and call these; nothing here knows how anything is drawn.
import { createActivity } from './activity.js';
import { coordinateTabs } from './ownership.js';
import { solveSchedule } from './solverClient.js';
import { campusNow, closureCancels } from './occurrences.js';
import { get, set, onStorageReset } from './store.js';
import { loadCatalog, onCatalog, getSections, getCourses } from './catalogClient.ts';
import { api, setAccountOwner, pushSubscription, currentPushSubscription, syncPush, syncPull, newSecret, shareLink as makeShareLink, deviceName } from './api.js';
import { initSync, noteChange, syncSoon, sync, adopt, resetSync, suspendSync, pending } from './sync.js';
import { rid, compact, DAYS } from './schedule.js';
import { orderTerms, defaultTerm, defaultWeek, addDays, ymd } from './dates.js';
import { createTaskActions } from './taskActions.svelte.js';
import { recordTerm } from './rings.js';
import { toast, toasts, alert, taskView } from './ui.svelte.js';
import { useGoogle, refreshGoogleEvents, finishGoogle, noteGoogleVersion, showingGoogle } from './google.svelte.js';

import { app, KEYS, IDLE_CAT, pref, COLORS, EVENT_COLOR } from './model.svelte.js';
export { app, COLORS, EVENT_COLOR };
setAccountOwner(() => app.account?.id || '');
const activity = createActivity({ app, request: api });
onStorageReset(() => {
  app.storageCleared = true;
  app.editable = false;
  clearTimeout(reminderTimer);
  ownership?.close();
  suspendLMS();
  void suspendSync().catch(() => {});
  clearAccountView();
});

// Navigation
// Editing the schedule's classes: a mode on wide screens, the Classes page
// on phones. focus puts the caret in the search entry.
export function openClasses(focus = true) {
  app.view = 'classes';
  ensureCatalog();
  if (focus) app.focusFind++;
}
export function closeClasses() {
  app.view = 'week';
  app.selected = '';
}
// date: the occurrence that was clicked, as yyyy-mm-dd, when there is one.
export function select(id, date = '') {
  app.selected = id;
  app.selectedDate = id ? date : '';
  if (id) setPane(true);
}
export function setPane(open) {
  app.pane = open;
  try { localStorage.setItem('pane', open ? 'open' : 'closed'); } catch {}
}
export function setTaskTimes(on) {
  app.taskTimes = on;
  try { localStorage.setItem('taskTimes', on ? 'on' : 'off'); } catch {}
}

// Restore local plans before making network requests.
let ownership;
export async function boot() {
  let initial = true;
  ownership = coordinateTabs({
    acquired: async () => {
      if (initial) { await bootOwned(); initial = false; return; }
      const saved = await get('state');
      if (!saved) { clearAccountView(); location.reload(); return; }
      const prev = [app.inst, app.term]; restore(saved); const next = [app.inst, app.term]; app.inst = prev[0]; app.term = prev[1]; selectTerm(...next);
      await initSync(app, syncHooks);
      await useGoogle(app.account?.id || '');
    },
    releasing: async () => {
      suspendLMS(true);
      cancelSeatRefresh(false);
      if (app.storageCleared) { await suspendSync().catch(() => {}); return; }
      await persist();
      // Edits still waiting to upload leave now rather than when this tab next
      // edits: a phone may not run a hidden page again for hours.
      if (app.account && pending()) await Promise.race([sync(), new Promise((resolve) => setTimeout(resolve, 2000))]);
      await suspendSync();
    },
    changed: (editable, error) => { app.editable = !app.storageCleared && editable; if (error) app.storageError = error.message; if (app.editable) { activity.visit(); if (app.account) syncSoon(0); refreshSeats(); refreshLMS().catch(() => {}); refreshReminders(); refreshGoogle(); } },
  });
  await ownership.ready;
}

async function bootOwned() {
  const saved = await get('state').catch((e) => { toast(`Local storage: ${e.message}`); return null; });
  const state = saved || {};
  const hash = location.hash.slice(1);
  const params = new URLSearchParams(hash);
  const share = params.get('share');
  if (share) history.replaceState(null, '', location.pathname);
  restore(state);
  await initSync(app, syncHooks);
  await useGoogle(app.account?.id || '');
  // Seat alert links: #inst/term/section.
  const deep = hash.match(/^([^/=]+)\/([^/]+)\/([^/]+)$/);
  if (deep) [, app.inst, app.term] = deep;
  if (app.inst) {
    selectTerm(app.inst, app.term);
    app.loading = false;
  }
  if (app.config) refreshConfig().catch(() => {});
  else await refreshConfig();
  const insts = app.config?.institutions || [];
  // A device that already holds plans skips the welcome pages.
  if (!app.onboarded && (app.schedules.some((s) => s.sections.length || s.blocks.length) || app.tasks.length || app.watches.length)) app.onboarded = true;
  const inst = insts.find((i) => i.id === app.inst) || insts[0];
  if (inst) {
    const term = inst.terms.some((t) => t.id === app.term) ? app.term : defaultTerm(inst.terms)?.id || '';
    if (inst.id !== app.inst || term !== app.term) selectTerm(inst.id, term);
  }
  app.loading = false;
  if (deep) select(deep[3]);
  if (app.account) syncSoon(0);
  if (share) {
    const shared = await syncPull(share).catch(() => null);
    if (shared?.share) {
      selectTerm(shared.inst, shared.term);
      importShared(shared.share);
    }
  }
  reconcilePush().catch(() => {});
  setInterval(() => { if (app.editable && document.visibilityState === 'visible') refreshSeats(); }, 120000);
  setInterval(() => { if (app.editable && document.visibilityState === 'visible') { syncSoon(0); activity.visit(); refreshLMS().catch(() => {}); refreshReminders(); if (showingGoogle()) refreshGoogle(); } }, 60000);
  document.addEventListener('visibilitychange', () => { if (app.editable && document.visibilityState === 'visible') { refreshSeats(); syncSoon(0); activity.visit(); refreshLMS().catch(() => {}); refreshReminders(); refreshGoogle(); } });
  window.addEventListener('online', () => { if (app.editable) { refreshSeats(); syncSoon(0); activity.visit(); refreshLMS().catch(() => {}); refreshReminders(); refreshGoogle(); } });
  activity.visit();
  refreshLMS().catch(() => {});
  // Returning from Google's consent finishes connecting; otherwise the
  // events are checked once.
  finishGoogle().then((notice) => { if (notice) toast(notice); else refreshGoogle(); });
}

// Google Calendar events, for accounts on servers that offer it.
function refreshGoogle() {
  if (app.account && app.config?.google_calendar) refreshGoogleEvents().catch(() => {});
}

function restore(saved) {
  for (const k of KEYS) if (saved[k] !== undefined) app[k] = saved[k];
  app.lms.map ??= {};
  app.prefExtras = saved.prefExtras || {};
}

let viewKey = ''; // inst/term the views are showing
export function selectTerm(inst, term) {
  const cfg = app.config?.institutions.find((i) => i.id === inst);
  if (!term && cfg) term = defaultTerm(cfg.terms)?.id || '';
  const changed = inst !== app.inst || term !== app.term;
  app.inst = inst;
  app.term = term;
  viewKey = inst + '/' + term;
  app.selected = '';
  app.weekOffset = 0;
  if (changed) {
    cancelSeatRefresh();
    app.cat = { ...IDLE_CAT };
    app.cache = {};
    app.seats = {};
  }
  const mine = mySchedules();
  if (!mine.some((s) => s.id === app.current)) mine.length ? (app.current = (mine.find((s) => s.primary) || mine[0]).id) : newSchedule();
  if (mine.some((s) => s.sections.some((id) => !s.pinned?.[id]))) ensureCatalog();
  refreshSeats();
}

// revalidate bypasses browser and edge caches, for when a newer publication
// is expected rather than merely possible.
export async function refreshConfig(revalidate = false) {
  try {
    app.config = await api('/config', revalidate ? { cache: 'no-cache' } : {});
    for (const i of app.config.institutions) i.terms = orderTerms(i.terms);
    const info = termInfo();
    if (app.cat.status === 'ready' && info?.structure_snapshot && info?.seats_snapshot && app.cat.hash !== `${info.structure_snapshot}:${info.seats_snapshot}`) {
      loadCatalog(app.inst, app.term, info.structure_snapshot, info.seats_snapshot);
    }
    app.online = true;
  } catch {
    app.online = false;
  }
}

export const institution = () => app.config?.institutions.find((i) => i.id === app.inst);
// Class times and due times are both shown on the campus clock.
export const campusZone = () => institution()?.timezone || '';

// Days classes do not meet at the school (holidays, breaks, reading days),
// as date → { name, follows, after }; follows names the weekday whose classes
// meet instead (a Tuesday that runs Monday's schedule), and after cancels only
// classes starting at or after that minute. Built once per school.
let closureCache = { key: '', map: {} };
export function closures() {
  const inst = institution();
  const key = inst ? inst.id + ':' + JSON.stringify(inst.closures || []) : '';
  if (closureCache.key === key) return closureCache.map;
  const map = {};
  for (const c of inst?.closures || []) {
    for (const d = new Date(c.from + 'T00:00:00'); ymd(d) <= c.to; d.setDate(d.getDate() + 1)) map[ymd(d)] = { name: c.name, follows: c.follows || '', after: c.after || 0 };
  }
  closureCache = { key, map };
  return map;
}
export const termInfo = () => { const inst = institution(); const term = inst?.terms.find((t) => t.id === app.term); return term ? { ...term, timezone: inst.timezone || '' } : undefined; };
// The first day of the week the calendar shows.
export const firstDay = () => addDays(defaultWeek(termInfo(), campusNow(institution()?.timezone), app.weekStart), app.weekOffset * 7);

// Catalog
export function warmCatalog() {
  (window.requestIdleCallback || ((fn) => setTimeout(fn, 1500)))(() => ensureCatalog());
}

export function ensureCatalog() {
  if (!app.inst || !app.term || app.cat.status === 'loading' || app.cat.status === 'ready') return;
  app.cat.status = 'loading';
  app.cat.progress = 0;
  const info = termInfo();
  loadCatalog(app.inst, app.term, info?.structure_snapshot || '', info?.seats_snapshot || '');
}

onCatalog((m) => {
  if (m.key !== `${app.inst}/${app.term}`) return;
  if (m.type === 'progress') app.cat.progress = m.total ? Math.min(1, m.loaded / m.total) : 0;
  else if (m.type === 'ready') {
    app.cat = { status: 'ready', progress: 1, hash: m.hash, attrs: m.attrs, count: m.courses };
    backfill().catch((e) => toast(e.message));
    if (m.term) { const inst = institution(); const term = inst?.terms.find((t) => t.id === app.term); if (term) Object.assign(term, m.term); }
  } else app.cat.status = m.type; // missing | error
});

// A section missing its saved copy fills in from the catalog, and saved
// copies pick up the exam sittings the school has published since they
// were added.
async function backfill() {
  const ids = new Set(mySchedules().flatMap((s) => s.sections));
  if (!ids.size) return;
  for (const sec of await getSections([...ids])) {
    app.cache[sec.id] = sec;
    for (const s of mySchedules()) if (s.sections.includes(sec.id) && !s.pinned?.[sec.id]) pin(s, sec);
    refreshExams(sec.id, sec.exams);
  }
}
function refreshExams(id, published) {
  const exams = compact({ exams: published }).exams;
  for (const s of mySchedules()) {
    const saved = s.sections.includes(id) && s.pinned?.[id];
    if (saved && JSON.stringify(exams) !== JSON.stringify(saved.exams || [])) saved.exams = exams;
  }
}

// Live seat counts for scheduled and watched sections. A handover to another
// tab keeps them fresh; a new term or owner starts over.
let seatJob, seatFreshKey = '', seatFreshAt = 0;
function cancelSeatRefresh(forget = true) { seatJob?.controller.abort(); seatJob = undefined; if (forget) seatFreshKey = ''; }
export function refreshSeats() {
  if (!app.editable && !app.loading) return Promise.resolve();
  const watched = app.watches.filter((w) => w.inst === app.inst && w.term === app.term).map((w) => w.section);
  const ids = [...new Set([...mySchedules().flatMap((s) => s.sections), ...watched])].sort();
  if (!ids.length || !app.inst || !app.term) return Promise.resolve();
  const requestKey = `${app.dataOwner || 'guest'}/${app.inst}/${app.term}/${ids.join(',')}`;
  if (seatJob?.key === requestKey) return seatJob.promise;
  if (seatFreshKey === requestKey && Date.now() - seatFreshAt < 120000) return Promise.resolve();
  seatJob?.controller.abort();
  const job = seatJob = { key: requestKey, controller: new AbortController() };
  job.promise = (async () => {
    try {
      const list = await api(`/seats?inst=${encodeURIComponent(app.inst)}&term=${encodeURIComponent(app.term)}&ids=${encodeURIComponent(ids.join(','))}`, { signal: job.controller.signal });
      if (seatJob !== job) return;
      for (const x of list) app.seats[x.id] = { available: x.available, capacity: x.capacity, waitlist: x.waitlist, fetched_at: x.fetched_at };
      // Exam sittings the school published since a section was saved arrive
      // with its seats, so the planner never needs the catalog for them.
      for (const x of list) if (x.exams) refreshExams(x.id, x.exams);
      seatFreshKey = requestKey; seatFreshAt = Date.now(); app.online = true;
    } catch {} finally { if (seatJob === job) seatJob = undefined; }
  })();
  return job.promise;
}

// Schedules
export const schedule = () => app.schedules.find((s) => s.id === app.current);
export const mySchedules = () => app.schedules.filter((s) => s.inst === app.inst && s.term === app.term);
export const watchOf = (id) => app.watches.find((w) => w.inst === app.inst && w.term === app.term && w.section === id);

// A section as the page knows it: the schedule's copy or one it has looked
// at, with live seat counts layered on top.
export function section(id) {
  const base = schedule()?.pinned?.[id] || app.cache[id];
  if (!base) return undefined;
  const live = app.seats[id];
  return live ? { ...base, ...live } : base;
}

const pin = (s, sec) => {
  s.pinned ??= {};
  s.pinned[sec.id] = compact(sec);
};

export function newSchedule(name, from) {
  const s = {
    id: rid(), name: name || `Schedule ${mySchedules().length + 1}`, inst: app.inst, term: app.term,
    primary: !mySchedules().some((x) => x.primary),
    sections: from ? [...from.sections] : [], pinned: from ? { ...(from.pinned || {}) } : {}, removed: from ? { ...(from.removed || {}) } : {},
    blocks: from ? from.blocks.map((b) => ({ ...b, id: rid() })) : [], colors: from ? { ...from.colors } : {},
  };
  app.schedules.push(s);
  app.current = s.id;
}

export function setPrimary(id) {
  for (const s of mySchedules()) s.primary = s.id === id;
}

// Delete the current schedule; returns an undo function.
export function deleteSchedule() {
  const snap = $state.snapshot(schedule());
  const had = mySchedules().map((s) => s.id);
  app.schedules = app.schedules.filter((s) => s.id !== app.current);
  app.current = '';
  app.selected = '';
  selectTerm(app.inst, app.term);
  const created = mySchedules().filter((s) => !had.includes(s.id)).map((s) => s.id); // a blank one made to stand in
  return () => {
    app.schedules = app.schedules.filter((s) => !created.includes(s.id));
    app.schedules.push(snap);
    if (snap.primary) setPrimary(snap.id);
    app.current = snap.id;
  };
}

function importShared(sh) {
  const s = { ...sh, id: rid(), name: `${sh.name} (shared)`, primary: false, colors: sh.colors || {}, pinned: sh.pinned || {}, blocks: (sh.blocks || []).map((b) => ({ ...b, id: rid() })) };
  app.schedules.push(s);
  app.current = s.id;
}

// A read-only copy, encrypted with a fresh secret the server never sees.
export async function shareLink() {
  const secret = newSecret();
  const s = $state.snapshot(schedule());
  await syncPush(secret, { share: { name: s.name, inst: s.inst, term: s.term, sections: s.sections, pinned: s.pinned || {}, blocks: s.blocks, colors: s.colors || {} }, inst: s.inst, term: s.term });
  return makeShareLink(secret);
}

// Add or remove a section. Adding takes the section object so the schedule
// keeps its own copy; returns true when it was added.
export function toggleSection(x) {
  const s = schedule();
  const id = typeof x === 'string' ? x : x.id;
  const i = s.sections.indexOf(id);
  if (i >= 0) {
    s.sections.splice(i, 1);
    if (s.pinned) delete s.pinned[id];
    return false;
  }
  const sec = typeof x === 'object' ? x : section(id);
  if (!sec) return false;
  s.sections.push(id);
  pin(s, sec);
  app.cache[id] = sec;
  return true;
}

// Classes found in Canvas: the picked sections join the found term's open
// schedule (made if the term has none), and each Canvas course is matched
// to its class, so its assignments take the class's color. picks are
// [{ course, code, ids }]; returns how many sections were added.
export function addFoundClasses(term, picks, sections) {
  if (term && term !== app.term && institution()?.terms.some((t) => t.id === term)) selectTerm(app.inst, term);
  const s = schedule();
  if (!s) return 0;
  let added = 0;
  for (const { course, code, ids } of picks) {
    for (const id of ids) {
      const sec = sections[id];
      if (!sec || s.sections.includes(id)) continue;
      s.sections.push(id);
      pin(s, sec);
      app.cache[id] = sec;
      added++;
    }
    const connection = app.tasks.find((t) => t.source === 'canvas' && t.courseId === course.id)?.canvasConnection || '';
    mapCourse(course.id, code, connection);
  }
  refreshSeats();
  return added;
}

export function applySections(secs, scheduleId = app.current) {
  const s = app.schedules.find((s) => s.id === scheduleId);
  if (!s) return;
  s.sections = secs.map((x) => x.id);
  s.pinned = Object.fromEntries(secs.map((x) => [x.id, compact(x)]));
}

// Remove one day of a section or event from the schedule; returns an undo.
export function removeDay(id, date) {
  const s = schedule();
  s.removed ??= {};
  s.removed[id] = [...(s.removed[id] || []), date].sort();
  return () => {
    const cur = app.schedules.find((x) => x.id === s.id);
    if (!cur?.removed?.[id]) return;
    cur.removed[id] = cur.removed[id].filter((d) => d !== date);
    if (!cur.removed[id].length) delete cur.removed[id];
  };
}

export function removeBlock(id) {
  const s = schedule();
  s.blocks = s.blocks.filter((b) => b.id !== id);
}

export function colorOf(course) {
  const s = schedule();
  if (s?.colors?.[course]) return s.colors[course];
  const codes = [...new Set((s?.sections || []).map((id) => s.pinned?.[id]?.course).filter(Boolean))];
  return COLORS[Math.max(codes.indexOf(course), 0) % COLORS.length];
}
export function setColor(course, c) {
  const s = schedule();
  s.colors = { ...(s.colors || {}), [course]: c };
}

// A personal event is colored like the class it belongs to, else by the
// label it was given, else in the neutral event color.
export const blockColor = (b) => (b.course ? colorOf(b.course) : b.color || EVENT_COLOR);
const dayLetter = (date) => DAYS[(new Date(date + 'T00:00:00Z').getUTCDay() + 6) % 7];

// Calendar items: sections colored per course, the exam sittings published
// with them (dated, in the class's color), plus personal events.
export const items = () => itemsOf(schedule(), true);
export const previewItems = (sections) => itemsOf({ ...schedule(), sections: sections.map((s) => s.id), pinned: Object.fromEntries(sections.map((s) => [s.id, s])) }, true);
export const itemOf = (sec, color = EVENT_COLOR) => ({ id: sec.id, label: `${sec.course}-${sec.num}`, sub: sec.type, color, meetings: sec.meetings, section: sec });
export const examId = (sectionId, index) => `${sectionId}:exam:${index}`;

function itemsOf(s, colored = false) {
  if (!s) return [];
  const out = [];
  const calendar = closures();
  const term = termInfo() || {};
  for (const id of s.sections) {
    const sec = s.pinned?.[id] || app.cache[id];
    if (!sec) continue;
    const color = colored ? colorOf(sec.course) : undefined;
    out.push({ ...itemOf({ ...sec, start: sec.start || term.start, end: sec.end || term.end }, color), closures: calendar, removed: s.removed?.[id] || [] });
    (sec.exams || []).forEach((e, i) => {
      if (!e.date || !(e.end > e.start)) return;
      const eid = examId(id, i);
      out.push({ id: eid, label: `${sec.course} Exam`, sub: `${sec.course}-${sec.num}`, color, meetings: [{ days: dayLetter(e.date), start: e.start, end: e.end, location: e.location || '' }], date: e.date, exam: e, of: id, removed: s.removed?.[eid] || [] });
    });
  }
  for (const b of s.blocks || []) out.push({ id: b.id, label: b.name, sub: b.course || '', color: colored ? blockColor(b) : undefined, meetings: [b], block: b, removed: s.removed?.[b.id] || [] });
  return out;
}

export const credits = () => (schedule()?.sections || []).reduce((n, id) => n + (section(id)?.credits || 0), 0);

// Every conflict-free way to take the schedule's courses; null when the
// catalog cannot answer (the toast says why).
export async function autoArrange(openOnly) {
  const s = schedule();
  const codes = [...new Set(s.sections.map((id) => section(id)?.course).filter(Boolean))];
  ensureCatalog();
  const courses = await getCourses(codes);
  if (courses.length !== codes.length) {
    toast(app.cat.status === 'ready' ? 'Some of these classes are no longer in the catalog' : 'Classes could not be loaded right now');
    return null;
  }
  const calendar = closures(), term = termInfo() || {};
  const fixed = itemsOf(s).filter((it) => it.block);
  for (const course of courses) for (const sec of course.sections) { sec.start ||= term.start; sec.end ||= term.end; sec.closures = calendar; }
  return solveSchedule(courses, fixed, { openOnly });
}

// Seat alerts
export async function watch(sec) {
  const { account, inst, term } = app;
  const entry = { inst, term, section: sec.id, label: `${sec.course}-${sec.num}`, title: sec.title };
  if (account) {
    // The account delivers to every device with notifications on.
    await registerPush(true);
    const { id } = await api('/watches', { account: account.id, method: 'POST', body: JSON.stringify({ inst, term, section: sec.id }) });
    if (app.account !== account) return;
    if (!app.watches.some((w) => w.id === id)) app.watches.push({ id, ...entry, channel: 'account', endpoint: '' });
    return;
  }
  const target = await pushSubscription(app.config.vapid);
  const body = JSON.stringify({ inst, term, section: sec.id, channel: 'push', target });
  const { id } = await api('/watches', { account: '', method: 'POST', body });
  if (app.account !== account) return;
  rememberPush(target);
  app.watches.push({ id, ...entry, channel: 'push', endpoint: endpointOf(target) });
}

export async function unwatch(w) {
  const account = app.account;
  await api(`/watches/${w.id}`, { account: account?.id || '', method: 'DELETE' });
  if (app.account !== account) return;
  app.watches = app.watches.filter((x) => x.id !== w.id);
}

// This device's push subscription as last given to the server. Device-local
// on purpose: each device has its own subscription and its own alerts.
const endpointOf = (target) => { try { return JSON.parse(target).endpoint || ''; } catch { return ''; } };
function rememberPush(target) {
  try { localStorage.setItem('push', target); } catch {}
}

// Browsers replace subscriptions now and then; make sure the server holds
// the one this browser has now.
export async function reconcilePush() {
  if (app.account) return registerPush(false);
  const prev = pref('push', '');
  if (!prev || !app.config?.vapid || typeof Notification === 'undefined' || Notification.permission !== 'granted') return;
  const prevEndpoint = endpointOf(prev);
  const mine = app.watches.some((w) => w.channel === 'push' && w.endpoint === prevEndpoint);
  if (!mine && !app.reminders.on) return;
  const cur = (await currentPushSubscription()) === prev ? prev : await pushSubscription(app.config.vapid);
  if (cur === prev) return;
  await api('/push/retarget', { account: '', method: 'POST', body: JSON.stringify({ from: prev, to: cur }) });
  if (app.account) return;
  const ep = endpointOf(cur);
  for (const w of app.watches) if (w.channel === 'push' && w.endpoint === prevEndpoint) w.endpoint = ep;
  rememberPush(cur);
}

// Class reminders. A signed-in account's reminders come from its documents on
// the server, which only needs this device's push subscription. A guest
// uploads the primary schedule's meeting pattern and the week's due tasks.
let reminderSent = '', reminderJob;
function refreshReminders() { if (app.reminders.on) syncReminders().catch(() => {}); }
export async function syncReminders() {
  const account = app.account, r = app.reminders;
  if (!r.on) return;
  if (account) { await registerPush(false); return; }
  const prim = mySchedules().find((s) => s.primary) || schedule();
  const term = termInfo() || {};
  const list = [];
  const calendar = closures();
  for (const it of itemsOf(prim)) for (const m of it.meetings) {
    const closed = Object.keys(calendar).filter((date) => closureCancels(calendar[date], m.start));
    const once = it.date || it.block?.date;
    list.push({ id: `${it.id}:${m.days}:${m.start}`, label: it.label, sub: it.sub, location: m.location || '', days: m.days, start: m.start, from: once || it.section?.start || term.start || '', to: once || it.section?.end || term.end || '', skip: [...new Set([...(it.removed || []), ...(it.section ? closed : [])])] });
  }
  for (const it of itemsOf(prim).filter((it) => it.section)) for (const [date, closure] of Object.entries(closures())) {
    if (!closure.follows || it.removed?.includes(date) || (it.section.start && date < it.section.start) || (it.section.end && date > it.section.end)) continue;
    for (const m of it.meetings.filter((m) => m.days.includes(closure.follows))) list.push({ id: `${it.id}:${m.days}:${m.start}`, label: it.label, sub: it.sub, location: m.location || '', days: 'MTWRFSU', start: m.start, from: date, to: date, skip: [] });
  }
  const soon = Date.now() + 7 * 86400e3;
  for (const t of app.tasks) if (!t.done && t.due && new Date(t.due) > new Date() && new Date(t.due) < soon) list.push({ id: t.id, label: t.title, sub: t.course, at: t.due });
  const tz = institution()?.timezone || Intl.DateTimeFormat().resolvedOptions().timeZone;
  const payload = { tz, items: list.slice(0, 100) };
  const signature = JSON.stringify([account?.id || 'guest', r.id, payload]);
  if (reminderSent === signature) return;
  if (reminderJob) { await reminderJob.catch(() => {}); if (app.account === account && app.reminders === r && r.on) return syncReminders(); return; }
  const sending = (async () => {
    if (!r.id) r.id = [...crypto.getRandomValues(new Uint8Array(16))].map((b) => b.toString(16).padStart(2, '0')).join('');
    const target = await pushSubscription(app.config.vapid);
    if (app.account !== account || app.reminders !== r || !r.on) return;
    await api(`/reminders/${r.id}`, { account: '', method: 'PUT', body: JSON.stringify({ target, ...payload }) });
    rememberPush(target);
    if (app.account === account && app.reminders === r && r.on) reminderSent = JSON.stringify([account?.id || 'guest', r.id, payload]);
  })();
  reminderJob = sending;
  try { await sending; } finally { if (reminderJob === sending) reminderJob = undefined; }
}

export async function stopReminders() {
  const account = app.account, reminders = app.reminders;
  reminders.on = false; reminderSent = '';
  if (reminderJob) await reminderJob.catch(() => {});
  if (app.account !== account || app.reminders !== reminders) return;
  try {
    // An account's preference syncs as a document; the server stops deriving.
    if (account) await persist();
    else if (reminders.id) await api(`/reminders/${reminders.id}`, { account: '', method: 'DELETE' });
  } catch (error) { if (app.account === account && app.reminders === reminders) reminders.on = true; throw error; }
}

// Earlier versions of a document the server kept when it was replaced.
export const docHistory = (id) => api(`/me/history/${encodeURIComponent(id)}`);
// Restoring is an ordinary edit: the body becomes the current version.
export function restoreVersion(kind, body) {
  const list = kind === 'schedule' ? app.schedules : app.tasks;
  const i = list.findIndex((x) => x.id === body.id);
  if (i >= 0) list[i] = body; else list.push(body);
  if (kind === 'schedule' && body.inst === app.inst && body.term === app.term) app.current = body.id;
  schedulePersist();
}

export async function requestCode(phone, signal) {
  return api('/auth/code', { method: 'POST', body: JSON.stringify({ phone, consent: '2026-09-08-v2' }), ...(signal ? { signal } : {}) });
}
// Nothing syncs while the server changes who is signed in; a request that
// fails resumes syncing where it was.
async function outsideSync(request) {
  await suspendSync();
  try { return await request(); }
  catch (error) { await initSync(app, syncHooks); syncSoon(0); throw error; }
}
export async function verifyCode(phone, code) {
  await persist();
  await signedIn(await outsideSync(() => api('/auth/verify', { method: 'POST', body: JSON.stringify({ phone, code, device: deviceName() }) })));
}
async function signedIn(me) {
  const saved = await get('state:' + me.id);
  if (saved) {
    const guest = !app.dataOwner ? $state.snapshot(snapshot()) : null;
    restore(saved);
    if (guest) {
      for (const key of ['schedules', 'tasks', 'watches']) {
        const ids = new Set(app[key].map(item => item.id));
        app[key].push(...guest[key].filter(item => !ids.has(item.id) && (key !== 'schedules' || item.sections.length || item.blocks?.length)));
      }
    }
  } else if (app.dataOwner && app.dataOwner !== me.id) {
    // Account data stays in its own local namespace. Only unowned guest data
    // is adopted into a new account.
    clearAccountView();
  }
  app.dataOwner = me.id;
  app.account = { id: me.id, phone: me.phone, device: me.device };
  await adopt();
  await useGoogle(me.id);
  refreshGoogle();
  activity.visit();
  refreshLMS().catch(() => {});
  migrateWatches(me.id).catch((error) => toast(`Seat alerts: ${error.message}`));
  registerPush(false).catch(() => {});
  if (app.reminders.on) syncReminders().catch(() => {});
}
async function migrateWatches(accountID) {
  const watches = app.watches.filter((watch) => watch.channel !== 'account');
  for (const watch of watches) {
    if (app.account?.id !== accountID) return;
    try {
      await api('/watches', { account: accountID, method: 'POST', body: JSON.stringify({ inst: watch.inst, term: watch.term, section: watch.section }) });
      if (app.account?.id !== accountID) return;
      await api(`/watches/${watch.id}`, { account: accountID, method: 'DELETE' });
      if (app.account?.id !== accountID) return;
      app.watches = app.watches.filter((item) => item.id !== watch.id);
    } catch {
      // Keep the device alert active if moving it to the account fails.
    }
  }
  if (watches.length && app.account?.id === accountID) syncSoon(0);
}
function clearAccountView() {
  cancelSeatRefresh(); reminderSent = '';
  app.schedules = []; app.tasks = []; app.watches = []; app.current = '';
  app.lms = { canvas: { base: '', token: '', synced: 0 }, feeds: [], map: {} };
  app.reminders = { id: '', on: false }; app.prefExtras = {};
  app.dataOwner = ''; app.account = null; app.streak = null; app.streakMissed = false;
  app.selected = ''; app.selectedDate = ''; app.editing = '';
  app.taskEditing = ''; app.connect = '';
  toasts.length = 0; alert.open = false; alert.buttons = [];
  taskView.filter = '';
}
async function forget(preserve = true) {
  await suspendSync();
  await useGoogle('');
  if (preserve) {
    // Save unsynced work under its account before switching the active view.
    await persist();
    clearAccountView();
    selectTerm(app.inst, app.term);
  }
  app.account = null; app.streak = null; app.streakMissed = false;
  if (app.lms.canvas.shared) app.lms.canvas = { base: '', token: '', synced: 0 };
  app.lms.feeds = app.lms.feeds.filter((f) => !f.shared);
  app.watches = app.watches.filter((w) => w.channel !== 'account');
  app.reminders.on = false;
  app.sync = { status: 'idle', at: 0, error: '' };
  try { localStorage.removeItem('pushsent'); } catch {}
  await resetSync(false);
  if (!preserve) {
    app.dataOwner = '';
    await resetSync(false);
  }
}
export async function signOut() {
  const account = app.account;
  await persist();
  await outsideSync(() => api('/auth/logout', { account: account?.id || '', method: 'POST' }));
  if (app.account !== account) return;
  await forget();
}
export async function deleteAccount() {
  const account = app.account;
  await outsideSync(() => api('/me', { account: account?.id || '', method: 'DELETE' }));
  try { localStorage.removeItem('activity:' + account.id); localStorage.removeItem('activity-zone:' + account.id); localStorage.removeItem('streak:' + account.id); } catch {}
  set('google:' + account.id, null).catch(() => {});
  if (app.account !== account) return;
  await forget(false);
}
export const loadMe = () => api('/me');
export const removeDevice = (id) => api(`/me/devices/${id}`, { method: 'DELETE' });

// This device's push subscription, kept on its session so account alerts
// and reminders reach it. ask: allowed to prompt for permission.
export async function registerPush(ask) {
  const account = app.account;
  if (!account) return;
  if (!app.config?.vapid) throw new Error('Push notifications are unavailable on this server');
  if (!ask && (typeof Notification === 'undefined' || Notification.permission !== 'granted')) return;
  const sub = await pushSubscription(app.config.vapid);
  const registration = `${account.device}:${sub}`;
  if (app.account !== account || pref('pushsent', '') === registration) return;
  await api('/me/push', { account: account.id, method: 'PUT', body: JSON.stringify({ subscription: sub }) });
  if (app.account === account) try { localStorage.setItem('pushsent', registration); } catch {}
}

// What the sync engine needs from the app.
const syncHooks = {
  snapshot: () => $state.snapshot(snapshot()),
  watches: (list) => {
    const next = [...app.watches.filter((w) => w.channel !== 'account'), ...list.map((w) => ({ ...w, endpoint: '' }))];
    if (JSON.stringify(next) === JSON.stringify(app.watches)) return false;
    app.watches = next; return true;
  },
  conflict: () => toast('Edits were combined. A conflicting version is available in Settings'),
  // The streak depends on the account's tasks, which the server now holds.
  tasksSynced: () => activity.visit(),
  googleVersion: noteGoogleVersion,
  signedOut: async () => { await forget(); toast('Signed out on this device'); },
  // After a pull: follow a school or term the account switched to, and keep a schedule open.
  after: () => {
    const inst = app.inst, term = app.term;
    if (inst + '/' + term !== viewKey) {
      const prev = viewKey.split('/');
      app.inst = prev[0] || '';
      app.term = prev[1] || '';
      selectTerm(inst, term);
    } else if (inst && !mySchedules().some((s) => s.id === app.current)) selectTerm(inst, term);
    refreshSeats();
  },
};

// upload: only when edits are waiting, for work that needs the server to have
// them but nothing from it yet. Each change reaches the outbox as it is made
// (App.svelte), so from a later task, nothing waiting means nothing to write.
export const { saveTask, deleteTask, setTaskDone, taskCourses, taskCourseCode, taskColor, canvasCourses, mapCourse, lmsOf, lmsConnected, connectCanvas, disconnectCanvas, findCanvasClasses, addFeed, removeFeed, syncLMS, refreshLMS, suspendLMS, lmsSync } = createTaskActions({ app, mySchedules, colorOf, EVENT_COLOR, institution, forgetTask: forgetTermTask, syncAccount: async ({ upload = false } = {}) => {
  if (upload && !pending()) return;
  await persist(); await sync();
  if (app.sync.status === 'error' || app.sync.status === 'offline') throw new Error(app.sync.error || 'Account sync is offline');
} });

export const snapshot = () => Object.fromEntries(KEYS.map((k) => [k, app[k]]));

// The This Term ring counts from a record on this device, one per owner,
// school and term, so work stays counted after imports drop it.
const termRecordKey = () => `term-record:${app.dataOwner || 'guest'}:${app.inst}:${app.term}`;
export const taskCode = (t) => taskCourseCode(t) || t.course || '';
export const termBounds = () => { const info = termInfo(); return { start: info?.start || '', end: info?.end || '' }; };
export function termRecord() {
  try { const record = JSON.parse(localStorage.getItem(termRecordKey()) || '{}'); return record && typeof record === 'object' ? record : {}; } catch { return {}; }
}
function saveTermRecord(record) { try { localStorage.setItem(termRecordKey(), JSON.stringify(record)); } catch {} }
function noteTerm() {
  const record = termRecord(), next = recordTerm(record, app.tasks, { ...termBounds(), codeOf: taskCode });
  if (next !== record) saveTermRecord(next);
}
// A task the student deletes leaves the count, however old it is.
function forgetTermTask(t) {
  const record = termRecord(), key = t.uid || t.id;
  if (key in record) { delete record[key]; saveTermRecord(record); }
}
let persistQueued = false;
let reminderTimer;
export function schedulePersist() {
  if (!app.editable) return;
  if (persistQueued) return;
  persistQueued = true;
  queueMicrotask(() => { persistQueued = false; persist().catch((e) => { app.storageError = e.message; toast(`Could not save on this device: ${e.message}`); }); });
}
export async function persist() {
  if (app.storageCleared) return;
  const data = $state.snapshot(snapshot());
  await noteChange(data); app.storageError = ''; // state and the exact pending edits commit atomically
  noteTerm();
  clearTimeout(reminderTimer);
  if (app.reminders.on && app.editable && !app.account) reminderTimer = setTimeout(() => syncReminders().catch((e) => toast(`Reminders: ${e.message}`)), 1500);
}
