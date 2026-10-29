import { test, expect, type Page } from '@playwright/test';
import { open, settle, localState, openSettings, settingsPage } from './helpers';

async function openAccount(page: Page) {
  await page.route('**/api/config', async route => {
    const response = await route.fetch();
    await route.fulfill({ json: { ...await response.json(), phone_auth: true } });
  });
  await open(page);
  await openSettings(page, 'Account');
  await page.getByRole('button', { name: 'Sign In', exact: true }).click();
  return page.getByRole('dialog', { name: 'Sign In', exact: true });
}

function deferred() {
  let resolve!: () => void;
  const promise = new Promise<void>(done => { resolve = done; });
  return { promise, resolve };
}

test('settings keeps devices in a separate menu with loading, retry and removal', async ({ page }) => {
  const me = { id: 'devices-account', phone: '+12015550123', device: 'this-device', devices: [
    { id: 'this-device', name: 'Current browser', current: true },
    { id: 'other-device', name: 'School laptop', current: false },
  ] };
  await page.route('**/api/auth/code', route => route.fulfill({ status: 202, json: { retry_after: 30 } }));
  await page.route('**/api/auth/verify', route => route.fulfill({ json: me }));
  const release = deferred();
  let requests = 0, removals = 0;
  await page.route('**/api/me', async route => {
    requests++;
    if (requests === 1) {
      await release.promise;
      await route.fulfill({ status: 503, json: { error: 'Devices could not be loaded' } });
    } else await route.fulfill({ json: me });
  });
  await page.route('**/api/me/devices/other-device', route => {
    expect(route.request().method()).toBe('DELETE');
    removals++;
    me.devices = me.devices.filter(d => d.current);
    return route.fulfill({ status: 204 });
  });
  await page.route(/\/api\/me\/(docs|sync)(\?.*)?$/, route => route.fulfill({ json: {
    applied: [], conflicts: [], docs: [], seq: 0, more: false,
  } }));
  const signIn = await openAccount(page);
  await signIn.getByLabel('Mobile number', { exact: true }).fill(me.phone);
  await signIn.getByRole('button', { name: 'Send Code', exact: true }).click();
  const code = page.getByRole('dialog', { name: 'Enter the Code', exact: true });
  await code.getByLabel('Code', { exact: true }).fill('123456');
  await expect(code).not.toBeVisible();
  const settings = page.getByRole('dialog', { name: 'Settings', exact: true });
  const devices = page.getByRole('dialog', { name: 'Devices', exact: true });
  await expect(settings.getByRole('button', { name: 'Devices', exact: true })).toBeVisible();
  expect(requests).toBe(0);
  const signOut = await settings.getByRole('button', { name: 'Sign Out', exact: true }).boundingBox();
  await settings.getByRole('button', { name: 'Devices', exact: true }).click();
  await expect(devices.getByRole('status')).toHaveText('Loading devices…');
  release.resolve();
  await expect(devices.getByRole('alert')).toHaveText('Devices could not be loaded');
  await devices.getByRole('button', { name: 'Try Again' }).click();
  await expect(devices.getByText('This device', { exact: true })).toBeVisible();
  await expect(devices.getByRole('button', { name: 'Remove Current browser' })).toHaveCount(0);
  await devices.getByRole('button', { name: 'Remove School laptop' }).click();
  const confirmation = page.getByRole('dialog', { name: 'Remove Device?', exact: true });
  await confirmation.getByRole('button', { name: 'Cancel', exact: true }).click();
  expect(removals).toBe(0);
  await devices.getByRole('button', { name: 'Remove School laptop' }).click();
  await confirmation.getByRole('button', { name: 'Remove', exact: true }).click();
  await expect(devices.getByText('School laptop', { exact: true })).not.toBeVisible();
  expect(removals).toBe(1);
  await page.keyboard.press('Escape');
  await expect(devices).not.toBeVisible();
  await expect(settings.getByRole('button', { name: 'Devices', exact: true })).toBeFocused();
  expect(await settings.getByRole('button', { name: 'Sign Out', exact: true }).boundingBox()).toEqual(signOut);
  await expect(settings.getByText('Current browser', { exact: true })).not.toBeVisible();
  await page.screenshot({ path: 'test-results/settings-desktop.png', animations: 'disabled' });
  await settings.getByRole('button', { name: 'Devices', exact: true }).click();
  await expect(devices.getByText('This device', { exact: true })).toBeVisible();
});

test('phone sign-in submits a completed code and does not wait for document sync', async ({ page }) => {
  await page.route('**/api/auth/code', route => {
    expect(route.request().postDataJSON()).toEqual({ phone: '+12015550123', consent: '2026-09-08-v2' });
    return route.fulfill({ status: 202, json: { retry_after: 30 } });
  });
  const me = { id: 'account-student', phone: '+12015550123', device: 'this-device', devices: [] };
  await page.route('**/api/auth/verify', route => route.fulfill({ json: me }));
  await page.route('**/api/me', route => route.fulfill({ json: me }));
  const downloading = deferred(), release = deferred();
  await page.route(/\/api\/me\/(docs|sync)(\?.*)?$/, async route => {
    downloading.resolve();
    await release.promise;
    await route.fulfill({ json: route.request().method() === 'POST'
      ? { applied: route.request().postDataJSON().docs.map((doc: { id: string }, i: number) => ({ id: doc.id, rev: i + 1 })), conflicts: [], docs: [], seq: 3, more: false }
      : { docs: [], seq: 0, more: false } });
  });

  const signIn = await openAccount(page);
  await expect(signIn.getByText(/Standard message and data rates may apply/)).toBeVisible();
  await expect(signIn.getByRole('link', { name: 'Terms', exact: true })).toHaveAttribute('href', '/legal.html#terms');
  await expect(signIn.getByRole('link', { name: 'Privacy Policy', exact: true })).toHaveAttribute('href', '/legal.html#privacy');
  await signIn.getByLabel('Mobile number', { exact: true }).fill(me.phone);
  await signIn.getByRole('button', { name: 'Send Code', exact: true }).click();
  const code = page.getByRole('dialog', { name: 'Enter the Code', exact: true });
  await expect(code.getByRole('status')).toContainText(me.phone);
  await expect(code.getByRole('button', { name: /Resend in/ })).toBeDisabled();
  await code.getByLabel('Code', { exact: true }).fill('123456');
  await downloading.promise;
  await expect(code).not.toBeVisible({ timeout: 1500 });
  await expect(page.getByRole('dialog', { name: 'Settings', exact: true }).getByText(me.phone, { exact: true })).toBeVisible();
  release.resolve();
});

test('the combined phone field formats typing, deletion and international paste', async ({ page }) => {
  const signIn = await openAccount(page);
  const number = signIn.getByLabel('Mobile number', { exact: true });
  const country = signIn.getByLabel('Country or region');
  await expect(signIn.getByText('We’ll text you a sign-in code.', { exact: true })).toBeVisible();
  await number.pressSequentially('2015550123');
  await expect(number).toHaveValue('(201) 555-0123');
  const box = await number.boundingBox(), prefix = await country.boundingBox();
  expect(Math.abs(box!.y - prefix!.y)).toBeLessThan(2);
  expect(prefix!.x + prefix!.width).toBeLessThanOrEqual(box!.x + 1);

  await number.fill('201');
  await number.press('Backspace');
  await expect(number).toHaveValue('20');
  await number.fill('2015550123');
  await number.evaluate((input: HTMLInputElement) => input.setSelectionRange(7, 8));
  await number.press('6');
  await expect(number).toHaveValue('(201) 565-0123');
  expect(await number.evaluate((input: HTMLInputElement) => input.selectionStart)).toBe(8);

  await number.fill('+44 7700 900123');
  await expect(country).toHaveValue('GB');
  await expect(number).toHaveValue('07700 900123');
  await expect(signIn.getByText('GB +44', { exact: true })).toBeVisible();
  await country.selectOption('US');
  await number.fill('2015550123');
  await expect(number).toHaveValue('(201) 555-0123');
  await expect(signIn.getByRole('button', { name: 'Send Code', exact: true })).toBeEnabled();
});

test('privacy and SMS terms are public and remain available offline', async ({ page, context }) => {
  await open(page);
  await page.evaluate(async () => { await navigator.serviceWorker.ready; });
  await expect.poll(() => page.evaluate(() => !!navigator.serviceWorker.controller)).toBe(true);
  await expect.poll(() => page.evaluate(() => caches.match('/legal.html').then(Boolean))).toBe(true);
  await context.setOffline(true);
  await page.goto('/legal.html#sms');
  await expect(page.getByRole('heading', { name: 'SMS sign-in terms', exact: true })).toBeVisible();
  await expect(page.getByRole('link', { name: 'support@honorum.app', exact: true })).toHaveAttribute('href', 'mailto:support@honorum.app');
  await expect(page.getByRole('heading', { name: 'Privacy Policy', exact: true })).toBeAttached();
});

test('closing an in-flight code request resets the dialog and ignores the late response', async ({ page }) => {
  const entered = deferred(), release = deferred();
  await page.route('**/api/auth/code', async route => {
    entered.resolve();
    await release.promise;
    await route.fulfill({ status: 202, json: { retry_after: 30 } }).catch(() => {});
  });
  const dialog = await openAccount(page);
  await dialog.getByLabel('Mobile number', { exact: true }).fill('+12015550123');
  await dialog.getByRole('button', { name: 'Send Code', exact: true }).click();
  await entered.promise;
  await expect(dialog.getByLabel('Mobile number', { exact: true })).toBeDisabled();
  await dialog.getByRole('button', { name: 'Cancel', exact: true }).click();
  await expect(dialog).not.toBeVisible();
  await page.getByRole('button', { name: 'Sign In', exact: true }).click();
  release.resolve();
  await expect(dialog.getByLabel('Mobile number', { exact: true })).toBeEnabled();
  await dialog.getByLabel('Mobile number', { exact: true }).fill('+12015550124');
  await expect(dialog.getByRole('button', { name: 'Send Code', exact: true })).toBeEnabled();
  await expect(page.getByRole('dialog', { name: 'Enter the Code', exact: true })).not.toBeVisible();
});

test('code entry, resend and cancellation do not call the browser SMS API', async ({ page }) => {
  await page.addInitScript(() => {
    // Some Chromium derivatives expose WebOTP but kill the tab when it is
    // called. Presence of OTPCredential must not make sign-in depend on it.
    const otp = { requests: 0 };
    Object.assign(window, { OTPCredential: class {}, testOTP: otp });
    Object.defineProperty(navigator, 'credentials', { value: {
      get: () => {
        otp.requests++;
        return Promise.reject(new DOMException('SMS API unavailable', 'NotSupportedError'));
      },
    } });
  });
  await page.clock.install();
  const me = { id: 'account-autofill', phone: '+12015550123', device: 'device', devices: [] };
  let sends = 0;
  await page.route('**/api/auth/code', route => {
    sends++;
    return route.fulfill({ status: 202, json: { retry_after: 30 } });
  });
  const checking = deferred(), release = deferred();
  await page.route('**/api/auth/verify', async route => {
    expect(route.request().postDataJSON().code).toBe('012345');
    checking.resolve();
    await release.promise;
    await route.fulfill({ json: me });
  });
  await page.route('**/api/me', route => route.fulfill({ json: me }));
  await page.route(/\/api\/me\/(docs|sync)(\?.*)?$/, route => route.fulfill({ status: 503, json: { error: 'Temporarily unavailable' } }));
  const signIn = await openAccount(page);
  const code = page.getByRole('dialog', { name: 'Enter the Code', exact: true });
  const requests = () => page.evaluate(() => (window as any).testOTP.requests);
  const send = async () => {
    await signIn.getByLabel('Mobile number', { exact: true }).fill(me.phone);
    await signIn.getByRole('button', { name: 'Send Code', exact: true }).click();
    await expect(code.getByLabel('Code', { exact: true })).toHaveAttribute('autocomplete', 'one-time-code');
  };
  await send();
  expect(sends).toBe(1);
  expect(await requests()).toBe(0);
  await code.getByRole('button', { name: 'Cancel', exact: true }).click();
  await page.clock.fastForward(31000);
  await page.getByRole('button', { name: 'Sign In', exact: true }).click();
  await send();
  expect(sends).toBe(2);
  await page.clock.fastForward(31000);
  await code.getByRole('button', { name: 'Resend Code', exact: true }).click();
  await expect.poll(() => sends).toBe(3);
  await code.getByLabel('Code', { exact: true }).fill('012345');
  await checking.promise;
  await expect(code.getByRole('button', { name: 'Cancel', exact: true })).toBeDisabled();
  await page.keyboard.press('Escape');
  await expect(code).toBeVisible();
  release.resolve();
  await expect(code).not.toBeVisible();
  expect(await requests()).toBe(0);
});

test('deleting and recreating an account keeps local plans and uploads them to the new account', async ({ page }) => {
  const me = { id: 'account-student', phone: '+12015550123', device: 'this-device', devices: [] };
  let uploads = 0, seq = 0;
  await page.route('**/api/auth/code', route => route.fulfill({ status: 202, json: { retry_after: 30 } }));
  await page.route('**/api/auth/verify', route => route.fulfill({ json: me }));
  await page.route('**/api/me', async route => {
    expect(route.request().headers()['x-honorum-account']).toBe(me.id);
    if (route.request().method() === 'DELETE') { seq = 0; me.id = 'account-recreated'; await route.fulfill({ status: 204 }); }
    else await route.fulfill({ json: me });
  });
  await page.route(/\/api\/me\/(docs|sync)(\?.*)?$/, route => {
    expect(route.request().headers()['x-honorum-account']).toBe(me.id);
    // A download is the same request with no documents; count only uploads.
    const docs = route.request().method() === 'POST' ? route.request().postDataJSON().docs || [] : [];
    if (docs.length) uploads++;
    return route.fulfill({ json: { applied: docs.map((doc: { id: string }) => ({ id: doc.id, rev: ++seq })), conflicts: [], docs: [], seq, more: false } });
  });
  const signIn = await openAccount(page);
  const settings = page.getByRole('dialog', { name: 'Settings', exact: true });
  for (let attempt = 1; attempt <= 2; attempt++) {
    if (attempt === 2) await settings.getByRole('button', { name: 'Sign In', exact: true }).click();
    await signIn.getByLabel('Mobile number', { exact: true }).fill(me.phone);
    await signIn.getByRole('button', { name: 'Send Code', exact: true }).click();
    const code = page.getByRole('dialog', { name: 'Enter the Code', exact: true });
    await code.getByLabel('Code', { exact: true }).fill('123456');
    await expect(code).not.toBeVisible();
    await expect.poll(() => uploads).toBe(attempt);
    if (attempt === 2) break;
    await settings.getByRole('button', { name: 'Delete Account…', exact: true }).click();
    await page.getByRole('dialog', { name: 'Delete Account?', exact: true }).getByRole('button', { name: 'Delete Account', exact: true }).click();
    await expect(settings.getByRole('button', { name: 'Sign In', exact: true })).toBeVisible();
    const saved = { state: await localState(page), sync: await localState(page, 'sync') };
    expect(saved.state.schedules).toHaveLength(1);
    expect(saved.state.account).toBeNull();
    expect(saved.sync.known).toEqual({});
    expect(saved.sync.owner).toBe('');
  }
});

async function addPrivateEvent(page: Page, name: string) {
  await page.getByTitle('Add Event', { exact: true }).click();
  const event = page.getByRole('dialog', { name: 'Add Event', exact: true });
  await event.getByLabel('Name', { exact: true }).fill(name);
  await event.getByRole('button', { name: 'Add', exact: true }).click();
  await expect(event).not.toBeVisible();
  await expect.poll(async () => JSON.stringify(await localState(page))).toContain(name);
}

test('sign-out hides private plans and preserves unsynced work for the same account', async ({ page }) => {
  const me = { id: 'account-private', phone: '+12015550123', device: 'device', devices: [] };
  await page.route('**/api/auth/code', route => route.fulfill({ status: 202, json: { retry_after: 30 } }));
  await page.route('**/api/auth/verify', route => route.fulfill({ json: me }));
  await page.route('**/api/auth/logout', route => route.fulfill({ status: 204 }));
  await page.route('**/api/me', route => route.fulfill({ json: me }));
  // No remote copy exists: only the account's local outbox can recover this plan.
  await page.route(/\/api\/me\/(docs|sync)(\?.*)?$/, route => route.fulfill({ status: 503, json: { error: 'Temporarily unavailable' } }));
  const signIn = await openAccount(page);
  const login = async () => {
    await signIn.getByLabel('Mobile number', { exact: true }).fill(me.phone);
    await signIn.getByRole('button', { name: 'Send Code', exact: true }).click();
    const code = page.getByRole('dialog', { name: 'Enter the Code', exact: true });
    await code.getByLabel('Code', { exact: true }).fill('123456');
    await expect(code).not.toBeVisible();
  };
  await login();
  const settings = page.getByRole('dialog', { name: 'Settings', exact: true });
  await settings.getByRole('button', { name: 'Close', exact: true }).click();
  await addPrivateEvent(page, 'Private account appointment');
  await openSettings(page, 'Account');
  await settings.getByRole('button', { name: 'Sign Out', exact: true }).click();
  await expect(settings.getByRole('button', { name: 'Sign In', exact: true })).toBeVisible();
  await expect.poll(async () => JSON.stringify(await localState(page))).not.toContain('Private account appointment');
  expect((await localState(page)).dataOwner).toBe('');
  expect(JSON.stringify(await localState(page, 'state:' + me.id))).toContain('Private account appointment');
  await page.reload();
  await expect(page.getByTitle('Add Event', { exact: true })).toBeEnabled();
  expect(JSON.stringify(await localState(page))).not.toContain('Private account appointment');
  await settings.getByRole('button', { name: 'Sign In', exact: true }).click();
  await login();
  await expect.poll(async () => JSON.stringify(await localState(page))).toContain('Private account appointment');
});

test('browser site-data clearing cannot be undone by an open tab', async ({ page, context }) => {
  await open(page);
  await addPrivateEvent(page, 'Erase this appointment');
  const second = await context.newPage();
  await page.evaluate(() => window.dispatchEvent(new Event('blur')));
  await second.goto('/');
  await second.bringToFront();
  await settle(second);
  expect(JSON.stringify(await localState(second))).toContain('Erase this appointment');
  const browser = await context.newCDPSession(second);
  await browser.send('Storage.clearDataForOrigin', { origin: new URL(page.url()).origin, storageTypes: 'all' });
  await expect(second.getByText('Site data was cleared. Reload to start fresh.', { exact: true })).toBeVisible();
  await expect(page.getByText('Site data was cleared. Reload to start fresh.', { exact: true })).toBeVisible();
  // The cookie that marks the browser as an app user went too, so the root
  // now shows the landing page; open the app from it.
  await second.reload();
  await second.getByRole('link', { name: 'Open Honorum' }).first().click();
  await page.waitForLoadState();
  await page.evaluate(() => window.dispatchEvent(new Event('blur')));
  await second.evaluate(() => window.dispatchEvent(new Event('focus')));
  await settle(second);
  await expect.poll(async () => JSON.stringify(await localState(second))).not.toContain('Erase this appointment');
  await second.evaluate(() => window.dispatchEvent(new Event('blur')));
  await page.bringToFront();
  await page.evaluate(() => window.dispatchEvent(new Event('focus')));
  await page.reload();
  await settle(page);
  expect(JSON.stringify(await localState(page))).not.toContain('Erase this appointment');
});


test('international phone entry, failed delivery, cooldown and incorrect codes can be retried', async ({ page }) => {
  await page.clock.install();
  let sends = 0, checks = 0;
  await page.route('**/api/auth/code', route => {
    expect(route.request().postDataJSON()).toEqual({ phone: '+447700900123', consent: '2026-09-08-v2' });
    return ++sends === 1
      ? route.fulfill({ status: 503, headers: { 'Retry-After': '60' }, json: { error: 'Phone verification is unavailable. Try again shortly.' } })
      : route.fulfill({ status: 202, json: { retry_after: 30 } });
  });
  await page.route('**/api/auth/verify', route => {
    checks++;
    expect(route.request().postDataJSON().phone).toBe('+447700900123');
    return route.fulfill({ status: 400, json: { error: 'That code is incorrect. Try again.' } });
  });
  const signIn = await openAccount(page);
  await signIn.getByLabel('Country or region').selectOption('GB');
  await signIn.getByLabel('Mobile number').fill('07700 900123');
  await signIn.getByRole('button', { name: 'Send Code', exact: true }).click();
  await expect(signIn.getByRole('alert')).toContainText('unavailable');
  await expect(signIn.getByRole('button', { name: /Try Again in/ })).toBeDisabled();
  await page.clock.fastForward(61000);
  await signIn.getByRole('button', { name: 'Send Code', exact: true }).click();
  const code = page.getByRole('dialog', { name: 'Enter the Code', exact: true });
  await expect(code.getByRole('status')).toContainText('+447700900123');
  await code.getByLabel('Code', { exact: true }).fill('111111');
  await expect(code.getByRole('alert')).toContainText('incorrect');
  await expect(code.getByLabel('Code', { exact: true })).toHaveValue('');
  expect(checks).toBe(1);
  await page.clock.fastForward(31000);
  await code.getByRole('button', { name: 'Resend Code', exact: true }).click();
  await expect.poll(() => sends).toBe(3);
  await expect(code.getByRole('alert')).not.toBeVisible();
  await code.getByRole('button', { name: 'Change Number', exact: true }).click();
  await expect(signIn.getByLabel('Mobile number')).toBeEnabled();
});

test('idle account polls do not rewrite storage and never upload reminder items', async ({ page }) => {
  await page.clock.install();
  await open(page);
  const state = await localState(page);
  state.account = { id: 'idle-account', phone: '+12015550123', device: 'idle-device' };
  state.dataOwner = state.account.id; state.reminders.on = true;
  state.tasks = [{ id: 'enter-window', title: 'Enters the reminder window', source: 'manual', done: false,
    due: new Date(Date.now() + 8 * 86400000 - 30000).toISOString() }];
  await page.evaluate(async state => {
    const db = await new Promise<IDBDatabase>(resolve => { const r = indexedDB.open('honorum'); r.onsuccess = () => resolve(r.result); });
    const docs: Record<string, { type: string; body: any }> = Object.fromEntries([
      ...state.schedules.map((s: any) => ['s_' + s.id, { type: 'schedule', body: s }]),
      ...state.tasks.map((t: any) => ['t_' + t.id, { type: 'task', body: t }]),
      ['prefs', { type: 'prefs', body: { inst: state.inst, term: state.term, current: state.current, onboarded: true, reminders: true, map: state.lms.map } }],
    ]);
    const known = Object.fromEntries(Object.entries(docs).map(([id, d]) => [id, { rev: 7, type: d.type, deleted: false, body: JSON.stringify(d.body) }]));
    await new Promise<void>((resolve, reject) => {
      const tx = db.transaction('kv', 'readwrite'), store = tx.objectStore('kv');
      store.put(state.account.id, 'activeOwner'); store.put(state, 'state:' + state.account.id);
      store.put({ owner: state.account.id, seq: 7, known, outbox: {}, conflicts: [], watchesVersion: 'w1' }, 'sync:' + state.account.id);
      tx.oncomplete = () => resolve(); tx.onabort = () => reject(tx.error);
    }); db.close();
  }, state);
  const requests: any[] = [], reminders: any[] = [];
  await page.route(/\/api\/me\/(docs|sync)(\?.*)?$/, route => {
    requests.push(route.request().postDataJSON());
    return route.fulfill({ json: { docs: [], seq: 7, more: false, watches_version: 'w1', applied: [], conflicts: [] } });
  });
  // An account's reminders are derived on the server from its documents.
  await page.route('**/api/me/reminders', route => { reminders.push(route.request().method()); return route.fulfill({ status: 404 }); });
  await page.reload();
  await expect(page.getByTitle('Add Event', { exact: true })).toBeEnabled();
  await expect.poll(() => requests.length).toBeGreaterThan(0);
  await page.waitForTimeout(1800);
  requests.length = 0;
  await page.evaluate(() => {
    const original = IDBObjectStore.prototype.put;
    (window as any).idleWrites = 0;
    IDBObjectStore.prototype.put = function (...args: Parameters<typeof original>) {
      (window as any).idleWrites++; return original.apply(this, args);
    };
  });
  await page.clock.fastForward(60000);
  await expect.poll(() => requests.length).toBe(1);
  await page.waitForTimeout(1800);
  expect(requests[0].docs).toEqual([]); expect(requests[0].since).toBe(7); expect(requests[0].watches_version).toBe('w1');
  expect(await page.evaluate(() => (window as any).idleWrites)).toBe(0);
  await page.clock.fastForward(86400000);
  await page.waitForTimeout(1800);
  expect(reminders).toEqual([]);
});

test('regaining editing ownership downloads remote edits before the minute timer', async ({ page }) => {
  await page.clock.install();
  const me = { id: 'focus-account', phone: '+12015550123', device: 'focus-device', devices: [] };
  const documents = new Map<string, any>();
  let seq = 0;
  await page.route('**/api/auth/code', route => route.fulfill({ status: 202, json: { retry_after: 30 } }));
  await page.route('**/api/auth/verify', route => route.fulfill({ json: me }));
  await page.route('**/api/me', route => route.fulfill({ json: me }));
  await page.route(/\/api\/me\/(docs|sync)(\?.*)?$/, route => {
    const request = route.request(), posted = request.method() === 'POST';
    const input = posted ? request.postDataJSON() : {};
    const since = posted ? input.since || 0 : Number(new URL(request.url()).searchParams.get('since'));
    const edits = input.docs || [], uploaded = new Set(edits.map((d: any) => d.id));
    const applied = edits.map((d: any) => { const rev = ++seq; documents.set(d.id, { ...d, rev }); return { id: d.id, rev }; });
    return route.fulfill({ json: { seq, applied, conflicts: [], more: false, watches_version: 'w1',
      docs: [...documents.values()].filter(d => d.rev > since && !uploaded.has(d.id)) } });
  });
  const signIn = await openAccount(page);
  await signIn.getByLabel('Mobile number', { exact: true }).fill(me.phone);
  await signIn.getByRole('button', { name: 'Send Code', exact: true }).click();
  const code = page.getByRole('dialog', { name: 'Enter the Code', exact: true });
  await code.getByLabel('Code', { exact: true }).fill('123456');
  await expect(code).not.toBeVisible();
  await page.getByRole('dialog', { name: 'Settings', exact: true }).getByRole('button', { name: 'Close', exact: true }).click();
  await expect.poll(async () => {
    const sync = await localState(page, 'sync');
    return sync?.owner === me.id && !sync.adopting && Object.keys(sync.outbox).length === 0;
  }).toBe(true);
  await page.evaluate(() => window.dispatchEvent(new Event('blur')));
  await page.waitForFunction(async () => !(await navigator.locks.query()).held?.some(lock => lock.name === 'honorum-editor'));
  documents.set('t_refocus', { id: 't_refocus', type: 'task', rev: ++seq,
    body: { id: 'refocus', title: 'Remote task after refocus', source: 'manual', done: false, due: '' } });
  // Only a focus event fires. The document stays visible and no minute elapses.
  await page.evaluate(() => window.dispatchEvent(new Event('focus')));
  await expect(page.getByText('Remote task after refocus', { exact: true })).toBeVisible({ timeout: 1500 });
  await expect.poll(async () => (await localState(page)).tasks.some((task: any) => task.id === 'refocus')).toBe(true);
});

test('an edit made just before leaving the tab reaches the account without waiting for its return', async ({ page }) => {
  const me = { id: 'leaving-account', phone: '+12015550123', device: 'leaving-device', devices: [] };
  const uploads: any[] = [];
  let seq = 0;
  await page.route('**/api/auth/code', route => route.fulfill({ status: 202, json: { retry_after: 30 } }));
  await page.route('**/api/auth/verify', route => route.fulfill({ json: me }));
  await page.route(/\/api\/me\/(docs|sync)(\?.*)?$/, route => {
    const input = route.request().postDataJSON() || {};
    uploads.push(...(input.docs || []));
    const applied = (input.docs || []).map((d: any) => ({ id: d.id, rev: ++seq }));
    return route.fulfill({ json: { seq, applied, conflicts: [], more: false, watches_version: 'w1', docs: [] } });
  });
  const signIn = await openAccount(page);
  await signIn.getByLabel('Mobile number', { exact: true }).fill(me.phone);
  await signIn.getByRole('button', { name: 'Send Code', exact: true }).click();
  const code = page.getByRole('dialog', { name: 'Enter the Code', exact: true });
  await code.getByLabel('Code', { exact: true }).fill('123456');
  await expect(code).not.toBeVisible();
  await page.getByRole('dialog', { name: 'Settings', exact: true }).getByRole('button', { name: 'Close', exact: true }).click();
  await expect.poll(async () => { const sync = await localState(page, 'sync'); return sync?.owner === me.id && !sync.adopting && Object.keys(sync.outbox).length === 0; }).toBe(true);
  await page.getByTitle('Add Event', { exact: true }).click();
  const dialog = page.getByRole('dialog', { name: 'Add Event' });
  await dialog.getByLabel('Name', { exact: true }).fill('Before leaving');
  await dialog.getByRole('button', { name: 'Add', exact: true }).click();
  // Another app takes focus at once; the tab does not come back.
  await page.evaluate(() => window.dispatchEvent(new Event('blur')));
  await expect.poll(() => uploads.some(d => d.id.startsWith('s_') && d.body?.blocks?.some((b: any) => b.name === 'Before leaving')), { timeout: 2000 }).toBe(true);
  await page.waitForFunction(async () => !(await navigator.locks.query()).held?.some(lock => lock.name === 'honorum-editor'));
});

test('account Canvas connects once, syncs without local credentials, and shows the account streak', async ({ page }) => {
  const me = { id: 'shared-account', phone: '+12015550123', device: 'browser', devices: [] };
  const connection = { id: 'canvas-connection', base: 'https://canvas.example.edu', connected: false, available: true, synced: 0, error: '' };
  let seq = 0, completions = 0, refreshes = 0, failCompletion = true;
  const docs = new Map<string, any>();
  const task = { id: 'shared-task', source: 'canvas', uid: 'canvas:assignment:42', title: 'Shared essay', due: '', course: '', done: false, note: '' };
  const write = (id: string, body: any, type = 'task') => docs.set(id, { id, type, body, rev: ++seq, updated: Date.now() });
  await page.route('**/api/auth/code', route => route.fulfill({ status: 202, json: { retry_after: 30 } }));
  await page.route('**/api/auth/verify', route => route.fulfill({ json: me }));
  await page.route('**/api/me/activity', route => {
    expect(route.request().headers()['x-honorum-account']).toBe(me.id);
    expect(route.request().postDataJSON().times).toBeUndefined();
    return route.fulfill({ json: { current: 3, longest: 8, timezone: 'America/Chicago', today: '2026-09-11', last_day: '2026-09-11', overdue: 0, days: [{ day: '2026-09-10', state: 'missed' }, { day: '2026-09-11', state: 'clean' }] } });
  });
  await page.route(/\/api\/me\/(docs|sync)(\?.*)?$/, route => {
    const input = route.request().method() === 'POST' ? route.request().postDataJSON() : {};
    const since = input.since ?? Number(new URL(route.request().url()).searchParams.get('since') || 0);
    const applied: any[] = [];
    for (const d of input.docs || []) {
      write(d.id, d.body, d.type);
      if (d.deleted) docs.get(d.id).deleted = true;
      applied.push({ id: d.id, rev: seq });
    }
    return route.fulfill({ json: { seq, applied, conflicts: [], more: false, docs: [...docs.values()].filter(d => d.rev > since && !applied.some(a => a.id === d.id)) } });
  });
  await page.route('**/api/me/canvas', route => {
    if (route.request().method() === 'PUT') {
      expect(route.request().postDataJSON()).toEqual({ base: connection.base, token: 'canvas-token-entered-only-once' });
      connection.connected = true; connection.synced = Math.floor(Date.now() / 1000);
      write('t_shared-task', task);
    }
    if (route.request().method() === 'DELETE') connection.connected = false;
    return route.fulfill({ json: connection });
  });
  await page.route('**/api/me/canvas/refresh', route => { refreshes++; return route.fulfill({ json: connection }); });
  await page.route('**/api/me/canvas/complete', route => {
    if (failCompletion) return route.fulfill({ status: 502, json: { error: 'Canvas could not be updated; the change will retry' } });
    const input = route.request().postDataJSON();
    const current = docs.get('t_shared-task').body;
    expect(input.document).toBe('t_shared-task');
    expect(input.intent).toBe(current.pendingCanvasDone);
    expect(current.done).toBe(true);
    const completed = { ...current }; delete completed.pendingCanvasDone;
    write('t_shared-task', completed); completions++;
    return route.fulfill({ json: { applied: true, module: true } });
  });
  await page.route('**/api/lms/canvas', () => { throw new Error('Shared Canvas must not use the guest relay'); });
  const signIn = await openAccount(page);
  await signIn.getByLabel('Mobile number', { exact: true }).fill(me.phone);
  await signIn.getByRole('button', { name: 'Send Code', exact: true }).click();
  await page.getByRole('dialog', { name: 'Enter the Code', exact: true }).getByLabel('Code', { exact: true }).fill('123456');
  const settings = page.getByRole('dialog', { name: 'Settings', exact: true });
  await expect(settings.getByText('3 days', { exact: true })).toBeVisible();
  await settingsPage(page, 'Assignments');
  await settings.getByRole('button', { name: 'Connect Canvas or a Calendar', exact: true }).click();
  await page.getByRole('dialog', { name: 'Connect Assignments' }).getByRole('button', { name: 'Canvas', exact: true }).click();
  const connect = page.getByRole('dialog', { name: 'Connect Canvas', exact: true });
  await expect(connect.getByText(/Connecting saves an encrypted Canvas token/)).toBeVisible();
  await connect.getByLabel('Canvas address', { exact: true }).fill(connection.base);
  await connect.getByLabel('Access token', { exact: true }).fill('canvas-token-entered-only-once');
  await connect.getByRole('button', { name: 'Connect Canvas', exact: true }).click();
  await page.getByRole('dialog', { name: 'Canvas Connected', exact: true }).getByRole('button', { name: 'Done', exact: true }).click();
  await settings.getByRole('button', { name: 'Close', exact: true }).click();
  await expect.poll(async () => (await localState(page)).lms.canvas.token).toBe('');
  await expect.poll(async () => (await localState(page)).tasks.map((t: any) => t.id)).toEqual(['shared-task']);
  await page.reload(); await settle(page);
  await expect.poll(() => refreshes).toBeGreaterThan(0);
  const progress = page.getByRole('region', { name: 'Progress', exact: true });
  // The streak heads the card: the count, the best, and this week from Monday
  // the 7th to the server's Friday.
  await expect(progress.getByText('3 day streak, best 8', { exact: true })).toBeAttached();
  await expect(progress.getByRole('img', { name: '1 of this week’s 5 days ended with nothing overdue' })).toBeVisible();
  const row = page.locator('.tasks li.row').filter({ has: page.getByText('Shared essay', { exact: true }) });
  await row.getByRole('checkbox', { name: 'Done', exact: true }).check();
  const failure = page.getByRole('status');
  await expect(failure).toContainText('Your token may have expired');
  await expect.poll(async () => !!(await localState(page)).tasks[0].pendingCanvasDone).toBe(true);
  await failure.getByRole('link', { name: 'Reconnect Canvas', exact: true }).click();
  await expect(settings).toBeVisible();
  await expect(settings.getByRole('button', { name: 'Reconnect Canvas', exact: false })).toBeVisible();
  await settings.getByRole('button', { name: 'Close', exact: true }).click();
  failCompletion = false;
  await page.getByRole('button', { name: 'Sync assignments', exact: true }).click();
  await expect.poll(() => completions).toBe(1);
  await expect.poll(async () => (await localState(page)).tasks[0].pendingCanvasDone).toBeUndefined();
  await openSettings(page, 'Assignments');
  await page.screenshot({ path: 'test-results/account-streak-canvas.png', animations: 'disabled' });
  await settings.getByRole('button', { name: 'Disconnect', exact: true }).click();
  const disconnect = page.getByRole('dialog', { name: 'Disconnect Canvas?', exact: true });
  await expect(disconnect.getByText(/Existing assignments stay saved/)).toBeVisible();
  await disconnect.getByRole('button', { name: 'Disconnect', exact: true }).click();
  await expect.poll(() => connection.connected).toBe(false);
  expect((await localState(page)).tasks).toHaveLength(1);
});
