import type { CatalogQuery, CatalogReply, Course, Filters, Section } from './domain';
let worker: Worker | undefined, seq = 0, key = '';
let requested = '', loading = false;
let listener: (message: CatalogReply) => void = () => {};
const waiting = new Map<number, { resolve: (reply: CatalogReply) => void; reject: (error: Error) => void; timer: ReturnType<typeof setTimeout> }>();
function rejectAll(message: string) {
  for (const pending of waiting.values()) { clearTimeout(pending.timer); pending.reject(new Error(message)); }
  waiting.clear();
}
function connection() {
  if (!worker) {
    worker = new Worker(new URL('./catalog.worker.ts', import.meta.url), { type: 'module' });
    worker.onmessage = ({ data }: MessageEvent<CatalogReply>) => {
      if (data.type === 'settled') { if (data.requestHash === requested) loading = false; return; }
      if (data.id !== undefined) {
        const pending = waiting.get(data.id);
        if (!pending) return;
        clearTimeout(pending.timer); waiting.delete(data.id);
        data.error ? pending.reject(new Error(data.error)) : pending.resolve(data);
      } else listener(data);
    };
    worker.onerror = () => {
      loading = false;
      rejectAll('The catalog worker could not start'); worker?.terminate(); worker = undefined;
      listener({ type: 'error', key, message: 'The catalog worker could not start' });
    };
  }
  return worker;
}
export const onCatalog = (fn: (message: CatalogReply) => void) => { listener = fn; };
// The hashes name the term's current structure and seat publications.
export function loadCatalog(inst: string, term: string, structureHash: string, seatsHash: string) {
  const requestHash = JSON.stringify([inst, term, structureHash, seatsHash]);
  if (loading && requested === requestHash) return;
  requested = requestHash; loading = true;
  rejectAll('The selected term changed'); key = `${inst}/${term}`;
  connection().postMessage({ type: 'load', inst, term, structureHash, seatsHash, requestHash });
}
function ask(message: CatalogQuery): Promise<CatalogReply> {
  return new Promise((resolve, reject) => {
    const id = ++seq;
    const timer = setTimeout(() => { waiting.delete(id); reject(new Error('Classes took too long to load. Try again.')); }, 25000);
    waiting.set(id, { resolve, reject, timer });
    connection().postMessage({ ...JSON.parse(JSON.stringify(message)), id, key });
  });
}
export const searchCatalog = (q: string, f: Filters, offset = 0, limit = 40) => ask({ type: 'search', q, f, offset, limit }).then((reply) => ({ courses: reply.courses || [], total: Number(reply.total) || 0, offset: Number(reply.offset) || 0 }));
export const getSections = (ids: string[]): Promise<Section[]> => ask({ type: 'sections', ids }).then((reply) => reply.sections || []);
export const getCourse = (code: string): Promise<Course | null> => ask({ type: 'course', code }).then((reply) => reply.course || null);
export const getCourses = (codes: string[]): Promise<Course[]> => ask({ type: 'courses', codes }).then((reply) => reply.courses || []);
