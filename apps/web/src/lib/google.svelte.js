// Google Calendar, for signed-in accounts. The server holds the Google
// credentials and does the work; this module reads the connection's status
// for Settings, keeps the shown calendars' events for the week (saved on
// the device per account, so they draw offline), and asks for changes.
// Events stay out of the planner state, so a busy calendar never slows
// down saving an edit.
import { api } from './api.js';
import { get, set } from './store.js';
import { GOOGLE_PREFIX } from './googleEvents.js';

const empty = () => ({ events: [], calendars: [], etag: '', version: 0 });
export const google = $state({ owner: '', status: null, ...empty() });

let fetching = null;

const saveEvents = (owner) => set('google:' + owner, $state.snapshot({ events: google.events, calendars: google.calendars, etag: google.etag, version: google.version })).catch(() => {});

// Switch to an account's saved events (or none, signed out) before asking
// the server for newer ones.
export async function useGoogle(owner) {
  if (google.owner === owner) return;
  Object.assign(google, { owner, status: null, ...empty() });
  fetching = null;
  if (!owner) return;
  const saved = await get('google:' + owner).catch(() => null);
  if (saved && google.owner === owner) Object.assign(google, { events: saved.events || [], calendars: saved.calendars || [], etag: saved.etag || '', version: saved.version || 0 });
}

// The events of the shown calendars. An unchanged answer is a 304, so the
// minute poll costs nothing; force skips it after a settings change.
export function refreshGoogleEvents({ force = false } = {}) {
  const owner = google.owner;
  if (!owner) return Promise.resolve();
  if (fetching && !force) return fetching;
  const request = fetching = (async () => {
    try {
      const out = await api('/me/google/events', { account: owner, headers: !force && google.etag ? { 'If-None-Match': google.etag } : {} });
      if (google.owner !== owner) return;
      Object.assign(google, { events: out.events || [], calendars: out.calendars || [], etag: out.etag || '', version: out.version || 0 });
      saveEvents(owner);
    } catch (e) {
      if (e.status !== 304) throw e;
    } finally {
      if (fetching === request) fetching = null;
    }
  })();
  return request;
}

// The live stream carries the events' version; a moved one is news.
export function noteGoogleVersion(version) {
  if (version && version !== google.version) refreshGoogleEvents().catch(() => {});
}

// Only accounts that show Google events poll for them.
export const showingGoogle = () => !!google.owner && google.calendars.length > 0;

export async function loadGoogle() {
  const owner = google.owner;
  if (!owner) return null;
  const status = await api('/me/google', { account: owner });
  if (google.owner === owner) google.status = status;
  return status;
}

// Turning a direction on without its permission goes to Google's consent,
// which returns to Settings.
export async function connectGoogle(want) {
  const { url } = await api('/me/google/start', { account: google.owner, method: 'POST', body: JSON.stringify({ want, back: 'web' }) });
  location.assign(url);
}

export async function setGoogle(patch) {
  const owner = google.owner;
  const status = await api('/me/google', { account: owner, method: 'PATCH', body: JSON.stringify(patch) });
  if (google.owner !== owner) return;
  google.status = status;
  if ('show' in patch || 'calendars' in patch) await refreshGoogleEvents({ force: true }).catch(() => {});
}

export async function disconnectGoogle() {
  const owner = google.owner;
  const status = await api('/me/google', { account: owner, method: 'DELETE' });
  if (google.owner !== owner) return;
  Object.assign(google, { status, ...empty() });
  saveEvents(owner);
}

// After Google's consent, the address names a one-time handoff that this
// session turns into the connection. Returns a problem to report, if any;
// the switches say the rest.
export async function finishGoogle() {
  const params = new URLSearchParams(location.search);
  const handoff = params.get('google_handoff'), error = params.get('google_error');
  if (!handoff && !error) return '';
  params.delete('google_handoff'); params.delete('google_error');
  history.replaceState(null, '', location.pathname + (params.size ? `?${params}` : '') + location.hash);
  if (error) return error === 'denied' ? '' : 'Google Calendar could not be connected';
  if (!google.owner) return 'Sign in to connect Google Calendar';
  try {
    const owner = google.owner;
    const status = await api('/me/google/finish', { account: owner, method: 'POST', body: JSON.stringify({ handoff }) });
    if (google.owner === owner) google.status = status;
    await refreshGoogleEvents({ force: true }).catch(() => {});
    return status.granted ? '' : 'Google did not allow access';
  } catch (e) {
    return e.message;
  }
}

export const isGoogle = (id) => typeof id === 'string' && id.startsWith(GOOGLE_PREFIX);
