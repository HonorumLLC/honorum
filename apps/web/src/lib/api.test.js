import { test } from 'node:test';
import assert from 'node:assert/strict';
import { api, newSecret, syncPush, syncPull, shareLink, setAccountOwner } from './api.js';

test('API cancellation preserves the callers abort reason', async (t) => {
  const controller = new AbortController();
  const reason = new DOMException('Dialog closed', 'AbortError');
  controller.abort(reason);
  t.mock.method(globalThis, 'fetch', async (_, { signal }) => { signal.throwIfAborted(); });
  await assert.rejects(api('/auth/code', { signal: controller.signal }), error => error === reason);
});

test('API errors retain the server status and message', async (t) => {
  t.mock.method(globalThis, 'fetch', async () => Response.json({ error: 'Wait before requesting another code' }, { status: 429 }));
  await assert.rejects(api('/auth/code'), { status: 429, message: 'Wait before requesting another code' });
});

test('shared schedules still round trip through encrypted blobs', async (t) => {
  const secret = newSecret();
  const schedule = { share: { name: 'Shared plan', sections: ['a'] }, inst: 'demo', term: 'fall' };
  let ciphertext, uploaded;
  t.mock.method(globalThis, 'fetch', async (path, options) => {
    assert.ok(!path.includes(secret));
    if (options?.method === 'PUT') {
      uploaded = path;
      ciphertext = options.body;
      return new Response(null, { status: 204 });
    }
    assert.equal(path, uploaded);
    return new Response(ciphertext);
  });
  await syncPush(secret, schedule);
  assert.ok(!new TextDecoder().decode(ciphertext).includes(schedule.share.name));
  assert.deepEqual(await syncPull(secret), schedule);
  globalThis.location = { origin: 'https://example.com' };
  t.after(() => { delete globalThis.location; });
  assert.equal(shareLink(secret), `https://example.com/#share=${secret}`);
});


test('account mutations carry the local owner even when the shared cookie has changed', async (t) => {
  setAccountOwner(() => 'account-local');
  t.after(() => setAccountOwner(() => ''));
  const calls = [];
  t.mock.method(globalThis, 'fetch', async (path, options) => {
    calls.push(path);
    assert.equal(options.headers.get('X-Honorum-Account'), 'account-local');
    return Response.json({ error: 'Account changed; sign in again before syncing' }, { status: 409 });
  });
  for (const [path, method] of [['/me', 'DELETE'], ['/auth/logout', 'POST'], ['/me/reminders', 'PUT']]) {
    await assert.rejects(api(path, { method }), { status: 409 });
  }
  assert.deepEqual(calls, ['/api/me', '/api/auth/logout', '/api/me/reminders']);
});

test('delayed actions retain their captured owner and guest requests send an explicit empty owner', async (t) => {
  setAccountOwner(() => 'account-new');
  t.after(() => setAccountOwner(() => ''));
  const owners = [];
  t.mock.method(globalThis, 'fetch', async (_, options) => {
    assert.equal(options.headers.has('X-Honorum-Account'), true);
    owners.push(options.headers.get('X-Honorum-Account'));
    assert.equal('account' in options, false);
    return new Response(null, { status: 204 });
  });
  await api('/me/reminders', { method: 'PUT', account: 'account-previous' });
  await api('/watches', { method: 'POST', account: '' });
  assert.deepEqual(owners, ['account-previous', '']);
});

test('sign-in endpoints omit account ownership so an intentional account switch is allowed', async (t) => {
  setAccountOwner(() => 'account-previous');
  t.after(() => setAccountOwner(() => ''));
  t.mock.method(globalThis, 'fetch', async (_, options) => {
    assert.equal(options.headers.has('X-Honorum-Account'), false);
    return Response.json({});
  });
  for (const path of ['/auth/code', '/auth/verify']) await api(path);
});
