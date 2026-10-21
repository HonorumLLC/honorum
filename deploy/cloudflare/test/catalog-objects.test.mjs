import { test } from 'node:test';
import assert from 'node:assert/strict';
import { catalogObject } from '../src/catalog-objects.ts';

test('R2 reads cannot reach replicas or arbitrary objects', async () => {
  const bucket = { get: () => { throw new Error('private bucket accessed'); } };
  for (const path of ['/honorum/db', '/catalog/ou/secrets.txt', '/catalog/../honorum/data.json', '/api/me']) {
    assert.equal(await catalogObject(new Request('https://honorum.app' + path), bucket), null);
  }
});

test('catalog encoding variants preserve opaque cache bytes and validators', async () => {
  const calls = [];
  const bucket = { get: async key => { calls.push(key); return { size: 3, body: new ReadableStream({ start(controller) { controller.enqueue(new Uint8Array([1, 2, 3])); controller.close(); } }) }; } };
  const path = '/catalog/ou/202610.structure.' + 'a'.repeat(64) + '.json';
  const response = await catalogObject(new Request('https://honorum.app' + path, { headers: { 'Accept-Encoding': 'gzip', 'X-Honorum-Store-Encoded': '1' } }), bucket);
  assert.equal(calls[0], 'honorum-public' + path + '.gz');
  assert.equal(response.headers.get('Content-Encoding'), null);
  assert.equal(response.headers.get('X-Honorum-Stored-Encoding'), 'gzip');
  assert.equal(response.headers.get('ETag'), '"' + 'a'.repeat(64) + '-gzip"');
  assert.match(response.headers.get('Cache-Control'), /immutable/);
  assert.deepEqual([...new Uint8Array(await response.arrayBuffer())], [1, 2, 3]);
});
