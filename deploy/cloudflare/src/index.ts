// Static assets and cached public publications are served at the edge.
// Private APIs and catalog rebuilds use the DigitalOcean tunnel bindings.
import { siteResponse } from "./landing";
import { isPublicRead, publicResponse, visitorEncoding } from "./public-cache";
import { assetResponse } from "./static-assets";
import { catalogObject } from "./catalog-objects";
import { digitalOceanConfigured, digitalOceanFetch, type DigitalOceanSettings } from "./digitalocean";

interface Env extends DigitalOceanSettings {
  ASSETS: Fetcher;
  CATALOG: R2Bucket;
  MAINTENANCE?: string;
}

export default {
  async fetch(request: Request, env: Env, ctx: ExecutionContext): Promise<Response> {
    const path = new URL(request.url).pathname;
    if (!digitalOceanConfigured(env) && (/^\/(api|catalog|status|admin)(\/|$)/.test(path) || ["/readyz", "/healthz"].includes(path))) {
      return new Response("Backend configuration is unavailable.", { status: 503, headers: { "Cache-Control": "no-store" } });
    }
    if (env.MAINTENANCE === "1" && /^\/(api|catalog|status|admin)(\/|$)/.test(path)) {
      return new Response("Account sync is temporarily unavailable during database maintenance.", { status: 503, headers: { "Retry-After": "60", "Cache-Control": "no-store" } });
    }
    const origin = async (request: Request) => {
      const object = await catalogObject(request, env.CATALOG);
      return object ?? digitalOceanFetch(request, env);
    };
    if (isPublicRead(request)) {
      return publicResponse(request, origin, caches.default, ctx);
    }
    if (/^\/(api|catalog)(\/|$)/.test(path) || ["/status", "/admin", "/healthz", "/readyz"].includes(path)) {
      return digitalOceanFetch(request, env);
    }
    const serve = (request: Request) => assetResponse(request, env.ASSETS, visitorEncoding(request));
    return (await siteResponse(request, serve)) ?? serve(request);
  },

} satisfies ExportedHandler<Env>;
