//! The coffee cup in the system tray (Linux) or notification area (Windows), and its menu.
use crate::{check_for_updates, cli_path, lock, platform, AgentStatus, Shared};
use barista_core::control::ControlServer;
use barista_core::hooks::HookAgent;
use barista_core::*;
use std::time::{Duration, Instant};
use tao::event::{Event, StartCause};
use tao::event_loop::{ControlFlow, EventLoopBuilder};
use tray_icon::menu::{CheckMenuItem, IsMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem, Submenu};
use tray_icon::{Icon, TrayIcon, TrayIconBuilder};

enum UserEvent {
    Menu(MenuEvent),
}

struct Icons {
    active: Icon,
    resting: Icon,
}

/// The app icon, full color while awake and faded while resting.
fn icons() -> Icons {
    let decoder = png::Decoder::new(std::io::Cursor::new(&include_bytes!("../assets/icon-64.png")[..]));
    let mut reader = decoder.read_info().expect("bundled icon is a valid PNG");
    let mut rgba = vec![0; reader.output_buffer_size().expect("icon size")];
    let info = reader.next_frame(&mut rgba).expect("bundled icon decodes");
    rgba.truncate(info.buffer_size());
    let (width, height) = (info.width, info.height);
    let mut faded = rgba.clone();
    for pixel in faded.as_chunks_mut::<4>().0 {
        let gray = ((u32::from(pixel[0]) * 30 + u32::from(pixel[1]) * 59 + u32::from(pixel[2]) * 11) / 100) as u8;
        pixel[0] = gray;
        pixel[1] = gray;
        pixel[2] = gray;
        pixel[3] = (u32::from(pixel[3]) * 3 / 5) as u8;
    }
    Icons {
        active: Icon::from_rgba(rgba, width, height).expect("icon"),
        resting: Icon::from_rgba(faded, width, height).expect("icon"),
    }
}

pub fn run(app: Shared, server: ControlServer, background: bool) -> ! {
    let event_loop = EventLoopBuilder::<UserEvent>::with_user_event().build();
    let proxy = event_loop.create_proxy();
    MenuEvent::set_event_handler(Some(move |event| {
        let _ = proxy.send_event(UserEvent::Menu(event));
    }));
    let icons = icons();
    let mut tray: Option<TrayIcon> = None;
    let mut shown = String::new();
    let mut last_agent_refresh = Instant::now();
    let mut server = Some(server);

    event_loop.run(move |event, _, control_flow| {
        *control_flow = ControlFlow::WaitUntil(Instant::now() + Duration::from_secs(1));
        match event {
            // The tray must be created once the event loop is running.
            Event::NewEvents(StartCause::Init) => {
                let (menu, signature, tooltip, title, active) = build(&app);
                let built = TrayIconBuilder::new()
                    .with_menu(Box::new(menu))
                    .with_tooltip(&tooltip)
                    .with_icon(if active { icons.active.clone() } else { icons.resting.clone() })
                    .with_title(&title)
                    .build();
                match built {
                    Ok(icon) => tray = Some(icon),
                    Err(error) => eprintln!("barista-app: couldn't show the tray icon ({error}). Barista keeps running."),
                }
                shown = signature;
                if !background {
                    let place = if cfg!(windows) {
                        "notification area (you may need to click ^ to see it)"
                    } else {
                        "system tray"
                    };
                    platform::notify("Barista is running", &format!("Find the coffee cup in the {place}."));
                }
            }
            Event::UserEvent(UserEvent::Menu(event)) => {
                if handle(&app, event.id.as_ref()) {
                    lock(&app).shutdown();
                    if let Some(server) = server.take() {
                        server.stop();
                    }
                    tray = None;
                    *control_flow = ControlFlow::Exit;
                    return;
                }
                shown.clear(); // Rebuild right away.
            }
            _ => {}
        }
        if last_agent_refresh.elapsed() > Duration::from_secs(30) {
            lock(&app).refresh_agents();
            last_agent_refresh = Instant::now();
        }
        if let Some(icon) = &tray {
            let (menu, signature, tooltip, title, active) = build(&app);
            if signature != shown {
                icon.set_menu(Some(Box::new(menu)));
                let _ = icon.set_tooltip(Some(&tooltip));
                let _ = icon.set_icon(Some(if active { icons.active.clone() } else { icons.resting.clone() }));
                icon.set_title(Some(&title));
                shown = signature;
            }
        }
    })
}

fn item(id: &str, text: &str) -> MenuItem {
    MenuItem::with_id(id, text, true, None)
}

fn label(text: &str) -> MenuItem {
    MenuItem::new(text, false, None)
}

fn check(id: &str, text: &str, checked: bool) -> CheckMenuItem {
    CheckMenuItem::with_id(id, text, true, checked, None)
}

fn add(menu: &Menu, entry: &dyn IsMenuItem) {
    let _ = menu.append(entry);
}

fn add_to(menu: &Submenu, entry: &dyn IsMenuItem) {
    let _ = menu.append(entry);
}

/// Builds the menu and a signature of everything it shows, so unchanged menus aren't rebuilt.
fn build(app: &Shared) -> (Menu, String, String, String, bool) {
    let mut state = lock(app);
    state.session.refresh();
    let state = &*state;
    let session = &state.session;
    let menu = Menu::new();
    let mut signature = Vec::<String>::new();
    let computer = if cfg!(windows) { "PC" } else { "computer" };

    if let Some(notice) = &state.notice {
        add(&menu, &label(notice));
        add(&menu, &item("dismiss-notice", "Dismiss"));
        add(&menu, &PredefinedMenuItem::separator());
        signature.push(notice.clone());
    }
    if let Some(error) = &session.error {
        add(&menu, &label(error));
        add(&menu, &PredefinedMenuItem::separator());
        signature.push(error.clone());
    }
    if let Some(update) = state.visible_update() {
        add(&menu, &item("open-update", &format!("Download Barista {}…", update.version)));
        add(&menu, &item("dismiss-update", "Not now"));
        add(&menu, &PredefinedMenuItem::separator());
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
    add(&menu, &label(&headline));
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
        let submenu = Submenu::new(&text, true);
        add_to(&submenu, &item(&format!("cancel:{}", lease.id), "Stop keeping awake"));
        add(&menu, &submenu);
        signature.push(format!("{}{text}", lease.id));
    }

    add(&menu, &PredefinedMenuItem::separator());
    if let Some(manual) = session.manual_session() {
        let remaining = if manual.expires_at.is_some() {
            format!("{} left", session.countdown_label())
        } else {
            "until stopped".into()
        };
        add(&menu, &label(&format!("Your timer · {remaining}")));
        if manual.expires_at.is_some() {
            add(&menu, &item("add:900", "Add 15 minutes"));
        }
        add(&menu, &item("stop-manual", "Stop"));
        signature.push(format!("manual {remaining}"));
    } else {
        add(&menu, &label("Keep awake for"));
        let enabled = !session.battery_blocked();
        for (id, text) in [
            ("start:900", "15 minutes"),
            ("start:3600", "1 hour"),
            ("start:14400", "4 hours"),
            ("start:forever", "Until stopped"),
        ] {
            add(&menu, &MenuItem::with_id(id, text, enabled, None));
        }
        if !enabled {
            add(
                &menu,
                &label(&format!("Battery protection is on below {}%", session.battery_threshold())),
            );
        }
        signature.push(format!("idle {enabled}"));
    }
    if session.sessions().len() > 1 || (session.is_active() && session.manual_session().is_none()) {
        add(&menu, &item("stop-all", "Stop all"));
        signature.push("stop-all".into());
    }

    add(&menu, &PredefinedMenuItem::separator());
    let settings = Submenu::new("Settings", true);
    let prefs = &state.prefs;
    add_to(
        &settings,
        &check("toggle:display", "Keep the display on during my timers", prefs.keep_display_awake),
    );
    if cfg!(target_os = "linux") {
        add_to(
            &settings,
            &check("toggle:countdown", "Show time left next to the cup", prefs.show_countdown),
        );
    }
    let battery = Submenu::new("Battery protection", true);
    for value in [0, 10, 15, 20, 30] {
        let text = if value == 0 {
            "Off".to_string()
        } else {
            format!("Stop at {value}% on battery")
        };
        add_to(
            &battery,
            &check(&format!("battery:{value}"), &text, prefs.battery_threshold == value),
        );
    }
    add_to(&settings, &battery);
    add_to(&settings, &check("toggle:login", "Launch at login", state.launch_at_login));
    add_to(
        &settings,
        &check("toggle:notifications", "Notify me when sessions end", prefs.notifications_enabled),
    );
    add_to(
        &settings,
        &check("toggle:updates", "Check for updates daily", prefs.checks_for_updates),
    );
    add_to(
        &settings,
        &item("check-updates", state.update_status.as_deref().unwrap_or("Check for updates now")),
    );
    add_to(&settings, &PredefinedMenuItem::separator());
    add_to(
        &settings,
        &check("toggle:agents", "Local agent access (CLI, MCP, hooks)", prefs.agent_access),
    );
    for (agent, status) in &state.agents {
        let name = match agent {
            HookAgent::Claude => "Claude Code",
            HookAgent::Codex => "ChatGPT Codex",
        };
        let (text, id) = match status {
            AgentStatus::Connected => (format!("{name}: connected · Disconnect"), "disconnect"),
            AgentStatus::Moved => (format!("{name}: needs repair · Reconnect"), "connect"),
            AgentStatus::Disconnected => (format!("{name}: Connect"), "connect"),
            AgentStatus::NotFound => (format!("{name}: not set up for this user"), ""),
        };
        let key = if *agent == HookAgent::Claude { "claude" } else { "codex" };
        add_to(&settings, &MenuItem::with_id(format!("{id}:{key}"), &text, !id.is_empty(), None));
        signature.push(text);
    }
    add_to(&settings, &item("copy:cli", "Copy CLI path"));
    add_to(&settings, &item("copy:mcp", "Copy MCP setup"));
    add(&menu, &settings);
    signature.push(format!("{prefs:?}{}{:?}", state.launch_at_login, state.update_status));

    let history = Submenu::new("Recent sessions", !session.history().is_empty());
    for event in session.history().iter().take(10) {
        add_to(&history, &label(&format!("{} · {}", event.title, event.reason.label())));
    }
    if !session.history().is_empty() {
        add_to(&history, &PredefinedMenuItem::separator());
        add_to(&history, &item("clear-history", "Clear history"));
    }
    add(&menu, &history);
    signature.push(session.history().first().map(|h| h.id.clone()).unwrap_or_default());

    add(&menu, &PredefinedMenuItem::separator());
    add(&menu, &label(&format!("Barista {VERSION}")));
    add(&menu, &item("quit", "Quit Barista"));

    let active = session.is_active();
    let tooltip = if active {
        session.sessions().iter().map(|s| s.title.as_str()).collect::<Vec<_>>().join(" · ")
    } else {
        "Barista · resting".to_string()
    };
    let title = if active && prefs.show_countdown {
        session.countdown_label()
    } else {
        String::new()
    };
    signature.push(tooltip.clone());
    signature.push(title.clone());
    (menu, signature.join("\u{1f}"), tooltip, title, active)
}

/// Performs a menu command. Returns true to quit.
fn handle(app: &Shared, id: &str) -> bool {
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
            _ => {}
        },
        "battery" => state.set_battery_threshold(argument.parse().unwrap_or(20)),
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
