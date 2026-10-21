const MAX_BYTES = 8 << 20;
const MAX_ERROR_LENGTH = 1024;

const invalid = () => new Error('Canvas returned an invalid sync response');
function parse(text) {
  try { return JSON.parse(text); } catch { throw invalid(); }
}
function taskList(tasks) {
  if (!Array.isArray(tasks) || tasks.some((task) => !task || typeof task !== 'object' || Array.isArray(task))) throw invalid();
  return tasks;
}

// A preview is safe to display; only a complete event permits reconciliation.
// Read through EOF so duplicate or trailing events cannot become a successful sync.
export async function readCanvas(response, onTasks) {
  const reader = response.body?.getReader();
  let buffer = '', bytes = 0, stage = 'start', finalTasks;
  const streaming = response.ok && response.headers.get('Content-Type')?.split(';')[0].trim().toLowerCase() === 'application/x-ndjson';
  const decoder = new TextDecoder('utf-8', { fatal: true });
  const httpError = (message) => {
    const error = new Error((message || response.statusText || `Canvas answered ${response.status}`).slice(0, MAX_ERROR_LENGTH));
    error.status = response.status;
    error.retryAfter = Number(response.headers.get('Retry-After')) || 0;
    return error;
  };
  const event = (line) => {
    if (!line.trim()) return;
    const item = parse(line);
    if (!item || typeof item !== 'object' || Array.isArray(item) || stage === 'complete') throw invalid();
    if (item.type === 'error') {
      if (typeof item.error !== 'string' || !item.error.trim()) throw invalid();
      throw new Error(item.error.slice(0, MAX_ERROR_LENGTH));
    }
    if (item.type === 'tasks' && stage === 'start') {
      const tasks = taskList(item.tasks);
      stage = 'preview';
      onTasks?.(tasks);
    } else if (item.type === 'complete' && stage === 'preview') {
      finalTasks = taskList(item.tasks);
      stage = 'complete';
    } else throw invalid();
  };
  try {
    if (!reader) throw response.ok ? invalid() : httpError();
    if (response.ok && !streaming) throw invalid();
    while (true) {
      const { done, value } = await reader.read();
      if (!done) {
        bytes += value.byteLength;
        if (bytes > MAX_BYTES) throw new Error('Canvas returned too much sync data');
      }
      try { buffer += done ? decoder.decode() : decoder.decode(value, { stream: true }); }
      catch { throw invalid(); }
      if (streaming) {
        let end;
        while ((end = buffer.indexOf('\n')) !== -1) {
          const line = buffer.slice(0, end);
          buffer = buffer.slice(end + 1);
          event(line);
        }
      }
      if (done) break;
    }
    if (!response.ok) {
      let message;
      try {
        const body = JSON.parse(buffer);
        if (typeof body?.error === 'string') message = body.error;
      } catch {}
      throw httpError(message);
    }
    event(buffer);
    if (stage !== 'complete') throw new Error('Canvas sync ended before it completed');
    return finalTasks;
  } catch (error) {
    if (!response.ok && error instanceof Error && error.status === undefined) error.status = response.status;
    throw error;
  } finally {
    if (reader) {
      try { await reader.cancel(); } catch {}
      reader.releaseLock();
    }
  }
}
