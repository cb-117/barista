# Changelog

What changed in each version of Barista. Every release needs an entry here: CI fails and the
Release workflow won't publish until the version in `Sources/BaristaCore/Version.swift` has a
`## [x.y.z] - YYYY-MM-DD` section. That section becomes the GitHub release notes.

Add changes under **Unreleased** as you go. When you bump the version, rename that heading to the
new version and date, and start a fresh **Unreleased** section above it.

## [Unreleased]

## [0.1.0] - 2026-11-06

### Changed
- In one-click mode, the menu-bar cup fills with coffee when you click it on and drains when you click it off, instead of switching straight between empty and full. If **Reduce motion** is on in macOS, the cup switches without the animation.

## [0.0.16] - 2026-11-05

### Added
- **One-click mode** (Mac). Turn it on in Settings → General, and a click on the menu-bar cup keeps your Mac awake until you click it again. The cup fills while it's on and empties when it's off. Option-click opens the panel, and right-click still shows quick timers.

### Changed
- The Mac disk image no longer explains the **Open Anyway** step, since Barista is notarized now. It says where to find Barista after you drag it into Applications instead.

## [0.0.15] - 2026-11-01

### Added
- **Support Barista.** Barista is free and open source. If you'd like to help, **Star on GitHub** helps other people find it, and **Buy Me a Coffee** ([buymeacoffee.com/baristatime](https://buymeacoffee.com/baristatime)) keeps it brewing. Both are in Settings → Support Barista and the right-click menu on a Mac, the panel's Settings and the tray menu on Windows, and the tray menu on Linux.

### Changed
- **The Mac app is signed and notarized by Apple.** Opening a new download no longer stops at "macOS can't verify Barista", and there's no **Open Anyway** step. Updating from 0.0.14 works as usual; macOS may ask you once to allow Barista as a login item again, since it now has a verified developer.

### Fixed
- The weekly summary never arrived unless you'd turned on another notification. It's on by default, but Barista didn't ask for notification permission. It now delivers quietly to Notification Center, without a prompt. Turning on **Notify when a session ends** still asks for alerts.
- Moving to another time zone could send the same weekly summary twice.
- If Barista couldn't reopen after an update, it deleted the previous version too. It now keeps it.

## [0.0.14] - 2026-11-01

### Added
- **Schedule** (Mac). Settings → Schedule keeps your Mac awake during hours you choose, such as Monday to Friday, 8:00 AM to 5:00 PM, by your Mac's clock and time zone. Add as many stretches as you like; hours that end at or before they start run past midnight. While scheduled hours are on, **Scheduled hours** appears in the panel with when they end. Stop it there to pause until that stretch ends, or **Resume** from the panel or right-click menu. Battery protection still applies, and the schedule starts again once you're on power.
- **Projects and History, inside Barista.** On a Mac, Usage & Limits is now a resizable window with **Overview**, **Projects**, and **History** tabs (⌘1–⌘3), a period menu (now including **All time**), **Export**, and **Share** in its toolbar. On Windows, the panel's **Projects & history** button, its new 7-day chart, and the tray menu open a usage window. The numbers update while it's open, instead of a snapshot in your browser. Linux, and `barista report`, still open the report in your browser.
- **Select a day** in History's chart to see only that day's time, tokens, and cost per project.
- **Right-click** a number, project name, or folder to copy it, and click the folder icon to show a project in Finder or File Explorer.
- **Export** as CSV (one row per project), a web page, or a PDF; on a Mac also an image, or **Share** the report. In a browser, the report has **Download CSV**.
- **Last 7 days**: a small chart of your active time in the menu-bar popover (Mac) and the panel (Windows). Click it for History.
- **Weekly summary**: on Monday, a notification with last week's active time, projects, and estimated cost. Click it for History. Turn it off in Settings (**Weekly usage summary**).

### Changed
- Windows: notifications now come from **Barista**, with its cup, instead of "Windows PowerShell", and the weekly summary has an **Open history** button.
- Windows: Apps & features (Control Panel) shows Barista's cup and **Barista** as the publisher, and `barista-app.exe` has the cup icon and version details (Start menu, taskbar, Task Manager).

## [0.0.13] - 2026-11-01

### Fixed
- Windows: the panel opened blank. Barista ignored the panel’s messages, including its request for the current state, because a page loaded from memory reports its address as `about:blank`. The panel now fills in and sizes itself to fit.

## [0.0.12] - 2026-11-01

### Added
- **Projects.** Usage & Limits, `barista usage --period …`, and the `barista_usage` MCP tool now show active time, sessions, tokens, models, and estimated cost for each project. A project is the Git repository an agent worked in (a worktree counts with its main checkout; a folder outside any repository is its own project). Active time counts stretches with an agent working, or you replying, with no gap over 15 minutes, across Claude Code, Codex, and Cursor; two agents working on a project at once count once.
- **Usage report.** **Open report** in Usage & Limits (on Windows, in the panel’s Usage view), **Usage Report…** in the Mac’s right-click menu, **Open usage report…** in the Windows and Linux tray menu, or `barista report` opens a visual report in your browser: totals, time (or tokens, or cost) per project, active time per day stacked by project, and each project's agents and models, for today, this week, this month, or all time, in light or dark mode. It's one HTML file in Barista's data folder (`--output` to save it elsewhere); nothing is uploaded.
- Cursor usage records the workspace each conversation belongs to, so Cursor turns count toward their project from now on. Earlier Cursor turns show under Unknown.

### Changed
- Barista reads your agent logs once more from the start after updating, to find each session's project. This takes a few seconds the first time Usage & Limits opens.

### Fixed
- macOS no longer keeps asking to let Barista use “Claude Code-credentials” in your Keychain. Barista now reads Claude Code’s login with Apple’s `security` tool, as Claude Code does, and the Keychain already allows that. Before, every Barista update had a new signature, so **Always Allow** didn’t last.

## [0.0.11] - 2026-11-01

### Added
- **Windows: a real Barista panel.** Click the cup in the notification area for a panel like the Mac app’s: your timer with 15m, 1h, 4h, ∞, or custom minutes, Keep awake and Stop, the tasks keeping your PC awake with their own stop buttons, a Usage & Limits view, and Settings. It follows Windows’ light or dark mode, sizes itself to fit, and closes when you click elsewhere or press Esc. Right-click the cup for a short menu. It uses WebView2, built into Windows 11 and current Windows 10; without it, the menu works as before.
- **Windows: an installer.** `Barista-Setup.exe` installs for just you (no admin needed) or for everyone on the PC (Program Files), adds Barista to the Start menu and your PATH, starts it when you sign in, connects Claude Code, Codex, and Cursor, and starts Barista. Uninstall from Settings → Apps; it disconnects Barista from your coding agents first. Upgrades keep your startup and agent choices. For an all-users install, each person’s Barista sets up its own startup the first time they open it, and uninstalling disconnects every user’s coding agents. The PowerShell one-liner now runs this installer, and Barista’s update notice on Windows downloads it. The installer isn’t code-signed yet, so Windows SmartScreen may ask you to confirm (More info → Run anyway).

### Fixed
- Windows: “Usage & Limits” showed as “Usage  Limits” in the tray menu.
- The Usage & Limits menu shows one line when there’s no agent activity on the computer, instead of rows of dashes.

## [0.0.10] - 2026-11-01

### Security
- Linux: Barista no longer uses GTK. The tray icon talks to the desktop directly over D-Bus (StatusNotifierItem), which removes the `glib` dependency flagged in [GHSA-wrw7-89jp-8q8g](https://github.com/advisories/GHSA-wrw7-89jp-8q8g) along with every other GTK library. The Linux app also no longer needs GTK or AppIndicator libraries installed.
- Linux: the time left now shows in the tray icon’s tooltip instead of beside the icon (that needed GTK), so the “Show time left next to the cup” setting is gone on Linux. Trays that only support XEmbed (i3bar, dwm, tint2) need `snixembed` to show the cup.
- Windows: the tray app sets up high-DPI scaling and dark-mode menus itself.

## [0.0.9] - 2026-11-01

### Changed
- The website moved to [baristatime.app](https://baristatime.app). Installers are now `curl -fsSL https://baristatime.app/install.sh | sh` and `irm https://baristatime.app/install.ps1 | iex`; the old `cb-117.github.io/barista` addresses keep working.

### Fixed
- Claude plan limits now match the Claude app. Barista used an old snapshot from the Claude app (it could show last week’s weekly usage), and Claude Code’s status line only reports limits in its terminal view. The Barista app now asks your Claude account for your session, weekly, and per-model limits and extra usage every few minutes, using Claude Code’s saved login read-only. On a Mac, click **Allow access** in Usage & Limits once and choose **Always Allow**; Barista never shows the Keychain prompt on its own, and may ask again after an update. Turn this off in **Settings → Agents**. Snapshots from the Claude app older than an hour are no longer used.
- Sessions Barista keeps for the Claude app and the ChatGPT app say “While it’s busy” instead of “Until the app quits”.

## [0.0.8] - 2026-11-01

### Added
- **Usage & Limits**: one place for every agent. It shows your Claude plan’s 5-hour and weekly limits (shared by Claude Code, Cowork, and Claude chat), your ChatGPT plan’s limits, plan, and credits (shared by Codex and the ChatGPT app), and sessions and tokens per agent and model for today, this week, and this month, with an estimated cost at API list prices (not what a subscription charges). Open it from the panel, the right-click menu, or Settings; on Windows and Linux it’s the tray’s **Usage & Limits** menu. Also available as `barista usage` and the `barista_usage` MCP tool. Everything is read from files on your computer; nothing is uploaded.
- Claude Code’s status line shows your plan limits (for example “Opus · 5h 24%”), which is how Barista reads them. `barista setup` and connecting Claude Code add it, and the Usage & Limits window offers it if you connected Claude Code before this version. If you already have a status line, Barista runs it unchanged and restores it when you disconnect. Manage it with `barista statusline install|uninstall|status`.
- Keeps your computer awake while Claude’s Cowork or the ChatGPT app is working, on Mac and Windows. Neither app has hooks, so Barista watches how busy each app is (including Cowork’s VM) and lets the computer sleep once it has been quiet for 5 minutes. Turn each off in **Settings → Agents**.
- Cursor support: **Settings → Agents → Cursor → Connect** (or `barista hooks install --agent cursor`) keeps your computer awake while Cursor’s agent works and records its turns and active time. Cursor doesn’t store token counts on your computer, so usage shows Cursor turns rather than tokens.

### Changed
- Codex’s hook review lists what each Barista hook does (for example “Barista: keep this computer awake while Codex works”) instead of “Hook 1” to “Hook 5”. Hooks from earlier versions keep working; reconnecting or running `barista setup` adds the labels, and Codex then asks you to review them once.

## [0.0.7] - 2026-11-01

### Fixed
- Barista 0.0.6 quit immediately on launch (a crash while reading notification permissions). If 0.0.6 won't open for you, it can't update itself: download Barista again from the release page, or delete Barista from Applications and run the install command again.

### Added
- Barista for Windows and Linux, built in Rust: a coffee cup in the notification area or system tray with quick timers, your tasks, Stop all, battery protection, launch at login, notifications, and update checks, plus the same `barista` CLI, `barista run`, MCP server, and Claude Code and Codex hooks as the Mac app. Install with `irm https://cb-117.github.io/barista/install.ps1 | iex` on Windows, or the usual `install.sh` on Linux. Releases include Windows x86_64 and Linux x86_64/arm64 downloads.

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
