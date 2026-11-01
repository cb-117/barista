//! Barista for Windows and Linux: a tray app that owns the awake sessions and serves the local
//! control endpoint the `barista` CLI, MCP server, and agent hooks talk to.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod platform;
mod prefs;
mod tray;

use barista_core::control::{default_path, ControlServer};
use barista_core::hooks::{self, HookAgent};
use barista_core::power::{self, FakePower};
use barista_core::session::AwakeSession;
use barista_core::update::{self, AvailableUpdate};
use barista_core::*;
use prefs::Prefs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AgentStatus {
    NotFound,
    Disconnected,
    Connected,
    /// Connected to another copy of Barista, or only partly.
    Moved,
}

pub struct App {
    pub session: AwakeSession,
    pub prefs: Prefs,
    pub notice: Option<String>,
    pub update: Option<AvailableUpdate>,
    pub update_status: Option<String>,
    pub checking_for_updates: bool,
    pub agents: Vec<(HookAgent, AgentStatus)>,
    pub launch_at_login: bool,
    /// Saves nothing and touches no OS settings (`--serve` test mode).
    pub ephemeral: bool,
}

pub type Shared = Arc<Mutex<App>>;

pub fn lock(app: &Shared) -> MutexGuard<'_, App> {
    app.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

pub fn cli_path() -> PathBuf {
    let executable = barista_core::connection::current_executable();
    executable.with_file_name(if cfg!(windows) { "barista.exe" } else { "barista" })
}

impl App {
    fn save(&self) {
        sync_notification_settings(&self.prefs);
        if !self.ephemeral {
            self.prefs.save();
        }
    }

    pub fn refresh_agents(&mut self) {
        let cli = cli_path().to_string_lossy().into_owned();
        self.agents = HookAgent::ALL
            .into_iter()
            .map(|agent| {
                let status = if !hooks::installed_commands(agent, None).is_empty() {
                    if hooks::is_fully_installed(&cli, agent, None) {
                        AgentStatus::Connected
                    } else {
                        AgentStatus::Moved
                    }
                } else if agent.is_installed() {
                    AgentStatus::Disconnected
                } else {
                    AgentStatus::NotFound
                };
                (agent, status)
            })
            .collect();
    }

    pub fn set_connected(&mut self, agent: HookAgent, connected: bool) {
        let cli = cli_path().to_string_lossy().into_owned();
        let result = if connected {
            hooks::install(&cli, agent, None)
        } else {
            hooks::uninstall(agent, None)
        };
        self.notice = Some(match result {
            Ok(()) if connected => {
                if !self.prefs.agent_access {
                    self.set_agent_access(true);
                }
                match agent {
                    HookAgent::Claude => "Claude Code connected. New prompts keep your computer awake until Claude finishes.".into(),
                    HookAgent::Codex => "Codex connected. One more step: Codex runs new hooks only after you trust them. In Codex, type /hooks and trust Barista’s hooks.".into(),
                }
            }
            Ok(()) => format!("{} disconnected.", agent.display_name()),
            Err(error) => format!("Couldn’t update {} settings: {error}", agent.display_name()),
        });
        self.refresh_agents();
    }

    pub fn set_agent_access(&mut self, enabled: bool) {
        self.prefs.agent_access = enabled;
        self.save();
        if !enabled {
            // Ends agent and command leases (not their work) and revokes any lapsed turn.
            self.session.revoke_agent_turns();
            let tasks: Vec<String> = self
                .session
                .sessions()
                .iter()
                .filter(|s| s.is_task())
                .map(|s| s.id.clone())
                .collect();
            for id in tasks {
                self.session.cancel(&id);
            }
        }
    }

    pub fn start_manual(&mut self, seconds: Option<f64>) {
        self.prefs.last_session_seconds = seconds.unwrap_or(-1.0);
        self.save();
        self.session.start(seconds);
    }

    pub fn toggle_manual(&mut self) {
        if self.session.manual_session().is_some() {
            self.session.stop_manual();
        } else {
            let saved = self.prefs.last_session_seconds;
            self.session.start(if saved < 0.0 { None } else { Some(saved.clamp(1.0, 86400.0)) });
        }
    }

    pub fn set_keep_display_awake(&mut self, value: bool) {
        self.prefs.keep_display_awake = value;
        self.session.set_keep_display_awake(value);
        self.save();
    }

    pub fn set_battery_threshold(&mut self, value: i32) {
        if [0, 10, 15, 20, 30].contains(&value) {
            self.prefs.battery_threshold = value;
            self.session.set_battery_threshold(value);
            self.save();
        }
    }

    pub fn set_notifications(&mut self, value: bool) {
        self.prefs.notifications_enabled = value;
        self.save();
        if value {
            platform::notify("Notifications are on", "Barista will tell you when your sessions finish.");
        }
    }

    pub fn set_show_countdown(&mut self, value: bool) {
        self.prefs.show_countdown = value;
        self.save();
    }

    pub fn set_checks_for_updates(&mut self, value: bool) {
        self.prefs.checks_for_updates = value;
        self.save();
    }

    pub fn set_launch_at_login(&mut self, enabled: bool) {
        let executable = barista_core::connection::current_executable();
        match platform::set_launch_at_login(enabled, &executable) {
            Ok(()) => {
                self.prefs.launch_at_login_choice_made = true;
                self.save();
            }
            Err(error) => self.notice = Some(format!("Couldn’t change login settings: {error}")),
        }
        self.launch_at_login = platform::launches_at_login();
    }

    /// Turns on Launch at login the first time an installed copy runs, unless the user already chose.
    fn enable_launch_at_login_on_first_run(&mut self) {
        let executable = barista_core::connection::current_executable();
        if self.prefs.launch_at_login_choice_made || platform::is_development_build(&executable) {
            return;
        }
        if platform::set_launch_at_login(true, &executable).is_ok() {
            self.prefs.launch_at_login_choice_made = true;
            self.save();
        }
        self.launch_at_login = platform::launches_at_login();
    }

    pub fn visible_update(&self) -> Option<&AvailableUpdate> {
        self.update
            .as_ref()
            .filter(|u| self.prefs.dismissed_update.as_deref() != Some(u.version.as_str()))
    }

    pub fn dismiss_update(&mut self) {
        self.prefs.dismissed_update = self.update.as_ref().map(|u| u.version.clone());
        self.save();
    }

    fn handle(&mut self, request: ControlRequest) -> ControlReply {
        // The endpoint stays up when access is off, so clients get a clear "disabled" reply
        // instead of concluding Barista isn't running and trying to start it again.
        if !self.prefs.agent_access {
            return ControlReply::failure("Local agent access is disabled in Barista Settings.");
        }
        self.session.handle(request)
    }

    pub fn shutdown(&mut self) {
        self.session.stop(EndReason::AppQuit);
    }
}

/// Asks GitHub for a newer release on a background thread.
pub fn check_for_updates(app: &Shared, user_initiated: bool) {
    {
        let mut state = lock(app);
        if state.checking_for_updates || state.ephemeral {
            return;
        }
        state.checking_for_updates = true;
        if user_initiated {
            state.update_status = Some("Checking…".into());
        }
    }
    let app = app.clone();
    std::thread::spawn(move || {
        let result = update::check();
        let mut state = lock(&app);
        state.checking_for_updates = false;
        match result {
            Ok(found) => {
                state.prefs.last_update_check = now();
                state.update = found;
                if user_initiated {
                    state.update_status = Some(match &state.update {
                        Some(update) => format!("Barista {} is available.", update.version),
                        None => format!("You’re up to date ({VERSION})."),
                    });
                    if state.update.is_some() {
                        state.prefs.dismissed_update = None;
                    }
                }
                state.save();
            }
            Err(_) if user_initiated => state.update_status = Some("Couldn’t reach GitHub. Try again later.".into()),
            Err(_) => {}
        }
    });
}

struct Options {
    background: bool,
    headless: bool,
    serve: Option<String>,
}

fn parse_options() -> Options {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    let serve = arguments
        .iter()
        .position(|a| a == "--serve")
        .and_then(|i| arguments.get(i + 1).cloned());
    let no_display = cfg!(target_os = "linux") && std::env::var_os("DISPLAY").is_none() && std::env::var_os("WAYLAND_DISPLAY").is_none();
    Options {
        background: arguments.iter().any(|a| a == "--background"),
        headless: serve.is_some() || arguments.iter().any(|a| a == "--headless") || no_display,
        serve,
    }
}

fn main() {
    let options = parse_options();
    if std::env::args().any(|a| a == "--version") {
        println!("barista-app {VERSION}");
        return;
    }
    let ephemeral = options.serve.is_some();
    let prefs = if ephemeral { Prefs::default() } else { Prefs::load() };
    // `--serve PATH` is the isolated test service: fake power, no saved state.
    let mut session = if ephemeral {
        AwakeSession::new(Box::new(FakePower::default()))
    } else {
        AwakeSession::new(power::native())
    };
    session.set_keep_display_awake(prefs.keep_display_awake);
    session.set_battery_threshold(prefs.battery_threshold);
    if !ephemeral {
        session.set_history(prefs::load_history());
        session.on_history_change = Some(Box::new(prefs::save_history));
    }
    let app: Shared = Arc::new(Mutex::new(App {
        session,
        prefs,
        notice: None,
        update: None,
        update_status: None,
        checking_for_updates: false,
        agents: Vec::new(),
        launch_at_login: false,
        ephemeral,
    }));
    if !ephemeral {
        sync_notification_settings(&lock(&app).prefs);
        lock(&app).session.on_completion = Some(Box::new(completion_notification));
    }

    let path = options.serve.clone().unwrap_or_else(default_path);
    let handler_app = app.clone();
    let server = match ControlServer::start(&path, Arc::new(move |request| lock(&handler_app).handle(request))) {
        Ok(server) => server,
        Err(error) => {
            eprintln!("barista-app: {error}");
            std::process::exit(1);
        }
    };

    // Every session decision is re-evaluated each second: expiry, process exit, battery.
    {
        let app = app.clone();
        std::thread::spawn(move || loop {
            std::thread::sleep(Duration::from_secs(1));
            lock(&app).session.refresh();
        });
    }

    if !ephemeral {
        let mut state = lock(&app);
        state.enable_launch_at_login_on_first_run();
        state.launch_at_login = platform::launches_at_login();
        state.refresh_agents();
        drop(state);
        // Check hourly whether a day has passed, so sleep and restarts don't skip or repeat checks.
        let app = app.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_secs(10));
            loop {
                let due = {
                    let state = lock(&app);
                    state.prefs.checks_for_updates && now() - state.prefs.last_update_check >= 86400.0
                };
                if due {
                    check_for_updates(&app, false);
                }
                std::thread::sleep(Duration::from_secs(3600));
            }
        });
    }

    if options.headless {
        // No tray (a server, a test, or no graphical session): just serve until terminated.
        loop {
            std::thread::park();
        }
    }
    tray::run(app, server, options.background);
}

/// Read by the completion callback, which runs while the app state is locked.
static NOTIFICATIONS: AtomicBool = AtomicBool::new(false);
static BATTERY_THRESHOLD: AtomicI32 = AtomicI32::new(20);

fn sync_notification_settings(prefs: &Prefs) {
    NOTIFICATIONS.store(prefs.notifications_enabled, Ordering::Relaxed);
    BATTERY_THRESHOLD.store(prefs.battery_threshold, Ordering::Relaxed);
}

fn completion_notification(completion: &SessionCompletion) {
    if NOTIFICATIONS.load(Ordering::Relaxed) {
        show_completion(completion, BATTERY_THRESHOLD.load(Ordering::Relaxed));
    }
}

fn show_completion(completion: &SessionCompletion, threshold: i32) {
    if !matches!(
        completion.reason,
        EndReason::Expired | EndReason::Released | EndReason::ProcessExited | EndReason::BatteryLow
    ) {
        return;
    }
    if completion.reason == EndReason::BatteryLow {
        platform::notify(
            "Time to save some battery",
            &format!("Awake sessions stopped at {threshold}% battery. Your work is still running."),
        );
    } else {
        platform::notify(
            "Coffee break complete",
            &format!("{} · {}.", completion.title, completion.reason.label()),
        );
    }
}
