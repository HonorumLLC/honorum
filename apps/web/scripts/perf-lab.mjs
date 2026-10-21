// A local stand-in for production hosting, so performance runs compare builds
// rather than networks. It serves a build the way the Cloudflare Worker does
// (a precompressed copy when the build has one, otherwise Brotli at the edge's
// on-the-fly quality 4, with the Worker's cache headers), public data recorded
// from a server, and forwards other API requests to the performance lab API
// (internal/server/perf_lab_test.go). HTTP/2 over a throwaway certificate
// multiplexes requests as production does.
//
// A browser context names its network in a cookie, lab=<id>:<latency ms>:
// <downlink bytes per second>. Requests reach the server after half the
// latency and responses start after the other half; bodies share the downlink.
// GET /__lab/log?id=<id> lists what that context transferred, including
// service worker and worker requests. POST /__lab/publish?kinds=seats,structure
// republishes the recorded terms, as a later day would.
//
// Standalone: node scripts/perf-lab.mjs DIST FIXTURE [API_URL] [PORT]
import http from 'node:http';
import http2 from 'node:http2';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import zlib from 'node:zlib';
import { createHash, createPublicKey } from 'node:crypto';
import { execFileSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';

const TYPES = { '.html': 'text/html', '.js': 'text/javascript', '.css': 'text/css', '.json': 'application/json', '.webmanifest': 'application/manifest+json', '.svg': 'image/svg+xml', '.woff2': 'font/woff2', '.png': 'image/png', '.ico': 'image/x-icon' };
const COMPRESSIBLE = new Set(['.html', '.js', '.css', '.json', '.webmanifest', '.svg', '.ico']);
const brotli = (body, quality) => zlib.brotliCompressSync(body, { params: { [zlib.constants.BROTLI_PARAM_QUALITY]: quality, [zlib.constants.BROTLI_PARAM_SIZE_HINT]: body.length } });
const etag = (body) => `"${createHash('sha1').update(body).digest('hex').slice(0, 20)}"`;
const HOP = new Set(['connection', 'keep-alive', 'transfer-encoding', 'upgrade', 'proxy-connection', 'http2-settings', 'te', 'host']);
const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

// Files a build publishes, as the Worker's asset binding would return them.
function readBuild(dist) {
  const files = new Map();
  for (const rel of fs.readdirSync(dist, { recursive: true })) {
    const file = path.join(dist, rel);
    if (!fs.statSync(file).isFile() || rel.endsWith('.map') || rel.endsWith('.br') || rel.endsWith('.gz')) continue;
    const body = fs.readFileSync(file), ext = path.extname(file), url = '/' + rel.replaceAll(path.sep, '/');
    const entry = { body, type: TYPES[ext] || 'application/octet-stream', etag: etag(body) };
    // The Worker serves the build's Brotli copy of the page, the service
    // worker and the bundles (deploy/cloudflare/src/static-assets.ts);
    // otherwise the edge compresses on the fly.
    if ((url === '/index.html' || url === '/sw.js' || /^\/assets\/[\w.-]+\.(?:js|css)$/.test(url)) && fs.existsSync(file + '.br')) entry.br = fs.readFileSync(file + '.br');
    else if (COMPRESSIBLE.has(ext)) entry.br = brotli(body, 4);
    entry.cache = url === '/index.html' || url === '/sw.js' ? 'no-cache' : url.startsWith('/assets/') ? 'public, max-age=31536000, immutable' : 'public, max-age=0, must-revalidate';
    files.set(url, entry);
  }
  files.set('/', files.get('/index.html'));
  return files;
}

// Recorded public data: config.json and <inst>-<term>.<structure|seats|desc>.json.
// Publications are compressed once at level 6 when they are written.
const publication = (raw, value = null) => ({ body: raw, br: brotli(raw, 6), etag: etag(raw), value });
function readFixture(fixture, phoneAuth) {
  const config = JSON.parse(fs.readFileSync(path.join(fixture, 'config.json'), 'utf8'));
  if (phoneAuth) config.phone_auth = true;
  const catalogs = new Map();
  for (const name of fs.readdirSync(fixture)) {
    const match = name.match(/^([A-Za-z0-9_-]+?)-([A-Za-z0-9_-]+)\.(structure|seats|desc)\.json$/);
    if (!match) continue;
    const raw = fs.readFileSync(path.join(fixture, name));
    catalogs.set(`${match[1]}/${match[2]}.${match[3]}`, publication(raw, match[3] === 'seats' ? JSON.parse(raw) : null));
  }
  return { config, catalogs };
}
const configEntry = (config) => { const body = Buffer.from(JSON.stringify(config)); return { body, br: brotli(body, 4), etag: etag(body), type: 'application/json', cache: 'public, max-age=30, s-maxage=60, must-revalidate' }; };

// Chromium trusts the certificate through its key's hash (spki). With errors
// merely ignored instead, it would not use its HTTP cache at all.
function certificate() {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'honorum-lab-'));
  execFileSync('openssl', ['req', '-x509', '-newkey', 'ec', '-pkeyopt', 'ec_paramgen_curve:P-256', '-nodes', '-keyout', path.join(dir, 'key.pem'), '-out', path.join(dir, 'cert.pem'), '-days', '2', '-subj', '/CN=127.0.0.1', '-addext', 'subjectAltName=IP:127.0.0.1,DNS:localhost'], { stdio: 'ignore' });
  const pair = { key: fs.readFileSync(path.join(dir, 'key.pem')), cert: fs.readFileSync(path.join(dir, 'cert.pem')) };
  fs.rmSync(dir, { recursive: true, force: true });
  const spki = createHash('sha256').update(createPublicKey(pair.key).export({ type: 'spki', format: 'der' })).digest('base64');
  return { ...pair, spki };
}

export async function startLab({ dist, fixture, api = '', port = 0, phoneAuth = false }) {
  const files = readBuild(dist);
  const { config: configValue, catalogs } = readFixture(fixture, phoneAuth);
  let config = configEntry(configValue), generation = 0;
  // Structure publications name their own hash, so a new one is a rewritten copy.
  const structures = new Map();
  // A later day: the recorded terms get new seat (and structure) publications
  // with the same content, the way the collector republishes after changes.
  function publish(kinds) {
    generation++;
    for (const inst of configValue.institutions) for (const term of inst.terms) {
      const key = `${inst.id}/${term.id}`, recorded = catalogs.get(key + '.structure');
      if (!recorded) continue;
      for (const kind of kinds) {
        const field = kind + '_snapshot', next = createHash('sha256').update(`${term[field]}:${generation}`).digest('hex');
        if (kind === 'structure') structures.set(next, publication(Buffer.from(recorded.body.toString().replace(`"hash":"${JSON.parse(recorded.body).hash}"`, `"hash":"${next}"`))));
        term[field] = next;
      }
    }
    config = configEntry(configValue);
  }
  const started = Date.now();
  const links = new Map(), logs = new Map();
  // One downlink per context: chunks leave in order, each after the ones before it.
  function linkOf(req) {
    const value = /(?:^|;\s*)lab=([^;]+)/.exec(req.headers.cookie || '')?.[1] || '';
    const [id = '', latency = '0', rate = '0'] = decodeURIComponent(value).split(':');
    if (!links.has(id)) links.set(id, { id, latency: +latency || 0, rate: +rate || 0, busyUntil: 0 });
    return links.get(id);
  }
  async function transmit(link, res, chunk) {
    if (!chunk?.length) return;
    if (link.rate) {
      for (let at = 0; at < chunk.length; at += 16384) {
        const part = chunk.subarray(at, at + 16384);
        const start = Math.max(Date.now(), link.busyUntil);
        link.busyUntil = start + (part.length / link.rate) * 1000;
        await sleep(Math.max(0, link.busyUntil - Date.now()));
        if (!res.destroyed) res.write(part);
      }
    } else if (!res.destroyed) res.write(chunk);
  }
  const acceptsBrotli = (req) => /\bbr\b/.test(req.headers['accept-encoding'] || '');
  const exams = new Map();
  const examsOf = (key) => {
    if (!exams.has(key)) {
      const structure = catalogs.get(key + '.structure');
      exams.set(key, new Map(structure ? JSON.parse(structure.body).courses.flatMap((c) => c.sections.map((s) => [s.id, s.exams || []])) : []));
    }
    return exams.get(key);
  };

  async function handle(req, res) {
    const url = new URL(req.url, 'https://lab');
    if (url.pathname === '/__lab/log') {
      const id = url.searchParams.get('id') || '';
      if (req.method === 'DELETE') logs.delete(id);
      res.writeHead(200, { 'content-type': 'application/json' });
      res.end(JSON.stringify(logs.get(id) || []));
      return;
    }
    if (url.pathname === '/__lab/publish') { publish((url.searchParams.get('kinds') || 'seats').split(',')); res.writeHead(204); res.end(); return; }
    const link = linkOf(req);
    const entry = { method: req.method, path: url.pathname + url.search, start: Date.now() - started, status: 0, bytes: 0, encoding: '' };
    if (!logs.has(link.id)) logs.set(link.id, []);
    logs.get(link.id).push(entry);
    const finish = () => { entry.end = Date.now() - started; };
    res.on('close', finish);
    await sleep(link.latency / 2);
    const respond = async (status, headers, body) => {
      await sleep(link.latency / 2);
      entry.status = status; entry.headers = Date.now() - started; entry.encoding = headers['content-encoding'] || '';
      if (res.destroyed) return;
      res.writeHead(status, headers);
      if (req.method !== 'HEAD' && body) { entry.bytes += body.length; await transmit(link, res, body); }
      res.end();
    };
    const serve = (item, extra = {}) => {
      const headers = { 'content-type': item.type || 'application/json', 'cache-control': item.cache, etag: item.etag, vary: 'Accept-Encoding', ...extra };
      if (req.headers['if-none-match'] === item.etag) return respond(304, headers);
      if (item.br && acceptsBrotli(req)) return respond(200, { ...headers, 'content-encoding': 'br' }, item.br);
      return respond(200, headers, item.body);
    };

    if (url.pathname === '/api/config') return serve(config);
    const catalog = url.pathname.match(/^\/catalog\/([A-Za-z0-9_-]+)\/([A-Za-z0-9_-]+)\.(structure|seats|desc)\.([a-f0-9]{64})\.json$/);
    if (catalog) {
      const item = (catalog[3] === 'structure' && structures.get(catalog[4])) || catalogs.get(`${catalog[1]}/${catalog[2]}.${catalog[3]}`);
      return item ? serve({ ...item, type: 'application/json', cache: 'public, max-age=31536000, immutable, no-transform' }) : respond(404, {});
    }
    if (url.pathname === '/api/seats') {
      const key = `${url.searchParams.get('inst')}/${url.searchParams.get('term')}`;
      const seats = catalogs.get(key + '.seats')?.value?.seats || {}, exams = examsOf(key);
      // Like the API: seat counts, and each section's published exam sittings.
      const out = (url.searchParams.get('ids') || '').split(',').filter((id) => seats[id]).map((id) => ({ id, capacity: seats[id][0], available: seats[id][1], waitlist: seats[id][2], fetched_at: Math.floor(Date.now() / 1000), ...(exams.has(id) ? { exams: exams.get(id) } : {}) }));
      const body = Buffer.from(JSON.stringify(out));
      return respond(200, { 'content-type': 'application/json', 'cache-control': 'no-store' }, body);
    }
    if (/^\/(api|status)(\/|$)/.test(url.pathname)) {
      if (!api) return respond(404, {});
      const headers = Object.fromEntries(Object.entries(req.headers).filter(([k]) => !k.startsWith(':') && !HOP.has(k)));
      headers.host = req.headers[':authority'] || req.headers.host;
      // Distinct students, as far as the server's quotas can tell.
      headers['x-forwarded-for'] = `10.${[...createHash('sha1').update(link.id).digest().subarray(0, 3)].join('.')}`;
      const relay = async (response) => {
        const out = Object.fromEntries(Object.entries(response.headers).filter(([k]) => !HOP.has(k)));
        const type = String(response.headers['content-type'] || '');
        // The edge compresses JSON on the fly; streams pass through as they arrive.
        if (type.startsWith('application/json') && acceptsBrotli(req) && !response.headers['content-encoding']) {
          const chunks = [];
          for await (const chunk of response) chunks.push(chunk);
          const body = Buffer.concat(chunks);
          delete out['content-length'];
          return respond(response.statusCode, body.length > 256 ? { ...out, 'content-encoding': 'br' } : out, body.length > 256 ? brotli(body, 4) : body);
        }
        await sleep(link.latency / 2);
        entry.status = response.statusCode; entry.headers = Date.now() - started;
        if (res.destroyed) { response.destroy(); return; }
        res.writeHead(response.statusCode, out);
        res.on('close', () => response.destroy());
        for await (const chunk of response) { entry.bytes += chunk.length; await transmit(link, res, chunk); }
        res.end();
      };
      const upstream = http.request(api + url.pathname + url.search, { method: req.method, headers });
      req.pipe(upstream);
      upstream.on('error', () => { if (res.headersSent) res.destroy(); else respond(502, {}).catch(() => {}); });
      // A closed page ends its event stream mid-read; that is not a lab failure.
      upstream.on('response', (response) => relay(response).catch(() => { response.destroy(); res.destroy(); }));
      return;
    }
    const item = files.get(url.pathname);
    if (!item) return respond(404, { 'content-type': 'text/plain' }, Buffer.from('Not found'));
    return serve(item);
  }

  const { key, cert, spki } = certificate();
  const server = http2.createSecureServer({ key, cert, allowHTTP1: true }, (req, res) => { handle(req, res).catch(() => { if (!res.headersSent) res.writeHead(500); res.end(); }); });
  // Open event streams would otherwise keep close() waiting.
  const sessions = new Set();
  server.on('session', (session) => { sessions.add(session); session.on('close', () => sessions.delete(session)); });
  await new Promise((resolve) => server.listen(port, '127.0.0.1', resolve));
  const url = `https://127.0.0.1:${server.address().port}`;
  return {
    url,
    // Launch Chromium with this flag so the lab certificate is trusted.
    flag: `--ignore-certificate-errors-spki-list=${spki}`,
    log: (id) => logs.get(id) || [],
    publish,
    close: () => new Promise((resolve) => { server.close(resolve); for (const session of sessions) session.destroy(); }),
  };
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  const [dist, fixture, api = '', port = '18443'] = process.argv.slice(2);
  if (!dist || !fixture) throw new Error('Usage: node scripts/perf-lab.mjs DIST FIXTURE [API_URL] [PORT]');
  const lab = await startLab({ dist, fixture, api, port: +port });
  console.log(`Performance lab on ${lab.url}; launch Chromium with ${lab.flag}`);
}
