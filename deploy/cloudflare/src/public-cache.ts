// These endpoints publish the same bytes for every visitor. Private APIs never
// enter this cache, and credentials are not forwarded on public cache misses.
export function isPublicRead(request: Request): boolean {
  if (request.method !== 'GET' && request.method !== 'HEAD') return false;
  const path = new URL(request.url).pathname;
  return path === '/api/config' || /^\/catalog\/[A-Za-z0-9_-]+\/[A-Za-z0-9_-]+\.(?:structure|seats|desc)\.[a-f0-9]{64}\.json$/.test(path);
}

export function publicEncoding(header: string): string {
  const values = new Map<string, number>();
  for (const item of header.toLowerCase().split(',')) {
    const [name, ...params] = item.trim().split(';');
    let quality = 1;
    for (const param of params) {
      const [key, raw] = param.trim().split('=');
      if (key === 'q') { const value = Number(raw); quality = Number.isFinite(value) && value >= 0 && value <= 1 ? value : 0; }
    }
    values.set(name, quality);
  }
  const quality = (name: string) => values.get(name) ?? values.get('*') ?? 0;
  const best = ['br', 'gzip'].sort((a, b) => quality(b) - quality(a)).find(name => quality(name) > 0);
  // Preserve an explicit rejection of identity so the origin can return 406.
  const identity = values.get('identity') ?? (values.get('*') === 0 ? 0 : 1);
  return best || (identity === 0 ? 'identity;q=0' : 'identity');
}

// Cloudflare normalizes the ordinary header to "br, gzip" at ingress; what this
// visitor actually accepts decides the representation.
export function visitorEncoding(request: Request): string {
  const original = request.cf?.clientAcceptEncoding;
  return publicEncoding(typeof original === 'string' ? original : request.headers.get('Accept-Encoding') || '');
}

interface PublicCache {
  match(request: Request): Promise<Response | undefined>;
  put(request: Request, response: Response): Promise<void>;
}
interface WaitContext { waitUntil(promise: Promise<unknown>): void }
const originPolicyHeader = 'X-Honorum-Origin-Cache-Control';
const storedEncodingHeader = 'X-Honorum-Stored-Encoding';

function clientResponse(response: Response, request: Request, hit: boolean): Response {
  const headers = new Headers(response.headers);
  // Cache API hits may reflect the zone's longer Browser Cache TTL. Restore
  // the publication's browser policy after retrieval, before sending it out.
  const originPolicy = headers.get(originPolicyHeader);
  if (originPolicy) headers.set('Cache-Control', originPolicy);
  headers.delete(originPolicyHeader);
  const storedEncoding = headers.get(storedEncodingHeader);
  headers.delete(storedEncodingHeader);
  if (storedEncoding) {
    headers.set('Content-Encoding', storedEncoding);
    headers.set('Content-Type', 'application/json');
    headers.set('Vary', 'Accept-Encoding');
  }
  headers.set('X-Honorum-Cache', hit ? 'HIT' : 'MISS');
  const etag = headers.get('ETag');
  const match = request.headers.get('If-None-Match')?.split(',').some(value => {
    const tag = value.trim();
    return tag === '*' || !!etag && tag.replace(/^W\//, '') === etag.replace(/^W\//, '');
  });
  if (response.ok && match) { headers.delete('Content-Length'); return new Response(null, { status: 304, headers }); }
  return new Response(request.method === 'HEAD' ? null : response.body, { status: response.status, statusText: response.statusText, headers, encodeBody: storedEncoding ? 'manual' : 'automatic' });
}

export async function publicResponse(request: Request, origin: (request: Request) => Promise<Response>, cache: PublicCache, ctx: WaitContext): Promise<Response> {
  const url = new URL(request.url);
  url.search = ''; // Public routes have no query-dependent representation.
  // Keep separate variants for each encoding visitors accept.
  const encoding = visitorEncoding(request);
  const headers = new Headers({ 'Accept-Encoding': encoding });
  if (request.headers.has('Range')) {
    headers.set('Range', request.headers.get('Range')!);
    if (request.headers.has('If-Range')) headers.set('If-Range', request.headers.get('If-Range')!);
    return origin(new Request(url, { method: request.method, headers, signal: request.signal }));
  }
  // Cache API decodes ordinary Content-Encoding responses when storing them.
  // Ask the origin for prepared bytes as opaque data, then restore the encoding
  // only on the response to the visitor.
  if (url.pathname.startsWith('/catalog/')) headers.set('X-Honorum-Store-Encoded', '1');
  const keyURL = new URL(url);
  keyURL.pathname = '/__public_cache/v3' + keyURL.pathname;
  keyURL.searchParams.set('encoding', encoding);
  const key = new Request(keyURL, { headers });
  // Explicit reloads may revalidate the short-lived public manifest immediately.
  const requestPolicy = request.headers.get('Cache-Control') || '';
  const noStore = /\bno-store\b/i.test(requestPolicy);
  const revalidate = /(?:no-cache|no-store|max-age\s*=\s*0)/i.test(requestPolicy);
  const saved = !revalidate && await cache.match(key);
  if (saved) return clientResponse(saved, request, true);
  let response = await origin(new Request(url, { headers, signal: request.signal }));
  let policy = response.headers.get('Cache-Control') || '';
  if (response.status === 200 && !response.headers.has('Set-Cookie') && /\bpublic\b/i.test(policy) && !/\b(?:private|no-store)\b/i.test(policy)) {
    const storedHeaders = new Headers(response.headers);
    // These bodies already have publication-time gzip/Brotli representations.
    // Keep cache hits from decompressing and recompressing them at lower quality.
    if (url.pathname.startsWith('/catalog/') && !/\bno-transform\b/i.test(policy)) policy += ', no-transform';
    storedHeaders.set('Cache-Control', policy);
    storedHeaders.set(originPolicyHeader, policy);
    response = new Response(response.body, { status: response.status, statusText: response.statusText, headers: storedHeaders });
    if (!noStore) ctx.waitUntil(cache.put(key, response.clone()).catch(() => {}));
  }
  return clientResponse(response, request, false);
}
