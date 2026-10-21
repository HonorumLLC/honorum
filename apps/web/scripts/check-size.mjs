import { readFileSync } from 'node:fs';
import { gzipSync } from 'node:zlib';
const html = readFileSync('dist/index.html', 'utf8');
const files = [...html.matchAll(/(?:src|href)="(\/assets\/[^" ]+\.(?:js|css))"/g)].map(m => m[1]);
if (files.length < 2) throw new Error('Entry assets were not found');
const bytes = [...new Set(files)].reduce((total, file) => total + gzipSync(readFileSync('dist' + file)).length, 0);
console.log(`Entry JS + CSS: ${(bytes / 1000).toFixed(2)} KB gzip (100 KB budget)`);
if (bytes > 100000) process.exitCode = 1;

// Lazy chunks and public files also transfer when the offline shell installs.
const sw = readFileSync('dist/sw.js', 'utf8');
const manifest = sw.match(/^const PRECACHE = (\[.*\]);$/m);
if (!manifest) throw new Error('Offline shell manifest was not found');
const shell = [...new Set([...JSON.parse(manifest[1]), '/sw.js'])];
const shellBytes = shell.reduce((total, file) => total + gzipSync(readFileSync('dist' + (file === '/' ? '/index.html' : file))).length, 0);
console.log(`Complete offline shell: ${(shellBytes / 1000).toFixed(2)} KB gzip (200 KB budget; excludes API/catalog data and HTTP headers)`);
if (shellBytes > 200000) process.exitCode = 1;
