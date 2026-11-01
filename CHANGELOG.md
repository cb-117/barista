# Changelog

What changed in each version of Barista. Every release needs an entry here: CI fails and the
Release workflow won't publish until the version in `Sources/BaristaCore/Version.swift` has a
`## [x.y.z] - YYYY-MM-DD` section. That section becomes the GitHub release notes.

Add changes under **Unreleased** as you go. When you bump the version, rename that heading to the
new version and date, and start a fresh **Unreleased** section above it.

## [Unreleased]

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
