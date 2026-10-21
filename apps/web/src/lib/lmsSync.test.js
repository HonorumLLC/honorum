import { test } from 'node:test';
import assert from 'node:assert/strict';
import { createLMSSync } from './lmsSync.js';

const deferred = () => {
  let resolve, reject;
  const promise = new Promise((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject };
};
const task = (uid, extra = {}) => ({ uid, title: uid, source: 'canvas', due: '', done: false, ...extra });
function setup(overrides = {}) {
  const app = {
    tasks: [task('old', { id: 'old', done: true, moduleItem: '2:3', reason: 'marked' })],
    account: { id: 'account-a' },
    lms: { canvas: { base: 'https://canvas.example', token: 'token', synced: 10 }, feeds: [] },
  };
  const status = {};
  const sync = createLMSSync({ app, status, now: () => 1000000, ...overrides });
  return { app, status, sync };
}

test('Canvas connects at the preview, coalesces imports, and deletes absent open tasks only on completion', async () => {
  const gate = deferred();
  let preview, calls = 0;
  const { app, status, sync } = setup({ canvasRequest: async (connection, onTasks, account) => {
    calls++; preview = onTasks;
    assert.equal(account, 'account-a');
    return gate.promise;
  } });
  app.tasks.push(task('gone'));
  const job = sync.canvas();
  assert.equal(sync.canvas(), job);
  const full = sync.sync();
  assert.equal(calls, 1);
  assert.equal(status.pending, true);
  preview([task('new')]);
  assert.equal(await job.ready, 1);
  assert.equal(status.phase, 'completion');
  assert.deepEqual(app.tasks.map(t => t.uid), ['old', 'gone', 'new']);
  assert.equal(app.lms.canvas.synced, 10);
  gate.resolve([task('new', { moduleItem: '4:5' })]);
  assert.equal(await full, 1);
  // A finished task outlives Canvas's short look back; an open one does not.
  assert.deepEqual(app.tasks.map(t => t.uid), ['old', 'new']);
  assert.equal(app.tasks[1].moduleItem, '4:5');
  assert.equal(app.lms.canvas.synced, 1000000);
  assert.equal(status.pending, false);
});

test('failed completion retains existing completion metadata and tasks without recording a successful sync', async () => {
  const gate = deferred();
  const { app, status, sync } = setup({ canvasRequest: async (_, preview) => {
    preview([task('old'), task('new')]);
    return gate.promise;
  } });
  const job = sync.canvas();
  await job.ready;
  assert.equal(app.tasks.find(t => t.uid === 'old').moduleItem, '2:3');
  gate.reject(new Error('Canvas is unavailable'));
  await assert.rejects(job.done, /unavailable/);
  assert.deepEqual(app.tasks.map(t => t.uid), ['old', 'new']);
  assert.equal(app.tasks[0].done, true);
  assert.equal(app.lms.canvas.synced, 10);
  assert.equal(status.pending, false);
  assert.match(status.error, /unavailable/);
});

test('a failure before the planner arrives leaves stored tasks intact and rejects connection readiness', async () => {
  const { app, sync } = setup({ canvasRequest: async () => { throw new Error('Invalid token'); } });
  const before = structuredClone(app.tasks);
  const job = sync.canvas();
  await assert.rejects(job.ready, /Invalid token/);
  await assert.rejects(job.done, /Invalid token/);
  assert.deepEqual(app.tasks, before);
});

test('late imports cannot overwrite a replacement connection or a different account', async () => {
  const gates = [deferred(), deferred()];
  let calls = 0;
  const { app, sync } = setup({ canvasRequest: async () => gates[calls++].promise });
  const old = sync.canvas();
  app.lms.canvas = { base: 'https://canvas.other', token: 'new-token', synced: 0 };
  const current = sync.canvas();
  gates[0].resolve([task('wrong')]);
  await assert.rejects(old.done, /connection changed/);
  gates[1].resolve([task('right')]);
  await current.done;
  assert.deepEqual(app.tasks.map(t => t.uid), ['old', 'right']);

  const gate = deferred();
  const other = setup({ canvasRequest: async () => gate.promise });
  const pending = other.sync.canvas();
  other.app.lms = { canvas: {}, feeds: [] };
  other.app.tasks = [];
  gate.resolve([task('wrong-account')]);
  await assert.rejects(pending.done, /connection changed/);
  assert.deepEqual(other.app.tasks, []);
});

test('local ticks and deletions made between snapshots survive final enrichment', async () => {
  const gate = deferred();
  const { app, sync } = setup({ canvasRequest: async (_, preview) => {
    preview([task('old', { done: true }), task('remove')]);
    return gate.promise;
  } });
  app.tasks.push({ id: 'manual', source: 'manual', done: false });
  const job = sync.canvas();
  await job.ready;
  app.tasks.find(t => t.uid === 'old').done = false;
  sync.noteDone('old', false);
  sync.noteEdit('old', { title: 'My corrected title', due: '2026-10-04T12:00:00Z', note: 'Local note' });
  sync.noteDone(undefined, true);
  sync.noteDeleted('remove');
  app.tasks = app.tasks.filter(t => t.uid !== 'remove');
  gate.resolve([task('old', { done: true, moduleItem: '8:9' }), task('remove')]);
  await job.done;
  assert.equal(app.tasks.find(t => t.uid === 'old').done, false);
  assert.equal(app.tasks.find(t => t.uid === 'old').moduleItem, '8:9');
  assert.equal(app.tasks.find(t => t.uid === 'old').title, 'My corrected title');
  assert.equal(app.tasks.find(t => t.uid === 'old').due, '2026-10-04T12:00:00Z');
  assert.equal(app.tasks.find(t => t.id === 'manual').done, false);
  assert.equal(app.tasks.some(t => t.uid === 'remove'), false);
});

test('Canvas and feeds start together and calendar requests have bounded concurrency', async () => {
  const canvas = deferred(), feeds = Array.from({ length: 5 }, deferred);
  let active = 0, peak = 0, calls = 0;
  const { app, sync } = setup({
    canvasRequest: async () => canvas.promise,
    feedRequest: async feed => {
      calls++; peak = Math.max(peak, ++active);
      await feeds[Number(feed.label)].promise;
      active--;
      return [task('feed:' + feed.label)];
    },
  });
  app.lms.feeds = feeds.map((_, i) => ({ url: `https://calendar.example/${i}`, label: String(i) }));
  const first = sync.sync(), second = sync.sync();
  assert.equal(calls, 3);
  canvas.resolve([task('canvas')]);
  feeds.forEach(g => g.resolve());
  assert.equal(await first, 6);
  assert.equal(await second, 6);
  assert.equal(calls, 5);
  assert.equal(peak, 3);
  assert.deepEqual(app.tasks.map(t => t.uid), ['old', 'canvas', ...feeds.map((_, i) => 'feed:' + i)]);
});

test('a failed calendar keeps its earlier tasks while successful Canvas data is still imported', async () => {
  const { app, status, sync } = setup({
    canvasRequest: async () => [task('canvas-new')],
    feedRequest: async () => { throw new Error('Calendar unavailable'); },
  });
  app.lms.feeds = [{ url: 'https://calendar.example', label: '' }];
  app.tasks.push(task('feed-old', { source: 'feed' }));
  await assert.rejects(sync.sync(), /Calendar unavailable/);
  assert.deepEqual(app.tasks.map(t => t.uid), ['old', 'feed-old', 'canvas-new']);
  assert.equal(app.lms.canvas.synced, 1000000);
  assert.equal(app.lms.feedSynced, undefined);
  assert.match(status.error, /Calendar unavailable/);
});

test('adding a calendar requests only that calendar and preserves other sources', async () => {
  const requests = [];
  const { app, sync } = setup({
    canvasRequest: async () => { throw new Error('Unexpected Canvas request'); },
    feedRequest: async feed => { requests.push(feed.url); return [task('feed-new')]; },
  });
  app.lms.feeds = [{ url: 'https://broken-calendar.example', label: '' }];
  app.tasks.push(task('feed-old', { source: 'feed' }));
  assert.equal(await sync.addFeed('https://new-calendar.example', 'New'), 1);
  assert.deepEqual(requests, ['https://new-calendar.example']);
  assert.deepEqual(app.tasks.map(t => t.uid), ['old', 'feed-old', 'feed-new']);
});

test('automatic refresh skips fresh sources and manual refresh still fetches them', async () => {
  let calls = 0;
  const { app, sync } = setup({ canvasRequest: async () => { calls++; return []; } });
  app.lms.canvas.synced = 999999;
  await sync.sync({ staleAfter: 300000 });
  assert.equal(calls, 0);
  await sync.sync();
  assert.equal(calls, 1);
  app.tasks = [task('pending', { pendingCanvasDone: 'intent' })];
  await sync.sync({ staleAfter: 300000 });
  assert.equal(calls, 2);
});

test('a failed calendar addition does not leave an error on already connected sources', async () => {
  const { status, sync } = setup({
    canvasRequest: async () => [],
    feedRequest: async () => { throw new Error('Invalid calendar'); },
  });
  await assert.rejects(sync.addFeed('https://invalid-calendar.example', ''), /Invalid calendar/);
  assert.equal(status.error, '');
  await sync.sync();
  assert.equal(status.error, '');
});

test('suspending a tab aborts requests and ignores late snapshots even without a connection change', async () => {
  const gate = deferred();
  let signal;
  const { app, status, sync } = setup({ canvasRequest: async (_, preview, account, cancellation) => {
    signal = cancellation;
    return gate.promise;
  } });
  const job = sync.canvas();
  sync.suspend();
  assert.equal(signal.aborted, true);
  assert.equal(status.pending, false);
  gate.resolve([task('late')]);
  await assert.rejects(job.done, /connection changed/);
  assert.deepEqual(app.tasks.map(t => t.uid), ['old']);
});
