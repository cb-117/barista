// Serves baristatime.app: the static site in ../site, a reverse proxy for PostHog analytics at
// /ingest/* (first-party, so ad blockers don't drop page views), and /api/ping, which counts
// active copies of the app without identifying them (docs/ACTIVE-USERS.md).
const POSTHOG_API = "us.i.posthog.com";
// The website's public PostHog project key (the same one site/index.html uses). It can only add events.
const POSTHOG_KEY = "phc_5uhbCTwz0ILHRzLH91kGwoVOmaMS1fATr3gIp0XdPqB";
const POSTHOG_ASSETS = "us-assets.i.posthog.com";
const CANONICAL_HOST = "baristatime.app";

export default {
  async fetch(request, env, ctx) {
    const url = new URL(request.url);

    if (url.hostname === `www.${CANONICAL_HOST}`) {
      url.hostname = CANONICAL_HOST;
      return Response.redirect(url.toString(), 301);
    }

    if (url.pathname === "/api/ping") {
      return ping(request, url, ctx);
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

// One anonymous "a copy of Barista is running" ping from the app, at most once a day per copy.
// The app works out locally whether it is its first ping this week, this month, or ever, so
// counting needs no install ID. Nothing from the request is kept except the six fields below:
// not the IP address, not the headers.
function ping(request, url, ctx) {
  const headers = { "Cache-Control": "no-store" };
  if (request.method !== "GET") {
    return new Response(null, { status: 405, headers: { ...headers, Allow: "GET" } });
  }
  const event = pingEvent(url.searchParams);
  if (!event) return new Response(null, { status: 400, headers });
  ctx.waitUntil(recordPing(event));
  return new Response(null, { status: 204, headers });
}

// The fields of a valid ping, or null. v: one to four dot-separated numbers. os: mac, windows,
// or linux. d: always 1. w, m, n: 0 or 1 (first ping this week, this month, ever).
function pingEvent(params) {
  const one = (name) => {
    const values = params.getAll(name);
    return values.length === 1 ? values[0] : null;
  };
  const flag = (name) => {
    const value = one(name);
    return value === "0" || value === "1" ? Number(value) : null;
  };
  const version = one("v");
  const os = one("os");
  const [weekly, monthly, fresh] = ["w", "m", "n"].map(flag);
  if (version === null || version.length > 20 || !/^\d+(\.\d+){0,3}$/.test(version)) return null;
  if (!["mac", "windows", "linux"].includes(os) || one("d") !== "1") return null;
  if (weekly === null || monthly === null || fresh === null) return null;
  return { version, os, daily: 1, weekly, monthly, new: fresh };
}

// Records the ping in PostHog as one event with a random ID made up for it, so events can't be
// linked to each other or to a person. The request comes from this Worker, so PostHog never sees
// the app's IP address; location lookup is off as well. A failure only loses the count.
async function recordPing(event) {
  try {
    await fetch(`https://${POSTHOG_API}/i/v0/e/`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({
        api_key: POSTHOG_KEY,
        event: "app_ping",
        distinct_id: crypto.randomUUID(),
        properties: { ...event, $process_person_profile: false, $geoip_disable: true },
      }),
    });
  } catch {
    // Not counted.
  }
}
