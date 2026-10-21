let accountOwner = () => '';
export const setAccountOwner = (read) => { accountOwner = read; };

export async function api(path, opts = {}) {
  const { account = accountOwner(), headers, timeoutMs = 25000, ...options } = opts;
  const requestHeaders = new Headers(headers);
  if (!requestHeaders.has('Content-Type')) requestHeaders.set('Content-Type', 'application/json');
  if (path !== '/auth/code' && path !== '/auth/verify') requestHeaders.set('X-Honorum-Account', account);
  const timeout = AbortSignal.timeout(timeoutMs);
  const signal = opts.signal ? AbortSignal.any([opts.signal, timeout]) : timeout;
  let r;
  try {
    r = await fetch('/api' + path, { ...options, headers: requestHeaders, signal });
  } catch (cause) {
    if (opts.signal?.aborted) throw opts.signal.reason;
    const e = new Error(timeout.aborted ? 'The request timed out. Try again' : 'The server could not be reached', { cause });
    e.status = 0;
    throw e;
  }
  if (r.status === 204) return null;
  let body = null;
  try { body = await r.json(); } catch {}
  if (!r.ok) {
    const e = new Error(body?.error || r.statusText || `Request failed (${r.status})`);
    e.status = r.status;
    e.retryAfter = Number(r.headers.get('Retry-After')) || 0;
    throw e;
  }
  return body;
}

// A name for this device in the account's device list: "Firefox on Linux".
export function deviceName() {
  const ua = navigator.userAgent;
  const browser = /Edg\//.test(ua) ? 'Edge' : /OPR\//.test(ua) ? 'Opera' : /Firefox\//.test(ua) ? 'Firefox' : /Chrome\//.test(ua) ? 'Chrome' : /Safari\//.test(ua) ? 'Safari' : 'Browser';
  const os = /iPhone/.test(ua) ? 'iPhone' : /iPad/.test(ua) || (/Macintosh/.test(ua) && navigator.maxTouchPoints > 1) ? 'iPad' : /Android/.test(ua) ? 'Android' : /Windows/.test(ua) ? 'Windows' : /Mac OS/.test(ua) ? 'Mac' : /CrOS/.test(ua) ? 'Chromebook' : /Linux/.test(ua) ? 'Linux' : '';
  return os ? `${browser} on ${os}` : browser;
}

const b64url = (bytes) => btoa(String.fromCharCode(...bytes)).replace(/\+/g, '-').replace(/\//g, '_').replace(/=+$/, '');
const unb64url = (s) => Uint8Array.from(atob(s.replace(/-/g, '+').replace(/_/g, '/')), (c) => c.charCodeAt(0));

// A push subscription bound to the server's current VAPID key. The browser
// keeps at most one subscription per site, so one created under an older key
// (the server's keys changed, or a different server ran on this origin) is
// replaced; the server would otherwise be refused by the push service.
export async function pushSubscription(vapid) {
  if (!('serviceWorker' in navigator) || !('PushManager' in window)) throw new Error('This browser does not support push notifications.');
  const key = unb64url(vapid);
  const reg = await Promise.race([
    navigator.serviceWorker.register('/sw.js').then(() => navigator.serviceWorker.ready),
    new Promise((_, rej) => setTimeout(() => rej(new Error('The app could not start its notification worker. Reload the page and try again.')), 15000)),
  ]);
  let sub = await reg.pushManager.getSubscription();
  if (sub && !sameKey(sub.options?.applicationServerKey, key)) {
    await sub.unsubscribe().catch(() => {});
    sub = null;
  }
  if (!sub) {
    try {
      sub = await reg.pushManager.subscribe({ userVisibleOnly: true, applicationServerKey: key });
    } catch (e) {
      throw new Error(explainPush(e));
    }
  }
  return JSON.stringify(sub);
}

// The subscription the browser currently holds, without asking for permission
// or creating one; '' when there is none.
export async function currentPushSubscription() {
  if (!('serviceWorker' in navigator) || !('PushManager' in window)) return '';
  const reg = await navigator.serviceWorker.getRegistration('/');
  const sub = reg && (await reg.pushManager.getSubscription());
  return sub ? JSON.stringify(sub) : '';
}

function sameKey(a, b) {
  if (!a) return true; // some browsers do not expose the key; trust the subscription
  const x = new Uint8Array(a);
  return x.length === b.length && x.every((v, i) => v === b[i]);
}

// Browsers report push-service trouble as a terse DOMException; say what it means.
function explainPush(e) {
  const msg = String(e?.message || e);
  if (e?.name === 'NotAllowedError' || /permission/i.test(msg)) return 'Notifications are blocked for this site. Allow them in the browser’s site settings, then try again.';
  if (/registration failed|push service|abort/i.test(msg) || e?.name === 'AbortError') {
    return 'This browser could not register with its push service, so it cannot receive notifications. Browsers built without Google services (many Linux Chromium packages, some privacy browsers) have no push service; try Firefox, Chrome, Edge or Safari.';
  }
  return `Could not set up push notifications: ${msg}`;
}

// Shared schedules use a random 128-bit secret. The server stores ciphertext
// under a hash of the secret; the secret travels only in the link fragment.
export const newSecret = () => b64url(crypto.getRandomValues(new Uint8Array(16)));

async function keys(secret) {
  const raw = unb64url(secret);
  const digest = async (label) => new Uint8Array(await crypto.subtle.digest('SHA-256', new Uint8Array([...new TextEncoder().encode(label), ...raw])));
  const id = [...(await digest('id')).slice(0, 16)].map((b) => b.toString(16).padStart(2, '0')).join('');
  const key = await crypto.subtle.importKey('raw', await digest('key'), 'AES-GCM', false, ['encrypt', 'decrypt']);
  return { id, key };
}

export async function syncPush(secret, data) {
  const { id, key } = await keys(secret);
  const iv = crypto.getRandomValues(new Uint8Array(12));
  const ct = new Uint8Array(await crypto.subtle.encrypt({ name: 'AES-GCM', iv }, key, new TextEncoder().encode(JSON.stringify(data))));
  const response = await fetch(`/api/sync/${id}`, { method: 'PUT', body: new Uint8Array([...iv, ...ct]) });
  if (!response.ok) throw new Error('The shared schedule could not be saved');
}

export async function syncPull(secret) {
  const { id, key } = await keys(secret);
  const r = await fetch(`/api/sync/${id}`);
  if (!r.ok) return null;
  const buf = new Uint8Array(await r.arrayBuffer());
  const pt = await crypto.subtle.decrypt({ name: 'AES-GCM', iv: buf.slice(0, 12) }, key, buf.slice(12));
  return JSON.parse(new TextDecoder().decode(pt));
}

export const shareLink = (secret) => `${location.origin}/#share=${secret}`;
