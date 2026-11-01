# Barista for Windows and Linux

The Rust build of Barista. It mirrors the Swift app's `BaristaCore` and CLI, so the control protocol, JSON, CLI, MCP server, and agent hooks behave identically on every platform. The Mac app stays the Swift one at the repository root.

```
rust/
  barista/          library (barista_core) + the `barista` CLI
    src/models.rs     wire format (same JSON keys, ISO 8601 dates, sorted keys)
    src/session.rs    independent expiring sessions sharing one power request (AwakeSession.swift)
    src/power.rs      Windows power requests, Linux D-Bus inhibitors, macOS caffeinate (dev only)
    src/control.rs    private Unix socket / per-user named pipe, one JSON request per connection
    src/system.rs     process identity (start time), parents/children, battery
    src/hooks.rs      Claude Code and Codex hook installer and runner (AgentHooks.swift + CLI)
    src/mcp.rs        stdio MCP server
    src/update.rs     GitHub release check (via the system's curl; no TLS stack bundled)
    src/bin/barista.rs
  barista-app/      the tray app (ksni on Linux, tray-icon on Windows; no tray on macOS dev builds), plus `--serve PATH` and `--headless` modes
```

The version comes from `Sources/BaristaCore/Version.swift` at build time (`build.rs`), so a release bumps one number for every platform. When you change behavior in the Swift CLI, hooks, or session logic, make the same change here; `Tests/agent_integration.py` runs against both.

## Build

Rust 1.89 or later; no system libraries. On Linux the tray is a StatusNotifierItem served over D-Bus
with `ksni` (left-click opens the menu), so there's no GTK; on Windows Barista pumps `tray-icon`'s window
messages itself and sets up per-monitor DPI and dark menus, so there's no `tao`.

```sh
cargo build --workspace --release      # target/release/barista and barista-app
```

Ship both binaries side by side: the CLI starts `barista-app` from its own folder when nothing is serving.

## Test

```sh
cargo test --workspace
python3 ../Tests/agent_integration.py --bin-dir target/debug --service target/debug/barista-app   # Linux/macOS
python3 ../Tests/portable_smoke.py --bin-dir target/debug                                          # any OS, incl. Windows
```

`barista-app --serve PATH` runs the control service with fake power and no saved state, for tests. Both suites also pass on macOS, which is handy for development; the macOS build keeps the Mac awake with `caffeinate`.

## Platform notes

**Keeping awake.** Every backend prevents idle sleep only; lids, the Sleep command, and shutdown still work, and the OS drops the request if Barista exits.
- Windows: `PowerCreateRequest` with `PowerRequestSystemRequired` (plus `PowerRequestDisplayRequired` when the display should stay on). Visible in `powercfg /requests`.
- Linux: GNOME's session manager (`org.gnome.SessionManager.Inhibit`: suspend, plus idle when the display should stay on), otherwise `org.freedesktop.PowerManagement.Inhibit` and `org.freedesktop.ScreenSaver.Inhibit` (KDE, Xfce, and others), plus a systemd-logind `idle` inhibitor (visible in `systemd-inhibit --list`). Starting a session fails cleanly if nothing accepts the request.

**Control endpoint.** Linux: `/tmp/com.connorbescos.barista-UID/control.sock` (directory 0700, socket 0600, peer UID checked with `SO_PEERCRED`). Windows: `\\.\pipe\com.connorbescos.barista-<SID>`, with an access list granting only the current user, `PIPE_REJECT_REMOTE_CLIENTS`, a first-instance claim to detect a second copy, and a client-side owner check against name squatting. Stuck reads are cancelled after a few seconds.

**Processes.** Identity is the start time (`/proc/PID/stat` field 22; `GetProcessTimes` on Windows), so a reused PID never inherits a session.

**Battery.** `/sys/class/power_supply` (ignoring peripherals such as mice) and `GetSystemPowerStatus`.

**Settings.** `~/.config/barista/` or `%APPDATA%\Barista\` (`settings.json`, `history.json`). Launch at login: `~/.config/autostart/barista.desktop` or the `HKCU\…\Run` registry key.

**Not yet ported.** The panel's custom minutes, end time, and "while an app runs" (use `barista start --pid`), the global shortcut, in-place signed updates (the menu links to the download), an `.exe` icon resource, and background-command tracking after Claude's Stop on Windows.
