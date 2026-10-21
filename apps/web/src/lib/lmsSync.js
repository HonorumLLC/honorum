import { mergeTasks, parseICS, recentTasks } from './tasks.js';
import { readCanvas } from './canvasStream.js';

async function fetchCanvas(connection, onTasks, account, cancellation) {
  // The server bounds an import at 30 seconds. Allow its final response to arrive.
  const timeout = AbortSignal.timeout(35000);
  const signal = AbortSignal.any([timeout, cancellation]);
  try {
    const response = await fetch('/api/lms/canvas', {
      method: 'POST', signal,
      headers: { 'Content-Type': 'application/json', Accept: 'application/x-ndjson', 'X-Honorum-Account': account },
      body: JSON.stringify({ base: connection.base, token: connection.token }),
    });
    return await readCanvas(response, onTasks);
  } catch (error) {
    if (timeout.aborted) throw new Error('Canvas took too long to finish. Try syncing again');
    throw error;
  }
}

async function fetchFeed(feed, cancellation, zone) {
  const response = await fetch('/api/lms/feed', {
    method: 'POST', signal: AbortSignal.any([AbortSignal.timeout(30000), cancellation]),
    headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({ url: feed.url }),
  });
  if (!response.ok) {
    const text = await response.text();
    let message;
    try { message = JSON.parse(text).error; } catch { message = text.trim(); }
    throw new Error(message || 'The calendar could not be refreshed');
  }
  return parseICS(await response.text(), 'feed', feed.label, zone);
}

// Credentials remain on the device. Jobs belong to the exact connection that
// started them, and overlapping requests for that connection share one import.
// zone names the campus time zone that floating feed times are read in.
export function createLMSSync({ app, status, canvasRequest = fetchCanvas, feedRequest = fetchFeed, onCanvasComplete = () => {}, now = Date.now, zone = () => '' }) {
  const jobs = new Set();
  const errors = new Map();
  let canvasJob, feedsJob;
  let generation = 0;
  // Account feeds are imported by the server; only this device's own links are fetched here.
  const localFeeds = (owner) => (owner.feeds || []).filter((f) => !f.shared);
  const sameFeeds = (owner, feeds) => app.lms === owner && localFeeds(owner).length === feeds.length
    && feeds.every((f, i) => f.url === localFeeds(owner)[i].url && f.label === localFeeds(owner)[i].label);

  function publish() {
    for (const job of jobs) if (!job.current()) job.controller.abort();
    const active = [...jobs].filter(j => j.current());
    const canvas = active.find(j => j.kind === 'canvas');
    Object.assign(status, {
      owner: app.lms, pending: active.length > 0,
      phase: canvas ? (canvas.preview ? 'completion' : 'canvas') : active.length ? 'feeds' : '',
      error: [...errors.values()].filter(e => e.current()).map(e => e.message).join('. '),
    });
  }

  function begin(kind, current) {
    const epoch = generation;
    const job = { kind, current: () => epoch === generation && current(), controller: new AbortController(), preview: false, edits: new Map(), deleted: new Set() };
    jobs.add(job);
    errors.delete(kind);
    publish();
    return job;
  }
  function check(job) {
    if (!job.current()) throw new Error('The account or connection changed');
  }
  function finish(job, error) {
    jobs.delete(job);
    // A failed addition belongs to its open connection form, not the status of
    // sources that were already connected and can be retried by Sync Now.
    if (error && job.kind !== 'adding-feed' && job.current()) errors.set(job.kind, { current: job.current, message: error.message });
    publish();
  }
  function apply(job, tasks, source, complete) {
    check(job);
    const incoming = tasks.filter(t => !job.deleted.has(t.uid)).map(t => ({ ...t, source }));
    app.tasks = mergeTasks(app.tasks, incoming, source, { complete }).map(t => {
      const edit = job.edits.get(t.uid);
      return edit ? { ...t, ...edit } : t;
    });
  }

  function canvas() {
    const owner = app.lms, connection = owner.canvas;
    if (!connection.base || !connection.token) return null;
    if (canvasJob?.current()) return canvasJob;
    canvasJob?.controller.abort();
    const { base, token } = connection;
    const current = () => app.lms === owner && owner.canvas === connection && connection.base === base && connection.token === token;
    const job = canvasJob = begin('canvas', current);
    let ready, failed;
    job.ready = new Promise((resolve, reject) => { ready = resolve; failed = reject; });
    // A manual refresh waits for completion; connecting waits only for assignments.
    job.ready.catch(() => {});
    job.done = (async () => {
      try {
        const tasks = await canvasRequest({ base, token }, tasks => {
          apply(job, tasks, 'canvas', false);
          job.preview = true;
          publish();
          ready(tasks.length);
        }, app.account?.id || '', job.controller.signal);
        apply(job, tasks, 'canvas', true);
        connection.synced = now();
        ready(tasks.length);
        finish(job);
        return tasks.length;
      } catch (error) {
        failed(error);
        finish(job, error);
        throw error;
      } finally {
        if (canvasJob === job) canvasJob = null;
      }
    })();
    job.done.catch(() => {});
    job.done.then(() => { if (job.current()) onCanvasComplete(); }, () => {});
    return job;
  }

  function feeds() {
    const owner = app.lms, snapshot = localFeeds(owner).map(f => ({ ...f }));
    if (!snapshot.length) return null;
    if (feedsJob?.current()) return feedsJob;
    feedsJob?.controller.abort();
    const job = feedsJob = begin('feeds', () => sameFeeds(owner, snapshot));
    job.done = (async () => {
      try {
        const results = new Array(snapshot.length);
        let next = 0;
        // Keep independent calendars moving without issuing an unbounded burst.
        const workers = await Promise.allSettled(Array.from({ length: Math.min(3, snapshot.length) }, async () => {
          while (next < snapshot.length) {
            check(job);
            const i = next++;
            results[i] = await feedRequest(snapshot[i], job.controller.signal, zone());
          }
        }));
        const failed = workers.find(r => r.status === 'rejected');
        if (failed) throw failed.reason;
        const tasks = recent(results.flat());
        apply(job, tasks, 'feed', true);
        owner.feedSynced = now();
        finish(job);
        return tasks.length;
      } catch (error) {
        finish(job, error);
        throw error;
      } finally {
        if (feedsJob === job) feedsJob = null;
      }
    })();
    job.done.catch(() => {});
    return job;
  }

  const recent = tasks => recentTasks(tasks, now());

  async function addFeed(url, label) {
    const owner = app.lms;
    if (owner.feeds.some(f => f.url === url)) throw new Error('That calendar is already connected');
    const job = begin('adding-feed', () => app.lms === owner);
    try {
      const tasks = recent(await feedRequest({ url, label }, job.controller.signal, zone()));
      check(job);
      if (owner.feeds.some(f => f.url === url)) throw new Error('That calendar is already connected');
      owner.feeds.push({ url, label });
      apply(job, tasks, 'feed', false);
      finish(job);
      return tasks.length;
    } catch (error) {
      finish(job, error);
      throw error;
    }
  }

  async function sync({ staleAfter = 0 } = {}) {
    const owner = app.lms, time = now();
    const selected = [];
    const pendingCompletion = app.tasks.some(t => t.source === 'canvas' && t.pendingCanvasDone);
    if (pendingCompletion || !staleAfter || time - (owner.canvas.synced || 0) >= staleAfter) selected.push(canvas());
    if (!staleAfter || time - (owner.feedSynced || 0) >= staleAfter) selected.push(feeds());
    const results = await Promise.allSettled(selected.filter(Boolean).map(j => j.done));
    if (app.lms !== owner) return 0;
    const failures = results.filter(r => r.status === 'rejected');
    if (failures.length) throw new Error(failures.map(r => r.reason.message).join('. '));
    return results.reduce((n, r) => n + (r.status === 'fulfilled' ? r.value : 0), 0);
  }

  function noteDone(uid, done) {
    noteEdit(uid, { done, reason: done ? 'marked' : '' });
  }
  function noteEdit(uid, fields) {
    if (!uid) return;
    for (const job of jobs) if (job.current()) job.edits.set(uid, { ...job.edits.get(uid), ...fields });
  }
  function noteDeleted(uid, deleted = true) {
    if (!uid) return;
    for (const job of jobs) if (job.current()) deleted ? job.deleted.add(uid) : job.deleted.delete(uid);
  }
  const settled = () => canvasJob?.current() ? canvasJob.done.catch(() => {}) : Promise.resolve();
  function suspend() {
    generation++;
    for (const job of jobs) job.controller.abort();
    publish();
  }
  return { canvas, feeds, addFeed, sync, noteDone, noteEdit, noteDeleted, settled, publish, suspend };
}
