//! Barista for Windows and Linux: a tray app that owns the awake sessions and serves the local
//! control endpoint the `barista` CLI, MCP server, and agent hooks talk to.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

// What the tray menu shows and does. macOS builds are development-only (the Mac app is the Swift one)
// and have no tray, so the menu goes unused there.
#[cfg_attr(not(any(windows, target_os = "linux")), allow(dead_code))]
mod menu;
mod platform;
mod prefs;
#[cfg(any(windows, target_os = "linux"))]
mod tray;

use barista_core::activity::{self, ActivityTracker, WatchedApp};
use barista_core::control::{default_path, ControlServer};
use barista_core::hooks::{self, HookAgent};
use barista_core::power::{self, FakePower};
use barista_core::session::AwakeSession;
use barista_core::update::{self, AvailableUpdate};
use barista_core::*;
use prefs::Prefs;
use std::collections::{HashMap, HashSet};
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
    /// Today, this week, and this month (see docs/USAGE.md), computed off the main thread.
    pub usage: Option<serde_json::Value>,
    pub usage_updated: Option<std::time::Instant>,
    pub usage_loading: bool,
    /// Claude account limits: when the app last asked (at most every 5 minutes), and whether it's asking now.
    pub account_last_attempt: Option<std::time::Instant>,
    pub account_fetching: bool,
    /// Watched apps (Cowork, ChatGPT): busy trackers, live session IDs, and apps whose session you
    /// stopped (they wait for a quiet spell before starting another).
    pub activity: HashMap<&'static str, ActivityTracker>,
    pub activity_leases: HashMap<&'static str, String>,
    pub activity_stopped: HashSet<&'static str>,
    /// When each app's current burst of activity began; the 8-hour cap covers the whole burst.
    pub activity_burst: HashMap<&'static str, f64>,
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
            // Claude Code shares its plan limits only with its status line, so it comes along.
            hooks::install(&cli, agent, None).and_then(|()| match agent {
                HookAgent::Claude => hooks::statusline_install(&cli, None),
                _ => Ok(()),
            })
        } else {
            hooks::uninstall(agent, None).and_then(|()| match agent {
                HookAgent::Claude => hooks::statusline_uninstall(None),
                _ => Ok(()),
            })
        };
        self.notice = Some(match result {
            Ok(()) if connected => {
                if !self.prefs.agent_access {
                    self.set_agent_access(true);
                }
                match agent {
                    HookAgent::Claude => "Claude Code connected. New prompts keep your computer awake until Claude finishes, and its status line shows your plan limits.".into(),
                    HookAgent::Codex => "Codex connected. One more step: Codex runs new hooks only after you trust them. In Codex, type /hooks and trust Barista’s hooks.".into(),
                    HookAgent::Cursor => "Cursor connected. Prompts in Cursor’s agent keep your computer awake until it finishes, and its turns appear in Usage.".into(),
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

    pub fn watches(&self, app: WatchedApp) -> bool {
        if app == WatchedApp::COWORK {
            self.prefs.watch_cowork
        } else {
            self.prefs.watch_chatgpt
        }
    }

    pub fn set_watches(&mut self, app: WatchedApp, value: bool) {
        if app == WatchedApp::COWORK {
            self.prefs.watch_cowork = value;
        } else {
            self.prefs.watch_chatgpt = value;
        }
        self.save();
        if !value {
            if let Some(id) = self.activity_leases.remove(app.id) {
                self.session.cancel(&id);
            }
        }
    }

    /// Starts, renews, or lets lapse a watched app's session after a sample.
    fn update_activity(&mut self, app: WatchedApp, root: Option<i32>, cpu: &HashMap<i32, f64>, time: f64) {
        // Turned off while this sample was being taken: discard it.
        if !self.watches(app) {
            if let Some(id) = self.activity_leases.remove(app.id) {
                self.session.cancel(&id);
            }
            self.activity_burst.remove(app.id);
            return;
        }
        let tracker = self.activity.entry(app.id).or_default();
        let busy = tracker.record(cpu, time);
        let last_busy = tracker.last_busy;
        // A burst starts with the first busy sample and ends after a quiet spell or when the app quits.
        let quiet_for = time - last_busy.unwrap_or(f64::MIN);
        if root.is_none() || (!busy && quiet_for >= activity::policy::QUIET_BEFORE_RESTART) {
            self.activity_burst.remove(app.id);
        } else if busy {
            self.activity_burst.entry(app.id).or_insert(time);
        }
        if let Some(id) = self.activity_leases.get(app.id).cloned() {
            if !self.session.sessions().iter().any(|s| s.id == id) {
                // Ended. If you stopped it (or battery protection did), don't restart it during this burst.
                let reason = self.session.history().iter().find(|h| h.id == id).map(|h| h.reason);
                if !matches!(reason, Some(EndReason::Expired) | Some(EndReason::ProcessExited)) {
                    self.activity_stopped.insert(app.id);
                }
                self.activity_leases.remove(app.id);
            }
        }
        if self.activity_stopped.contains(app.id) {
            if !busy && quiet_for >= activity::policy::QUIET_BEFORE_RESTART {
                self.activity_stopped.remove(app.id);
            }
            return;
        }
        let (true, Some(root)) = (busy, root) else { return };
        if let Some(id) = self.activity_leases.get(app.id).cloned() {
            self.session.renew(&id, activity::policy::LEASE);
            return;
        }
        // A new session gets only what's left of the burst's cap; once it's used up, wait for quiet.
        let started = self.activity_burst.get(app.id).copied().unwrap_or(time);
        let remaining = activity::policy::HARD_CAP - (time - started);
        if remaining <= 1.0 {
            self.activity_stopped.insert(app.id);
            return;
        }
        if let Ok(lease) = self.session.acquire(
            app.title,
            SessionSource::Application,
            Some(activity::policy::LEASE.min(remaining)),
            false,
            Some(root),
            Some(remaining),
            false,
        ) {
            self.activity_leases.insert(app.id, lease.id);
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

/// Recomputes usage on a background thread if it's older than `max_age` (parsing logs can take a
/// moment, so the event loop never waits for it).
pub fn refresh_usage(app: &Shared, max_age: Duration) {
    {
        let mut state = lock(app);
        let fresh = state.usage_updated.is_some_and(|at| at.elapsed() < max_age);
        if state.ephemeral || state.usage_loading || fresh {
            return;
        }
        state.usage_loading = true;
    }
    let app = app.clone();
    std::thread::spawn(move || {
        use barista_core::usage::{report, Period};
        let value = report(&[Period::Today, Period::Week, Period::Month]);
        let mut state = lock(&app);
        state.usage = Some(value);
        state.usage_updated = Some(std::time::Instant::now());
        state.usage_loading = false;
    });
}

/// Asks your Claude account for its limits on a background thread: at most once every 5 minutes
/// from the last attempt (unless `now`, for turning the setting on), never two at once.
pub fn fetch_claude_account(app: &Shared, now: bool) {
    {
        let mut state = lock(app);
        let recent = state.account_last_attempt.is_some_and(|at| at.elapsed() < Duration::from_secs(300));
        if state.ephemeral || !state.prefs.claude_account_limits || state.account_fetching || (recent && !now) {
            return;
        }
        state.account_fetching = true;
        state.account_last_attempt = Some(std::time::Instant::now());
    }
    let app = app.clone();
    std::thread::spawn(move || {
        let refreshed = barista_core::claude_account::refresh().is_ok();
        lock(&app).account_fetching = false;
        if refreshed {
            refresh_usage(&app, Duration::ZERO);
        }
    });
}

/// Turns reading limits from your Claude account on or off. Off forgets what was saved.
pub fn set_claude_account_limits(app: &Shared, enabled: bool) {
    {
        let mut state = lock(app);
        state.prefs.claude_account_limits = enabled;
        state.save();
    }
    if enabled {
        fetch_claude_account(app, true);
    } else {
        barista_core::claude_account::delete_saved(&barista_core::usage::data_dir());
        refresh_usage(app, Duration::ZERO);
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
        usage: None,
        usage_updated: None,
        usage_loading: false,
        account_last_attempt: None,
        account_fetching: false,
        activity: HashMap::new(),
        activity_leases: HashMap::new(),
        activity_stopped: HashSet::new(),
        activity_burst: HashMap::new(),
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
        // Watched apps: sample CPU off the lock, then decide under it.
        let watcher = app.clone();
        std::thread::spawn(move || loop {
            std::thread::sleep(Duration::from_secs_f64(activity::policy::SAMPLE_INTERVAL));
            let apps: Vec<WatchedApp> = {
                let state = lock(&watcher);
                WatchedApp::ALL.into_iter().filter(|app| state.watches(*app)).collect()
            };
            if apps.is_empty() {
                continue;
            }
            let samples = activity::sample(&apps);
            let time = now();
            let mut state = lock(&watcher);
            for (app, (root, cpu)) in apps.into_iter().zip(samples) {
                state.update_activity(app, root, &cpu, time);
            }
        });
        // Usage: soon after launch, then every five minutes. Claude's limits come from your Claude
        // account first (read-only use of Claude Code's login), unless turned off.
        let usage_app = app.clone();
        std::thread::spawn(move || loop {
            fetch_claude_account(&usage_app, false);
            refresh_usage(&usage_app, Duration::from_secs(60));
            std::thread::sleep(Duration::from_secs(300));
        });
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

    #[cfg(any(windows, target_os = "linux"))]
    if !options.headless {
        tray::run(app, server, options.background);
    }
    #[cfg(not(any(windows, target_os = "linux")))]
    if !options.headless && !options.background {
        eprintln!("barista-app: no tray on this platform; serving the control endpoint only. Use the Mac app for a menu-bar icon.");
    }
    // No tray (a server, a test, no graphical session, or macOS): just serve until terminated.
    let _ = (&app, &server);
    loop {
        std::thread::park();
    }
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
