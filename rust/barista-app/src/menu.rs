//! What the tray menu shows and what its commands do, independent of how each platform draws it
//! (muda on Windows, ksni over D-Bus on Linux).
use crate::{check_for_updates, cli_path, lock, platform, refresh_usage, AgentStatus, Shared};
use barista_core::activity::WatchedApp;
use barista_core::hooks::HookAgent;
use barista_core::*;
use std::time::Duration;

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
    settings.push(check("toggle:updates", "Check for updates daily", prefs.checks_for_updates));
    settings.push(item(
        "check-updates",
        state.update_status.as_deref().unwrap_or("Check for updates now"),
    ));
    settings.push(Entry::Separator);
    settings.push(check("toggle:agents", "Local agent access (CLI, MCP, hooks)", prefs.agent_access));
    settings.push(check(
        "toggle:claude-account",
        "Show Claude plan limits from your Claude account",
        prefs.claude_account_limits,
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
        "start" => state.start_manual(argument.parse::<f64>().ok()),
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
            "updates" => {
                let value = !state.prefs.checks_for_updates;
                state.set_checks_for_updates(value)
            }
            "agents" => {
                let value = !state.prefs.agent_access;
                state.set_agent_access(value)
            }
            "claude-account" => {
                let value = !state.prefs.claude_account_limits;
                drop(state);
                crate::set_claude_account_limits(app, value);
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
        "usage-refresh" => {
            drop(state);
            refresh_usage(app, Duration::ZERO);
            return false;
        }
        _ => {}
    }
    false
}

/// Copies text, keeping the clipboard owner alive on Linux (X11 and Wayland clipboards are
/// served by the copying app until something else is copied).
fn copy(text: &str) -> std::result::Result<(), String> {
    use std::sync::{Mutex, OnceLock};
    static CLIPBOARD: OnceLock<Mutex<Option<arboard::Clipboard>>> = OnceLock::new();
    let holder = CLIPBOARD.get_or_init(|| Mutex::new(arboard::Clipboard::new().ok()));
    let mut guard = holder.lock().map_err(|_| "clipboard unavailable".to_string())?;
    let clipboard = guard.as_mut().ok_or("no clipboard is available")?;
    clipboard.set_text(text.to_string()).map_err(|error| error.to_string())
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
        Some(report) => {
            lines.push("Limits".to_string());
            for limit in report["limits"].as_array().into_iter().flatten() {
                lines.push(format!("{} · {}", limit["name"].as_str().unwrap_or(""), limit_summary(limit)));
            }
            for period in report["periods"].as_array().into_iter().flatten() {
                lines.push(String::new());
                lines.push(period["label"].as_str().unwrap_or("").to_string());
                for agent in period["agents"].as_array().into_iter().flatten() {
                    lines.push(format!("{}: {}", agent["name"].as_str().unwrap_or(""), summary(agent)));
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
