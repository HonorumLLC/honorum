import { test, expect } from '@playwright/test';

test.use({ storageState: { cookies: [], origins: [] } });

test('new visitors see the landing page at the root until they open the app', async ({ page }) => {
  await page.goto('/');
  await expect(page.getByRole('heading', { name: 'Honorum', level: 1 })).toBeVisible();
  await expect(page).toHaveURL('/');
  await page.getByRole('link', { name: 'Open Honorum' }).first().click();
  await expect(page.getByRole('heading', { name: 'Welcome to Honorum' })).toBeVisible();
  await expect(page).toHaveURL('/');
  await page.reload();
  await expect(page.getByRole('heading', { name: 'Welcome to Honorum' })).toBeVisible();
});

test('a guest from before the landing page keeps the app', async ({ page, context }) => {
  await page.goto('/app');
  await expect(page.getByRole('heading', { name: 'Welcome to Honorum' })).toBeVisible();
  await expect.poll(() => page.evaluate(() => !!navigator.serviceWorker.controller)).toBe(true);
  await context.clearCookies();
  // The offline shell is fetched rather than navigated to, so an update
  // installs the app even without the cookie, and marks the browser.
  const shell = await page.request.get('/', { headers: { 'Sec-Fetch-Mode': 'cors', Accept: '*/*' } });
  expect(await shell.text()).toContain('<div id="app">');
  expect(shell.headers()['set-cookie']).toContain('hn_app=1');
  await context.clearCookies();
  await page.reload();
  await expect(page.getByRole('heading', { name: 'Welcome to Honorum' })).toBeVisible();
});

test('links into the app reach it from a new browser', async ({ page }) => {
  await page.goto('/#settings');
  await expect(page.getByRole('heading', { name: 'Honorum', level: 1 })).toBeHidden();
  await expect(page.getByRole('heading', { name: 'Welcome to Honorum' })).toBeVisible();
  expect(new URL(page.url()).pathname).toBe('/');
  expect((await page.context().cookies()).map((cookie) => cookie.name)).toContain('hn_app');
});
