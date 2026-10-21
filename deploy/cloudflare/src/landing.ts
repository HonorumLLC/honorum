// The site root shows the landing page to people new to Honorum and the app
// to everyone else. Opening the app from the landing page goes through /app,
// which records the choice in a cookie; a session counts too. Each app
// response renews the cookie, so a guest whose installed app fetches its
// shell is recognized without visiting the landing page. The standalone
// server applies the same rule in internal/server/landing.go.

// Browsers cap cookie lifetimes at 400 days.
const APP_COOKIE = "hn_app=1; Path=/; Max-Age=34560000; HttpOnly; Secure; SameSite=Lax";

// A document navigation to the bare root by a browser that has neither opened
// the app nor signed in. The app's own requests, such as the offline shell's,
// are not navigations. Browsers that predate Sec-Fetch-Mode are told apart by
// whether they ask for HTML.
export function showsLanding(request: Request): boolean {
  const url = new URL(request.url);
  if (url.pathname !== "/" || url.search || (request.method !== "GET" && request.method !== "HEAD")) return false;
  const mode = request.headers.get("Sec-Fetch-Mode");
  if (mode ? mode !== "navigate" : !request.headers.get("Accept")?.includes("text/html")) return false;
  return !/(?:^|;)\s*(?:hn_app|hn_session)=/.test(request.headers.get("Cookie") ?? "");
}

// The response for the root or /app, or null for any other path. serve fetches
// a static file the usual way.
export async function siteResponse(request: Request, serve: (request: Request) => Promise<Response>): Promise<Response | null> {
  const path = new URL(request.url).pathname;
  if (path === "/app") {
    return new Response(null, { status: 303, headers: { Location: "/", "Set-Cookie": APP_COOKIE, "Cache-Control": "no-store" } });
  }
  if (path !== "/") return null;
  const landing = showsLanding(request);
  const response = await serve(landing ? new Request(new URL("/about", request.url), request) : request);
  response.headers.set("Cache-Control", "no-cache");
  response.headers.append("Vary", "Cookie");
  if (!landing) response.headers.append("Set-Cookie", APP_COOKIE);
  return response;
}
