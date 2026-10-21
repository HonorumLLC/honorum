import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { runInNewContext } from 'node:vm';
import { IDBFactory } from 'fake-indexeddb';

test('subscription renewal carries the stored account owner; guests never update a session', async () => {
  for (const [owner, canonical] of [['', false], ['account-student', false], ['account-student', true], ['', true]]) {
    const indexedDB = new IDBFactory();
    if (owner || canonical) await new Promise((resolve, reject) => {
      const request = indexedDB.open('honorum', 1);
      request.onupgradeneeded = () => request.result.createObjectStore('kv');
      request.onerror = () => reject(request.error);
      request.onsuccess = () => {
        const db = request.result, tx = db.transaction('kv', 'readwrite');
        const store = tx.objectStore('kv');
        if (canonical) {
          store.put(owner || 'guest', 'activeOwner');
          store.put({ account: { id: owner } }, `state:${owner || 'guest'}`);
          store.put({ account: { id: 'retired-owner' } }, 'state');
        } else store.put({ account: { id: owner } }, 'state');
        tx.oncomplete = () => { db.close(); resolve(); };
      };
    });
    const events = {}, requests = [];
    runInNewContext(readFileSync(new URL('../../public/sw.js', import.meta.url), 'utf8'), {
      __PRECACHE__: [], indexedDB,
      self: { addEventListener: (name, handler) => { events[name] = handler; } },
      fetch: async (url, options) => { requests.push({ url, options }); },
    });
    let finished;
    events.pushsubscriptionchange({
      oldSubscription: { endpoint: 'https://push.example/old', options: { applicationServerKey: 'key' } },
      newSubscription: { endpoint: 'https://push.example/new' },
      waitUntil: promise => { finished = promise; },
    });
    await finished;
    assert.deepEqual(requests.map(r => r.url), owner ? ['/api/push/retarget', '/api/me/push'] : ['/api/push/retarget']);
    for (const { options } of requests) assert.equal(options.headers['X-Honorum-Account'], owner);
    if (!owner && !canonical) assert.deepEqual(await indexedDB.databases(), []);
  }
});
