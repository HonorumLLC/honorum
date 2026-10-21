// The web build publishes Brotli copies, at the highest quality, beside the
// page, the service worker and the hashed bundles (apps/web/vite.config.js).
// Served as they are, they are about 15% smaller than the edge's on-the-fly
// compression of the same files. Everything else is the asset binding's own
// response, with the site's security and cache headers. encoding is the one
// the visitor prefers (visitorEncoding in public-cache.ts).
const TYPES: Record<string, string> = { '.js': 'text/javascript; charset=utf-8', '.css': 'text/css; charset=utf-8', '.html': 'text/html; charset=utf-8' };

// The Brotli copy to try for a request, or null.
export function brotliCopy(request: Request, encoding: string): string | null {
  if ((request.method !== 'GET' && request.method !== 'HEAD') || request.headers.has('Range') || encoding !== 'br') return null;
  const path = new URL(request.url).pathname;
  if (path === '/') return '/index.html.br';
  // The root's landing page (landing.ts) is served from its clean URL.
  if (path === '/about') return '/about.html.br';
  return path === '/sw.js' || /^\/assets\/[\w.-]+\.(?:js|css)$/.test(path) ? path + '.br' : null;
}

export async function assetResponse(request: Request, assets: Fetcher, encoding: string): Promise<Response> {
  const path = new URL(request.url).pathname;
  const copy = brotliCopy(request, encoding);
  let asset: Response | null = null;
  if (copy) {
    const url = new URL(request.url);
    url.pathname = copy;
    // Validators travel along, so a revalidation compares the copy's own ETag.
    const response = await assets.fetch(new Request(url, { method: request.method, headers: request.headers }));
    if (response.status === 200 || response.status === 304) asset = response;
  }
  const precompressed = !!asset;
  asset ??= await assets.fetch(request);
  const headers = new Headers(asset.headers);
  if (precompressed) {
    headers.set('Content-Type', TYPES[copy!.slice(0, -3).match(/\.\w+$/)![0]]);
    headers.set('Content-Encoding', 'br');
    headers.set('Vary', 'Accept-Encoding');
  }
  headers.set('X-Content-Type-Options', 'nosniff');
  headers.set('X-Frame-Options', 'DENY');
  headers.set('Referrer-Policy', 'no-referrer');
  headers.set('Content-Security-Policy', "default-src 'self'; img-src 'self' data:; style-src 'self' 'unsafe-inline'; connect-src 'self'");
  if (['/', '/index.html', '/sw.js'].includes(path)) headers.set('Cache-Control', 'no-cache');
  if (path.startsWith('/assets/')) headers.set('Cache-Control', 'public, max-age=31536000, immutable');
  // A precompressed body is sent as it is; the runtime must not encode it again.
  return new Response(asset.body, { status: asset.status, statusText: asset.statusText, headers, encodeBody: precompressed ? 'manual' : 'automatic' });
}
