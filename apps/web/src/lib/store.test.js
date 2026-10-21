import { test } from 'node:test';
import assert from 'node:assert/strict';
import 'fake-indexeddb/auto';
import { get, set, setMany } from './store.js';

test('legacy active aliases migrate atomically without mixing account caches', async () => {
  const db = await new Promise((resolve, reject) => {
    const r = indexedDB.open('honorum', 1);
    r.onupgradeneeded = () => r.result.createObjectStore('kv');
    r.onsuccess = () => resolve(r.result); r.onerror = () => reject(r.error);
  });
  const state = { account: { id: 'a' }, dataOwner: 'a', tasks: [{ id: 'private' }], lms: { canvas: { token: 'local-only' } } };
  await new Promise((resolve, reject) => {
    const tx = db.transaction('kv', 'readwrite'), store = tx.objectStore('kv');
    store.put(state, 'state'); store.put({ owner: 'a', outbox: { t_private: { body: state.tasks[0] } } }, 'sync');
    store.put({ tasks: [{ id: 'other-account' }] }, 'state:b');
    tx.oncomplete = resolve; tx.onabort = () => reject(tx.error);
  });
  let versionChanged = false;
  db.onversionchange = () => { versionChanged = true; db.close(); };
  assert.deepEqual(await get('state'), state);
  assert.equal(versionChanged, true);
  await assert.rejects(new Promise((resolve, reject) => { const r = indexedDB.open('honorum', 1); r.onsuccess = () => resolve(r.result); r.onerror = () => reject(r.error); }), { name: 'VersionError' });
  assert.equal(await get('activeOwner'), 'a');
  assert.equal((await get('sync')).owner, 'a');
  assert.equal((await get('state:b')).tasks[0].id, 'other-account');
  const current = await new Promise(resolve => { const r = indexedDB.open('honorum'); r.onsuccess = () => resolve(r.result); });
  const old = await new Promise(resolve => { const r = current.transaction('kv').objectStore('kv').get('state'); r.onsuccess = () => resolve(r.result); });
  assert.equal(old, undefined); current.close();
});

test('multi-key writes commit together and a clone error aborts earlier writes', async () => {
  await setMany([['state-test', { name: 'old' }], ['sync-test', { seq: 1 }]]);
  await assert.rejects(setMany([['state-test', { name: 'new' }], ['sync-test', () => {}]]));
  assert.deepEqual(await get('state-test'), { name: 'old' });
  assert.deepEqual(await get('sync-test'), { seq: 1 });
  await set('state-test', { name: 'committed' });
  assert.deepEqual(await get('state-test'), { name: 'committed' });
});

test('a failed account save cannot switch the active owner or replace another cache', async () => {
  const before = await get('state:b');
  await assert.rejects(setMany([['state:b', { tasks: [] }], ['activeOwner', 'b'], ['sync:b', () => {}]]));
  assert.equal(await get('activeOwner'), 'a');
  assert.deepEqual(await get('state:b'), before);
  assert.equal((await get('state')).account.id, 'a');
});

test('deleting browser storage prevents an old connection from recreating it', async () => {
  await set('private-plan', { name: 'Must stay cleared' });
  await new Promise((resolve, reject) => {
    const request = indexedDB.deleteDatabase('honorum');
    request.onsuccess = resolve;
    request.onerror = () => reject(request.error);
  });
  await assert.rejects(set('private-plan', { name: 'Must stay cleared' }), /Site data was cleared/);
  await assert.rejects(get('private-plan'), /Site data was cleared/);
  assert.deepEqual(await indexedDB.databases(), []);
});
