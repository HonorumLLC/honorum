import { test } from 'node:test';
import assert from 'node:assert/strict';
import { assetResponse, brotliCopy } from '../src/static-assets.ts';
import { visitorEncoding } from '../src/public-cache.ts';

const req = (path, headers = {}, method = 'GET') => new Request('https://honorum.example' + path, { headers, method });
// An asset binding holding a bundle, the page and the worker with Brotli copies,
// and a manifest without one.
function binding() {
  const files = { '/assets/app-1.js': 'bundle', '/assets/app-1.js.br': 'bundle.br', '/index.html': 'page', '/index.html.br': 'page.br', '/sw.js': 'worker', '/sw.js.br': 'worker.br', '/manifest.webmanifest': 'manifest' };
  const calls = [];
  const fetch = async (request) => {
    const path = new URL(request.url).pathname === '/' ? '/index.html' : new URL(request.url).pathname;
    calls.push(path);
    if (!(path in files)) return new Response('missing', { status: 404 });
    const etag = `"${path}"`;
    if (request.headers.get('If-None-Match') === etag) return new Response(null, { status: 304, headers: { ETag: etag } });
    return new Response(request.method === 'HEAD' ? null : files[path], { headers: { ETag: etag, 'Content-Type': path.endsWith('.br') ? 'application/octet-stream' : 'text/plain' } });
  };
  return { fetch, calls };
}

test('Brotli visitors get the prepared copy of bundles, the page and the worker', async () => {
  for (const [path, file, body, type, cache] of [['/assets/app-1.js', '/assets/app-1.js.br', 'bundle.br', 'text/javascript; charset=utf-8', 'public, max-age=31536000, immutable'], ['/', '/index.html.br', 'page.br', 'text/html; charset=utf-8', 'no-cache'], ['/sw.js', '/sw.js.br', 'worker.br', 'text/javascript; charset=utf-8', 'no-cache']]) {
    const assets = binding();
    const response = await assetResponse(req(path), assets, 'br');
    assert.deepEqual(assets.calls, [file]);
    assert.equal(await response.text(), body);
    assert.equal(response.headers.get('Content-Encoding'), 'br');
    assert.equal(response.headers.get('Content-Type'), type);
    assert.equal(response.headers.get('Vary'), 'Accept-Encoding');
    assert.equal(response.headers.get('Cache-Control'), cache);
    assert.equal(response.headers.get('X-Content-Type-Options'), 'nosniff');
  }
});

test('other visitors, files and requests keep the binding response', async () => {
  for (const [request, encoding] of [[req('/assets/app-1.js'), 'gzip'], [req('/assets/app-1.js'), 'identity'], [req('/manifest.webmanifest'), 'br'], [req('/assets/app-1.js', { Range: 'bytes=0-1' }), 'br'], [req('/assets/app-1.js', {}, 'POST'), 'br']]) {
    const assets = binding();
    const response = await assetResponse(request, assets, encoding);
    assert.equal(response.headers.get('Content-Encoding'), null);
    assert.equal(assets.calls.length, 1);
    assert.ok(!assets.calls[0].endsWith('.br'));
    assert.equal(response.headers.get('Content-Security-Policy').startsWith("default-src 'self'"), true);
  }
  assert.equal(brotliCopy(req('/legal'), 'br'), null);
  assert.equal(brotliCopy(req('/assets/nested/app.js'), 'br'), null);
});

test('a missing copy falls back, and revalidation compares the copy', async () => {
  const assets = binding();
  const missing = await assetResponse(req('/assets/other-2.js'), assets, 'br');
  assert.deepEqual(assets.calls, ['/assets/other-2.js.br', '/assets/other-2.js']);
  assert.equal(missing.status, 404);
  assert.equal(missing.headers.get('Content-Encoding'), null);
  const unchanged = await assetResponse(req('/sw.js', { 'If-None-Match': '"/sw.js.br"' }), binding(), 'br');
  assert.equal(unchanged.status, 304);
  assert.equal(unchanged.headers.get('Content-Encoding'), 'br');
  const head = await assetResponse(req('/sw.js', {}, 'HEAD'), binding(), 'br');
  assert.equal(await head.text(), '');
});

test('the visitor’s own Accept-Encoding decides, not the normalized header', () => {
  const visitor = (header, original) => visitorEncoding({ headers: new Headers({ 'Accept-Encoding': header }), cf: original === undefined ? undefined : { clientAcceptEncoding: original } });
  assert.equal(visitor('br, gzip', 'gzip'), 'gzip');
  assert.equal(visitor('br, gzip', 'gzip, br'), 'br');
  assert.equal(visitor('gzip, br'), 'br');
  assert.equal(visitor('br;q=0, gzip'), 'gzip');
});
