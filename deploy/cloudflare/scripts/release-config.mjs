// Keep relative asset paths unchanged by writing beside the source.
import { readFile, writeFile } from 'node:fs/promises';
const version = process.argv[2];
if (!/^[a-f0-9]{7,40}$/.test(version || '')) throw new Error('Supply a Git commit ID');
const input = new URL('../wrangler.jsonc', import.meta.url);
const source = await readFile(input, 'utf8');
const updated = source.replace(/"VERSION"\s*:\s*"[^"]*"/, `"VERSION": "${version}"`);
if (updated === source) throw new Error('Worker VERSION setting was not found or already matches');
await writeFile(new URL('../wrangler.release.jsonc', import.meta.url), updated);
