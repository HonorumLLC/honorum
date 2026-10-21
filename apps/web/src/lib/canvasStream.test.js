import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readCanvas } from './canvasStream.js';

const encode = (text) => new TextEncoder().encode(text);
const line = (type, tasks) => JSON.stringify({ type, tasks }) + '\n';
function streamResponse(chunks, options = {}) {
  let cancelled = false;
  const body = new ReadableStream({
    pull(controller) {
      if (chunks.length) controller.enqueue(chunks.shift());
      else controller.close();
    },
    cancel() { cancelled = true; },
  });
  const response = new Response(body, { headers: { 'Content-Type': 'application/x-ndjson' }, ...options });
  return { response, cancelled: () => cancelled };
}

test('Canvas preview arrives before completion and split UTF-8 survives chunk boundaries', async () => {
  let controller, previewed;
  const preview = new Promise((resolve) => { previewed = resolve; });
  const body = new ReadableStream({ start(value) { controller = value; } });
  const response = new Response(body, { headers: { 'Content-Type': 'Application/X-NDJSON; charset=utf-8' } });
  const tasks = [{ uid: 'canvas:assignment:1', title: 'Español 📚' }];
  let settled = false;
  const result = readCanvas(response, previewed).finally(() => { settled = true; });
  for (const byte of encode(line('tasks', tasks))) controller.enqueue(new Uint8Array([byte]));
  assert.deepEqual(await preview, tasks);
  assert.equal(settled, false);
  const completed = [{ ...tasks[0], done: true }];
  controller.enqueue(encode(line('complete', completed).trimEnd()));
  controller.close();
  assert.deepEqual(await result, completed);
  assert.equal(body.locked, false);
});

test('Canvas stream accepts CRLF and blank lines', async () => {
  const { response } = streamResponse([encode('\r\n' + line('tasks', []).trimEnd() + '\r\n\n' + line('complete', []) + '\n')]);
  assert.deepEqual(await readCanvas(response), []);
  assert.equal(response.body.locked, false);
});

test('Canvas rejects a truncated preview instead of treating it as the final snapshot', async () => {
  const { response } = streamResponse([encode(line('tasks', [{ uid: 'one' }]))]);
  const previews = [];
  await assert.rejects(readCanvas(response, (tasks) => previews.push(tasks)), /ended before it completed/);
  assert.deepEqual(previews, [[{ uid: 'one' }]]);
  assert.equal(response.body.locked, false);
});

test('Canvas errors after a preview cancel the response and release the reader', async () => {
  const { response, cancelled } = streamResponse([
    encode(line('tasks', [])),
    encode(JSON.stringify({ type: 'error', error: 'Canvas module lookup failed' }) + '\n'),
    encode('unread trailing data'),
  ]);
  await assert.rejects(readCanvas(response), /Canvas module lookup failed/);
  assert.equal(cancelled(), true);
  assert.equal(response.body.locked, false);
});

test('Canvas cancels when the preview callback rejects the current connection', async () => {
  const failure = new Error('The account or connection changed');
  const { response, cancelled } = streamResponse([encode(line('tasks', [])), encode(line('complete', []))]);
  await assert.rejects(readCanvas(response, () => { throw failure; }), (error) => error === failure);
  assert.equal(cancelled(), true);
  assert.equal(response.body.locked, false);
});

test('Canvas rejects invalid, duplicate and out-of-order events', async (t) => {
  const cases = {
    'complete before preview': line('complete', []),
    'duplicate preview': line('tasks', []) + line('tasks', []) + line('complete', []),
    'duplicate completion': line('tasks', []) + line('complete', []) + line('complete', []),
    'preview after completion': line('tasks', []) + line('complete', []) + line('tasks', []),
    'error after completion': line('tasks', []) + line('complete', []) + '{"type":"error","error":"late"}\n',
    'unknown event': '{"type":"progress"}\n',
    'malformed JSON': '{no}\n',
    'null event': 'null\n',
    'empty error': '{"type":"error","error":""}\n',
    'invalid preview tasks': line('tasks', {}),
    'invalid task entry': line('tasks', [null]),
    'invalid completion tasks': line('tasks', []) + line('complete', null),
  };
  for (const [name, content] of Object.entries(cases)) await t.test(name, async () => {
    const { response } = streamResponse([encode(content)]);
    await assert.rejects(readCanvas(response), /invalid sync response/);
    assert.equal(response.body.locked, false);
  });
});

test('Canvas rejects invalid UTF-8 and upstream read failures and releases the reader', async () => {
  const { response } = streamResponse([new Uint8Array([0xc3])]);
  await assert.rejects(readCanvas(response), /invalid sync response/);
  assert.equal(response.body.locked, false);
  const failure = new Error('Connection interrupted');
  const failed = new Response(new ReadableStream({ pull(controller) { controller.error(failure); } }), { headers: { 'Content-Type': 'application/x-ndjson' } });
  await assert.rejects(readCanvas(failed), (error) => error === failure);
  assert.equal(failed.body.locked, false);
});

test('Canvas bounds total bytes even when they are spread across separate events', async () => {
  const preview = line('tasks', [{ title: 'x'.repeat(4 << 20) }]);
  const completion = line('complete', [{ title: 'x'.repeat(4 << 20) }]);
  const { response, cancelled } = streamResponse([encode(preview), encode(completion), encode('\n')]);
  await assert.rejects(readCanvas(response), /too much sync data/);
  assert.equal(cancelled(), true);
  assert.equal(response.body.locked, false);
});

test('Canvas rejects a successful response that is not an event stream', async () => {
  const response = Response.json([{ uid: 'canvas:assignment:1', title: 'Reading' }]);
  await assert.rejects(readCanvas(response, () => assert.fail('No preview without a stream')), /invalid sync response/);
  assert.equal(response.body.locked, false);
});

test('Canvas HTTP errors preserve status, server message and retry delay', async () => {
  const response = Response.json({ error: 'Wait before syncing again' }, { status: 429, headers: { 'Retry-After': '60' } });
  await assert.rejects(readCanvas(response), { message: 'Wait before syncing again', status: 429, retryAfter: 60 });
  assert.equal(response.body.locked, false);
  await assert.rejects(readCanvas(new Response('upstream unavailable', { status: 502, statusText: 'Bad Gateway' })), { status: 502, message: 'Bad Gateway' });
  await assert.rejects(readCanvas(new Response(null, { status: 503 })), { status: 503, message: 'Canvas answered 503' });
  await assert.rejects(readCanvas(Response.json({ error: 'x'.repeat(2048) }, { status: 502 })), (error) => error.status === 502 && error.message.length === 1024);
});
