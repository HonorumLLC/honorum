import { test, expect, type Page } from '@playwright/test';
import { open, openSettings, settingsPage } from './helpers';

// Google Calendar needs an account and a server configured for it; the
// Rust tests cover the server. These tests mock its endpoints.
const me = { id: 'google-account', phone: '+12015550123', device: 'this-device' };

type Status = { connected: boolean; email: string; error: string; publish: Record<string, unknown>; show: { on: boolean; granted: boolean; calendars: Record<string, unknown>[] } };
const disconnected = (): Status => ({ connected: false, email: '', error: '', publish: { on: false, granted: false, tasks: true, synced: 0, error: '' }, show: { on: false, granted: false, calendars: [] } });

// A weekday of the week on screen, at hours in Chicago, the test school's campus.
function thisWeek(day: number, hours: [number, number]) {
  const now = new Date();
  const monday = new Date(Date.UTC(now.getUTCFullYear(), now.getUTCMonth(), now.getUTCDate() - ((now.getUTCDay() + 6) % 7) + day));
  const at = (h: number) => new Date(monday.getTime() + (h + 5) * 3600e3).toISOString();
  return { start: at(hours[0]), end: at(hours[1]) };
}

async function signIn(page: Page, offered = true) {
  await page.route('**/api/config', async route => {
    const response = await route.fetch();
    await route.fulfill({ json: { ...await response.json(), phone_auth: true, google_calendar: offered } });
  });
  await page.route('**/api/auth/code', route => route.fulfill({ status: 202, json: { retry_after: 30 } }));
  await page.route('**/api/auth/verify', route => route.fulfill({ json: me }));
  await page.route(/\/api\/me\/sync(\?.*)?$/, route => route.fulfill({ json: { applied: [], conflicts: [], docs: [], seq: 0, more: false } }));
  await page.route('**/api/me/activity', route => route.fulfill({ json: { current: 0, longest: 0, today: 'clean', last_day: '', timezone: 'America/Chicago', overdue: 0, days: [] } }));
  await page.route('**/api/me/events?**', route => route.fulfill({ status: 204 }));
  await open(page);
  await openSettings(page, 'Account');
  await page.getByRole('button', { name: 'Sign In', exact: true }).click();
  const dialog = page.getByRole('dialog', { name: 'Sign In', exact: true });
  await dialog.getByLabel('Mobile number', { exact: true }).fill(me.phone);
  await dialog.getByRole('button', { name: 'Send Code', exact: true }).click();
  await page.getByRole('dialog', { name: 'Enter the Code', exact: true }).getByLabel('Code', { exact: true }).fill('123456');
  await expect(page.getByRole('dialog', { name: 'Enter the Code', exact: true })).not.toBeVisible();
}

test('Google Calendar connects each direction on its own and shows events on the week', async ({ page }) => {
  let status = disconnected();
  const patches: unknown[] = [], starts: unknown[] = [];
  const calendars = [{ id: 'me@example.com', name: 'Me', color: '#9fe1e7', primary: true, on: true, synced: 1, error: '' }, { id: 'work', name: 'Shifts', color: '#f83a22', primary: false, on: false, synced: 0, error: '' }];
  const dentist = { id: 'a', calendar: 'me@example.com', title: 'Dentist', location: 'Main St', link: 'https://calendar.google.com/event?eid=a', ...thisWeek(2, [15, 16]) };
  const shift = { id: 'w', calendar: 'work', title: 'Shift', ...thisWeek(3, [9, 11]) };
  const shown = () => status.show.on ? calendars.filter(c => c.on) : [];
  await page.route('**/api/me/google', async route => {
    const method = route.request().method();
    if (method === 'PATCH') {
      const patch = route.request().postDataJSON();
      patches.push(patch);
      if ('publish' in patch) status.publish = { ...status.publish, on: patch.publish, synced: 0 };
      if (patch.calendars) for (const c of calendars) if (c.id in patch.calendars) c.on = patch.calendars[c.id];
      status.show.calendars = calendars.map(c => ({ ...c }));
    }
    if (method === 'DELETE') status = disconnected();
    await route.fulfill({ json: { available: true, ...status } });
  });
  // Consent itself is Google's page; it ends by sending the browser back to
  // Settings with a handoff for this session, which is where this goes.
  await page.route('**/api/me/google/start', async route => {
    starts.push(route.request().postDataJSON());
    await route.fulfill({ json: { url: '/?google_handoff=one-time#settings/schedule' } });
  });
  // Finishing turns on the direction just granted, as the server does.
  await page.route('**/api/me/google/finish', async route => {
    expect(route.request().postDataJSON()).toEqual({ handoff: 'one-time' });
    const want = (starts.at(-1) as { want: string }).want;
    status = { ...status, connected: true, email: 'student@gmail.com',
      publish: want === 'publish' ? { ...status.publish, on: true, granted: true } : status.publish,
      show: want === 'show' ? { on: true, granted: true, calendars: calendars.map(c => ({ ...c })) } : status.show };
    await route.fulfill({ json: { available: true, ...status, granted: true } });
  });
  await page.route('**/api/me/google/events', route => {
    const on = shown().map(c => c.id);
    route.fulfill({ json: { version: on.length, etag: `"v${on.length}"`, calendars: shown(), events: [dentist, shift].filter(e => on.includes(e.calendar)) } });
  });

  // Signing in happens in Settings, which stays open.
  await signIn(page);
  await settingsPage(page, 'Schedule');
  const settings = page.getByRole('dialog', { name: 'Settings', exact: true });
  const showEvents = settings.getByRole('checkbox', { name: /^Show Google Events/ });
  await expect(showEvents).not.toBeChecked();
  await showEvents.click();
  // Consent for showing events only, then back to Settings, connected, with
  // nothing to announce.
  await expect(settings.getByText('student@gmail.com', { exact: true })).toBeVisible();
  expect(starts).toEqual([{ want: 'show', back: 'web' }]);
  expect(new URL(page.url()).search).toBe('');
  await expect(page.locator('.toast')).toHaveCount(0);
  await expect(settings.getByRole('checkbox', { name: /^Show Google Events/ })).toBeChecked();
  await expect(settings.getByRole('checkbox', { name: 'Show Me' })).toBeChecked();
  await settings.getByRole('checkbox', { name: 'Show Shifts' }).click();
  await expect(settings.getByRole('checkbox', { name: 'Show Shifts' })).toBeChecked();
  expect(patches).toEqual([{ calendars: { work: true } }]);

  // Publishing was not permitted by that consent, so it asks Google again.
  const publish = settings.getByRole('checkbox', { name: /^Add Classes to Google Calendar/ });
  await publish.click();
  await expect.poll(() => starts.length).toBe(2);
  expect(starts[1]).toEqual({ want: 'publish', back: 'web' });
  await expect(settings.getByRole('checkbox', { name: /^Add Classes to Google Calendar/ })).toBeChecked();
  await expect(settings.getByRole('checkbox', { name: /^Include Due Dates/ })).toBeChecked();
  await expect(settings.getByRole('checkbox', { name: 'Show Shifts' })).toBeChecked();
  await page.screenshot({ path: 'test-results/google-settings.png', animations: 'disabled' });
  // With permission granted, turning it off and on is a plain setting;
  // off asks first, since it deletes the calendar in Google.
  await settings.getByRole('checkbox', { name: /^Add Classes to Google Calendar/ }).click();
  const removal = page.getByRole('dialog', { name: 'Remove the Honorum Calendar?', exact: true });
  await removal.getByRole('button', { name: 'Cancel', exact: true }).click();
  await expect(settings.getByRole('checkbox', { name: /^Add Classes to Google Calendar/ })).toBeChecked();
  await settings.getByRole('checkbox', { name: /^Add Classes to Google Calendar/ }).click();
  await removal.getByRole('button', { name: 'Remove', exact: true }).click();
  await expect(settings.getByRole('checkbox', { name: /^Add Classes to Google Calendar/ })).not.toBeChecked();
  await settings.getByRole('checkbox', { name: /^Add Classes to Google Calendar/ }).click();
  await expect(settings.getByRole('checkbox', { name: /^Add Classes to Google Calendar/ })).toBeChecked();
  expect(patches.slice(1)).toEqual([{ publish: false }, { publish: true }]);
  expect(starts).toHaveLength(2);
  await settings.getByRole('button', { name: 'Close', exact: true }).click();

  // The events are on the week, read-only, with a way back to Google Calendar.
  const event = page.getByRole('button', { name: /^Dentist/ });
  await expect(event).toBeVisible();
  await expect(page.getByRole('button', { name: /^Shift/ })).toBeVisible();
  await event.click();
  const pane = page.getByRole('complementary', { name: 'Side pane' });
  await expect(pane.getByRole('heading', { name: 'Dentist', exact: true })).toBeVisible();
  await expect(pane.getByText('Main St', { exact: true })).toBeVisible();
  await expect(pane.getByRole('link', { name: 'Open in Google Calendar' })).toHaveAttribute('href', dentist.link);
  await page.screenshot({ path: 'test-results/google-event.png', animations: 'disabled' });

  // Disconnecting takes the events off the week.
  await openSettings(page, 'Schedule');
  await settings.getByRole('button', { name: 'Disconnect', exact: true }).click();
  await page.getByRole('dialog', { name: 'Disconnect Google Calendar?', exact: true }).getByRole('button', { name: 'Disconnect', exact: true }).click();
  await expect(settings.getByText('student@gmail.com', { exact: true })).not.toBeVisible();
  await settings.getByRole('button', { name: 'Close', exact: true }).click();
  await expect(page.getByRole('button', { name: /^Dentist/ })).toHaveCount(0);
});

test('Google Calendar asks guests to sign in, and is absent where the server does not offer it', async ({ page }) => {
  await page.route('**/api/config', async route => {
    const response = await route.fetch();
    await route.fulfill({ json: { ...await response.json(), phone_auth: true, google_calendar: true } });
  });
  await open(page);
  await openSettings(page, 'Schedule');
  const settings = page.getByRole('dialog', { name: 'Settings', exact: true });
  await expect(settings.getByRole('heading', { name: 'Google Calendar', exact: true })).toBeVisible();
  await expect(settings.getByRole('button', { name: 'Sign In', exact: true })).toBeVisible();
  await page.unroute('**/api/config');
  await page.goto('/');
  await openSettings(page, 'Schedule');
  await expect(settings.getByRole('heading', { name: 'Google Calendar' })).toHaveCount(0);
});
