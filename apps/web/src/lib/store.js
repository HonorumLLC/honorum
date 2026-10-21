// Account state and pending edits share an IndexedDB transaction.
const NAME = 'honorum', STORE = 'kv';
let connection;
let invalidated = false;
const resetListeners = new Set();
export const onStorageReset = (listener) => { resetListeners.add(listener); return () => resetListeners.delete(listener); };
function invalidate() {
  if (invalidated) return;
  invalidated = true;
  connection = undefined;
  for (const listener of resetListeners) listener();
}
function open() {
  if (invalidated) return Promise.reject(new Error('Site data was cleared. Reload the page'));
  if (!connection) connection = new Promise((resolve, reject) => {
    const request = indexedDB.open(NAME, 2);
    request.onupgradeneeded = () => { if (!request.result.objectStoreNames.contains(STORE)) request.result.createObjectStore(STORE); };
    request.onsuccess = () => {
      const db = request.result;
      // An open tab must never recreate deleted storage from its old memory.
      db.onversionchange = () => { db.close(); invalidate(); };
      db.onclose = invalidate;
      // Upgrade the former active aliases in one transaction. Existing owner
      // caches remain separate; the active copy was the latest committed pair.
      const tx = db.transaction(STORE, 'readwrite'), store = tx.objectStore(STORE);
      tx.oncomplete = () => resolve(db);
      tx.onabort = () => { db.close(); connection = undefined; reject(tx.error || new Error('Local storage upgrade failed')); };
      const active = store.get('activeOwner');
      active.onsuccess = () => {
        if (active.result !== undefined) return;
        const state = store.get('state'), sync = store.get('sync');
        sync.onsuccess = () => {
          const owner = state.result?.dataOwner || state.result?.account?.id || 'guest';
          if (state.result !== undefined) store.put(state.result, 'state:' + owner);
          if (sync.result !== undefined) store.put(sync.result, 'sync:' + owner);
          store.put(owner, 'activeOwner');
          store.delete('state'); store.delete('sync');
        };
      };
    };
    request.onerror = () => { connection = undefined; reject(request.error); };
  });
  return connection;
}

async function transaction(mode, operation) {
  const db = await open();
  if (invalidated) throw new Error('Site data was cleared. Reload the page');
  return new Promise((resolve, reject) => {
    const tx = db.transaction(STORE, mode);
    let result;
    tx.oncomplete = () => resolve(result);
    tx.onabort = () => reject(tx.error || new Error('Local storage transaction was cancelled'));
    tx.onerror = () => {}; // abort reports the transaction's failure once
    try { operation(tx.objectStore(STORE), (value) => { result = value; }); }
    catch (error) { tx.abort(); reject(error); }
  });
}

export const get = (key) => transaction('readonly', (store, done) => {
  if (key === 'state' || key === 'sync') {
    const active = store.get('activeOwner');
    active.onsuccess = () => {
      const request = store.get(key + ':' + (active.result || 'guest'));
      request.onsuccess = () => done(request.result);
    };
  } else {
    const request = store.get(key);
    request.onsuccess = () => done(request.result);
  }
});
export const setMany = (entries) => transaction('readwrite', (store) => {
  for (const [key, value] of entries) store.put(value, key);
});
export const set = (key, value) => setMany([[key, value]]);
