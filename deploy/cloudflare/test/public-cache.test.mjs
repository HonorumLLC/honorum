import { test } from 'node:test';
import assert from 'node:assert/strict';
import { gzipSync, gunzipSync } from 'node:zlib';
import { isPublicRead, publicEncoding, publicResponse } from '../src/public-cache.ts';

const req = (path, headers = {}, method = 'GET') => new Request('https://honorum.example' + path, { headers, method });
const catalog = '/catalog/ou/202610.structure.' + 'a'.repeat(64) + '.json';
function fixture() {
  const values = new Map(), calls = [], pending = [];
  const cache = { match: async request => values.get(request.url)?.clone(), put: async (request, response) => { values.set(request.url, response.clone()); } };
  const origin = async request => { calls.push(request); return new Response('catalog:' + request.headers.get('Accept-Encoding'), { headers: { 'Cache-Control': 'public, max-age=30', ETag: '"one"', Vary: 'Accept-Encoding' } }); };
  return { cache, calls, pending, origin, ctx: { waitUntil: promise => pending.push(promise) } };
}
test('only published public reads are eligible', () => {
  for (const path of ['/api/config',catalog,'/catalog/ou/202610.seats.' + 'a'.repeat(64) + '.json']) assert.ok(isPublicRead(req(path)));
  for (const path of ['/api/me','/api/me/docs','/api/me/sync','/api/lms/canvas','/catalog/ou/secrets.txt','/catalog/ou/202610.json','/catalog/ou/202610.desc.json']) assert.equal(isPublicRead(req(path)),false);
  assert.equal(isPublicRead(req('/api/config',{},'POST')),false);
});
test('public miss strips credentials and hit avoids origin', async () => {
  const f=fixture();
  const first=await publicResponse(req('/api/config?ignored=1',{Cookie:'session=secret',Authorization:'Bearer secret','X-Honorum-Account':'someone'}), f.origin, f.cache, f.ctx);
  assert.equal(first.headers.get('X-Honorum-Cache'),'MISS');
  assert.equal(f.calls[0].headers.get('cookie'),null);assert.equal(f.calls[0].headers.get('authorization'),null);assert.equal(f.calls[0].headers.get('x-honorum-account'),null);
  await Promise.all(f.pending);
  const second=await publicResponse(req('/api/config'),f.origin,f.cache,f.ctx);
  assert.equal(second.headers.get('X-Honorum-Cache'),'HIT');assert.equal(f.calls.length,1);
  assert.equal(await second.text(),'catalog:identity');
});
test('encoding variants, validators, HEAD and forced refresh remain correct', async () => {
  const f=fixture();
  await publicResponse(req('/api/config',{'Accept-Encoding':'gzip, br'}),f.origin,f.cache,f.ctx);await Promise.all(f.pending);
  await publicResponse(req('/api/config',{'Accept-Encoding':'gzip'}),f.origin,f.cache,f.ctx);await Promise.all(f.pending);
  assert.equal(f.calls.length,2);
  const hit=await publicResponse(req('/api/config',{'Accept-Encoding':'br, gzip','If-None-Match':'W/"one"'}),f.origin,f.cache,f.ctx);
  assert.equal(hit.status,304);assert.equal(await hit.text(),'');assert.equal(f.calls.length,2);
  const head=await publicResponse(req('/api/config',{'Accept-Encoding':'br'},'HEAD'),f.origin,f.cache,f.ctx);assert.equal(await head.text(),'');
  await publicResponse(req('/api/config',{'Accept-Encoding':'br','Cache-Control':'no-cache'}),f.origin,f.cache,f.ctx);assert.equal(f.calls.length,3);
});
test('errors, private responses, and cookies are never cached', async () => {
  for (const [status,headers] of [[500,{'Cache-Control':'public, max-age=30'}],[200,{'Cache-Control':'private'}],[200,{'Cache-Control':'public, no-store'}],[200,{'Cache-Control':'public, max-age=30','Set-Cookie':'session=private'}]]) {
    const f=fixture();let count=0;const origin=async()=>{count++;return new Response('error',{status,headers})};
    for(let i=0;i<2;i++){await publicResponse(req('/api/config'),origin,f.cache,f.ctx);await Promise.all(f.pending)}
    assert.equal(count,2);
  }
});
test('quality negotiation honors exclusions and preference',()=>{
  assert.equal(publicEncoding('gzip;q=1,br;q=0.5'),'gzip');
  assert.equal(publicEncoding('br;q=0,gzip;q=0'),'identity');
  assert.equal(publicEncoding('*;q=0'),'identity;q=0');
  assert.equal(publicEncoding('identity;q=1,*;q=0'),'identity');
  assert.equal(publicEncoding('br;q=oops,gzip'),'gzip');
});

test('no-store request does not populate the public cache',async()=>{
  const f=fixture();
  await publicResponse(req('/api/config',{'Cache-Control':'no-store'}),f.origin,f.cache,f.ctx);
  await Promise.all(f.pending);
  await publicResponse(req('/api/config'),f.origin,f.cache,f.ctx);
  assert.equal(f.calls.length,2);
});

test('Cloudflare original client encodings select distinct cache variants',async()=>{
  const f=fixture();
  for(const encoding of ['gzip','br','identity']) {
    const request=req(catalog,{'Accept-Encoding':'br, gzip'});
    Object.defineProperty(request,'cf',{value:{clientAcceptEncoding:encoding}});
    await publicResponse(request,f.origin,f.cache,f.ctx);
    await Promise.all(f.pending);
  }
  assert.deepEqual(f.calls.map(r=>r.headers.get('Accept-Encoding')),['gzip','br','identity']);
});

test('cache retrieval restores browser freshness and keeps prepared catalogs untransformed',async()=>{
  const f=fixture(), originalPut=f.cache.put;
  f.cache.put=async(request,response)=>{
    // Cloudflare's zone Browser Cache TTL can alter headers on a cache hit.
    const headers=new Headers(response.headers);
    headers.set('Cache-Control','public, max-age=14400');
    return originalPut(request,new Response(response.body,{headers}));
  };
  for(const path of ['/api/config',catalog]) {
    const first=await publicResponse(req(path),f.origin,f.cache,f.ctx);
    await Promise.all(f.pending);
    const hit=await publicResponse(req(path),f.origin,f.cache,f.ctx);
    const policy=path.startsWith('/catalog/')?'public, max-age=30, no-transform':'public, max-age=30';
    assert.equal(first.headers.get('Cache-Control'),policy);
    assert.equal(hit.headers.get('Cache-Control'),policy);
    assert.equal(hit.headers.get('X-Honorum-Cache'),'HIT');
    assert.equal(hit.headers.get('X-Honorum-Origin-Cache-Control'),null);
  }
});

test('prepared gzip bytes survive opaque cache storage and regain their encoding for clients',async()=>{
  const f=fixture(), raw=gzipSync('published catalog'); let calls=0;
  const origin=async request=>{
    calls++; assert.equal(request.headers.get('X-Honorum-Store-Encoded'),'1');
    return new Response(raw,{headers:{'Content-Type':'application/octet-stream','X-Honorum-Stored-Encoding':'gzip','Cache-Control':'public, max-age=31536000, immutable, no-transform',ETag:'"encoded-gzip"'}});
  };
  const put=f.cache.put;
  f.cache.put=async(request,response)=>{
    assert.equal(response.headers.get('Content-Encoding'),null);
    assert.equal(response.headers.get('Content-Type'),'application/octet-stream');
    await put(request,response);
  };
  for(let n=0;n<2;n++) {
    const response=await publicResponse(req(catalog,{'Accept-Encoding':'gzip'}),origin,f.cache,f.ctx);
    assert.equal(response.headers.get('Content-Encoding'),'gzip');
    assert.equal(response.headers.get('Content-Type'),'application/json');
    assert.equal(response.headers.get('X-Honorum-Stored-Encoding'),null);
    assert.equal(response.headers.get('X-Honorum-Origin-Cache-Control'),null);
    const bytes=Buffer.from(await response.arrayBuffer());
    assert.deepEqual(bytes,raw); assert.equal(gunzipSync(bytes).toString(),'published catalog');
    await Promise.all(f.pending);
  }
  assert.equal(calls,1);
});
