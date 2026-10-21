import { api } from './api.js';
import { rid } from './schedule.js';
import { createCourseMatcher } from './tasks.js';
import { createLMSSync } from './lmsSync.js';
import { toast } from './ui.svelte.js';

// Canvas does not distinguish expired tokens from other authentication
// failures. Without a connection, the token is not the likely cause.
function canvasUpdateFailed(e) {
  if (e?.status === 0) { toast(`Canvas was not updated. ${e.message}`); return; }
  toast('Canvas was not updated. Your token may have expired', { label: 'Reconnect', ariaLabel: 'Reconnect Canvas', href: '#settings/assignments' });
}

// Inject planner lookups; this feature has no dependency on the app coordinator.
export function createTaskActions({ app, mySchedules, colorOf, EVENT_COLOR, institution, syncAccount, forgetTask = () => {} }) {
const lmsSync = $state({ owner: null, pending: false, phase: '', error: '' });
const imports = createLMSSync({ app, status: lmsSync, onCanvasComplete: retryCanvasDone, zone: () => institution()?.timezone || '' });
const completionWrites = new Map();
let completionGeneration = 0;
// Tasks
function saveTask(data, id) {
  const t = app.tasks.find((x) => x.id === id);
  if (t) {
    Object.assign(t, data, { courseEdited: t.courseEdited || data.course !== t.course });
    imports.noteEdit(t.uid, data);
  }
  else app.tasks.push({ id: rid(), source: 'manual', done: false, ...data });
}

// Delete a task; returns an undo function.
function deleteTask(id) {
  const i = app.tasks.findIndex((t) => t.id === id);
  if (i < 0) return () => {};
  const snap = $state.snapshot(app.tasks[i]);
  imports.noteDeleted(snap.uid);
  app.tasks.splice(i, 1);
  forgetTask(snap);
  return () => { imports.noteDeleted(snap.uid, false); app.tasks.splice(Math.min(i, app.tasks.length), 0, snap); };
}

// Tick a task. Canvas items are marked done in Canvas as well; the local
// tick never waits on Canvas.
function setTaskDone(t, done) {
  t.done = done;
  // When it was done decides which day it counts toward in the streak.
  if (done) t.completedAt = new Date().toISOString(); else delete t.completedAt;
  imports.noteDone(t.uid, done);
  if (t.source !== 'canvas' || !(app.lms.canvas.token || app.lms.canvas.shared) || !t.uid?.startsWith('canvas:')) return;
  // This intent is saved with the task so a reload or tab handover cannot
  // discard a checkbox change waiting for Canvas's module mapping.
  t.pendingCanvasDone = rid();
  writeCanvasDone(t);
}

function retryCanvasDone() {
  if (!app.editable || !(app.lms.canvas.token || app.lms.canvas.shared)) return;
  for (const t of app.tasks) if (t.source === 'canvas' && t.pendingCanvasDone && !completionWrites.has(t.uid)) writeCanvasDone(t);
}

function writeCanvasDone(t) {
  if (app.lms.canvas.shared) { writeAccountCanvasDone(t); return; }
  const done = t.done, intent = t.pendingCanvasDone;
  const owner = app.lms, connection = owner.canvas;
  const generation = completionGeneration;
  const { base, token } = connection;
  if (t.source !== 'canvas' || !token || !t.uid?.startsWith('canvas:')) return;
  const [, type, id] = t.uid.split(':');
  // A preview may not have its module mapping yet. The local tick is instant;
  // wait for enrichment before writing, and serialize changes to the same item.
  const previous = completionWrites.get(t.uid);
  const write = (async () => {
    await previous;
    await imports.settled();
    if (generation !== completionGeneration || app.lms !== owner || owner.canvas !== connection) return;
    const task = app.tasks.find(x => x.id === t.id);
    if (!task) return;
    const r = await api('/lms/canvas/complete', { method: 'POST', body: JSON.stringify({ base, token, type, id, done, overrideId: task.overrideId || '', courseId: task.courseId || '', moduleItem: task.moduleItem || '' }) });
    if (generation !== completionGeneration || app.lms !== owner || owner.canvas !== connection) return;
    const latest = app.tasks.find(x => x.id === t.id);
    if (!latest) return;
    if (r?.overrideId) latest.overrideId = r.overrideId;
    if (r && r.module === false) toast('Canvas kept the planner in step, but the module’s “Mark as done” could not be changed');
    if (latest.pendingCanvasDone === intent) {
      latest.reason = done ? 'marked' : '';
      if (r?.module !== false) delete latest.pendingCanvasDone;
    }
  })().catch(e => { if (app.lms === owner && owner.canvas === connection) canvasUpdateFailed(e); });
  completionWrites.set(t.uid, write);
  write.finally(() => {
    if (completionWrites.get(t.uid) === write) completionWrites.delete(t.uid);
    if (generation !== completionGeneration) retryCanvasDone();
  });
}

// Course codes on this term's schedules, used to color and filter tasks.
const courses = $derived([...new Set(mySchedules().flatMap((s) => s.sections.map((id) => s.pinned?.[id]?.course).filter(Boolean)))]);
const taskCourses = () => courses;
function taskTitles() {
  const out = {};
  for (const s of mySchedules()) for (const p of Object.values(s.pinned || {})) {
    if (!p.title) continue;
    (out[p.course] ??= []).includes(p.title) || out[p.course].push(p.title);
  }
  return out;
}
const matchTaskCourse = $derived(createCourseMatcher(courses, taskTitles()));

// Which class a task belongs to: the mapping chosen for its LMS course on
// its connection, else a mapping made before connections had identities,
// else the code or title found in the LMS course's code and name.
const mappingKey = (connection, courseId) => (connection ? `${connection}:${courseId}` : courseId);
function taskCourseCode(t) {
  if (t.courseId) {
    const scoped = app.lms.map?.[mappingKey(t.canvasConnection, t.courseId)];
    if (scoped) return scoped;
    if (app.lms.map?.[t.courseId]) return app.lms.map[t.courseId];
  }
  return matchTaskCourse([t.courseCode, t.course].filter(Boolean).join(' '));
}
const taskColor = (t) => { const code = taskCourseCode(t); return code ? colorOf(code) : EVENT_COLOR; };

function canvasCourses() {
  const seen = new Map();
  for (const t of app.tasks) if (t.source === 'canvas' && t.courseId && !seen.has(t.courseId)) seen.set(t.courseId, { id: t.courseId, name: t.course || '', code: t.courseCode || '', connection: t.canvasConnection || '' });
  return [...seen.values()].sort((a, b) => (a.code || a.name).localeCompare(b.code || b.name));
}

// A choice belongs to the connection that supplied the course, so the same
// upstream course id on another Canvas site never inherits it.
function mapCourse(courseId, code, connection = '') {
  app.lms.map ??= {};
  const key = mappingKey(connection, courseId);
  if (code) app.lms.map[key] = code;
  else { delete app.lms.map[key]; delete app.lms.map[courseId]; }
}

const lmsOf = () => institution()?.lms || null;
const lmsConnected = () => !!(app.lms.canvas.token || app.lms.canvas.shared) || app.lms.feeds.length > 0;

// Connect as soon as the planner arrives. Later completion failures can retry
// with the same token; a failure before any assignments restores the old setup.
async function connectCanvas(base, token) {
  const account = app.account;
  if (account) {
    suspendLMS();
    const generation = completionGeneration;
    const current = () => app.account === account && completionGeneration === generation && app.editable;
    await syncAccount({ upload: true });
    if (!current()) return;
    const c = await api('/me/canvas', { account: account.id, method: 'PUT', timeoutMs: 40000, body: JSON.stringify({ base, token }) });
    if (!current()) return;
    suspendLMS();
    app.lms.canvas = { base: c.base, token: '', shared: true, synced: c.synced * 1000 };
    await syncAccount();
    return;
  }
  const owner = app.lms;
  const prev = { ...app.lms.canvas };
  app.lms.canvas = { base: base.replace(/\/+$/, ''), token, synced: 0 };
  const connection = app.lms.canvas;
  try {
    return await imports.canvas().ready;
  } catch (e) {
    if (app.lms === owner && owner.canvas === connection) { app.lms.canvas = prev; imports.publish(); }
    throw e;
  }
}

async function disconnectCanvas() {
  const account = app.account;
  if (account && app.lms.canvas.shared) {
    suspendLMS();
    const generation = completionGeneration;
    await api('/me/canvas', { account: account.id, method: 'DELETE' });
    if (app.account === account && generation === completionGeneration) { suspendLMS(); app.lms.canvas = { base: '', token: '', synced: 0 }; }
    return;
  }
  app.lms.canvas = { base: '', token: '', synced: 0 };
  app.tasks = app.tasks.filter((t) => t.source !== 'canvas');
  imports.publish();
}

// The student's Canvas courses matched to the catalog by the server, which
// reads Canvas with the account's saved token or with this device's own.
// The term is the one Canvas names, else the one the planner shows.
async function findCanvasClasses() {
  const account = app.account, canvas = app.lms.canvas;
  const where = { inst: app.inst, term: app.term };
  if (canvas.shared) return api('/me/canvas/classes', { account: account?.id || '', method: 'POST', timeoutMs: 40000, body: JSON.stringify(where) });
  if (!canvas.token) throw new Error('Connect Canvas first');
  return api('/lms/canvas/classes', { method: 'POST', timeoutMs: 40000, body: JSON.stringify({ ...where, base: canvas.base, token: canvas.token }) });
}

// Signed in, a calendar link is saved encrypted with the account and its
// tasks arrive through document sync on every device. As a guest it stays here.
const sharedFeed = (f) => ({ id: f.id, label: f.label, synced: f.synced * 1000, error: f.error || '', shared: true });
function applyAccountFeeds(list) {
  app.lms.feeds = [...app.lms.feeds.filter((f) => !f.shared), ...list.map(sharedFeed)];
}
async function addFeed(url, label) {
  const account = app.account;
  if (account) {
    const generation = completionGeneration;
    const out = await api('/me/feeds', { account: account.id, method: 'PUT', timeoutMs: 40000, body: JSON.stringify({ url: url.trim(), label: label.trim() }) });
    if (app.account !== account || generation !== completionGeneration) return 0;
    applyAccountFeeds(out.feeds);
    await syncAccount();
    return app.tasks.filter((t) => t.source === 'feed').length;
  }
  return imports.addFeed(url.trim(), label.trim());
}

async function removeFeed(i) {
  const feed = app.lms.feeds[i];
  if (!feed) return;
  if (feed.shared) {
    const account = app.account;
    if (!account) { app.lms.feeds.splice(i, 1); return; }
    const out = await api(`/me/feeds/${encodeURIComponent(feed.id)}`, { account: account.id, method: 'DELETE' });
    if (app.account !== account) return;
    applyAccountFeeds(out.feeds);
    await syncAccount();
    return;
  }
  app.lms.feeds.splice(i, 1);
  if (!app.lms.feeds.some((f) => !f.shared)) app.tasks = app.tasks.filter((t) => t.source === 'feed' && !t.canvasConnection ? false : true);
  else imports.feeds()?.done.catch(() => {});
  imports.publish();
}

// Shared Canvas and calendar feeds are imported by the server and published
// through account sync; this asks it to check them and pulls the result.
// The minute timer only checks freshness: a check that succeeded within the
// last five minutes stands unless the student asks for a sync, and a tab
// handover keeps it. Local edits upload first, since an import reconciles
// with the account's documents. Both refreshes answer with their connections,
// so they go together, and nothing is downloaded unless one imported.
let accountImport, accountChecked;
async function refreshAccountCanvas(force) {
  const account = app.account;
  if (!account || !app.editable) return;
  if (!force && accountChecked?.id === account.id && Date.now() - accountChecked.at < 5 * 60000) return;
  const generation = completionGeneration;
  const current = () => app.account === account && generation === completionGeneration && app.editable;
  if (accountImport?.account === account) return accountImport.promise;
  const job = { account, promise: null };
  job.promise = (async () => {
    await syncAccount({ upload: true });
    if (!current()) return;
    // Upload completion intents before importing upstream completion.
    if (app.tasks.some((t) => t.source === 'canvas' && t.pendingCanvasDone) && (await api('/me/canvas', { account: account.id })).connected) {
      for (const t of app.tasks) if (t.source === 'canvas' && t.pendingCanvasDone) await writeAccountCanvasDone(t);
      if (!current()) return;
    }
    const known = () => JSON.stringify([app.lms.canvas.shared ? app.lms.canvas.synced : 0, app.lms.feeds.filter((f) => f.shared).map((f) => [f.id, f.synced])]);
    const before = known();
    if (force || app.lms.canvas.shared || app.lms.feeds.some((f) => f.shared)) Object.assign(lmsSync, { owner: app.lms, pending: true, error: '', phase: 'canvas' });
    const refresh = (path) => api(path, { account: account.id, method: 'POST', timeoutMs: 40000, body: JSON.stringify({ force }) });
    const [canvas, feeds] = await Promise.allSettled([refresh('/me/canvas/refresh'), refresh('/me/feeds/refresh')]);
    if (!current()) return;
    if (feeds.status === 'fulfilled') applyAccountFeeds(feeds.value.feeds);
    if (canvas.status === 'rejected') throw canvas.reason;
    const c = canvas.value;
    if (!c.connected) {
      if (app.lms.canvas.shared) app.lms.canvas = { base: '', token: '', synced: 0 };
    } else if (!app.lms.canvas.shared || app.lms.canvas.base !== c.base) {
      imports.suspend();
      app.lms.canvas = { base: c.base, token: '', shared: true, synced: c.synced * 1000 };
    } else app.lms.canvas.synced = c.synced * 1000;
    lmsSync.error = [c.connected ? c.error : '', feeds.status === 'rejected' ? feeds.reason.message : ''].filter(Boolean).join('. ');
    if (force || known() !== before) await syncAccount();
    if (current() && !lmsSync.error) accountChecked = { id: account.id, at: Date.now() };
  })().catch(error => {
    if (current()) { lmsSync.owner = app.lms; lmsSync.error = error.message; }
    throw error;
  }).finally(() => {
    if (current()) { lmsSync.pending = false; lmsSync.phase = ''; }
    if (accountImport === job) accountImport = null;
  });
  accountImport = job;
  return job.promise;
}
function writeAccountCanvasDone(t) {
  const account = app.account, intent = t.pendingCanvasDone;
  if (!account || !intent || !app.editable) return Promise.resolve();
  const generation = completionGeneration;
  const current = () => app.account === account && generation === completionGeneration && app.editable;
  if (completionWrites.has(t.uid)) return completionWrites.get(t.uid);
  const write = (async () => {
    await syncAccount();
    if (!current()) return;
    const r = await api('/me/canvas/complete', { account: account.id, method: 'POST', timeoutMs: 40000, body: JSON.stringify({ document: 't_' + t.id, intent }) });
    if (!current()) return;
    if (r.module === false) throw new Error('Canvas module completion is still pending');
    await syncAccount();
  })().catch(e => { if (current()) canvasUpdateFailed(e); })
    .finally(() => { if (completionWrites.get(t.uid) === write) completionWrites.delete(t.uid); });
  completionWrites.set(t.uid, write);
  return write;
}
async function syncLMS() {
  try { await refreshAccountCanvas(true); }
  catch (error) { await imports.feeds()?.done; throw error; }
  return imports.sync();
}
// A handover to another tab keeps the account's last check: the same account's
// connections were checked, whichever tab edits next.
const suspendLMS = (handover = false) => { completionGeneration++; imports.suspend(); accountImport = null; if (!handover) accountChecked = null; completionWrites.clear(); lmsSync.pending = false; lmsSync.phase = ''; };
async function refreshLMS() {
  if (!app.editable || (typeof navigator !== 'undefined' && navigator.onLine === false)) return 0;
  try { await refreshAccountCanvas(false); }
  catch (error) { await imports.feeds()?.done; throw error; }
  return imports.sync({ staleAfter: 5 * 60000 });
}

return { saveTask, deleteTask, setTaskDone, taskCourses, taskCourseCode, taskColor, canvasCourses, mapCourse, lmsOf, lmsConnected, connectCanvas, disconnectCanvas, findCanvasClasses, addFeed, removeFeed, syncLMS, refreshLMS, suspendLMS, lmsSync };
}
