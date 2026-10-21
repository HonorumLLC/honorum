import { test } from 'node:test';
import assert from 'node:assert/strict';
import { IDBFactory } from 'fake-indexeddb';
import { loadDescriptions } from './catalog.js';
import { get } from './store.js';

test('descriptions load by content hash, reuse saved copies, and survive a failed newer request', async () => {
  globalThis.indexedDB = new IDBFactory();
  const original = globalThis.fetch, requests = [];
  globalThis.fetch = async url => {
    requests.push(url);
    if (url.includes('.expired.')) return { status: 404, ok: false };
    return { status: 200, ok: true, json: async () => ({ CS: { d: 'Current version' } }) };
  };
  try {
    assert.deepEqual(await loadDescriptions('demo', 'fall', ''), {});
    assert.equal(requests.length, 0);
    assert.equal((await loadDescriptions('demo', 'fall', 'current')).CS.d, 'Current version');
    assert.deepEqual(requests, ['/catalog/demo/fall.desc.current.json']);
    assert.equal((await get('desc:demo:fall')).hash, 'current');
    assert.equal((await loadDescriptions('demo', 'fall', 'current')).CS.d, 'Current version');
    assert.equal(requests.length, 1);
    assert.equal((await loadDescriptions('demo', 'fall', 'expired')).CS.d, 'Current version');
    assert.equal((await get('desc:demo:fall')).hash, 'current');
  } finally { globalThis.fetch = original; }
});
