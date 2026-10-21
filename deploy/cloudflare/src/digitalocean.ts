interface PrivateBackend { fetch(request: Request): Promise<Response> }
export interface DigitalOceanSettings {
  DIGITALOCEAN_API?: PrivateBackend;
  DIGITALOCEAN_WORKER?: PrivateBackend;
}

export function digitalOceanConfigured(env: DigitalOceanSettings): boolean {
  return !!(env.DIGITALOCEAN_API && env.DIGITALOCEAN_WORKER);
}

// Workers VPC selects the registered private host and port. HTTP here is only
// between cloudflared and loopback; the tunnel encrypts the external connection.
export async function digitalOceanFetch(request: Request, env: DigitalOceanSettings): Promise<Response> {
  const url = new URL(request.url);
  const service = url.pathname.startsWith('/catalog/') ? env.DIGITALOCEAN_WORKER : env.DIGITALOCEAN_API;
  if (!service) return new Response('Backend configuration is unavailable.', {status:503, headers:{'Cache-Control':'no-store'}});
  const headers = new Headers(request.headers);
  headers.delete('Forwarded');
  headers.delete('X-Real-IP');
  headers.delete('X-Forwarded-For');
  const client = request.headers.get('CF-Connecting-IP');
  if (client) headers.set('X-Forwarded-For', client);
  headers.set('X-Forwarded-Proto', 'https');
  url.protocol = 'http:';
  return service.fetch(new Request(new Request(url, request), {headers, redirect:'manual'}));
}
