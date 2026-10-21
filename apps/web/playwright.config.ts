import { defineConfig } from '@playwright/test';
const port = Number(process.env.HONORUM_TEST_PORT || 18080);
if (!Number.isInteger(port) || port < 1 || port > 65535) throw new Error('Invalid HONORUM_TEST_PORT');
const baseURL = `http://127.0.0.1:${port}`;
export default defineConfig({
  testDir: './e2e', fullyParallel: false, workers: 1,
  use: {
    baseURL, trace: 'retain-on-failure',
    launchOptions: { executablePath: process.env.HONORUM_TEST_BROWSER || undefined },
    // Tests start in a browser that has opened the app; the root shows new
    // visitors the landing page instead (landing.spec.ts).
    storageState: { cookies: [{ name: 'hn_app', value: '1', domain: '127.0.0.1', path: '/', expires: -1, httpOnly: true, secure: false, sameSite: 'Lax' }], origins: [] },
  },
  webServer: { command: 'sh e2e/server.sh', url: `${baseURL}/api/config`, timeout: 600000, reuseExistingServer: false },
});
