import { test, expect } from '@playwright/test';
import { createServer } from 'node:http';
import { readFileSync } from 'node:fs';
import type { AddressInfo } from 'node:net';

test('the policy and landing pages follow hosting redirects and remain available offline', async ({ page, context }) => {
  const worker = readFileSync(new URL('../public/sw.js', import.meta.url), 'utf8')
    .replace('__BUILD_ID__', 'policy-redirect-test').replace('__PRECACHE__', JSON.stringify(['/', '/legal.html', '/about.html']));
  const policy = readFileSync(new URL('../public/legal.html', import.meta.url), 'utf8');
  const landing = readFileSync(new URL('../public/about.html', import.meta.url), 'utf8');
  // Match Cloudflare's .html redirect with real HTTP responses so the browser
  // enforces service-worker navigation rules that route mocks cannot exercise.
  const server = createServer((request, response) => {
    if (request.url === '/legal.html') {
      response.writeHead(307, { Location: '/legal' }).end();
    } else if (request.url === '/legal') {
      response.writeHead(200, { 'Content-Type': 'text/html' }).end(policy);
    } else if (request.url === '/about.html') {
      response.writeHead(307, { Location: '/about' }).end();
    } else if (request.url === '/about') {
      response.writeHead(200, { 'Content-Type': 'text/html' }).end(landing);
    } else if (request.url === '/sw.js') {
      response.writeHead(200, { 'Content-Type': 'text/javascript' }).end(worker);
    } else if (request.url === '/') {
      response.writeHead(200, { 'Content-Type': 'text/html' }).end('<script>navigator.serviceWorker.register("/sw.js")</script>');
    } else response.writeHead(404).end();
  });
  await new Promise<void>(resolve => server.listen(0, '127.0.0.1', resolve));
  const origin = `http://127.0.0.1:${(server.address() as AddressInfo).port}`;
  try {
    await page.goto(origin);
    await page.evaluate(() => navigator.serviceWorker.ready.then(() => {}));
    await expect.poll(() => page.evaluate(() => !!navigator.serviceWorker.controller)).toBe(true);
    await expect.poll(() => page.evaluate(() => caches.match('/legal.html').then(response => response?.redirected))).toBe(true);

    await page.goto(`${origin}/legal.html#privacy`);
    await expect(page).toHaveURL(`${origin}/legal#privacy`);
    await expect(page.getByRole('heading', { name: 'Privacy Policy', exact: true })).toBeVisible();

    await context.setOffline(true);
    await page.reload();
    await expect(page.getByRole('heading', { name: 'Privacy Policy', exact: true })).toBeVisible();
    await page.goto(`${origin}/legal.html#terms`);
    await expect(page).toHaveURL(`${origin}/legal#terms`);
    await expect(page.getByRole('heading', { name: 'Terms of Service', exact: true })).toBeVisible();
    await expect(page.getByRole('link', { name: 'support@honorum.app', exact: true })).toHaveAttribute('href', 'mailto:support@honorum.app');
    await page.goto(`${origin}/about.html`);
    await expect(page).toHaveURL(`${origin}/about`);
    await expect(page.getByRole('heading', { name: 'Honorum', level: 1 })).toBeVisible();
    await expect(page.getByRole('link', { name: 'Open Honorum' }).first()).toHaveAttribute('href', '/app');
  } finally {
    await context.setOffline(false);
    server.closeAllConnections();
    await new Promise<void>((resolve, reject) => server.close(error => error ? reject(error) : resolve()));
  }
});
