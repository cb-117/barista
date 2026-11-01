// Serves baristatime.app: the static site in ../site, plus a reverse proxy for PostHog
// analytics at /ingest/* (first-party, so ad blockers don't drop page views).
const POSTHOG_API = "us.i.posthog.com";
const POSTHOG_ASSETS = "us-assets.i.posthog.com";
const CANONICAL_HOST = "baristatime.app";

export default {
  async fetch(request, env) {
    const url = new URL(request.url);

    if (url.hostname === `www.${CANONICAL_HOST}`) {
      url.hostname = CANONICAL_HOST;
      return Response.redirect(url.toString(), 301);
    }

    if (url.pathname === "/ingest" || url.pathname.startsWith("/ingest/")) {
      return proxyPostHog(request, url);
    }

    return env.ASSETS.fetch(request);
  },
};

async function proxyPostHog(request, url) {
  const path = url.pathname.slice("/ingest".length) || "/";
  const host = path.startsWith("/static/") || path.startsWith("/array/") ? POSTHOG_ASSETS : POSTHOG_API;
  const target = new URL(path + url.search, `https://${host}`);

  const upstream = new Request(target, request);
  upstream.headers.delete("cookie");
  const ip = request.headers.get("CF-Connecting-IP");
  if (ip) upstream.headers.set("X-Forwarded-For", ip);

  return fetch(upstream);
}
