import { test } from 'node:test';
import assert from 'node:assert/strict';
import { docsOf, diff, applyDoc, createSyncEngine, mergeBody, uploadBatch } from './sync.js';

const state = () => ({
  inst: 'demo', term: '2026FA', current: 'a', onboarded: true, reminders: { on: false }, lms: { map: { 1: 'CS 101' } },
  schedules: [{ id: 'a', name: 'One', inst: 'demo', term: '2026FA', sections: ['1'], blocks: [] }],
  tasks: [{ id: 'x', title: 'Read', done: false }],
});

test('docsOf splits the state into prefixed documents', () => {
  const d = docsOf(state());
  assert.deepEqual(Object.keys(d).sort(), ['prefs', 's_a', 't_x']);
  assert.equal(d.s_a.type, 'schedule');
  assert.equal(d.prefs.body.map[1], 'CS 101');
  assert.equal(d.prefs.body.reminders, false);
});

test('diff finds new, changed and deleted docs against the known copies', () => {
  const docs = docsOf(state());
  const known = { s_a: { rev: 3, type: 'schedule', body: JSON.stringify(docs.s_a.body) }, t_gone: { rev: 2, type: 'task', body: '{}' } };
  const out = diff(docs, known, 99);
  const ids = out.map((d) => d.id).sort();
  assert.deepEqual(ids, ['prefs', 't_gone', 't_x']);
  assert.equal(out.find((d) => d.id === 't_gone').deleted, true);
  assert.equal(out.find((d) => d.id === 't_gone').base, 2);
  assert.equal(out.find((d) => d.id === 't_x').base, 0);
  assert.equal(out.find((d) => d.id === 't_x').updated, 99);
  assert.equal(diff(docs, Object.fromEntries(Object.entries(docs).map(([id, d]) => [id, { rev: 1, type: d.type, body: JSON.stringify(d.body) }])), 1).length, 0);
});

test('applyDoc replaces, inserts and removes, and adopts navigation only when asked', () => {
  const app = state();
  applyDoc(app, { id: 's_a', type: 'schedule', rev: 4, body: { id: 'a', name: 'Renamed', inst: 'demo', term: '2026FA', sections: [], blocks: [] } });
  assert.equal(app.schedules[0].name, 'Renamed');
  applyDoc(app, { id: 's_b', type: 'schedule', rev: 5, body: { id: 'b', name: 'Two', inst: 'demo', term: '2026FA', sections: [], blocks: [] } });
  assert.equal(app.schedules.length, 2);
  applyDoc(app, { id: 't_x', type: 'task', rev: 6, deleted: true });
  assert.equal(app.tasks.length, 0);
  applyDoc(app, { id: 'prefs', type: 'prefs', rev: 7, body: { inst: 'other', term: 'T', current: 'b', onboarded: true, reminders: true, map: {} } });
  assert.equal(app.reminders.on, true);
  assert.equal(app.inst, 'demo'); // navigation untouched
  applyDoc(app, { id: 'prefs', type: 'prefs', rev: 7, body: { inst: 'other', term: 'T', current: 'b' } }, true);
  assert.equal(app.inst, 'other');
  assert.equal(app.current, 'b');
});

test('preference fields from another client survive a round trip', () => {
  const app = state();
  const watching = [{ inst: 'demo', term: '2026FA', section: '10001', label: 'CS 101-001', title: 'Intro', lastAvailable: 0 }];
  applyDoc(app, { id: 'prefs', type: 'prefs', rev: 3, body: { ...docsOf(app).prefs.body, watching } });
  assert.deepEqual(app.prefExtras, { watching });
  assert.deepEqual(docsOf(app).prefs.body.watching, watching);
  applyDoc(app, { id: 'prefs', type: 'prefs', rev: 4, body: docsOf(state()).prefs.body });
  assert.deepEqual(app.prefExtras, {}); // the other client removed them
});

// Every exchange is a POST to /me/sync; a download simply carries no documents.
const isUpload = (opts) => !!opts?.body && (JSON.parse(opts.body).docs || []).length > 0;
function engine(request, options = {}) {
  const app = { ...state(), account: { id: 'account-test' }, sync: {}, watches: [] };
  const saved = new Map();
  const latest = key => saved.get(key + ':' + (saved.get('activeOwner') || 'guest'));
  const exchangeRequest = async (path, opts) => {
    const res = await request(path, opts);
    return path === '/me/sync' ? { docs: [], seq: res.seq ?? Math.max(0, ...(res.applied || []).map(d => d.rev)), more: false, ...res } : res;
  };
  const e = createSyncEngine({ read: async () => null, write: async (entries) => { for (const [k,v] of entries) saved.set(k, structuredClone(v)); }, request: exchangeRequest, delay: () => 0, cancel: () => {}, ...options });
  const init = () => e.initSync(app, { snapshot: () => structuredClone(app), watches: (list) => { app.watches = list; }, after() {}, signedOut() {} });
  return { app, saved, latest, e, init };
}
const deferred = () => { let resolve; const promise = new Promise((r) => { resolve = r; }); return { promise, resolve }; };

test('an upload acknowledges its captured version and sends an edit made in flight', async () => {
  const entered = deferred(), release = deferred(); let posts = [], seq = 0;
  const h = engine(async (_, opts) => {
    if (!isUpload(opts)) return { docs: [], seq, more: false };
    const docs = JSON.parse(opts.body).docs; posts.push(docs);
    if (posts.length === 1) { entered.resolve(); await release.promise; }
    return { applied: docs.map((d) => ({ id: d.id, rev: ++seq })), conflicts: [] };
  });
  await h.init(); await h.e.noteChange(structuredClone(h.app));
  const running = h.e.sync(); await entered.promise;
  h.app.schedules[0].name = 'Changed while uploading'; await h.e.noteChange(structuredClone(h.app));
  release.resolve(); await running;
  assert.equal(posts[0].find((d) => d.id === 's_a').body.name, 'One');
  assert.equal(posts[1].find((d) => d.id === 's_a').body.name, 'Changed while uploading');
  assert.equal(h.e.pending(), 0);
  assert.equal(JSON.parse(h.latest('sync').known.s_a.body).name, 'Changed while uploading');
});

test('an old account response is ignored after reset', async () => {
  const entered = deferred(), release = deferred();
  const h = engine(async (_, opts) => {
    entered.resolve(); await release.promise;
    return { applied: JSON.parse(opts.body).docs.map((d) => ({ id: d.id, rev: 1 })), conflicts: [] };
  });
  await h.init(); await h.e.noteChange(structuredClone(h.app));
  const running = h.e.sync(); await entered.promise;
  h.app.account = null; await h.e.resetSync(); release.resolve(); await running;
  assert.deepEqual(h.latest('sync').known, {});
});

test('independent changes merge without comparing device clocks', () => {
  const base = { name: 'One', sections: ['a'], colors: { a: 'red' } };
  const local = { ...base, sections: ['a', 'b'] };
  const remote = { ...base, name: 'Renamed', sections: ['a', 'c'] };
  const result = mergeBody(base, local, remote);
  assert.equal(result.conflicted, false);
  assert.deepEqual(result.body, { name: 'Renamed', sections: ['a', 'b', 'c'], colors: { a: 'red' } });
  assert.equal(mergeBody('A', 'B', 'C').conflicted, true);
});

test('local state and outbox are one write and a failed commit rejects the save', async () => {
  const app = { ...state(), account: { id: 'account-a' }, sync: {} };
  let keys;
  const e = createSyncEngine({ read: async () => null, write: async (entries) => { keys = entries.map(([key]) => key); throw new Error('disk full'); } });
  await e.initSync(app, { snapshot: () => structuredClone(app) });
  await assert.rejects(e.noteChange(structuredClone(app)), /disk full/);
  assert.deepEqual(keys, ['state:account-a', 'sync:account-a', 'activeOwner']);
});

test('signing out preserves pending edits in the account namespace', async () => {
  const h = engine(async () => { throw new Error('offline'); });
  await h.init(); h.app.dataOwner = h.app.account.id;
  await h.e.noteChange(structuredClone(h.app));
  h.app.account = null; await h.e.resetSync(true);
  assert.equal(h.saved.get('sync:account-test').outbox.s_a.body.name, 'One');
  assert.equal(h.saved.get('state:account-test').schedules[0].name, 'One');
  h.app.account = { id: 'account-other' };
  await assert.rejects(h.e.noteChange(structuredClone(h.app)), /Account changed/);
  assert.equal(h.saved.has('state:account-other'), false);
});

test('suspending a tab ignores a late upload acknowledgment', async () => {
  const entered = deferred(), release = deferred();
  const h = engine(async (_, opts) => { entered.resolve(); await release.promise; return { applied: JSON.parse(opts.body).docs.map(d => ({ id: d.id, rev: 1 })), conflicts: [] }; });
  await h.init(); await h.e.noteChange(structuredClone(h.app));
  const upload = h.e.sync(); await entered.promise;
  await h.e.suspendSync(); release.resolve(); await upload;
  assert.ok(h.e.pending() > 0);
  assert.deepEqual(h.latest('sync').known, {});
});


test('adoption saves ownership immediately and serializes the first download with ordinary sync', async () => {
  const entered = deferred(), release = deferred();
  const calls = [];
  let seq = 0;
  const h = engine(async (_, opts) => {
    calls.push(isUpload(opts) ? 'upload' : 'download');
    if (calls.length === 1) { entered.resolve(); await release.promise; }
    if (isUpload(opts)) return { applied: JSON.parse(opts.body).docs.map((d) => ({ id: d.id, rev: ++seq })), conflicts: [] };
    return { docs: [], seq, more: false };
  });
  await h.init();
  await h.e.adopt();
  assert.equal(calls.length, 0);
  assert.equal(h.latest('sync').adopting, true);
  assert.equal(h.latest('sync').owner, h.app.account.id);
  const first = h.e.sync();
  await entered.promise;
  h.app.schedules[0].name = 'Edited while downloading';
  await h.e.noteChange(structuredClone(h.app));
  const second = h.e.sync();
  assert.deepEqual(calls, ['download']);
  release.resolve();
  await Promise.all([first, second]);
  assert.deepEqual(calls, ['download', 'upload']);
  assert.equal(JSON.parse(h.latest('sync').known.s_a.body).name, 'Edited while downloading');
  assert.equal(h.latest('sync').adopting, false);
});

test('a failed initial download remains recoverable after a reload', async () => {
  const h = engine(async () => { throw new Error('offline'); });
  await h.init();
  await h.e.adopt();
  await h.e.sync();
  assert.equal(h.app.sync.status, 'error');
  assert.equal(h.latest('sync').adopting, true);
  const calls = [];
  let seq = 0;
  const restored = engine(async (_, opts) => {
    calls.push(isUpload(opts) ? 'upload' : 'download');
    if (isUpload(opts)) return { applied: JSON.parse(opts.body).docs.map((d) => ({ id: d.id, rev: ++seq })), conflicts: [] };
    return { docs: [], seq, more: false };
  }, { read: async (key) => structuredClone(key === 'sync' ? h.latest(key) : h.saved.get(key)) });
  await restored.init();
  await restored.e.sync();
  assert.equal(calls[0], 'download');
  assert.equal(restored.latest('sync').adopting, false);
  assert.equal(restored.app.sync.status, 'ok');
});

test('saving during account restoration waits for the new account outbox', async () => {
  const entered = deferred(), release = deferred();
  const h = engine(async () => { throw new Error('unexpected request'); }, {
    read: async (key) => {
      if (!key.startsWith('sync:')) return null;
      entered.resolve(); await release.promise;
      return { owner: 'account-test', seq: 5, known: {}, outbox: {}, conflicts: [] };
    },
  });
  await h.init();
  const adoption = h.e.adopt();
  await entered.promise;
  const saving = h.e.noteChange(structuredClone(h.app));
  assert.equal(h.saved.size, 0);
  release.resolve();
  await Promise.all([adoption, saving]);
  assert.equal(h.latest('sync').seq, 5);
  assert.equal(h.latest('sync').owner, h.app.account.id);
  assert.equal(h.latest('sync').outbox.s_a.body.name, 'One');
});

test('a remote deletion wins when an in-flight local edit was reverted to its base', async () => {
  const base = state().schedules[0];
  const saved = {
    owner: 'account-test', seq: 1,
    known: { s_a: { rev: 1, type: 'schedule', body: JSON.stringify(base) } },
    outbox: { s_a: { id: 's_a', type: 'schedule', base: 1, updated: 1, body: base } },
    conflicts: [],
  };
  let uploads = 0;
  const h = engine(async (_, opts) => {
    if (!isUpload(opts)) return { docs: [], seq: 2, more: false };
    uploads++;
    return { applied: [], conflicts: [{ id: 's_a', type: 'schedule', rev: 2, deleted: true }] };
  }, { read: async () => structuredClone(saved) });
  await h.init();
  await h.e.sync();
  assert.deepEqual(h.app.schedules, []);
  assert.equal(h.e.pending(), 0);
  assert.equal(uploads, 1);
  assert.deepEqual(h.e.conflicts(), []);
});

test('preferences edited during adoption merge with the account download', async () => {
  const entered = deferred(), release = deferred();
  const remote = { ...docsOf(state()).prefs.body, inst: 'remote-school', map: { 1: 'MATH 200' } };
  let downloads = 0, seq = 1, uploaded;
  const h = engine(async (_, opts) => {
    if (isUpload(opts)) {
      uploaded = JSON.parse(opts.body).docs;
      return { applied: uploaded.map((d) => ({ id: d.id, rev: ++seq })), conflicts: [] };
    }
    if (++downloads === 1) {
      entered.resolve(); await release.promise;
      return { docs: [{ id: 'prefs', type: 'prefs', rev: 1, body: remote }], seq, more: false };
    }
    return { docs: [], seq, more: false };
  });
  await h.init(); await h.e.adopt();
  const running = h.e.sync(); await entered.promise;
  h.app.reminders.on = true;
  await h.e.noteChange(structuredClone(h.app));
  assert.equal(h.latest('sync').outbox.prefs.body.reminders, true);
  release.resolve(); await running;
  assert.equal(h.app.reminders.on, true);
  assert.equal(h.app.inst, 'remote-school');
  assert.deepEqual(h.app.lms.map, { 1: 'MATH 200' });
  assert.equal(uploaded.find(d => d.id === 'prefs').body.reminders, true);
  assert.deepEqual(h.e.conflicts(), []);
  assert.equal(h.latest('sync').adoptionBase, undefined);
});

test('a deletion during adoption survives an incoming account copy', async () => {
  const entered = deferred(), release = deferred();
  const remote = { ...state().schedules[0], name: 'Remote edit' };
  let downloads = 0, seq = 1, uploaded;
  const h = engine(async (_, opts) => {
    if (isUpload(opts)) {
      uploaded = JSON.parse(opts.body).docs;
      return { applied: uploaded.map(d => ({ id: d.id, rev: ++seq })), conflicts: [] };
    }
    if (++downloads === 1) {
      entered.resolve(); await release.promise;
      return { docs: [{ id: 's_a', type: 'schedule', rev: 1, body: remote }], seq, more: false };
    }
    return { docs: [], seq, more: false };
  });
  await h.init(); await h.e.adopt();
  const running = h.e.sync(); await entered.promise;
  h.app.schedules = [];
  await h.e.noteChange(structuredClone(h.app));
  release.resolve(); await running;
  assert.deepEqual(h.app.schedules, []);
  assert.equal(uploaded.find(d => d.id === 's_a').deleted, true);
  assert.equal(h.e.conflicts()[0].body.name, 'Remote edit');
});

test('an old adoption and a waiting save cannot overwrite a switched account', async () => {
  const entered = deferred(), release = deferred();
  const h = engine(async () => { throw new Error('unexpected request'); }, {
    read: async key => {
      if (key !== 'sync:account-test') return null;
      entered.resolve(); await release.promise;
      return { owner: 'account-test', known: {}, outbox: {}, conflicts: [], seq: 99 };
    },
  });
  await h.init();
  const firstAdoption = h.e.adopt(); await entered.promise;
  const staleSave = h.e.noteChange(structuredClone(h.app));
  h.app.account = { id: 'account-second' };
  h.app.dataOwner = h.app.account.id;
  h.app.schedules = [];
  await h.e.adopt();
  release.resolve(); await Promise.all([firstAdoption, staleSave]);
  assert.equal(h.latest('sync').owner, 'account-second');
  assert.equal(h.latest('sync').seq, 0);
  assert.deepEqual(h.saved.get('state:account-second').schedules, []);
  assert.equal(h.saved.has('state:account-test'), false);
});

test('an unchanged poll performs no persistence or view mutations', async () => {
  let requests = 0, after = 0;
  const app = { ...state(), account: { id: 'account-test' }, dataOwner: 'account-test', sync: {} };
  const saved = { owner: 'account-test', seq: 8, known: {}, outbox: {}, conflicts: [], watchesVersion: 'watch-hash' };
  const e = createSyncEngine({ read: async () => saved,
    write: async () => { throw new Error('unchanged poll must not write'); },
    request: async (path, opts) => { requests++; assert.equal(path, '/me/sync'); const sent = JSON.parse(opts.body); assert.equal(sent.since, 8); assert.equal(sent.watches_version, 'watch-hash'); assert.deepEqual(sent.docs, []); return { docs: [], seq: 8, more: false, watches_version: 'watch-hash' }; },
    delay: () => 0, cancel() {} });
  await e.initSync(app, { snapshot: () => structuredClone(app), after: () => after++, watches: () => { throw new Error('unchanged watches must be absent'); } });
  await e.sync();
  assert.equal(app.sync.status, 'ok'); assert.equal(requests, 1); assert.equal(after, 0);
});

test('combined uploads retain unseen remote changes and the returned download cursor', async () => {
  const h = engine(async (path, opts) => {
    assert.equal(path, '/me/sync');
    const sent = JSON.parse(opts.body);
    assert.equal(sent.since, 0);
    return { applied: sent.docs.map((d, i) => ({ id: d.id, rev: i + 2 })), conflicts: [],
      docs: [{ id: 't_remote', type: 'task', rev: 1, body: { id: 'remote', title: 'Another device' } }], seq: 4, more: false };
  });
  await h.init(); await h.e.noteChange(structuredClone(h.app)); await h.e.sync();
  assert.equal(h.app.tasks.at(-1).title, 'Another device');
  assert.equal(h.latest('sync').seq, 4); assert.equal(h.e.pending(), 0);
});

test('a live event with a newer sequence schedules a sync and closes on reset', async () => {
  const scheduled = [];
  let source;
  const h = engine(async () => ({ docs: [], seq: 3, more: false }), {
    delay: (fn) => { scheduled.push(fn); return scheduled.length; },
    events: (url) => {
      assert.match(url, /^\/api\/me\/events\?account=account-test$/);
      const listeners = {};
      source = { closed: false, addEventListener: (name, fn) => { listeners[name] = fn; }, close() { this.closed = true; }, emit: (name, data) => listeners[name]({ data: JSON.stringify(data) }) };
      return source;
    },
  });
  await h.init(); await h.e.sync();
  assert.ok(source, 'live stream not opened after a successful sync');
  const before = scheduled.length;
  source.emit('seq', { seq: 3 });
  assert.equal(scheduled.length, before, 'an unchanged sequence must not schedule work');
  source.emit('seq', { seq: 5 });
  assert.equal(scheduled.length, before + 1);
  h.app.account = null; await h.e.resetSync();
  assert.equal(source.closed, true);
});

test('upload batches respect UTF-8 bytes as well as the document limit', () => {
  const docs = Object.fromEntries(Array.from({ length: 200 }, (_, i) => ['t_' + i, { id: 't_' + i, type: 'task', body: { title: '📚'.repeat(8000) } }]));
  const batch = uploadBatch(docs);
  assert.ok(batch.length > 1 && batch.length < 200);
  assert.ok(new TextEncoder().encode(JSON.stringify({ docs: batch })).length < 4 << 20);
  assert.equal(Object.keys(docs).length, 200);
  assert.throws(() => uploadBatch(docs, 20), /too large/);
});

test('queued account switches commit the final active pointer with its own state', async () => {
  const entered = deferred(), release = deferred(), records = new Map();
  const app = { ...state(), account: { id: 'a' }, dataOwner: 'a', sync: {} };
  const e = createSyncEngine({ read: async () => null, delay: () => 0, cancel() {},
    write: async entries => {
      if (entries.some(([key]) => key === 'state:b')) { entered.resolve(); await release.promise; }
      for (const [key, value] of entries) records.set(key, value);
    } });
  await e.initSync(app, { snapshot: () => structuredClone(app) });
  await e.noteChange(structuredClone(app));
  app.account = { id: 'b' }; app.dataOwner = 'b';
  const b = e.resetSync(); await entered.promise;
  app.account = { id: 'a' }; app.dataOwner = 'a';
  const a = e.resetSync(); release.resolve(); await Promise.all([b, a]);
  assert.equal(records.get('activeOwner'), 'a');
  assert.equal(records.get('state:a').account.id, 'a');
  assert.equal(records.get('state:b').account.id, 'b');
});
