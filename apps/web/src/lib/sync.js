import { get, setMany } from './store.js';
import { api } from './api.js';

// Another client may keep fields here that this one does not read, such as the
// iOS watchlist. They ride along so an upload from the web never drops them.
const PREF_KEYS = ['inst', 'term', 'current', 'onboarded', 'reminders', 'map'];

export function docsOf(state) {
  const out = {};
  for (const s of state.schedules || []) out['s_' + s.id] = { type: 'schedule', body: s };
  for (const t of state.tasks || []) out['t_' + t.id] = { type: 'task', body: t };
  out.prefs = {
    type: 'prefs',
    body: { ...state.prefExtras, inst: state.inst || '', term: state.term || '', current: state.current || '', onboarded: !!state.onboarded, reminders: !!state.reminders?.on, map: state.lms?.map || {} },
  };
  return out;
}

export function diff(docs, known, now) {
  const out = [];
  for (const [id, d] of Object.entries(docs)) {
    const body = JSON.stringify(d.body);
    if (known[id]?.deleted || known[id]?.body !== body) out.push({ id, type: d.type, base: known[id]?.rev || 0, updated: now, body: d.body });
  }
  for (const id of Object.keys(known)) if (!docs[id] && !known[id].deleted) out.push({ id, type: known[id].type, base: known[id].rev, updated: now, deleted: true });
  return out;
}

// Restore account navigation only during the first download.
export function applyDoc(app, d, nav = false) {
  const body = d.body;
  if (d.id.startsWith('s_') || d.id.startsWith('t_')) {
    const list = d.id.startsWith('s_') ? app.schedules : app.tasks;
    const id = d.id.slice(2);
    const i = list.findIndex((x) => x.id === id);
    if (d.deleted) { if (i >= 0) list.splice(i, 1); }
    else if (i >= 0) list[i] = body;
    else list.push(body);
  } else if (d.id === 'prefs' && !d.deleted && body) {
    if (body.onboarded) app.onboarded = true;
    app.reminders.on = !!body.reminders;
    app.lms.map = body.map || {};
    app.prefExtras = Object.fromEntries(Object.entries(body).filter(([key]) => !PREF_KEYS.includes(key)));
    if (nav) {
      if (body.inst) app.inst = body.inst;
      if (body.term) app.term = body.term;
      if (body.current) app.current = body.current;
    }
  }
}

// Objects in a list keyed by their string ids, or null when any lacks a unique one.
function byId(list) {
  const items = new Map();
  for (const x of list) {
    if (typeof x?.id !== 'string' || items.has(x.id)) return null;
    items.set(x.id, x);
  }
  return items;
}

// Three-way merge preserves independent fields and additions/removals in ID
// lists; lists of objects with ids, such as personal events, merge item by item.
// Same-field conflicts retain the local intent and archive the remote version.
export function mergeBody(base, local, remote) {
  const equal = (a, b) => JSON.stringify(a) === JSON.stringify(b);
  let conflicted = false;
  function merge(b, l, r) {
    if (equal(l, r) || equal(r, b)) return l;
    if (equal(l, b)) return r;
    if ([b, l, r].every(Array.isArray) && [...b, ...l, ...r].every((x) => typeof x === 'string')) {
      return [...new Set([...l.filter((x) => !b.includes(x) || r.includes(x)), ...r.filter((x) => !b.includes(x))])];
    }
    const lists = [b, l, r].every(Array.isArray) && [b, l, r].map(byId);
    if (lists && lists.every(Boolean)) {
      const [bm, lm, rm] = lists, out = [];
      // An item removed on one side goes, unless the other side edited it.
      for (const [id, item] of lm) {
        if (rm.has(id)) out.push(merge(bm.get(id), item, rm.get(id)));
        else if (!bm.has(id)) out.push(item);
        else if (!equal(item, bm.get(id))) { conflicted = true; out.push(item); }
      }
      for (const [id, item] of rm) {
        if (lm.has(id)) continue;
        if (!bm.has(id)) out.push(item);
        else if (!equal(item, bm.get(id))) conflicted = true;
      }
      return out;
    }
    if ([b, l, r].every((x) => x && typeof x === 'object' && !Array.isArray(x))) {
      return Object.fromEntries([...new Set([...Object.keys(b), ...Object.keys(l), ...Object.keys(r)])]
        .map((key) => [key, merge(b[key], l[key], r[key])]).filter(([, value]) => value !== undefined));
    }
    conflicted = true;
    return l;
  }
  return { body: merge(base, local, remote), conflicted };
}

// Count UTF-8 bytes, since JSON character counts understate non-ASCII uploads.
export function uploadBatch(outbox, maxBytes = (4 << 20) - 2048) {
  const docs = [], encoder = new TextEncoder();
  let size = 0;
  for (const d of Object.values(outbox)) {
    const bytes = encoder.encode(JSON.stringify(d)).byteLength + 1;
    if (docs.length === 200 || size + bytes > maxBytes) {
      if (!docs.length) throw new Error('A saved document is too large to sync');
      break;
    }
    docs.push(d); size += bytes;
  }
  return structuredClone(docs);
}

const browserEvents = typeof EventSource === 'undefined' ? null : (url) => new EventSource(url);

export function createSyncEngine({ read = get, write = setMany, request = api, delay = setTimeout, cancel = clearTimeout, online = () => typeof navigator === 'undefined' || navigator.onLine !== false, events = browserEvents } = {}) {
  const fresh = () => ({ seq: 0, known: {}, outbox: {}, conflicts: [], owner: '', adopting: false });
  let store = fresh(), app, hooks, timer, active, epoch = 0, backoff = 0, suspended = false;
  let writes = Promise.resolve(), preparing, savedNamespace, live;
  let controller = new AbortController();
  let inFlight = new Map();
  const owner = () => app?.account?.id || '';
  const save = (data = hooks.snapshot()) => {
    // Capture before awaiting: later mutations must never change this transaction.
    const namespace = data.dataOwner || data.account?.id || 'guest';
    const entries = structuredClone([['state:' + namespace, data], ['sync:' + namespace, store]]);
    const operation = writes.catch(() => {}).then(() => write(savedNamespace === namespace ? entries : [...entries, ['activeOwner', namespace]])).then(() => { savedNamespace = namespace; });
    writes = operation;
    return operation;
  };
  const remember = (d) => {
    store.known[d.id] = { rev: d.rev, type: d.type, deleted: !!d.deleted, body: d.deleted ? '' : JSON.stringify(d.body) };
  };
  async function initSync(a, h) {
    restart();
    app = a; hooks = h; suspended = false; savedNamespace = undefined;
    store = { ...fresh(), ...await read('sync') };
  }
  function restart() {
    controller.abort(); controller = new AbortController();
    epoch++;
    inFlight = new Map();
    active = null;
    preparing = null;
    cancel(timer);
    backoff = 0;
    if (live) { live.close(); live = null; }
    return epoch;
  }
  // The server announces the account's document sequence after every commit.
  // The stream is only a wakeup: a moved sequence triggers the ordinary sync,
  // and the browser reconnects the stream itself after a drop.
  function connectLive(generation) {
    if (!events || live || !valid(generation)) return;
    let source;
    try { source = events(`/api/me/events?account=${encodeURIComponent(owner())}`); } catch { return; }
    live = source;
    source.addEventListener('seq', (event) => {
      if (live !== source || !valid(generation)) return;
      try {
        const frame = JSON.parse(event.data);
        if (frame.seq > store.seq) syncSoon(0);
        if (frame.google) hooks.googleVersion?.(frame.google);
      } catch {}
    });
  }
  function resetSync(preserve = false) {
    restart();
    if (!preserve) store = fresh();
    return save();
  }
  const pending = () => Object.keys(store.outbox).length;
  const conflicts = () => structuredClone(store.conflicts);
  function queue(data) {
    const docs = docsOf(data);
    const known = store.adopting ? { ...store.adoptionBase, ...store.known } : store.known;
    const changes = diff(docs, known, Date.now());
    const next = Object.fromEntries(changes.map((d) => [d.id, d]));
    // A user can revert to the confirmed version while another version is uploading.
    for (const [id, sent] of inFlight) if (!next[id]) {
      const d = docs[id];
      next[id] = { id, type: d?.type || sent.type, base: store.known[id]?.rev || 0, updated: Date.now(), ...(d ? { body: d.body } : { deleted: true }) };
    }
    for (const [id, d] of Object.entries(next)) {
      const previous = store.outbox[id];
      if (previous && previous.deleted === d.deleted && JSON.stringify(previous.body) === JSON.stringify(d.body)) next[id] = previous;
    }
    store.outbox = next;
  }
  async function noteChange(data) {
    const generation = epoch;
    if (preparing) await preparing;
    if (generation !== epoch) return;
    if (app.account) {
      if (store.owner && store.owner !== owner()) throw new Error('Account changed; sign in again before syncing');
      store.owner = owner();
      queue(data);
    }
    await save(data);
    if (app.account && pending()) { app.sync.status = 'pending'; syncSoon(); }
  }
  // Edits upload shortly after the last one, together when made in a burst.
  function syncSoon(ms = 300) {
    cancel(timer);
    timer = delay(() => { sync().catch(() => {}); }, ms);
  }
  function valid(generation) { return generation === epoch && !suspended && !!app.account && (!store.owner || store.owner === owner()); }
  function reconcile(remote, nav = false) {
    const mine = store.outbox[remote.id];
    if (!mine) return;
    const known = store.known[remote.id] || store.adoptionBase?.[remote.id];
    const base = known?.body ? JSON.parse(known.body) : undefined;
    const same = !!mine.deleted === !!remote.deleted && JSON.stringify(mine.body) === JSON.stringify(remote.body);
    if (same) { remember(remote); delete store.outbox[remote.id]; return; }
    const merged = mergeBody(base, mine.deleted ? undefined : mine.body, remote.deleted ? undefined : remote.body);
    if (merged.conflicted) {
      store.conflicts.push({ ...remote, savedAt: Date.now() });
      hooks.conflict?.();
    }
    remember(remote);
    const next = { id: mine.id, type: mine.type, base: remote.rev, updated: mine.updated,
      ...(merged.body === undefined ? { deleted: true } : { body: merged.body }) };
    applyDoc(app, next, nav);
    if (!!next.deleted === !!remote.deleted && JSON.stringify(next.body) === JSON.stringify(remote.body)) delete store.outbox[remote.id];
    else store.outbox[remote.id] = next;
  }
  function acknowledge(res, sentVersions) {
    for (const a of res.applied || []) {
      const sent = inFlight.get(a.id);
      if (!sent) continue;
      remember({ ...sent, rev: a.rev });
      const current = store.outbox[a.id];
      if (current === sentVersions.get(a.id)) delete store.outbox[a.id];
      else if (current) current.base = a.rev;
    }
    for (const remote of res.conflicts || []) reconcile(remote);
    inFlight.clear();
  }
  async function receive(res, nav, uploaded = false) {
    let changed = uploaded, viewChanged = false, tasks = false;
    for (const d of res.docs || []) {
      if (store.known[d.id]?.rev === d.rev) continue;
      if (d.type === 'task') tasks = true;
      if (store.outbox[d.id]) reconcile(d, nav);
      else { applyDoc(app, d, nav); remember(d); }
      changed = viewChanged = true;
    }
    if (res.seq !== store.seq) { store.seq = res.seq; changed = true; }
    if (res.watches_version && res.watches_version !== store.watchesVersion) {
      store.watchesVersion = res.watches_version; changed = true;
    }
    if (res.watches && hooks.watches(res.watches) !== false) changed = viewChanged = true;
    if (viewChanged || (res.conflicts || []).length) hooks.after();
    if (changed) await save(); // documents and the download cursor still commit together
    if (tasks) hooks.tasksSynced?.();
  }
  async function pull(generation, nav = false) {
    let more = true;
    while (more && valid(generation)) {
      const res = await request('/me/sync', { account: owner(), method: 'POST', signal: controller.signal,
        body: JSON.stringify({ since: store.seq, watches_version: store.watchesVersion || '', docs: [] }) });
      if (!valid(generation)) return;
      await receive(res, nav);
      more = res.more;
    }
  }
  async function exchange(generation) {
    for (let round = 0; round < 4 && valid(generation); round++) {
      const docs = uploadBatch(store.outbox);
      inFlight = new Map(docs.map((d) => [d.id, d]));
      const sentVersions = new Map(docs.map((d) => [d.id, store.outbox[d.id]]));
      const res = await request('/me/sync', { account: owner(), method: 'POST', signal: controller.signal,
        body: JSON.stringify({ since: store.seq, watches_version: store.watchesVersion || '', docs }) });
      if (!valid(generation)) return;
      acknowledge(res, sentVersions);
      await receive(res, false, docs.length > 0);
      if (docs.some((d) => d.type === 'task')) hooks.tasksSynced?.();
      if (res.more) await pull(generation);
      if (!pending()) return;
    }
  }
  async function sync() {
    if (preparing) await preparing;
    if (!app?.account || suspended) return;
    if (active) return active;
    if (!online()) { app.sync.status = 'offline'; return; }
    const generation = epoch;
    active = (async () => {
      app.sync.status = 'syncing';
      try {
        await writes;
        if (!valid(generation)) return;
        store.owner = owner();
        if (store.adopting) {
          await pull(generation, true);
          if (!valid(generation)) return;
          const planned = new Set(app.schedules.filter((s) => store.known['s_' + s.id]).map((s) => s.inst + '/' + s.term));
          app.schedules = app.schedules.filter((s) => store.known['s_' + s.id] || s.sections.length || s.blocks?.length || !planned.has(s.inst + '/' + s.term));
          store.adopting = false;
          delete store.adoptionBase;
          hooks.after();
          queue(hooks.snapshot());
          await save();
          if (!valid(generation)) return;
        }
        await exchange(generation);
        if (!valid(generation)) return;
        app.sync = { status: pending() ? 'pending' : 'ok', at: Date.now(), error: '' };
        backoff = 0;
        connectLive(generation);
        if (pending()) syncSoon(200);
      } catch (e) {
        if (!valid(generation)) return;
        if (e.status === 401 || e.status === 409) { await hooks.signedOut(); return; }
        app.sync = { status: online() ? 'error' : 'offline', at: app.sync.at, error: e.message };
        backoff = Math.min((backoff || 2500) * 2, 300000);
        syncSoon(backoff);
      } finally { if (generation === epoch) { inFlight.clear(); active = null; } }
    })();
    return active;
  }
  async function adopt() {
    const generation = restart(), accountID = owner();
    suspended = false;
    const loading = (async () => {
      const restored = await read('sync:' + accountID);
      if (generation !== epoch || accountID !== owner()) return;
      store = restored?.owner === accountID ? { ...fresh(), ...restored } : { ...fresh(), owner: accountID, adopting: true };
      if (store.adopting && !store.adoptionBase) {
        // Keep edits made during the first download separate from imported guest data.
        store.adoptionBase = Object.fromEntries(Object.entries(docsOf(hooks.snapshot()))
          .map(([id, doc]) => [id, { type: doc.type, rev: 0, body: JSON.stringify(doc.body) }]));
      }
      queue(hooks.snapshot());
      await save();
    })();
    preparing = loading;
    try { await loading; }
    finally { if (preparing === loading) preparing = null; }
    if (valid(generation)) syncSoon(0);
  }
  async function suspendSync() {
    suspended = true;
    restart();
    await writes;
  }
  return { initSync, resetSync, pending, conflicts, noteChange, syncSoon, sync, adopt, suspendSync };
}
export const { initSync, resetSync, pending, conflicts: syncConflicts, noteChange, syncSoon, sync, adopt, suspendSync } = createSyncEngine();
