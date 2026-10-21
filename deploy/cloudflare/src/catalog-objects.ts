// Only immutable public catalog paths can read this private R2 bucket.
import { publicEncoding } from "./public-cache.ts";

export async function catalogObject(request: Request, bucket: R2Bucket): Promise<Response | null> {
  const path = new URL(request.url).pathname;
  const match = /^\/catalog\/[A-Za-z0-9_-]+\/[A-Za-z0-9_-]+\.(?:structure|seats|desc)\.([a-f0-9]{64})\.json$/.exec(path);
  if (!match || !["GET", "HEAD"].includes(request.method) || request.headers.has("Range")) return null;
  const encoding = publicEncoding(request.headers.get("Accept-Encoding") || "");
  if (encoding === "identity;q=0") return new Response(null, { status: 406 });
  const suffix = encoding === "br" ? ".br" : encoding === "gzip" ? ".gz" : "";
  const object = await bucket.get("honorum-public" + path + suffix);
  if (!object) return null;
  const opaque = request.headers.get("X-Honorum-Store-Encoded") === "1" && suffix !== "";
  const headers = new Headers({
    "Content-Type": opaque ? "application/octet-stream" : "application/json",
    "Cache-Control": "public, max-age=31536000, immutable, no-transform",
    "ETag": `"${match[1]}-${encoding}"`,
    "Vary": "Accept-Encoding",
    "Content-Length": String(object.size),
  });
  if (suffix) headers.set(opaque ? "X-Honorum-Stored-Encoding" : "Content-Encoding", encoding);
  return new Response(request.method === "HEAD" ? null : object.body, { headers, encodeBody: "manual" });
}
