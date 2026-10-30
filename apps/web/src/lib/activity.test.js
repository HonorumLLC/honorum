import test from 'node:test';
import assert from 'node:assert/strict';
import { createActivity } from './activity.js';
const memory = () => { const map = new Map(); return { map, getItem: k => map.get(k) ?? null, setItem: (k, v) => map.set(k, v), removeItem: k => map.delete(k) }; };

test('only a signed-in page in the foreground checks in, and failures leave the streak alone', async () => {
  const app = { account: { id: 'a' }, inst: 'demo', streak: { current: 1 } }, storage = memory();
  let visible = false, fail = true;
  const calls = [];
  const request = async (path, options) => { calls.push([path, JSON.parse(options.body)]); if (fail) throw Error('offline'); return { current: 2, longest: 2, timezone: 'UTC' }; };
  const activity = createActivity({ app, storage, request, visible: () => visible });
  await activity.visit(); assert.equal(calls.length, 0);
  visible = true; await activity.visit();
  assert.deepEqual(calls[0], ['/me/activity', { inst: 'demo' }]); assert.equal(app.streak.current, 1);
  storage.setItem('activity:a', '[1]');
  fail = false; await activity.visit();
  assert.equal(app.streak.current, 2); assert.equal(storage.getItem('activity:a'), null);
  app.account = null; await activity.visit(); assert.equal(calls.length, 2);
});

test('overlapping check-ins share one request', async () => {
  const app = { account: { id: 'a' } };
  let calls = 0, resolve;
  const activity = createActivity({ app, storage: memory(), visible: () => true, request: () => { calls++; return new Promise(r => { resolve = r; }); } });
  const first = activity.visit(), second = activity.visit();
  resolve({ current: 3, timezone: 'UTC' }); await Promise.all([first, second]);
  assert.equal(calls, 1); assert.equal(app.streak.current, 3);
});

test('a reply for an account that signed out is ignored', async () => {
  const app = { account: { id: 'a' } };
  let resolve;
  const activity = createActivity({ app, storage: memory(), request: () => new Promise(r => { resolve = r; }), visible: () => true });
  const pending = activity.visit();
  app.account = { id: 'b' };
  resolve({ current: 10, timezone: 'UTC' }); await pending;
  assert.equal(app.streak, undefined);
});

test('blocked browser storage still permits a check-in', async () => {
  const app = { account: { id: 'a' } };
  const storage = { getItem() { throw Error('denied'); }, setItem() { throw Error('denied'); }, removeItem() { throw Error('denied'); } };
  const activity = createActivity({ app, storage, visible: () => true, request: async () => ({ current: 1, timezone: 'UTC' }) });
  await activity.visit();
  assert.equal(app.streak.current, 1);
});

test('the last streak shows before the server answers, and only a failed first check is missed', async () => {
  const storage = memory();
  storage.setItem('streak:a', JSON.stringify({ current: 4 }));
  let resolve;
  const app = { account: { id: 'a' }, streak: null };
  const activity = createActivity({ app, storage, visible: () => true, request: () => new Promise(r => { resolve = r; }) });
  const pending = activity.visit();
  assert.equal(app.streak.current, 4);
  resolve({ current: 5 }); await pending;
  assert.equal(JSON.parse(storage.getItem('streak:a')).current, 5);
  const fresh = { account: { id: 'b' }, streak: null };
  await createActivity({ app: fresh, storage, visible: () => true, request: async () => { throw Error('offline'); } }).visit();
  assert.equal(fresh.streak, null); assert.equal(fresh.streakMissed, true);
});
