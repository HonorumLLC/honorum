// Vite replaces these values with a manifest of the complete offline shell.
const CACHE = 'honorum-__BUILD_ID__';
const PRECACHE = __PRECACHE__;
// A new build takes over as soon as it is cached; open tabs keep running the
// shell they loaded and pick up the new one on their next visit.
self.addEventListener('install', (event) => {
  event.waitUntil((async () => {
    const cache = await caches.open(CACHE);
    await Promise.all(PRECACHE.map(async (url) => {
      // Content hashes make these safe to copy from an earlier offline shell,
      // even when the browser's separate HTTP cache has been evicted.
      const saved = url.startsWith('/assets/') && await caches.match(url);
      if (saved) await cache.put(url, saved);
      else await cache.add(url);
    }));
    await self.skipWaiting();
  })());
});
self.addEventListener('activate', (event) => {
  // Keep the previous shell so already-open tabs can still lazy-load their views.
  event.waitUntil((async () => {
    const keys = (await caches.keys()).filter((key) => key.startsWith('honorum-'));
    const previous = keys.filter((key) => key !== CACHE).at(-1);
    await Promise.all(keys.filter((key) => key !== CACHE && key !== previous).map((key) => caches.delete(key)));
    await self.clients.claim();
  })());
});
self.addEventListener('fetch', (event) => {
  const request = event.request, url = new URL(request.url);
  if (request.method !== 'GET' || url.origin !== location.origin || /^\/(api|catalog|status|admin|healthz|readyz)(\/|$)/.test(url.pathname)) return;
  event.respondWith((async () => {
    const cache = await caches.open(CACHE);
    if (request.mode !== 'navigate') {
      const saved = (await cache.match(request)) || (await caches.match(request));
      if (saved) return saved;
      const response = await fetch(request);
      if (response.ok) { const copy = response.clone(); event.waitUntil(cache.put(request, copy)); }
      return response;
    }
    // Installation publishes a complete shell. Let worker updates replace that
    // shell in the background instead of waiting for HTML on every local visit.
    // Opening the app from the landing page renews the server's record that
    // this browser uses the app, then returns to the root.
    if (url.pathname === '/app') {
      try { return await fetch(request); } catch { return Response.redirect('/', 303); }
    }
    // The landing and legal pages are static; every other path is the app.
    const page = url.pathname.match(/^\/(about|legal)(?:\.html)?\/?$/)?.[1];
    const saved = await cache.match(page ? `/${page}.html` : '/');
    // Replay the hosting redirect from the installed response's final URL.
    // At that canonical URL, reconstruct the followed response for navigation.
    if (saved?.redirected) {
      const canonical = new URL(saved.url);
      if (page && canonical.origin === url.origin && canonical.pathname !== url.pathname) {
        canonical.search = url.search; canonical.hash = url.hash;
        return Response.redirect(canonical.href, 307);
      }
      return new Response(saved.body, { status: saved.status, headers: saved.headers });
    }
    if (saved) return saved;
    try { return await fetch(request); } catch {}
    return new Response('Honorum is offline. Open the app online once to save it.', { status: 503, headers: { 'Content-Type': 'text/plain' } });
  })());
});

self.addEventListener('push', (e) => {
  const d = e.data ? e.data.json() : {};
  e.waitUntil(self.registration.showNotification(d.title || 'Seat open', { body: d.body, icon: '/icon-192.png', data: d }));
});

// The browser may replace a subscription (expiry, key change). Subscribe again
// with the same server key and move this device's alerts to the new one.
async function accountOwner() {
  return new Promise((resolve) => {
    const request = indexedDB.open('honorum');
    // Reading an absent app database must not create a second schema.
    request.onupgradeneeded = () => request.transaction.abort();
    request.onerror = () => resolve('');
    request.onsuccess = () => {
      const db = request.result;
      try {
        const store = db.transaction('kv').objectStore('kv');
        const active = store.get('activeOwner');
        active.onsuccess = () => {
          // The pointer and owner record are read in the same transaction.
          // An older installation may not have migrated its global state yet.
          const read = store.get(active.result === undefined ? 'state' : `state:${active.result}`);
          read.onsuccess = () => { db.close(); resolve(read.result?.account?.id || ''); };
          read.onerror = () => { db.close(); resolve(''); };
        };
        active.onerror = () => { db.close(); resolve(''); };
      } catch { db.close(); resolve(''); }
    };
  });
}

self.addEventListener('pushsubscriptionchange', (e) => {
  const old = e.oldSubscription;
  const key = e.newSubscription?.options?.applicationServerKey || old?.options?.applicationServerKey;
  if (!old || !key) return;
  e.waitUntil(
    (e.newSubscription ? Promise.resolve(e.newSubscription) : self.registration.pushManager.subscribe({ userVisibleOnly: true, applicationServerKey: key }))
      .then(async (sub) => {
        const owner = await accountOwner();
        const headers = { 'Content-Type': 'application/json', 'X-Honorum-Account': owner };
        await Promise.all([
          fetch('/api/push/retarget', { method: 'POST', headers, body: JSON.stringify({ from: JSON.stringify(old), to: JSON.stringify(sub) }) }),
          ...(owner ? [fetch('/api/me/push', { method: 'PUT', headers, body: JSON.stringify({ subscription: JSON.stringify(sub) }) })] : []),
        ]);
      })
      .catch(() => {})
  );
});

self.addEventListener('notificationclick', (e) => {
  e.notification.close();
  e.waitUntil(clients.openWindow((e.notification.data && e.notification.data.url) || '/'));
});
