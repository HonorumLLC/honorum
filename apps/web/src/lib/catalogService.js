import { indexCatalog, search } from './catalog.js';
import { applyFilters, attributeList } from './filters.js';

// A filter sees every ranked candidate, including matches beyond the first
// page. Pages count section rows, so one course with hundreds of sections cannot
// create an unbounded worker reply or an unbounded result list.
export function searchPage(catalog, query, filters, offset = 0, limit = 40) {
  limit = Math.max(1, Math.min(100, Math.floor(Number(limit) || 40)));
  const matches = applyFilters(search(catalog, query, catalog.courses.length), filters);
  const total = matches.reduce((n, course) => n + course.sections.length, 0);
  offset = Math.max(0, Math.min(Math.floor(Number(offset) || 0), Math.max(0, Math.ceil(total / limit) - 1) * limit));
  const courses = [];
  let skip = offset, remaining = limit;
  for (const course of matches) {
    if (skip >= course.sections.length) { skip -= course.sections.length; continue; }
    const sections = course.sections.slice(skip, skip + remaining);
    courses.push({ ...course, sections });
    remaining -= sections.length; skip = 0;
    if (!remaining) break;
  }
  return { courses, total, offset };
}

export function createCatalogService({ post, read, write, fetcher = fetch }) {
  let catalog = null, key = '', generation = 0, status = 'idle', controller;
  let pending = [];
  const respond = (m) => {
    if (m.key !== key) { post({ id: m.id, error: 'The selected term changed' }); return; }
    if (!catalog) { post({ id: m.id, error: 'Classes are not available. Try loading the term again.' }); return; }
    try {
      switch (m.type) {
        case 'search': post({ id: m.id, ...searchPage(catalog, m.q, m.f, m.offset, m.limit) }); break;
        case 'sections': post({ id: m.id, sections: m.ids.map((id) => catalog.byId.get(id)).filter(Boolean) }); break;
        case 'course': post({ id: m.id, course: catalog.courses.find((c) => c.code === m.code) || null }); break;
        case 'courses': post({ id: m.id, courses: m.codes.map((code) => catalog.courses.find((c) => c.code === code)).filter(Boolean) }); break;
        default: post({ id: m.id, error: 'Unknown catalog request' });
      }
    } catch (error) { post({ id: m.id, error: error.message }); }
  };
  const flush = () => { const requests = pending; pending = []; requests.forEach(respond); };
  async function load(m) {
    controller?.abort();
    const current = ++generation;
    controller = new AbortController();
    const signal = controller.signal;
    const timeout = setTimeout(() => controller && current === generation && controller.abort(), 20000);
    key = `${m.inst}/${m.term}`; catalog = null; status = 'loading';
    // Requests from the previous catalog never run against the replacement.
    const old = pending; pending = []; old.forEach((q) => post({ id: q.id, error: 'The selected term changed' }));
    // The structure makes the term searchable; seat counts are layered on
    // top whenever they arrive, from storage or the network.
    let structure = null, seats = null, indexed, attrs;
    const publish = () => {
      if (current !== generation || !structure) return;
      if (indexed !== structure.value) { catalog = indexCatalog(structure.value); indexed = structure.value; attrs = attributeList(structure.value); }
      status = 'ready';
      if (seats) for (const course of catalog.courses) for (const section of course.sections) {
        const counts = seats.value.seats[section.id];
        section.capacity = counts?.[0] ?? 0; section.available = counts?.[1] ?? -1; section.waitlist = counts?.[2] ?? 0;
        Object.assign(catalog.byId.get(section.id), { capacity: section.capacity, available: section.available, waitlist: section.waitlist });
      }
      post({ type: 'ready', key, hash: `${structure.hash}:${seats?.hash || ''}`, courses: structure.value.courses.length, attrs, term: structure.value.term || null });
      flush();
    };
    const save = (k, value) => { void Promise.resolve().then(() => write(k, value)).catch(() => {}); };
    // Published objects are content addressed, so the HTTP cache may keep them.
    const fetchJSON = async (url) => {
      const response = await fetcher(url, { signal });
      if (!response.ok) throw Object.assign(new Error(`Classes could not be loaded (${response.status})`), { missing: response.status === 404 });
      return response.json();
    };
    const validStructure = (value) => Array.isArray(value?.courses) && value.inst === m.inst && value.term?.id === m.term;
    const validSeats = (value) => value?.inst === m.inst && value.term === m.term && !!value.seats && typeof value.seats === 'object';
    const base = `/catalog/${encodeURIComponent(m.inst)}/${encodeURIComponent(m.term)}`;
    try {
      if (!m.structureHash || !m.seatsHash) throw Object.assign(new Error('Classes have not been published for this term yet'), { missing: true });
      const structureKey = `structure:${m.inst}:${m.term}`, seatsKey = `seats:${m.inst}:${m.term}`;
      [structure, seats] = await Promise.all([read(structureKey).catch(() => null), read(seatsKey).catch(() => null)]);
      if (current !== generation) return;
      if (!validStructure(structure?.value)) structure = null;
      if (!validSeats(seats?.value)) seats = null;
      publish();
      if (!catalog) post({ type: 'progress', key, loaded: 0, total: 0 });
      const jobs = [];
      const structureJob = structure?.hash !== m.structureHash && (async () => {
        const value = await fetchJSON(`${base}.structure.${encodeURIComponent(m.structureHash)}.json`);
        if (!validStructure(value) || value.hash !== m.structureHash) throw new Error('The catalog has an invalid format');
        if (current !== generation) return;
        structure = { hash: m.structureHash, value }; save(structureKey, structure); publish();
      })();
      if (structureJob) jobs.push(structureJob);
      if (seats?.hash !== m.seatsHash) jobs.push((async () => {
        // Search needs only the structure. Without a saved one, seats wait for
        // it instead of sharing a slow connection with the download that the
        // student is waiting on, and are not fetched if it cannot be read.
        if (!structure && structureJob) {
          await structureJob.catch(() => {});
          if (current !== generation || !structure) return;
        }
        const value = await fetchJSON(`${base}.seats.${encodeURIComponent(m.seatsHash)}.json`);
        if (!validSeats(value)) throw new Error('The seat catalog has an invalid format');
        if (current !== generation) return;
        seats = { hash: m.seatsHash, value }; save(seatsKey, seats); publish();
      })());
      const outcomes = await Promise.allSettled(jobs);
      // A saved catalog stays usable when a newer publication cannot be read.
      if (current !== generation || catalog) return;
      throw outcomes.find((result) => result.status === 'rejected')?.reason || new Error('Classes could not be loaded');
    } catch (error) {
      if (current !== generation || catalog) return;
      status = error.missing ? 'missing' : 'error';
      post({ type: status, key, message: error.message }); flush();
    } finally {
      clearTimeout(timeout);
      if (current === generation) post({ type: 'settled', key, requestHash: m.requestHash });
    }
  }
  return (message) => {
    if (message.type === 'load') return load(message);
    if (status === 'loading' && message.key === key) pending.push(message);
    else respond(message);
  };
}
