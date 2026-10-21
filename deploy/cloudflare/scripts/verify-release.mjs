// Verify the deployed frontend and the independently released DigitalOcean API.
import { readFile } from 'node:fs/promises';
const [version, base = 'https://honorum.app'] = process.argv.slice(2);
if (!/^[a-f0-9]{7,40}$/.test(version || '')) throw new Error('Supply the deployed commit ID');
const local = await readFile(new URL('../../../apps/web/dist/index.html', import.meta.url), 'utf8');
const expected = [...local.matchAll(/(?:src|href)="(\/assets\/[^" ]+)"/g)].map(match => match[1]);
if (!expected.length) throw new Error('No built entry assets found');
const deadline = Date.now() + 10 * 60_000;
let reason = '';
while (Date.now() < deadline) {
  try {
    const load = async path => {
      const response = await fetch(base + path, { headers: { 'Cache-Control': 'no-cache' }, signal: AbortSignal.timeout(15_000) });
      if (!response.ok) throw new Error(`${path} returned ${response.status}`);
      return response;
    };
    await load('/readyz');
    const html = await (await load('/')).text();
    if (!expected.every(asset => html.includes(asset))) throw new Error('Frontend entry assets have not switched yet');
    const config = await (await load('/api/config')).json();
    if (!/^[a-f0-9]{7,40}$/.test(config.version || '')) throw new Error('Backend release version is unavailable');
    console.log(`Release verified: frontend ${version}, backend ${config.version}, readiness 200`);
    process.exit(0);
  } catch (error) {
    const current = error.message;
    if (current !== reason) { console.log(current); reason = current; }
  }
  await new Promise(resolve => setTimeout(resolve, 5000));
}
throw new Error(`Release did not become ready within ten minutes: ${reason}`);
