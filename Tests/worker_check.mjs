// Checks the website Worker's /api/ping (docs/ACTIVE-USERS.md) without the network.
//   node Tests/worker_check.mjs
import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';

const source = await readFile(new URL('../worker/index.js', import.meta.url), 'utf8');
const worker = (await import('data:text/javascript,' + encodeURIComponent(source))).default;

const sent = [];
let failing = false;
globalThis.fetch = async (target, init) => {
  if (failing) throw new Error('offline');
  sent.push({ target: String(target instanceof Request ? target.url : target), init, request: target instanceof Request ? target : null });
  return new Response('{"status":"Ok"}');
};
const assets = [];
const env = { ASSETS: { fetch: async (request) => { assets.push(request.url); return new Response('site'); } } };

async function call(path, init = {}) {
  const waiting = [];
  const response = await worker.fetch(new Request('https://baristatime.app' + path, init), env, { waitUntil: (promise) => waiting.push(promise) });
  await Promise.all(waiting);
  return response;
}

// A valid ping: 204, never cached, one event with exactly the six fields and nothing from the request.
const headers = { 'CF-Connecting-IP': '203.0.113.7', 'User-Agent': 'Barista/0.1.0', Cookie: 'session=secret' };
let response = await call('/api/ping?v=0.1.0&os=mac&d=1&w=1&m=0&n=0', { headers });
assert.equal(response.status, 204);
assert.equal(response.headers.get('Cache-Control'), 'no-store');
assert.equal(await response.text(), '');
assert.equal(sent.length, 1);
assert.equal(sent[0].target, 'https://us.i.posthog.com/i/v0/e/');
assert.equal(sent[0].init.method, 'POST');
assert.deepEqual(sent[0].init.headers, { 'Content-Type': 'application/json' });
const body = JSON.parse(sent[0].init.body);
assert.deepEqual(Object.keys(body).sort(), ['api_key', 'distinct_id', 'event', 'properties']);
assert.equal(body.event, 'app_ping');
assert.deepEqual(body.properties, { version: '0.1.0', os: 'mac', daily: 1, weekly: 1, monthly: 0, new: 0, $process_person_profile: false, $geoip_disable: true });
assert.match(body.distinct_id, /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/);
for (const secret of ['203.0.113.7', 'Barista/0.1.0', 'session=secret']) assert.ok(!sent[0].init.body.includes(secret), `${secret} must not be recorded`);

// Each event gets its own random ID, so two pings can't be linked.
await call('/api/ping?v=0.1.0&os=windows&d=1&w=0&m=0&n=1', { headers });
assert.equal(sent.length, 2);
assert.notEqual(JSON.parse(sent[1].init.body).distinct_id, body.distinct_id);
assert.deepEqual(JSON.parse(sent[1].init.body).properties.os, 'windows');
assert.equal((await call('/api/ping?v=12.3&os=linux&d=1&w=1&m=1&n=1')).status, 204);

// Anything else is refused and not counted.
const before = sent.length;
for (const query of [
  '', 'v=0.1.0&os=mac&d=1&w=1&m=0', 'v=0.1.0&os=mac&d=0&w=1&m=0&n=0', 'v=0.1.0&os=mac&d=1&w=2&m=0&n=0',
  'v=0.1.0&os=ios&d=1&w=1&m=0&n=0', 'v=abc&os=mac&d=1&w=1&m=0&n=0', 'v=1.2.3.4.5&os=mac&d=1&w=1&m=0&n=0',
  'v=1.2.3-beta&os=mac&d=1&w=1&m=0&n=0', 'v=' + '1'.repeat(21) + '&os=mac&d=1&w=1&m=0&n=0',
  'v=0.1.0&v=9.9.9&os=mac&d=1&w=1&m=0&n=0', 'v=0.1.0&os=mac&d=1&w=1&w=0&m=0&n=0', 'v=0.1.0&os=Mac&d=1&w=1&m=0&n=0',
]) {
  assert.equal((await call('/api/ping?' + query)).status, 400, query);
}
response = await call('/api/ping?v=0.1.0&os=mac&d=1&w=1&m=0&n=0', { method: 'POST' });
assert.equal(response.status, 405);
assert.equal(response.headers.get('Allow'), 'GET');
assert.equal(sent.length, before);

// If PostHog can't be reached the app still gets its answer; only the count is lost.
failing = true;
assert.equal((await call('/api/ping?v=0.1.0&os=mac&d=1&w=0&m=0&n=0')).status, 204);
failing = false;

// The rest of the site is untouched: pages come from the assets, /ingest is still proxied without cookies.
assert.equal(await (await call('/')).text(), 'site');
assert.equal(await (await call('/api/other')).text(), 'site');
assert.deepEqual(assets, ['https://baristatime.app/', 'https://baristatime.app/api/other']);
await call('/ingest/e/?v=1', { method: 'POST', body: '{}', headers });
const proxied = sent.at(-1);
assert.equal(proxied.target, 'https://us.i.posthog.com/e/?v=1');
assert.equal(proxied.request.headers.get('cookie'), null);

console.log('PASS worker: valid pings counted with only six fields and a random ID; bad pings refused; site and /ingest unchanged');
