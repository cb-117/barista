# Usage tracking

Barista reports how much Claude Code, Codex, and Cursor have been used: sessions, requests, and tokens
per agent and model for today, this week, and this month, with an estimated API-price cost. Everything
is computed on this computer from files the agents already write; nothing is sent anywhere.

This page is the specification both implementations follow (Swift for macOS, Rust for Windows and
Linux). `Tests/usage_check.py` checks both against the same fixture
(`Tests/fixtures/usage/`), so they must produce identical numbers.

## Where the numbers come from

**Claude Code**: `*.jsonl` files anywhere under each projects folder (subfolders included, for
subagents). Projects folders: each comma-separated entry of `CLAUDE_CONFIG_DIR` plus `/projects`; if
unset, `~/.claude/projects` and `~/.config/claude/projects` (whichever exist).

- Count a line only if it is a JSON object with `"type": "assistant"`, an object at `message.usage`, and
  `message.model` a string other than `<synthetic>`. Skip anything else, including unparseable lines.
- Time: top-level `timestamp` (ISO 8601, with or without fractional seconds). Skip the line if missing.
- Session: top-level `sessionId`, else the file name without `.jsonl`.
- Deduplicate: when `message.id` exists, the key is `message.id + ":" + requestId` (`requestId` may be
  absent: use an empty string). Claude Code writes one line per content block with the same usage, and
  resumed sessions copy earlier messages into new files, so a key is counted once across all files.
- Tokens: `input_tokens` → input, `output_tokens` → output, `cache_read_input_tokens` → cache read.
  Cache writes: if `usage.cache_creation` is an object, `ephemeral_5m_input_tokens` → 5-minute writes and
  `ephemeral_1h_input_tokens` → 1-hour writes; otherwise all of `cache_creation_input_tokens` → 5-minute.
  Missing numbers are 0. One counted line is one request.

**Codex**: `*.jsonl` under `$CODEX_HOME/sessions` and `$CODEX_HOME/archived_sessions` (`CODEX_HOME`
defaults to `~/.codex`), read top to bottom with per-file state:

- `"type": "session_meta"` → session = `payload.id` (else `payload.session_id`, else the file name).
- `"type": "turn_context"` → current model = `payload.model`. Before any, the model is `unknown`.
- `"type": "event_msg"` with `payload.type == "token_count"` and an object at `payload.info.total_token_usage`:
  let `total` be that object and `prev` the last counted total in this file.
  - If `prev` exists and `total.total_tokens <= prev.total_tokens`: duplicate, skip.
  - Else delta = `total - prev` field by field (or `total` itself when there is no `prev`), and `prev = total`.
  - Map: input = `input_tokens - cached_input_tokens` (not below 0), cache read = `cached_input_tokens`,
    output = `output_tokens` (it already includes `reasoning_output_tokens`); no cache writes.
  - Time is the line's `timestamp`. One counted event is one request.

**Cursor**: Cursor's local files have no token counts, so Barista records what its hooks see, in
`cursor-usage.jsonl` in Barista's data folder (below). One JSON object per line, written by
`barista hook --agent cursor` with a single append:

```json
{"conversation":"…","event":"prompt","generation":"…","model":"…","ts":"2026-11-01T16:20:04Z"}
{"conversation":"…","event":"stop","generation":"…","model":"…","status":"completed","ts":"2026-11-01T16:25:04Z"}
```

`model` and `status` are omitted when Cursor doesn't send them. Cursor reports a **turn** for each
`stop` (by distinct `generation`), its **active time** as `stop.ts - prompt.ts` for the same generation
(clamped to 0…4 hours; no prompt means 0), and **sessions** as distinct `conversation` values. A turn's
model is its stop's `model`, else its prompt's, else `unknown`, normalized like every model (below). Cursor has no token or cost figures.
Cursor's own token and cost data needs its Admin API (Enterprise teams); Barista doesn't call it.

**For every source**: log files are read in ascending byte order of their full path, so which copy of a
duplicate counts is deterministic. A request's **model** is its model ID cut at the first `[` or `@`
(`claude-haiku-5-5[1m]` → `claude-haiku-5-5`); date suffixes stay (`claude-sonnet-4-5-20250929`). A line
without a trailing newline is still being written: it is read on a later run, not now.

## Periods

Local time, ending now: **today** (from midnight), **week** (from Monday 00:00), **month** (from the 1st,
00:00), and **all**. A Claude or Codex request belongs to the period containing its timestamp; a Cursor
turn to the period containing its `stop`, and a Cursor session to every period with any of its events.

## Plan limits

Each plan's remaining allowance, shown beside usage. Percentages come from the agents themselves.

**Claude plan** (Claude Code, Cowork, and Claude chat share it). Three sources; the one observed most
recently wins:

0. **Your Claude account** (what the Claude app's Settings → Usage shows). The Barista app fetches it
   in the background at most once every 5 minutes (counting from the last attempt), unless turned off in
   Settings (preference `claudeAccountLimits`, default on). The only other fetch is the explicit
   `barista limits refresh` command.
   - Credential: Claude Code's saved login: on macOS the Keychain generic password with service
     `Claude Code-credentials`; on Linux and Windows `<CLAUDE_CONFIG_DIR or ~/.claude>/.credentials.json`
     (`BARISTA_CLAUDE_CREDENTIALS` overrides with a JSON file path, for tests). The JSON holds
     `claudeAiOauth.accessToken` and `claudeAiOauth.expiresAt` (Unix ms). Use the token only while it
     expires more than 60 seconds from now. **Never refresh it** (that would rotate Claude Code's login),
     never write it anywhere, never log it, and never pass it on a command line.
   - macOS Keychain access: background reads never show a prompt (no user interaction allowed). If the
     read needs your permission, the app shows **Allow access** in Usage & Limits; clicking it, or turning
     the setting on, does one read that may show macOS's prompt. If you deny it (or cancel), Barista
     remembers and stops trying until you click Allow access or turn the setting off and on. The token is
     kept in memory until it expires, so the Keychain is read rarely.
   - Request: `GET https://api.anthropic.com/api/oauth/usage` with `Authorization: Bearer <token>`,
     `anthropic-beta: oauth-2025-04-20`, `User-Agent: Barista/<version>`, 15-second timeout, no redirects
     followed, no cookies or caching. `BARISTA_CLAUDE_USAGE_URL` replaces the URL **only when
     `BARISTA_CLAUDE_CREDENTIALS` is also set** (tests), so nothing can redirect a real login. Rust runs
     `curl -q` (first argument, so no `.curlrc` applies) with headers on stdin (`-H @-`), and on Windows
     without a console window. Anything but HTTP 200 with a JSON object fails and keeps the previous result.
   - Result: written atomically to `claude-account-limits.json` in the data folder as
     `{"observedAt": "<ISO now>", "usage": <the response>}`, and read by `barista usage`, the MCP tool, and
     the app. Results older than **1 hour** are ignored. Turning the setting off deletes the file.
   - `barista limits refresh [--json]` fetches once, ignoring the 5-minute spacing. It prints
     `Claude account limits updated.` and exits 0, or exits 1 with one of: `No Claude Code login found.`,
     `Claude Code's login has expired. Open Claude Code to renew it.`, `Couldn't reach Anthropic
     (<detail>).`, `Anthropic replied <HTTP status>.` With `--json` it prints `{"ok": <bool>,
     "message": "<text>"}`.
   - Windows come from the response's `limits` array, keeping 0.0.8's ids: `kind` `session` → id
     `five_hour`, label `Session`, 300 minutes; `weekly_all` → id `seven_day`, label `Weekly`, 10080
     minutes; `weekly_scoped` → id `seven_day_<display name lowercased, spaces as _>`, label `Weekly ·
     <display name>` (from `scope.model.display_name`, else `scope.surface`, else `Scoped`), 10080
     minutes; any other kind → id = kind, label = kind with `_` as spaces, minutes null. `usedPercent` =
     `percent`, `resetsAt` = `resets_at`. If `limits` is missing, use `five_hour` / `seven_day`
     (`utilization`, `resets_at`). If `spend.enabled` is true, add id `extra_usage`, label `Extra
     usage`, `usedPercent` = `spend.percent`, `usedUSD`/`limitUSD` = `amount_minor / 10^exponent` of
     `spend.used` / `spend.limit`, minutes null; text output shows it as `Extra usage 5% ($12.50 of
     $250.00)`. Source label: `Claude account`.
1. **Claude Code's status line.** `barista statusline` is a status line command: Claude Code runs it after
   each reply and sends `rate_limits` (`five_hour`, `seven_day`, optional `spend_limit`, each with
   `used_percentage` and `resets_at` in Unix seconds; present only for Pro/Max/Ultra or a gateway spend
   limit). When `rate_limits` is an object, Barista writes `claude-limits.json` in its data folder,
   atomically: `{"observedAt": "<ISO now>", "rateLimits": <rate_limits as received>}`. Then it prints one
   line: `model.display_name`, then `5h N%` and `7d N%` for windows that haven't reset (N rounded to a
   whole number), joined by ` · `. If the user already had a status line, Barista runs it instead and
   prints its output (see Installing below).
2. **The Claude desktop app** keeps `plan-usage-history.json` (macOS `~/Library/Application Support/Claude`,
   Windows `%APPDATA%\Claude`; `BARISTA_CLAUDE_APP_DIR` overrides the folder): `{"samples": [{"t": <Unix
   ms>, "u": {"fh": <5-hour %>, "sd": <7-day %>}}]}`. Barista uses the sample with the largest `t`. It has
   no reset times. This file is undocumented and rarely updated, so a missing or unreadable file, or a
   newest sample older than **1 hour**, is ignored.

**ChatGPT plan** (Codex and the ChatGPT app share it). While reading Codex logs, a `token_count` event
whose `payload.rate_limits` has a non-null `primary` or `secondary` replaces the latest limits if its
timestamp is newer: `primary`/`secondary` each `{used_percent, window_minutes, resets_at}`, plus
`plan_type` and `credits` (`{balance, unlimited, has_credits}`). This is read even from events skipped as
duplicate token counts. Implementations cache it with the rest of the usage cache.

**Cursor**: not available on this computer; reported with `available: false`.

A window whose `resetsAt` has passed is reported with `usedPercent: 0` and `reset: true`. Labels come
from the window length: 300 minutes → `5-hour`, 10080 → `Weekly`, otherwise `N-hour` (whole hours under a
day) or `N-day`. Claude's `five_hour` is 300 minutes and `seven_day` 10080; the desktop app's `fh` and
`sd` likewise.

**Installing the status line.** `barista statusline install|uninstall|status [--settings PATH]` edits
`~/.claude/settings.json` (or PATH), preserving everything else, with the same backup and
concurrent-edit rules as hooks. Install sets `statusLine` to `{"type": "command", "command": <the CLI
quoted as for hooks> + " statusline"}`. If another status line was set, Barista first saves that whole
object, with the settings file's path, in `statusline-chain-<KEY>.json` in its data folder, where KEY is
the first 16 hex digits of the FNV-1a 64-bit hash of the settings file's resolved absolute path (UTF-8),
and the command gets ` --chain KEY` appended. `barista statusline --chain KEY` runs the saved `command`
(through `/bin/sh -c`, or `cmd /C` on Windows) with the same stdin and prints its stdout instead of its
own line (if the backup is missing, it prints its own line). Each settings file has its own backup, so
installing into one never touches another's. Uninstall restores the saved object (or removes `statusLine`
if the command has no `--chain`) and deletes that backup. If the command has `--chain KEY` but the backup
is missing or unreadable, uninstall fails with an error and leaves the settings file unchanged (the
`.barista-backup` copy from install still holds the original). Status prints `Connected`, `Connected (in front of your own status line)`, or
`Not connected`. `barista setup` and connecting Claude Code in the app also install it.

## Cost estimates

Prices come from `Sources/BaristaCore/ModelPrices.swift` (USD per million tokens; the Rust build reads
the same JSON). The model is cut at the first `[` or `@`, then the longest price entry that equals it or
is followed in it by `-` wins. Cost = input × `input` + output × `output` + cache read × `cacheRead` +
5-minute writes × `cacheWrite5m` + 1-hour writes × `cacheWrite1h`, divided by 1,000,000. Tokens of an
unlisted model are counted as **unpriced**. These are pay-as-you-go API rates, not what a Claude,
ChatGPT, or Cursor subscription charges, and every display says so.

## Cache

Re-reading every log each time would be slow (Claude Code keeps tens of MB), so each implementation
keeps a private cache in the data folder (`usage-cache.json` for Swift, `usage-cache-portable.json` for
Rust). Per log file it stores the byte offset read so far (always just after a newline, so a line being
written is re-read later), the per-file parser state (Codex `prev`, model, session), the dedupe keys the
file contributed, and the file's hourly buckets: `(model, session, hour start)` → input, output, cache
read, 5-minute writes, 1-hour writes, requests. A file that shrinks is re-read from the start (its own
keys and buckets dropped first). Buckets of files that disappear are kept, so totals outlive the agents'
own log cleanup. Deleting the cache rebuilds it from whatever logs remain.

Hourly buckets would miss requests near local midnight in time zones offset by 30 or 45 minutes, so
buckets are **15 minutes** long (`(model, session, quarter-hour start)`), and a bucket counts in a period
when its start is at or after the period's start and at or before now. Codex logs are identified by
**file name** (not path), so archiving a conversation (Codex moves its log from `sessions` to
`archived_sessions`) continues the same cache entry instead of counting it twice. A log that becomes
empty, or shorter than what was read, drops what it contributed and is read again from the start.

**Data folder**: `BARISTA_DATA_DIR` if set; else macOS `~/Library/Application Support/Barista`,
Linux `$XDG_DATA_HOME/barista` or `~/.local/share/barista`, Windows `%LOCALAPPDATA%\Barista`.

## Output

`barista usage [--period today|week|month|all] [--json]`. Without `--period`: today, week, and month. The text output starts with a **Limits** block (one line per plan, e.g. `Claude plan   5-hour 24% (resets 3:10 PM) · Weekly 0% (reset)` with `as of …` when older than 10 minutes).
With `--period`, that period alone, broken down by model. Runs locally; never needs the app.

JSON (keys sorted, like every Barista reply):

```json
{
  "estimateNote": "Costs are estimates at API list prices (checked 2026-11-01), not what a subscription charges.",
  "generatedAt": "2026-11-01T18:00:00Z",
  "limits": [
    {"agent": "claude", "name": "Claude plan", "available": true, "source": "Claude Code",
     "observedAt": "…", "plan": null, "credits": null,
     "windows": [{"id": "five_hour", "label": "5-hour", "windowMinutes": 300, "usedPercent": 23.5,
                  "resetsAt": "…", "reset": false}]},
    {"agent": "codex", "name": "ChatGPT plan", "available": true, "source": "Codex",
     "observedAt": "…", "plan": "prolite", "credits": {"balance": "75000.0000000000", "unlimited": false},
     "windows": [{"id": "primary", "label": "Weekly", "windowMinutes": 10080, "usedPercent": 6, "resetsAt": "…", "reset": false}]},
    {"agent": "cursor", "name": "Cursor", "available": false, "source": null, "observedAt": null,
     "plan": null, "credits": null, "windows": []}
  ],
  "periods": [{
    "id": "today", "label": "Today", "start": "2026-11-01T07:00:00Z",
    "totalTokens": 0, "estimatedCost": 0,
    "agents": [{
      "agent": "claude", "name": "Claude Code",
      "sessions": 0, "requests": 0, "turns": 0, "activeSeconds": 0,
      "inputTokens": 0, "outputTokens": 0, "cacheReadTokens": 0, "cacheWriteTokens": 0,
      "totalTokens": 0, "unpricedTokens": 0, "estimatedCost": 0,
      "models": [{"model": "claude-opus-5-5", "sessions": 0, "requests": 0, "turns": 0,
                  "inputTokens": 0, "outputTokens": 0, "cacheReadTokens": 0, "cacheWriteTokens": 0,
                  "totalTokens": 0, "unpricedTokens": 0, "estimatedCost": 0}]
    }]
  }]
}
```

- `limits` always lists `claude`, `codex`, `cursor` in that order. `available` is false when nothing has
  been observed (then `source`, `observedAt` are null and `windows` empty). Claude `source` is `Claude Code`
  or `Claude app`; window ids are `five_hour`, `seven_day`, `spend_limit` (status line) or `five_hour`,
  `seven_day` (app); `spend_limit` adds `usedUSD`, `limitUSD`, `period` when present. `resetsAt` is ISO or
  null. Codex window ids are `primary`, `secondary`. `credits` is null unless reported.
- `agents` always lists `claude`, `codex`, `cursor` in that order; `models` is sorted by `totalTokens`
  descending, then `turns` descending, then name.
- `cacheWriteTokens` = 5-minute + 1-hour writes. `totalTokens` = input + output + cache read + cache write.
- `estimatedCost` is `null` when the entry has tokens and none of them are priced, and for Cursor;
  otherwise the priced part's cost rounded to 6 decimals. Period `estimatedCost` sums the agents' non-null
  costs. `turns`/`activeSeconds` are 0 for Claude and Codex; token fields are 0 for Cursor.
- Human output shows tokens as `950`, `12.4K`, `3.1M`, `1.2B`, and cost as `~$9.87` (`<$0.01` when
  positive but under a cent, `no price` when nothing is priced).

The MCP tool `barista_usage` (optional `period`, default `week`) returns the same JSON, limits included.

## Cursor hooks

`barista hooks install --agent cursor` (and `barista setup` when `~/.cursor` exists) writes
`~/.cursor/hooks.json` (`{"version": 1, "hooks": {event: [{"command": …, "timeout": 10}]}}`) for
`beforeSubmitPrompt`, `postToolUse`, `afterAgentResponse`, `stop`, and `sessionEnd`. The command is
`'…/barista' hook --agent cursor` on macOS and Linux and `& "C:/…/barista.exe" hook --agent cursor` on
Windows, where Cursor runs hooks through PowerShell.

| Event | Effect | Prints |
| --- | --- | --- |
| `beforeSubmitPrompt` | Record a prompt; start or renew the conversation's lease (may start Barista) | `{"continue":true}` |
| `postToolUse`, `afterAgentResponse` | Renew (same rules as Claude tool events) | `{}` |
| `stop` | Record a stop; release the lease | `{}` |
| `sessionEnd` | Release the lease | `{}` |

Leases are keyed `cursor-<conversation_id>` and titled `Cursor · <first workspace root's folder>`.
Uninstalling removes only Barista's entries and deletes `hooks.json` if nothing but `"version"` remains
(unless it's a symlink).
