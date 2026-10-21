import { test } from 'node:test';
import assert from 'node:assert/strict';
import { siteResponse } from '../src/landing.ts';

const navigate = { 'Sec-Fetch-Mode': 'navigate', Accept: 'text/html,*/*' };
const req = (path, headers = {}) => new Request('https://honorum.example' + path, { headers });
// Serves the app at the root and the landing page at its clean URL.
async function site(request) {
  const served = [];
  const response = await siteResponse(request, async (r) => {
    const path = new URL(r.url).pathname;
    served.push(path);
    return new Response(path === '/about' ? 'landing' : 'app', { headers: { Vary: 'Accept-Encoding' } });
  });
  return { response, served };
}

test('the root shows the landing page to new visitors and the app to everyone else', async () => {
  for (const [name, request, body] of [
    ['new visitor', req('/', navigate), 'landing'],
    ['browser without fetch metadata', req('/', { Accept: 'text/html' }), 'landing'],
    ['opened the app', req('/', { ...navigate, Cookie: 'hn_app=1' }), 'app'],
    ['signed in', req('/', { ...navigate, Cookie: 'other=1; hn_session=token' }), 'app'],
    ['offline shell', req('/', { 'Sec-Fetch-Mode': 'cors', Accept: '*/*' }), 'app'],
    ['older offline shell', req('/', { Accept: '*/*' }), 'app'],
    ['returning from Google', req('/?google_connected=1', navigate), 'app'],
  ]) {
    const { response } = await site(request);
    assert.equal(await response.text(), body, name);
    assert.equal(response.headers.get('Cache-Control'), 'no-cache', name);
    assert.equal(response.headers.get('Vary'), 'Accept-Encoding, Cookie', name);
    assert.equal(response.headers.get('Set-Cookie')?.startsWith('hn_app=1;') ?? false, body === 'app', name);
  }
});

test('opening the app records it and returns to the root', async () => {
  const { response, served } = await site(req('/app', navigate));
  assert.equal(response.status, 303);
  assert.equal(response.headers.get('Location'), '/');
  assert.match(response.headers.get('Set-Cookie'), /^hn_app=1; Path=\/; Max-Age=34560000; HttpOnly; Secure; SameSite=Lax$/);
  assert.deepEqual(served, []);
});

test('other paths are left to the static files', async () => {
  for (const path of ['/about', '/legal', '/assets/app-1.js', '/sw.js']) assert.equal((await site(req(path, navigate))).response, null);
});
