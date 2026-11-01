# Barista agent guide

Barista exposes independent, expiring awake sessions to local agents. Prefer **command wrapping** for a discrete job, or **MCP tools** for a multi-step agent task.

## Quick setup

Not installed yet? With the user’s OK, run:

```sh
curl -fsSL https://cb-117.github.io/barista/install.sh | sh
```

It installs the latest release to Applications (checked against the release’s SHA-256), connects Claude Code and Codex if they’re set up for this user, and starts Barista. Already installed: run `barista setup` (the CLI is `/Applications/Barista.app/Contents/Helpers/barista`). Codex then needs one manual step from the user: type `/hooks` in Codex and trust Barista’s hooks. A summary for agents is at https://cb-117.github.io/barista/llms.txt.

## Claude Code hooks (recommended)

In the app choose **Settings → Agents → Claude Code → Connect**, or run `barista hooks install` (or `barista setup`). Use `hooks status` (reports *Needs repair* if any event is missing or points at another copy) or `hooks uninstall` to check or remove it, and `--settings PATH` to target a different settings file. Barista adds one `barista hook` command to each of these events in `~/.claude/settings.json`. Other hooks and settings are preserved, and the previous file is saved as `settings.json.barista-backup`.

| Event | Effect |
| --- | --- |
| `UserPromptSubmit` | Start a turn: acquire or renew this conversation’s session (20-minute lease, 8-hour cap per turn, launches the app if needed). Only a prompt can start a session. |
| `PreToolUse`, `PostToolUse` | Renew the turn’s lease. If it lapsed during a long tool call it is replaced within the turn’s cap, but only if Barista confirms nothing was stopped since the turn began. Never restarts a session after a user Stop or Stop all (including stopping your own timer), battery protection, an app restart, Claude’s `Stop`, or the cap; never launches the app. |
| `Stop` | Release the session, unless Bash commands Claude started in the background are still running. Then a detached `barista hook-wait` renews a two-minute lease every 30 seconds until they exit, shown as “Waiting on background work”. A new prompt, a user Stop, or the turn’s cap ends the wait. Other child processes, like MCP servers, never count. |
| `SessionEnd` | Release the session. |

Each conversation (`session_id`) gets its own lease, tied to the Claude Code process. If Claude exits or crashes, the session ends. The hook never writes to stdout and always exits 0, so it can’t block or alter Claude. The lease is renewed on tool calls. A single tool call that runs longer than 20 minutes can lapse until the next call (which restores it), so wrap long jobs with `barista run`. Interrupting Claude with Esc skips the `Stop` hook. The lease then expires within 20 minutes, or immediately when Claude exits.

`barista hook` reads the Claude Code hook JSON (`session_id`, `hook_event_name`, `cwd`) from stdin. Any tool that emits the same events can use it.

## Codex hooks (ChatGPT and the Codex CLI)

In the app choose **Settings → Agents → ChatGPT Codex → Connect**, or run `barista hooks install --agent codex`. This writes the same five events to `~/.codex/hooks.json` with the command `barista hook --agent codex` (10-second timeout; 3 seconds for `SessionEnd`, Codex’s maximum).

Codex runs new or changed hooks only after you trust them: type `/hooks` in Codex and trust Barista’s hooks once. Until then Codex skips them silently and doesn’t keep the Mac awake. Barista never trusts its own hooks for you.

Differences from Claude Code:
- Sessions are titled “Codex · <folder>” and never share a lease with a Claude session, even if IDs collide.
- On `Stop`, the hook prints `{}` because Codex requires JSON from Stop hooks; every other event prints nothing.
- Background-command tracking after `Stop` is Claude-only.
- Codex runs `SessionEnd` when a conversation is archived, deleted, Codex quits, or it’s idle for 30 minutes.

Disconnecting removes Barista’s entries, and deletes `hooks.json` if nothing else is left in it.

## MCP setup

In the Mac app, choose **Settings → Copy MCP setup**. Paste the resulting server entry into your agent client’s MCP configuration. The client must support local stdio MCP servers. The command is the bundled native executable, with `mcp` as its argument:

```json
{
  "mcpServers": {
    "barista": {
      "command": "/Applications/Barista.app/Contents/Helpers/barista",
      "args": ["mcp"]
    }
  }
}
```

Use your app’s actual path if it is installed elsewhere. The copied setup uses its current location. The app opens automatically when a tool first needs it. Configuration key placement varies by agent client; use its supported MCP settings. Nothing is installed into or changed in other agent clients automatically.

Tools:

| Tool | Use |
| --- | --- |
| `barista_status` | Inspect awake state, owners, display needs, battery protection, and expiry. |
| `barista_acquire` | Acquire a named session; returns its unique ID. Parameters: `title`, optional `ttl_seconds` (default 300), `max_awake_seconds` (default 28,800), `display_awake` (default false), and local `pid`. |
| `barista_heartbeat` | Renew your ID’s lease; optionally send `progress` (0–1) and a one-line `message` (up to 200 characters). |
| `barista_extend` | Add `seconds` to a live session within its original hard limit. |
| `barista_release` | Release your own ID. Repeated release is safe. |
| `barista_history` | Read the last 20 local completion summaries. |

**Agent behavior:** acquire only for active work that needs the Mac awake. Keep the returned ID. Heartbeat well before expiry (for example every 60 seconds for a 300-second lease). Release on success, failure, cancellation, or when waiting for user input. Do not stop another owner’s session. If your lease disappears because the user stopped it or battery protection ended it, do not automatically reacquire to override that choice. If work legitimately needs a new session, let the user initiate or authorize that work.

MCP uses newline-delimited JSON-RPC 2.0 over stdin/stdout. Supported protocol versions: 2025-11-25, 2025-06-18, 2025-03-26, and 2024-11-05. Tool results include text JSON and `structuredContent`; `isError` indicates tool errors. No diagnostic text is written to protocol stdout.

## Command wrapper

```sh
"/Applications/Barista.app/Contents/Helpers/barista" run \
  --name "Build and test" -- make test
```

The wrapper preserves stdin, stdout, stderr, and the child’s exit code. It renews a 45-second lease every ten seconds, detects runner exit by PID/start-time identity, and releases its session when the command exits. SIGINT, SIGTERM, and SIGHUP are forwarded to the direct child. Descendants that detach may outlive the direct child; wrap the actual long-lived job.

`--display` also keeps the screen on. `--max-awake SECONDS` changes the eight-hour hard cap (maximum 86,400 seconds). Reaching that cap, stopping the session in the UI, or battery protection ends **sleep prevention**, not the running command. Heartbeats never revive a stopped/expired lease.

Arguments after `--` are passed directly to `/usr/bin/env`; they are not interpreted as shell source. For pipelines, explicitly invoke your shell:

```sh
"/Applications/Barista.app/Contents/Helpers/barista" run \
  --name "Pipeline" -- /bin/zsh -c 'make build && make test'
```

## Direct CLI control

```sh
CTL="/Applications/Barista.app/Contents/Helpers/barista"
SESSION_ID="$("$CTL" start --name "Research" --minutes 5)"
"$CTL" heartbeat "$SESSION_ID" --ttl 300 --progress 0.4 --message "Checking sources"
"$CTL" extend "$SESSION_ID" --minutes 15
"$CTL" status --json
"$CTL" stop "$SESSION_ID"
"$CTL" history --json
```

`acquire` aliases `start`; `release` aliases `stop`. `--pid PID` attaches a lease to a local process and its start-time token. `--seconds` is an alternative to `--minutes`. `stop --all` is for an explicit user request to end all awake sessions; prefer releasing your own ID.

`--json` returns a machine-readable object with `ok`, optional `session`, `state`, and `error`. Errors exit nonzero. Human-readable `start` and `heartbeat` print the session ID. `run` leaves stdout for the child, even with `--json`. Global options go after the command: `--json`, `--no-launch`, and `--socket PATH` (for isolated testing). `BARISTA_APP` can point to an app bundle if automatic discovery cannot locate it.

## Ownership, limits, and transport

- All sessions combine their requirements. If any active session needs the display on, it stays on. Stopping one session leaves others alone.
- Agent sessions must expire; TTL and hard caps are greater than zero and at most 24 hours. The hard cap starts at acquisition. Ordinary heartbeats cannot extend it; a heartbeat that sets `maxSeconds` restarts it from now (the Claude Code hook does this only when you send a new prompt). The cap is a safety net for stuck agents, not a security boundary: any local client can also acquire a new lease.
- At most 64 sessions are active simultaneously. Task names fit on one line and are at most 100 characters.
- Battery protection applies to every owner. Low battery while on external power does not block sessions. Reconnecting power permits new sessions; old sessions stay ended.
- The private Unix socket is `/tmp/com.connorbescos.barista-UID/control.sock` (directory mode 700, socket mode 600). Peer user IDs are checked. No TCP port or network service is used. This is user-account isolation, not isolation between mutually untrusted processes running as the same account.
- Sandboxed command tools may need permission to reach the local socket. A client-managed MCP server runs under that client’s applicable permissions. The app cannot override a host’s sandbox policy.
- Disabling **Local agent access** makes the control socket reply “disabled” to every request (it stays bound so clients don’t try to relaunch Barista), ends agent/command awake leases, and revokes any lapsed Claude turn so it needs a new prompt. It does not kill their work. App-owned/manual sessions remain.
- Closing the laptop lid or manually choosing Sleep can still put the Mac to sleep. Barista does not claim to bypass those actions.
