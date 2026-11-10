//! What the tray menu shows and what its commands do, independent of how each platform draws it
//! (muda on Windows, ksni over D-Bus on Linux).
use crate::{check_for_updates, cli_path, lock, open_usage_report, platform, refresh_usage, AgentStatus, Shared};
use barista_core::activity::WatchedApp;
use barista_core::hooks::HookAgent;
use barista_core::*;
use std::time::Duration;

/// Where people can support Barista: a GitHub star, or a coffee.
const REPO_URL: &str = "https://github.com/cb-117/barista";
const COFFEE_URL: &str = "https://buymeacoffee.com/baristatime";

/// One menu entry. Ids are the commands `handle` understands; labels have none.
#[derive(Clone, Debug, PartialEq)]
pub enum Entry {
    Item { id: String, text: String, enabled: bool },
    Check { id: String, text: String, checked: bool },
    Label(String),
    Separator,
    Submenu { text: String, enabled: bool, children: Vec<Entry> },
}

/// Everything the tray shows, plus a signature so unchanged menus aren't redrawn.
pub struct Model {
    pub entries: Vec<Entry>,
    pub signature: String,
    pub tooltip: String,
    /// The countdown for your own timer, empty when there's nothing to count down.
    pub title: String,
    pub active: bool,
}

/// The app icon as RGBA pixels: full color while awake, faded while resting.
pub struct Icons {
    pub width: u32,
    pub height: u32,
    pub active: Vec<u8>,
    pub resting: Vec<u8>,
}

pub fn icons() -> Icons {
    let decoder = png::Decoder::new(std::io::Cursor::new(&include_bytes!("../assets/icon-64.png")[..]));
    let mut reader = decoder.read_info().expect("bundled icon is a valid PNG");
    let mut rgba = vec![0; reader.output_buffer_size().expect("icon size")];
    let info = reader.next_frame(&mut rgba).expect("bundled icon decodes");
    rgba.truncate(info.buffer_size());
    let mut faded = rgba.clone();
    for pixel in faded.as_chunks_mut::<4>().0 {
        let gray = ((u32::from(pixel[0]) * 30 + u32::from(pixel[1]) * 59 + u32::from(pixel[2]) * 11) / 100) as u8;
        pixel[0] = gray;
        pixel[1] = gray;
        pixel[2] = gray;
        pixel[3] = (u32::from(pixel[3]) * 3 / 5) as u8;
    }
    Icons {
        width: info.width,
        height: info.height,
        active: rgba,
        resting: faded,
    }
}

fn item(id: &str, text: &str) -> Entry {
    Entry::Item {
        id: id.into(),
        text: text.into(),
        enabled: true,
    }
}

fn label(text: &str) -> Entry {
    Entry::Label(text.into())
}

fn check(id: &str, text: &str, checked: bool) -> Entry {
    Entry::Check {
        id: id.into(),
        text: text.into(),
        checked,
    }
}

/// Builds the menu and a signature of everything it shows, so unchanged menus aren't rebuilt.
pub fn build(app: &Shared) -> Model {
    let mut state = lock(app);
    state.session.refresh();
    let state = &*state;
    let session = &state.session;
    let mut menu: Vec<Entry> = Vec::new();
    let mut signature = Vec::<String>::new();
    let computer = if cfg!(windows) { "PC" } else { "computer" };

    if let Some(notice) = &state.notice {
        menu.push(label(notice));
        menu.push(item("dismiss-notice", "Dismiss"));
        menu.push(Entry::Separator);
        signature.push(notice.clone());
    }
    if let Some(error) = &session.error {
        menu.push(label(error));
        menu.push(Entry::Separator);
        signature.push(error.clone());
    }
    if let Some(update) = state.visible_update() {
        menu.push(item("open-update", &format!("Download Barista {}…", update.version)));
        menu.push(item("dismiss-update", "Not now"));
        menu.push(Entry::Separator);
        signature.push(update.version.clone());
    }

    let headline = if session.is_active() {
        let count = session.sessions().len();
        format!(
            "Keeping your {computer} awake · {}",
            if count == 1 {
                "1 session".into()
            } else {
                format!("{count} sessions")
            }
        )
    } else {
        format!("Resting · your {computer} can sleep")
    };
    menu.push(label(&headline));
    signature.push(headline);

    let tasks: Vec<&CoffeeLease> = session.sessions().iter().filter(|s| s.source != SessionSource::Manual).collect();
    for lease in &tasks {
        let mut text = lease.title.clone();
        if let Some(progress) = lease.progress {
            text.push_str(&format!(" · {}%", (progress * 100.0).round()));
        }
        if let Some(message) = &lease.message {
            text.push_str(&format!(" · {message}"));
        }
        let mut submenu = Vec::new();
        submenu.push(item(&format!("cancel:{}", lease.id), "Stop keeping awake"));
        menu.push(Entry::Submenu {
            text: text.clone(),
            enabled: true,
            children: submenu,
        });
        signature.push(format!("{}{text}", lease.id));
    }

    menu.push(Entry::Separator);
    if let Some(manual) = session.manual_session() {
        let remaining = if manual.expires_at.is_some() {
            format!("{} left", session.countdown_label())
        } else {
            "until stopped".into()
        };
        menu.push(label(&format!("Your timer · {remaining}")));
        if manual.expires_at.is_some() {
            menu.push(item("add:900", "Add 15 minutes"));
        }
        menu.push(item("stop-manual", "Stop"));
        signature.push(format!("manual {remaining}"));
    } else {
        menu.push(label("Keep awake for"));
        let enabled = !session.battery_blocked();
        for (id, text) in [
            ("start:900", "15 minutes"),
            ("start:3600", "1 hour"),
            ("start:14400", "4 hours"),
            ("start:forever", "Until stopped"),
        ] {
            menu.push(Entry::Item {
                id: id.into(),
                text: text.into(),
                enabled,
            });
        }
        if !enabled {
            menu.push(label(&format!("Battery protection is on below {}%", session.battery_threshold())));
        }
        signature.push(format!("idle {enabled}"));
    }
    if session.sessions().len() > 1 || (session.is_active() && session.manual_session().is_none()) {
        menu.push(item("stop-all", "Stop all"));
        signature.push("stop-all".into());
    }

    menu.push(Entry::Separator);
    let mut settings = Vec::new();
    let prefs = &state.prefs;
    settings.push(check(
        "toggle:display",
        "Keep the display on during my timers",
        prefs.keep_display_awake,
    ));
    let mut battery = Vec::new();
    for value in [0, 10, 15, 20, 30] {
        let text = if value == 0 {
            "Off".to_string()
        } else {
            format!("Stop at {value}% on battery")
        };
        battery.push(check(&format!("battery:{value}"), &text, prefs.battery_threshold == value));
    }
    settings.push(Entry::Submenu {
        text: "Battery protection".into(),
        enabled: true,
        children: battery,
    });
    settings.push(check("toggle:login", "Launch at login", state.launch_at_login));
    settings.push(check(
        "toggle:notifications",
        "Notify me when sessions end",
        prefs.notifications_enabled,
    ));
    settings.push(check(
        "toggle:weekly-summary",
        "Weekly usage summary on Mondays",
        prefs.weekly_summary,
    ));
    settings.push(check("toggle:limit-alerts", "Limit alerts", prefs.limit_alerts));
    settings.push(check("toggle:updates", "Check for updates daily", prefs.checks_for_updates));
    settings.push(item(
        "check-updates",
        state.update_status.as_deref().unwrap_or("Check for updates now"),
    ));
    settings.push(check("toggle:active-count", "Help count active users", prefs.share_active_count));
    settings.push(Entry::Separator);
    settings.push(check("toggle:agents", "Local agent access (CLI, MCP, hooks)", prefs.agent_access));
    settings.push(check(
        "toggle:claude-account",
        "Show Claude plan limits from your Claude account",
        prefs.claude_account_limits,
    ));
    settings.push(check(
        "toggle:codex-account",
        "Show ChatGPT plan limits from your ChatGPT account",
        prefs.codex_account_limits,
    ));
    // The Claude and ChatGPT desktop apps have no hooks: Barista watches how busy they are.
    // Neither exists for Linux.
    if cfg!(windows) {
        settings.push(check(
            "toggle:watch-cowork",
            "Keep awake while the Claude app works (Cowork)",
            prefs.watch_cowork,
        ));
        settings.push(check(
            "toggle:watch-chatgpt",
            "Keep awake while the ChatGPT app works",
            prefs.watch_chatgpt,
        ));
    }
    for (agent, status) in &state.agents {
        let name = match agent {
            HookAgent::Claude => "Claude Code",
            HookAgent::Codex => "ChatGPT Codex",
            HookAgent::Cursor => "Cursor",
        };
        let (text, id) = match status {
            AgentStatus::Connected => (format!("{name}: connected · Disconnect"), "disconnect"),
            AgentStatus::Moved => (format!("{name}: needs repair · Reconnect"), "connect"),
            AgentStatus::Disconnected => (format!("{name}: Connect"), "connect"),
            AgentStatus::NotFound => (format!("{name}: not set up for this user"), ""),
        };
        let key = agent.name();
        settings.push(Entry::Item {
            id: format!("{id}:{key}"),
            text: text.clone(),
            enabled: !id.is_empty(),
        });
        signature.push(text);
    }
    settings.push(item("copy:cli", "Copy CLI path"));
    settings.push(item("copy:mcp", "Copy MCP setup"));
    menu.push(Entry::Submenu {
        text: "Settings".into(),
        enabled: true,
        children: settings,
    });
    signature.push(format!("{prefs:?}{}{:?}", state.launch_at_login, state.update_status));

    let (usage, usage_signature) = usage_menu(state.usage.as_ref(), state.usage_loading);
    signature.push(usage_signature);
    menu.push(Entry::Submenu {
        text: "Usage & Limits".into(),
        enabled: true,
        children: usage,
    });
    menu.push(item(
        "usage-report",
        if cfg!(windows) {
            "Projects & history…"
        } else {
            "Open usage report…"
        },
    ));

    let mut history = Vec::new();
    for event in session.history().iter().take(10) {
        history.push(label(&format!("{} · {}", event.title, event.reason.label())));
    }
    if !session.history().is_empty() {
        history.push(Entry::Separator);
        history.push(item("clear-history", "Clear history"));
    }
    menu.push(Entry::Submenu {
        text: "Recent sessions".into(),
        enabled: !session.history().is_empty(),
        children: history,
    });
    signature.push(session.history().first().map(|h| h.id.clone()).unwrap_or_default());

    menu.push(Entry::Separator);
    menu.push(Entry::Submenu {
        text: "Support Barista".into(),
        enabled: true,
        children: vec![item("support:github", "Star on GitHub"), item("support:coffee", "Buy Me a Coffee")],
    });
    menu.push(label(&format!("Barista {VERSION}")));
    menu.push(item("quit", "Quit Barista"));

    let active = session.is_active();
    let tooltip = if active {
        session.sessions().iter().map(|s| s.title.as_str()).collect::<Vec<_>>().join(" · ")
    } else {
        "Barista · resting".to_string()
    };
    // Linux has no setting for this (no panel can draw it beside the icon), so it always shows.
    let title = if active && (prefs.show_countdown || cfg!(target_os = "linux")) {
        session.countdown_label()
    } else {
        String::new()
    };
    signature.push(tooltip.clone());
    signature.push(title.clone());
    Model {
        entries: menu,
        signature: signature.join("\u{1f}"),
        tooltip,
        title,
        active,
    }
}

/// Performs a menu command. Returns true to quit.
pub fn handle(app: &Shared, id: &str) -> bool {
    let (command, argument) = id.split_once(':').unwrap_or((id, ""));
    let mut state = lock(app);
    match command {
        "quit" => return true,
        "dismiss-notice" => state.notice = None,
        "start" => match argument.parse::<f64>() {
            // Presets and custom minutes from the panel: 1 minute to 24 hours.
            Ok(seconds) if (60.0..=86400.0).contains(&seconds) => state.start_manual(Some(seconds)),
            Ok(_) => state.notice = Some("Choose from 1 to 1,440 minutes.".into()),
            Err(_) => state.start_manual(None),
        },
        "add" => state.session.add_time(argument.parse().unwrap_or(900.0)),
        "stop-manual" => state.session.stop_manual(),
        "cancel" => state.session.cancel(argument),
        "stop-all" => state.session.stop(EndReason::Stopped),
        "toggle" => match argument {
            "display" => {
                let value = !state.prefs.keep_display_awake;
                state.set_keep_display_awake(value)
            }
            "countdown" => {
                let value = !state.prefs.show_countdown;
                state.set_show_countdown(value)
            }
            "login" => {
                let value = !state.launch_at_login;
                state.set_launch_at_login(value)
            }
            "notifications" => {
                let value = !state.prefs.notifications_enabled;
                state.set_notifications(value)
            }
            "weekly-summary" => {
                state.prefs.weekly_summary = !state.prefs.weekly_summary;
                state.save();
            }
            "limit-alerts" => {
                state.prefs.limit_alerts = !state.prefs.limit_alerts;
                state.save();
            }
            "updates" => {
                let value = !state.prefs.checks_for_updates;
                state.set_checks_for_updates(value)
            }
            "active-count" => {
                state.prefs.share_active_count = !state.prefs.share_active_count;
                state.save();
            }
            "agents" => {
                let value = !state.prefs.agent_access;
                state.set_agent_access(value)
            }
            "claude-account" => {
                let value = !state.prefs.claude_account_limits;
                drop(state);
                crate::set_account_limits(app, crate::LimitsAccount::Claude, value);
                return false;
            }
            "codex-account" => {
                let value = !state.prefs.codex_account_limits;
                drop(state);
                crate::set_account_limits(app, crate::LimitsAccount::ChatGpt, value);
                return false;
            }
            "watch-cowork" => {
                let value = !state.prefs.watch_cowork;
                state.set_watches(WatchedApp::COWORK, value)
            }
            "watch-chatgpt" => {
                let value = !state.prefs.watch_chatgpt;
                state.set_watches(WatchedApp::CHATGPT, value)
            }
            _ => {}
        },
        "battery" => state.set_battery_threshold(argument.parse().unwrap_or(20)),
        "statusline-install" => {
            let cli = cli_path().to_string_lossy().into_owned();
            state.notice = Some(match barista_core::hooks::statusline_install(&cli, None) {
                Ok(()) => "Added to Claude Code’s status line. Your Claude limits appear after Claude Code’s next reply.".into(),
                Err(error) => format!("Couldn’t update Claude Code settings: {error}"),
            });
        }
        "connect" | "disconnect" => {
            if let Some(agent) = HookAgent::parse(argument) {
                state.set_connected(agent, command == "connect");
            }
        }
        "copy" => {
            let cli = cli_path().to_string_lossy().into_owned();
            let (text, message) = if argument == "mcp" {
                let value = serde_json::json!({"mcpServers": {"barista": {"command": cli, "args": ["mcp"]}}});
                (
                    serde_json::to_string_pretty(&value).unwrap_or_default(),
                    "MCP setup copied. Paste it into your agent’s MCP settings.",
                )
            } else {
                (cli, "CLI path copied.")
            };
            state.notice = Some(match copy(&text) {
                Ok(()) => message.to_string(),
                Err(error) => format!("Couldn’t copy: {error}. {text}"),
            });
        }
        "check-updates" => {
            drop(state);
            check_for_updates(app, true);
            return false;
        }
        "open-update" => {
            if let Some(update) = &state.update {
                platform::open_url(update.download_url.as_deref().unwrap_or(&update.page_url));
            }
        }
        "dismiss-update" => state.dismiss_update(),
        "clear-history" => state.session.clear_history(),
        "support" => match argument {
            "github" => platform::open_url(REPO_URL),
            "coffee" => platform::open_url(COFFEE_URL),
            _ => {}
        },
        "usage-refresh" => {
            drop(state);
            refresh_usage(app, Duration::ZERO);
            return false;
        }
        "usage-report" => {
            // `usage-report:month` opens the report on that period; the tray's item opens this week.
            drop(state);
            let period = barista_core::usage::Period::parse(argument).unwrap_or(barista_core::usage::Period::Week);
            open_usage_report(app, period);
            return false;
        }
        _ => {}
    }
    false
}

/// Copies text, keeping the clipboard owner alive on Linux (X11 and Wayland clipboards are
/// served by the copying app until something else is copied).
pub(crate) fn copy(text: &str) -> std::result::Result<(), String> {
    use std::sync::{Mutex, OnceLock};
    static CLIPBOARD: OnceLock<Mutex<Option<arboard::Clipboard>>> = OnceLock::new();
    let holder = CLIPBOARD.get_or_init(|| Mutex::new(arboard::Clipboard::new().ok()));
    let mut guard = holder.lock().map_err(|_| "clipboard unavailable".to_string())?;
    let clipboard = guard.as_mut().ok_or("no clipboard is available")?;
    clipboard.set_text(text.to_string()).map_err(|error| error.to_string())
}

/// An agent's totals for a period show any use.
fn agent_active(agent: &serde_json::Value) -> bool {
    ["totalTokens", "turns", "sessions", "requests"]
        .iter()
        .any(|key| agent[*key].as_u64().unwrap_or(0) > 0)
}

/// Any plan limits known, or any agent activity in any period.
fn has_activity(report: &serde_json::Value) -> bool {
    let limits = report["limits"]
        .as_array()
        .into_iter()
        .flatten()
        .any(|limit| limit["available"].as_bool() == Some(true));
    let used = report["periods"]
        .as_array()
        .into_iter()
        .flatten()
        .flat_map(|period| period["agents"].as_array().into_iter().flatten())
        .any(agent_active);
    limits || used
}

/// Plan limits, then today, this week, and this month per agent, plus a signature of what it shows.
fn usage_menu(report: Option<&serde_json::Value>, loading: bool) -> (Vec<Entry>, String) {
    use barista_core::usage::{limit_summary, summary};
    let mut menu = Vec::new();
    let mut lines = Vec::new();
    match report {
        None => lines.push(if loading {
            "Reading agent logs…".to_string()
        } else {
            "No usage yet".to_string()
        }),
        Some(report) if !has_activity(report) => {
            lines.push("No Claude Code, Codex, or Cursor activity on this computer yet".to_string());
        }
        Some(report) => {
            let limits: Vec<&serde_json::Value> = report["limits"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|limit| limit["available"].as_bool() == Some(true))
                .collect();
            if !limits.is_empty() {
                lines.push("Limits".to_string());
                for limit in limits {
                    lines.push(format!("{} · {}", limit["name"].as_str().unwrap_or(""), limit_summary(limit)));
                }
            }
            for period in report["periods"].as_array().into_iter().flatten() {
                if !lines.is_empty() {
                    lines.push(String::new());
                }
                let label = period["label"].as_str().unwrap_or("").to_string();
                let active: Vec<String> = period["agents"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter(|agent| agent_active(agent))
                    .map(|agent| format!("{}: {}", agent["name"].as_str().unwrap_or(""), summary(agent)))
                    .collect();
                if active.is_empty() {
                    lines.push(format!("{label}: no activity"));
                } else {
                    lines.push(label);
                    lines.extend(active);
                }
            }
        }
    }
    for line in &lines {
        if line.is_empty() {
            menu.push(Entry::Separator);
        } else {
            menu.push(label(line));
        }
    }
    menu.push(Entry::Separator);
    menu.push(label("Costs are estimates at API prices, not your subscription bill"));
    menu.push(item("usage-refresh", if loading { "Refreshing…" } else { "Refresh" }));
    // Upgraded with Claude Code already connected: offer the status line that reports Claude's limits.
    let offer = HookAgent::Claude.is_installed() && barista_core::hooks::statusline_status(None) == "Not connected";
    if offer {
        menu.push(item("statusline-install", "Add live limits to Claude Code’s status line"));
    }
    lines.push(format!("{loading}{offer}"));
    (menu, lines.join("\u{1f}"))
}

/// Windows' right-click menu when the panel is available: just the essentials. Everything else lives
/// in the panel (left-click).
pub fn build_short(app: &Shared) -> Model {
    let full = build(app);
    let state = lock(app);
    let session = &state.session;
    let mut menu = Vec::new();
    let mut signature = vec![full.tooltip.clone(), full.title.clone()];
    if let Some(manual) = session.manual_session() {
        let remaining = if manual.expires_at.is_some() {
            format!("{} left", session.countdown_label())
        } else {
            "until stopped".into()
        };
        menu.push(label(&format!("Your timer · {remaining}")));
        if manual.expires_at.is_some() {
            menu.push(item("add:900", "Add 15 minutes"));
        }
        menu.push(item("stop-manual", "Stop"));
        signature.push(remaining);
    } else {
        menu.push(label("Keep awake for"));
        let enabled = !session.battery_blocked();
        for (id, text) in [
            ("start:900", "15 minutes"),
            ("start:3600", "1 hour"),
            ("start:14400", "4 hours"),
            ("start:forever", "Until stopped"),
        ] {
            menu.push(Entry::Item {
                id: id.into(),
                text: text.into(),
                enabled,
            });
        }
        signature.push(format!("idle {enabled}"));
    }
    if session.is_active() {
        menu.push(item("stop-all", "Stop all"));
    }
    menu.push(Entry::Separator);
    menu.push(item(
        "usage-report",
        if cfg!(windows) {
            "Projects & history…"
        } else {
            "Open usage report…"
        },
    ));
    menu.push(item("panel:open", "Open Barista"));
    menu.push(item("quit", "Quit Barista"));
    signature.push(session.is_active().to_string());
    Model {
        entries: menu,
        signature: signature.join("\u{1f}"),
        tooltip: full.tooltip,
        title: full.title,
        active: full.active,
    }
}

/// A one-line summary of plan limits for the panel's footer ("Claude 24% · ChatGPT 6%"), else
/// today's tokens, else nothing.
#[cfg(windows)]
fn usage_summary(report: Option<&serde_json::Value>) -> Option<String> {
    let report = report?;
    let mut parts = Vec::new();
    for (agent, short, preferred) in [("claude", "Claude", "five_hour"), ("codex", "ChatGPT", "primary")] {
        let Some(limit) = report["limits"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|limit| limit["agent"] == agent && limit["available"].as_bool() == Some(true))
        else {
            continue;
        };
        let windows: Vec<&serde_json::Value> = limit["windows"].as_array().into_iter().flatten().collect();
        let window = windows.iter().find(|w| w["id"] == preferred).or_else(|| windows.first());
        if let Some(percent) = window.and_then(|w| w["usedPercent"].as_f64()) {
            parts.push(format!("{short} {}%", percent.round() as i64));
        }
    }
    if !parts.is_empty() {
        return Some(parts.join(" · "));
    }
    let today = report["periods"].as_array()?.iter().find(|p| p["id"] == "today")?;
    let tokens = today["totalTokens"].as_u64().unwrap_or(0);
    (tokens > 0).then(|| format!("Today {} tokens", barista_core::usage::format_tokens(tokens)))
}

#[cfg(windows)]
thread_local! {
    /// Whether to offer Claude Code's status line, and when that was last read: it reads Claude
    /// Code's settings from disk, too slow to repeat every second the panel is open.
    static STATUSLINE_OFFER: std::cell::Cell<Option<(std::time::Instant, bool)>> = const { std::cell::Cell::new(None) };
}

/// Forget cached panel facts (after a command, which may have changed them).
#[cfg(windows)]
pub fn invalidate_panel_cache() {
    STATUSLINE_OFFER.with(|offer| offer.set(None));
}

#[cfg(windows)]
fn statusline_offer() -> bool {
    STATUSLINE_OFFER.with(|cache| match cache.get() {
        Some((read, offer)) if read.elapsed() < std::time::Duration::from_secs(30) => offer,
        _ => {
            let offer = HookAgent::Claude.is_installed() && barista_core::hooks::statusline_status(None) == "Not connected";
            cache.set(Some((std::time::Instant::now(), offer)));
            offer
        }
    })
}

/// Everything the Windows panel shows, as JSON for its page (see assets/panel.html). Nothing in it
/// changes by itself from one second to the next (countdowns are end times the page counts down
/// to), so the panel can skip posting an unchanged snapshot.
#[cfg(windows)]
pub fn panel_state(app: &Shared) -> serde_json::Value {
    use serde_json::json;
    let offer = statusline_offer();
    let mut state = lock(app);
    state.session.refresh();
    let state = &*state;
    let session = &state.session;
    let manual = session.manual_session().map(|lease| {
        json!({
            "indefinite": lease.expires_at.is_none(),
            "total": lease.expires_at.map(|end| (end - lease.started_at).max(1.0)),
            "endsAt": lease.expires_at.map(|end| (end * 1000.0) as i64),
        })
    });
    let tasks: Vec<serde_json::Value> = session
        .sessions()
        .iter()
        .filter(|lease| lease.source != SessionSource::Manual)
        .map(|lease| {
            let display = if lease.keep_display_awake {
                "display on"
            } else {
                "display may sleep"
            };
            // A timed lease counts down on the page ("12:34 left · display on") from endsAt.
            let (until, ends_at) = match (lease.source, lease.expires_at) {
                (SessionSource::Application, Some(_)) => (Some("While it’s busy"), None),
                (SessionSource::Application, None) => (Some("Until the app quits"), None),
                (SessionSource::Command, _) => (Some("Until the command finishes"), None),
                (_, Some(end)) => (None, Some((end * 1000.0) as i64)),
                (_, None) => (Some("Until stopped"), None),
            };
            let detail = lease.message.clone().or_else(|| until.map(|until| format!("{until} · {display}")));
            json!({
                "id": lease.id,
                "title": lease.title,
                "source": lease.source.as_str(),
                "message": lease.message,
                "progress": lease.progress,
                "detail": detail,
                "endsAt": ends_at,
                "display": display,
            })
        })
        .collect();
    let history: Vec<serde_json::Value> = session
        .history()
        .iter()
        .take(10)
        .map(|event| json!({"title": event.title, "reason": event.reason.label(), "endedAt": (event.ended_at * 1000.0) as i64}))
        .collect();
    let agents: Vec<serde_json::Value> = state
        .agents
        .iter()
        .map(|(agent, status)| {
            let status = match status {
                AgentStatus::Connected => "connected",
                AgentStatus::Moved => "moved",
                AgentStatus::Disconnected => "disconnected",
                AgentStatus::NotFound => "notFound",
            };
            let name = if *agent == HookAgent::Codex {
                "ChatGPT Codex"
            } else {
                agent.display_name()
            };
            json!({"key": agent.name(), "name": name, "status": status})
        })
        .collect();
    let prefs = &state.prefs;
    json!({
        "version": VERSION,
        "awake": session.is_active(),
        "notice": state.notice,
        "error": session.error,
        "update": state.visible_update().map(|update| json!({"version": update.version})),
        "updateStatus": state.update_status,
        "batteryBlocked": session.battery_blocked(),
        "batteryThreshold": session.battery_threshold(),
        "manual": manual,
        "tasks": tasks,
        "sessionCount": session.sessions().len(),
        "history": history,
        "prefs": {
            "keepDisplayAwake": prefs.keep_display_awake,
            "batteryThreshold": prefs.battery_threshold,
            "notifications": prefs.notifications_enabled,
            "checksForUpdates": prefs.checks_for_updates,
            "shareActiveCount": prefs.share_active_count,
            "agentAccess": prefs.agent_access,
            "claudeAccountLimits": prefs.claude_account_limits,
            "codexAccountLimits": prefs.codex_account_limits,
            "watchCowork": prefs.watch_cowork,
            "watchChatgpt": prefs.watch_chatgpt,
            "launchAtLogin": state.launch_at_login,
            "weeklySummary": prefs.weekly_summary,
            "limitAlerts": prefs.limit_alerts,
        },
        "week": state.usage_week,
        "agents": agents,
        "usage": state.usage_panel,
        "usageLoading": state.usage_loading,
        "usageSummary": usage_summary(state.usage.as_ref()),
        "usageHasActivity": state.usage.as_ref().map(has_activity).unwrap_or(false),
        "statuslineOffer": offer,
    })
}
