<p align="center"><img src="site/assets/app-icon-1024.png" width="128" alt="Barista icon"></p>

# Barista ☕

**Keeps your Mac awake while the work brews.** A free, open-source Mac menu-bar app for people, commands, and AI agents. [Website](https://cb-117.github.io/barista/)

Keep your Mac awake for a timer, until an app quits, or while a command or agent is actually working. Independent tasks share one power assertion; one task finishing never ends somebody else’s session.

## What’s included

- A coffee-cup menu-bar control with one simple panel: pick 15m, 1h, 4h, or ∞ and press **Keep awake**. **More** has custom minutes, an end time, and “while an app runs.”
- **Right-click** the cup for one-click timers. **Option-click** toggles your last session.
- **One-click Claude Code support:** Settings → Agents → **Connect**. Your Mac stays awake while Claude works and can sleep once it finishes. No MCP setup needed.
- Tasks that keep the Mac awake (agents, commands, watched apps) appear under the main button with live progress and individual stop buttons. Settings shows the last sessions.
- **Task-owned sessions:** commands renew their own short leases and release them on exit. Agents heartbeat during work and release when done. A missing heartbeat or watched process exit ends the session automatically.
- Separate display control. Agent/command sessions allow the display to sleep by default.
- Battery protection: stops awake sessions at 20% while on battery by default, with a configurable cutoff. Your actual work continues; awake sessions do not restart automatically.
- Launch at login, configurable global shortcut, Option-click quick toggle, optional completion notifications, and saved preferences.
- A bundled CLI and stdio MCP server. No third-party runtime, API key, cloud account, simulated input, or network listener.

## Build and run

Requires macOS 13+ and Swift 6 command-line tools. No package dependencies.

```sh
zsh scripts/build-app.sh
open build/Barista.app
```

For a universal Intel + Apple Silicon bundle:

```sh
zsh scripts/build-app.sh --universal
```

Move `build/Barista.app` to Applications for everyday use. The app opens its panel on a normal launch; agent-triggered launches stay in the background. The power icon quits the app. **Stop** ends only your own timer; **Stop all** stops every awake session without terminating work.

Settings opens in its own window from the gear icon, the right-click menu, or ⌘, in the panel. The default shortcut is **Control–Option–Command–C**. Change it in Settings. The first time Barista runs from Applications, it adds itself to macOS Login Items. At login it starts quietly in the menu bar. Turn this off in Settings → **Launch at login**; it stays off. macOS may ask you to approve it in System Settings → General → Login Items. Notifications request permission only when you enable them.

## Claude Code

Choose **Settings → Agents → Connect**, or run:

```sh
build/barista hooks install     # status | uninstall
```

This adds Barista hooks to `~/.claude/settings.json` and leaves your other settings and hooks in place. A backup is saved next to the file as `settings.json.barista-backup`. Each conversation gets its own session: a prompt starts it, tool calls renew it, and it ends when Claude finishes its turn or exits. If you stop it in Barista, it stays stopped until your next prompt. If you move the app, choose **Reconnect**.

## Use from a terminal or agent

```sh
# Automatically keep awake for exactly the lifetime of a command.
build/barista run --name "Build and test" -- make test

# Acquire an independent session; prints its ID.
build/barista start --name "Research agent" --minutes 5

# Renew and publish progress. Replace SESSION_ID with the returned ID.
build/barista heartbeat SESSION_ID --ttl 300 --progress 0.5 --message "Checking sources"
build/barista stop SESSION_ID
build/barista status --json
```

See [the agent guide](docs/AGENTS.md) for MCP setup, all commands, ownership rules, JSON responses, hard limits, and sandbox considerations. Settings also has **Copy CLI path** and **Copy MCP setup**.

## Limits and privacy

This prevents **idle sleep**. It does not override closing a laptop lid, choosing Sleep, shutdown, or a depleted battery. Timers use wall-clock deadlines, so manual sleep does not pause them. Watched-app sessions monitor process lifetime, not whether an app is doing useful work. Use the command runner or agent heartbeats for precise job lifetimes.

Agent leases default to five minutes and an eight-hour hard cap. Commands renew a 45-second lease every ten seconds, tied to the runner’s PID and process start time. TTL and hard limits cannot exceed 24 hours. An explicit Stop or battery cutoff cannot be reversed by a heartbeat. Never reacquire automatically to override a user’s Stop.

Sessions are visible and controllable in the app. The local control socket is restricted to your user account; no TCP port is opened. History stores only the last 20 task names, start/end times, and completion reasons in local preferences. Command arguments/output are not recorded. Active sessions are never restored after an app restart. You can clear history or disable agent access in Settings.

The build is ad-hoc signed for local use, and release builds aren’t notarized yet. After the first launch, allow the app in System Settings → Privacy & Security → **Open Anyway**. Intel binaries are cross-compiled; testing on an Intel Mac is recommended before a public release.

## Verify

```sh
swift build
swift run BaristaChecks --power
python3 Tests/agent_integration.py
```

`BaristaChecks` covers session expiration, independent owners, display aggregation, heartbeat expiry/hard caps, process reuse, battery protection, custom input, history, and concurrent real Unix-socket requests. `--power` briefly creates real macOS sleep-prevention assertions and confirms cleanup. The Python suite uses an isolated service with fake power assertions to exercise the CLI, command runner, JSON output, MCP handshake/tools/errors, user stops, and crash recovery.

Run local-socket integration checks outside a network-restricted command sandbox; a sandbox can prohibit even private Unix sockets. These checks need no administrator privileges. To test a different binary directory, use `python3 Tests/agent_integration.py --bin-dir PATH`.

## License

MIT. See [LICENSE](LICENSE).
