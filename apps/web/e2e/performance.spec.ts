import { test, expect } from '@playwright/test';
import { createServer } from 'node:http';
import { readFile } from 'node:fs/promises';

test('onboarding downloads only the chosen school and keeps search available offline', async ({ page, context }) => {
  const config = await (await page.request.get('/api/config')).json();
  const school = config.institutions[0];
  await context.route('**/api/config', route => route.fulfill({ json: {
    ...config,
    institutions: [{ ...school, id: 'unchosen', name: 'Another school' }, school],
  } }));
  const catalogs: string[] = [];
  context.on('request', request => {
    const path = new URL(request.url()).pathname;
    if (path.startsWith('/catalog/')) catalogs.push(path);
  });
  await page.goto('/');
  const welcome = page.getByRole('region', { name: 'Welcome', exact: true });
  await expect(welcome.getByRole('button', { name: school.name, exact: true })).toBeEnabled();
  await page.evaluate(() => navigator.serviceWorker.ready.then(() => true));
  // Let startup's idle work run before checking for an unwanted download.
  await page.evaluate(() => new Promise<void>(resolve => requestIdleCallback(() => setTimeout(resolve, 100))));
  expect(catalogs).toEqual([]);
  await welcome.getByRole('button', { name: school.name, exact: true }).click();
  await expect.poll(() => catalogs.length).toBeGreaterThan(0);
  await welcome.getByRole('button', { name: 'Continue', exact: true }).click();
  await welcome.getByRole('button', { name: 'Skip', exact: true }).click();
  await welcome.getByRole('button', { name: 'Search Classes', exact: true }).click();
  await expect(page.locator('.results .course').first()).toBeVisible();
  expect(catalogs.every(path => path.startsWith(`/catalog/${school.id}/`))).toBe(true);
  await context.setOffline(true);
  await page.reload();
  await expect(page.locator('.results .course').first()).toBeVisible();
});

test('service worker updates reuse hashed assets and preserve old offline views', async ({ browser }) => {
  const source = await readFile('public/sw.js', 'utf8');
  let version = 'one';
  const requests: string[] = [];
  const server = createServer((req, res) => {
    const url = req.url!;
    requests.push(url);
    res.setHeader('Cache-Control', 'no-store'); // Exercise Cache Storage without an HTTP cache.
    if (url === '/sw.js') {
      res.setHeader('Content-Type', 'text/javascript');
      res.end(source.replace('__BUILD_ID__', version).replace('__PRECACHE__', JSON.stringify([
        '/', '/legal.html', '/assets/shared-hash.js', `/assets/view-${version}.js`, '/manifest.webmanifest',
      ])));
    } else if (url === '/') {
      res.setHeader('Content-Type', 'text/html');
      res.end(`<!doctype html><title>Service worker update</title><p>Shell ${version}</p>`);
    } else if (url === '/legal.html') { res.writeHead(302, { Location: '/legal' }); res.end(); }
    else if (url === '/legal') { res.setHeader('Content-Type', 'text/html'); res.end(`<!doctype html><p>Legal ${version}</p>`); }
    else res.end(url === '/manifest.webmanifest' ? version : url);
  });
  await new Promise<void>(resolve => server.listen(0, '127.0.0.1', resolve));
  const { port } = server.address() as { port: number };
  const context = await browser.newContext();
  try {
    const page = await context.newPage();
    await page.goto(`http://127.0.0.1:${port}`);
    await page.evaluate(async () => {
      await navigator.serviceWorker.register('/sw.js');
      await navigator.serviceWorker.ready;
    });
    await page.waitForFunction(() => !!navigator.serviceWorker.controller);
    requests.length = 0;
    version = 'two';
    await page.evaluate(async () => {
      const changed = new Promise<void>(resolve => navigator.serviceWorker.addEventListener('controllerchange', () => resolve(), { once: true }));
      await (await navigator.serviceWorker.ready).update();
      await changed;
    });
    expect(requests).toContain('/assets/view-two.js');
    expect(requests).not.toContain('/assets/shared-hash.js');
    expect(requests).toContain('/manifest.webmanifest');
    await context.setOffline(true);
    await page.reload();
    await expect(page.getByText('Shell two', { exact: true })).toBeVisible();
    expect(await page.evaluate(async () => {
      const urls = ['/assets/shared-hash.js', '/assets/view-one.js', '/assets/view-two.js', '/manifest.webmanifest'];
      return Promise.all(urls.map(async url => (await fetch(url)).text()));
    })).toEqual(['/assets/shared-hash.js', '/assets/view-one.js', '/assets/view-two.js', 'two']);
    await page.goto(`http://127.0.0.1:${port}/legal`);
    await expect(page.getByText('Legal two', { exact: true })).toBeVisible();
  } finally {
    await context.close();
    await new Promise<void>((resolve, reject) => server.close(error => error ? reject(error) : resolve()));
  }
});

test('installed shell navigation does not wait for an unresponsive HTML server', async ({ browser }) => {
  const source = await readFile('public/sw.js', 'utf8');
  let stall = false;
  const server = createServer((req, res) => {
    res.setHeader('Cache-Control', 'no-store');
    if (req.url === '/sw.js') {
      res.setHeader('Content-Type', 'text/javascript');
      res.end(source.replace('__BUILD_ID__', 'cached-navigation').replace('__PRECACHE__', JSON.stringify(['/'])));
    } else if (!stall) { res.setHeader('Content-Type', 'text/html'); res.end('<!doctype html><p>Installed shell</p>'); }
  });
  await new Promise<void>(resolve => server.listen(0, '127.0.0.1', resolve));
  const context = await browser.newContext();
  try {
    const page = await context.newPage();
    await page.goto(`http://127.0.0.1:${(server.address() as { port: number }).port}`);
    await page.evaluate(async () => { await navigator.serviceWorker.register('/sw.js'); await navigator.serviceWorker.ready; });
    await page.waitForFunction(() => !!navigator.serviceWorker.controller);
    stall = true;
    await page.reload({ waitUntil: 'domcontentloaded', timeout: 1500 });
    await expect(page.getByText('Installed shell', { exact: true })).toBeVisible();
  } finally {
    await context.close(); server.closeAllConnections();
    await new Promise<void>((resolve, reject) => server.close(error => error ? reject(error) : resolve()));
  }
});

test('search pages bound section rows while keeping every section reachable', async ({ page, context }) => {
  const config = await (await page.request.get('/api/config')).json();
  const school = config.institutions[0], term = { ...school.terms[0], structure_snapshot: 'paging', seats_snapshot: 'paging', descriptions_snapshot: '' };
  await context.route('**/api/config', route => route.fulfill({ json: { ...config, institutions: [{ ...school, terms: [term] }] } }));
  const sections = Array.from({ length: 103 }, (_, n) => ({ id: `page-${n}`, num: String(n).padStart(3, '0'), meetings: [], instructors: [] }));
  await context.route('**/catalog/**', route => route.fulfill({ json: route.request().url().includes('.seats.')
    ? { inst: school.id, term: term.id, seats: Object.fromEntries(sections.map(section => [section.id, [0, 1, 0]])) }
    : { inst: school.id, term, hash: 'paging', courses: [{ code: 'PAGE 1000', subject: 'PAGE', title: 'Paging course', sections }] } }));
  await page.goto('/');
  const welcome = page.getByRole('region', { name: 'Welcome', exact: true });
  await welcome.getByRole('button', { name: 'Continue', exact: true }).click();
  await welcome.getByRole('button', { name: 'Skip', exact: true }).click();
  await welcome.getByRole('button', { name: 'Search Classes', exact: true }).click();
  await page.getByRole('searchbox', { name: 'Search classes' }).fill('PAGE');
  const rows = page.locator('.results .sec');
  await expect(rows).toHaveCount(40);
  const paging = page.getByRole('navigation', { name: 'Search result pages' });
  await expect(paging).toContainText('1–40 of 103 sections');
  await paging.getByRole('button', { name: 'Next', exact: true }).click();
  await expect(paging).toContainText('41–80 of 103 sections');
  await expect(rows).toHaveCount(40);
  await expect(page.getByRole('list', { name: 'Class search results' })).toBeFocused();
  await paging.getByRole('button', { name: 'Next', exact: true }).click();
  await expect(paging).toContainText('81–103 of 103 sections');
  await expect(rows).toHaveCount(23);
  await expect(rows.last()).toContainText('102');
  await expect(paging.getByRole('button', { name: 'Next', exact: true })).toBeDisabled();
  await paging.getByRole('button', { name: 'Previous', exact: true }).click();
  await expect(paging).toContainText('41–80 of 103 sections');
  await page.getByRole('searchbox', { name: 'Search classes' }).fill('PAGE 1000 102');
  await expect(rows).toHaveCount(1);
  await expect(rows.first()).toContainText('102');
});
