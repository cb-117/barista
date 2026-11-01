# Changelog

What changed in each version of Barista. Every release needs an entry here: CI fails and the
Release workflow won't publish until the version in `Sources/BaristaCore/Version.swift` has a
`## [x.y.z] - YYYY-MM-DD` section. That section becomes the GitHub release notes.

Add changes under **Unreleased** as you go. When you bump the version, rename that heading to the
new version and date, and start a fresh **Unreleased** section above it.

## [Unreleased]

## [0.0.6] - 2026-11-01

### Changed
- The built-in agent guide starts with one-step setup (`install.sh` or `barista setup`) for Claude Code and Codex.
- Lower idle energy use: when nothing is keeping the Mac awake, Barista no longer redraws the menu bar and panel every second, and its timer lets macOS batch wakeups.

### Fixed
- `barista run` works with interactive commands. Commands that prompt (`sudo`, `ssh`), open a pager, or read the terminal used to freeze while keeping the Mac awake; they now run in the foreground as usual, and Ctrl-C and Ctrl-Z reach them directly. Barista keeps the Mac awake until the command actually exits, even if it catches Ctrl-C and keeps going.
- `barista run` handles signals the way shells expect: a script running it stops when you press Ctrl-C, signals ignored with `nohup` stay ignored, `kill` of the wrapper reaches the command once, and a command started with a closed input still runs.
- After Claude resumes a turn that was waiting on background commands, its session keeps its full 20-minute renewal instead of dropping back to two minutes.
- Claude Code hooks no longer miss tool calls with very large inputs or outputs (over 1 MB).
- Long-running helpers that Claude started, such as MCP servers, can no longer be mistaken for background commands, which could keep the Mac awake after Claude finished.
- Connecting or disconnecting an agent no longer overwrites a settings change the agent saved at the same moment.
- Disconnecting Codex no longer deletes a symlinked `hooks.json` (for example, one kept in a dotfiles repo); it’s emptied instead.
- If macOS refuses to keep the Mac awake, the sessions that ended now appear in history and send notifications like any other ending.
- Updates: if swapping in the new version fails and restoring the old one fails too, the old copy is kept instead of deleted. Downloads are size-checked before they’re loaded into memory.
- The control socket stays responsive when a misbehaving local client opens many connections or sends slowly.
- The installer script runs nothing if its download is cut off partway.

## [0.0.5] - 2026-11-01

### Added
- **ChatGPT Codex support.** Settings → Agents → ChatGPT Codex → **Connect** keeps your Mac awake while Codex works, in the ChatGPT app or the Codex CLI. Codex asks you to approve new hooks once: type `/hooks` in Codex and trust Barista’s. Also available as `barista hooks install --agent codex`.
- **Agents can set Barista up for you.** A one-line installer (`curl -fsSL https://cb-117.github.io/barista/install.sh | sh`) installs the latest release, checks its checksum, connects Claude Code and Codex, and starts Barista, with no “Open Anyway” step. The website’s `llms.txt` tells AI agents how and when to use it.
- `barista setup` connects every agent it finds and starts Barista.
- If Claude Code or Codex is installed but not connected, the panel offers to **Connect** it in one click.
- A **Permissions** section in Settings with one-click buttons to the right pages in System Settings: Login Items, Notifications, and Privacy & Security. Each shows its current status, and it refreshes when you come back.
- Messages that ask you to approve something in System Settings now have an **Open System Settings** button.

### Fixed
- Updating no longer drops what was keeping your Mac awake. Timers, commands, and Claude or Codex sessions running when you click **Update** carry over to the new version.

## [0.0.4] - 2026-11-01

### Added
- A **What’s new** link in Settings → Updates that opens this changelog.
- Settings shows where the running copy of Barista lives (for example `/Applications/Barista.app`), so you can confirm which one is open.

## [0.0.3] - 2026-11-01

### Added
- **One-click updates.** When a new version is out, click **Update** in the panel. Barista downloads it, checks its signature, replaces itself, and relaunches. Updates installed this way don’t trigger the “Apple could not verify” warning.
- **Drag-to-Applications installer.** New installs come as `Barista.dmg`: open it and drag Barista into Applications. The window also explains the one-time **Open Anyway** step.
- Releases include `SHA256SUMS.txt`, and every update archive is signed. Barista refuses an update that isn’t signed with the release key.

### Changed
- The website’s **Download for Mac** button downloads the installer directly.

## [0.0.2] - 2026-11-01

### Fixed
- When Claude Code finishes a reply but commands it started in the background are still running (shown as “Waiting” in Claude Code and T3), Barista now keeps the Mac awake until they finish. The task shows “Waiting on background work”. MCP servers and other helper processes never count.

### Changed
- Claude Code tasks show what they’re doing in the panel: “Working” or “Waiting on background work”.

## [0.0.1] - 2026-11-01

First release.

### Added
- A coffee-cup menu-bar app that keeps your Mac awake: 15 minutes, 1 hour, 4 hours, until stopped, custom minutes, until a time, or while an app is open.
- Keep the display on or let it sleep. Add 15 minutes to a running timer.
- Every timer, command, and agent gets its own session; stopping one never ends another.
- One-click Claude Code support (Settings → Agents → Connect): your Mac stays awake while Claude works.
- `barista` command-line tool, including `barista run -- COMMAND`, and a built-in MCP server.
- Battery protection: stops keeping the Mac awake at 20% on battery (adjustable).
- The menu bar shows your timer’s time left, or how many tasks are running. Right-click the cup for quick timers and a list of running tasks.
- Launch at login, a global shortcut (⌃⌥⌘C), optional notifications, and recent history.
- A daily update check that shows a Download link when a new version is out.
