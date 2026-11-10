# Counting active users

Barista counts how many copies are in use each day, week, and month, without identifying any of them.
This page is everything that is sent, when, and what happens to it. It is also the specification both
implementations follow (Swift for macOS, Rust for Windows and Linux). The server's code is
[`worker/index.js`](../worker/index.js).

Turn it off in Settings → Updates → **Help count active users** (the tray menu's Settings on Windows and
Linux). It is on by default.

## What the app sends

At most once per day (UTC), while it is running, the Barista app makes one request:

```
GET https://baristatime.app/api/ping?v=0.1.0&os=mac&d=1&w=0&m=0&n=0
User-Agent: Barista/0.1.0
```

| Field | Meaning |
| --- | --- |
| `v` | Barista's version. |
| `os` | `mac`, `windows`, or `linux`. |
| `d` | Always `1`: the first ping today (UTC). |
| `w` | `1` on the first ping of the ISO week (Monday to Sunday, UTC), else `0`. |
| `m` | `1` on the first ping of the calendar month (UTC), else `0`. |
| `n` | `1` on this copy's first ping ever, else `0`. |

That is all. There is no install ID, account, or cookie, and nothing about you, your computer, your
sessions, your agents, or your usage. (The Mac app also sends the fixed header `Accept-Language: en`, so
the system doesn't add your preferred languages.) The `w`, `m`, and `n` flags are worked out on your computer from
the date of the last ping, which is what lets the server count weekly and monthly users without being
able to tell one copy from another.

Only the app pings. The `barista` command, the agent hooks, and the MCP server never do.

## When

The app remembers the UTC date of its last successful ping as `YYYY-MM-DD` (preference `lastActivePing`).
Ten seconds after it starts, and then every hour, it checks: if the setting is on (preference
`shareActiveCount`, default on) and today's UTC date is not the remembered date, it pings. With `L` the
remembered date (if any) and `T` today's:

- `d` = `1`.
- `w` = `1` if there is no `L`, or `L` and `T` are in different ISO weeks (compare ISO week-year and ISO
  week number); else `0`.
- `m` = `1` if there is no `L`, or `L` and `T` differ in year or month; else `0`.
- `n` = `1` if there is no `L`; else `0`.

A remembered date that isn't a valid `YYYY-MM-DD` counts as no `L`. On any HTTP 2xx answer the app
remembers `T`. On anything else it remembers nothing and tries again at the next hourly check.

The request has a 15-second timeout, follows no redirects, and keeps no cookies or cache. The Rust build
runs the system's `curl -q` (first argument, so no `.curlrc` applies) without `-L`. `BARISTA_PING_URL`
replaces `https://baristatime.app/api/ping` (for tests; the request carries no login, so there is nothing
to protect). The Rust app never pings when run for tests (`ephemeral`), and the Windows installer's
smoke test points the ping at an unused local address.

## What the server does

`/api/ping` on baristatime.app (a Cloudflare Worker) checks the fields: `v` is one to four dot-separated
numbers and at most 20 characters, `os` is one of the three, `d` is `1`, and `w`, `m`, `n` are each `0` or
`1`. Anything else gets `400` and is not counted. A valid ping gets `204` with no body, and the Worker
records one event, `app_ping`, in PostHog (the analytics service the website already uses) with exactly
these properties: `version`, `os`, `daily`, `weekly`, `monthly`, `new`.

- The event's ID is random and made up for that one event, so events can't be linked to each other or
  to a person.
- The Worker doesn't read, log, or pass on your IP address, and location lookup is switched off for
  the event. Cloudflare's per-request logs for the Worker are switched off too
  ([`wrangler.jsonc`](../wrangler.jsonc)). Cloudflare, which hosts the site, handles the connection
  the way any web host does.
- Nothing else from the request is kept.

## Reading the counts

In PostHog, on the `app_ping` event:

- **Daily active:** the number of events that day.
- **Weekly active:** the number of events with `weekly = 1` in that ISO week.
- **Monthly active:** the number of events with `monthly = 1` in that month.
- **New installs:** the number of events with `new = 1`.

Break any of them down by `os` or `version`; the version breakdown shows how fast updates reach people.

## What the numbers mean

- They count copies of the app (one per user account on a computer), not people.
- A copy counts on a day it ran with the setting on. Days are UTC, and weeks and months are calendar
  ones, not rolling 7- or 30-day windows.
- Anyone can send a made-up ping, so treat the numbers as an estimate.
