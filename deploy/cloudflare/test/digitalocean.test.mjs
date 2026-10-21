import { test } from 'node:test';
import assert from 'node:assert/strict';
import { digitalOceanConfigured, digitalOceanFetch } from '../src/digitalocean.ts';

test('both private services are required', async () => {
  assert.equal(digitalOceanConfigured({}), false);
  assert.equal(digitalOceanConfigured({DIGITALOCEAN_API:{fetch(){}}}), false);
  assert.equal(digitalOceanConfigured({DIGITALOCEAN_API:{fetch(){}}, DIGITALOCEAN_WORKER:{fetch(){}}}), true);
  assert.equal(digitalOceanConfigured({DIGITALOCEAN_WORKER:{fetch(){}}}), false);
  const response=await digitalOceanFetch(new Request('https://honorum.app/api/me'), {});
  assert.equal(response.status,503);
});

test('private proxy preserves mutations and credentials and replaces forwarded addresses', async () => {
  let received;
  const service={fetch: async request => {received=request; return new Response('ok')}};
  const input=new Request('https://honorum.app/api/me/sync?since=7', {method:'POST', body:'{"docs":[]}', headers:{Cookie:'session=private',Authorization:'Bearer private','CF-Connecting-IP':'192.0.2.1','X-Forwarded-For':'spoofed',Forwarded:'for=spoofed','X-Real-IP':'spoofed',Origin:'https://honorum.app'},duplex:'half'});
  await digitalOceanFetch(input,{DIGITALOCEAN_API:service});
  assert.equal(received.url,'http://honorum.app/api/me/sync?since=7');
  assert.equal(received.method,'POST');
  assert.equal(await received.text(),'{"docs":[]}');
  assert.equal(received.headers.get('Cookie'),'session=private');
  assert.equal(received.headers.get('Authorization'),'Bearer private');
  assert.equal(received.headers.get('X-Forwarded-For'),'192.0.2.1');
  assert.equal(received.headers.get('Forwarded'),null);
  assert.equal(received.headers.get('X-Real-IP'),null);
  assert.equal(received.headers.get('X-Forwarded-Proto'),'https');
});

test('catalog misses use the worker and event responses remain streamed', async () => {
  const api={fetch:async()=>new Response('data: hello\n\n',{headers:{'Content-Type':'text/event-stream'}})};
  const worker={fetch:async()=>new Response('catalog')};
  const env={DIGITALOCEAN_API:api,DIGITALOCEAN_WORKER:worker};
  assert.equal(await (await digitalOceanFetch(new Request('https://honorum.app/catalog/ou/file.json'),env)).text(),'catalog');
  const events=await digitalOceanFetch(new Request('https://honorum.app/api/me/events'),env);
  assert.equal(events.headers.get('Content-Type'),'text/event-stream');
  assert.equal(await events.text(),'data: hello\n\n');
});
