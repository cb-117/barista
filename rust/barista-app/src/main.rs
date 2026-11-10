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
// Windows: the panel that opens when you left-click the cup (WebView2).
#[cfg(windows)]
mod panel;
#[cfg(windows)]
mod report;

use barista_core::active_ping;
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
    /// The day's active-user ping is on its way (docs/ACTIVE-USERS.md).
    pub sending_active_ping: bool,
    pub agents: Vec<(HookAgent, AgentStatus)>,
    pub launch_at_login: bool,
    /// Saves nothing and touches no OS settings (`--serve` test mode).
    pub ephemeral: bool,
    /// Today, this week, and this month (see docs/USAGE.md), computed off the main thread.
    pub usage: Option<serde_json::Value>,
    /// What the panel shows, worked out once per refresh: `usage` without the all-time period
    /// (only the usage window needs every day), and the last 7 days for its chart.
    pub usage_panel: Option<serde_json::Value>,
    pub usage_week: Option<serde_json::Value>,
    pub usage_updated: Option<std::time::Instant>,
    pub usage_loading: bool,
    /// Account limits (Claude, ChatGPT): when the app last asked each (at most every 5 minutes),
    /// and which it's asking now.
    pub account_last_attempt: HashMap<LimitsAccount, std::time::Instant>,
    pub account_fetching: HashSet<LimitsAccount>,
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
        let value = report(&[Period::Today, Period::Week, Period::Month, Period::All]);
        let today = chrono::Local::now().date_naive();
        let week = barista_core::usage::daily_active_seconds(&value, 7, today).map(|days| {
            serde_json::Value::Array(
                days.into_iter()
                    .map(|(date, seconds)| {
                        serde_json::json!({
                            "date": date.format("%Y-%m-%d").to_string(),
                            "label": date.format("%a %b %-d").to_string(),
                            "seconds": seconds,
                        })
                    })
                    .collect(),
            )
        });
        let mut panel = value.clone();
        if let Some(periods) = panel["periods"].as_array_mut() {
            periods.retain(|period| period["id"] != "all");
        }
        let mut state = lock(&app);
        state.usage_panel = Some(panel);
        state.usage_week = week;
        send_weekly_summary_if_due(&mut state, &value, chrono::Local::now().naive_local());
        send_limit_alerts_if_due(&mut state, &value, now());
        state.usage = Some(value);
        state.usage_updated = Some(std::time::Instant::now());
        state.usage_loading = false;
    });
}

/// Once a week, from Monday 9:00 through Tuesday: last week's totals, if there were any and
/// notifications are on (docs/USAGE.md, "Weekly summary").
fn send_weekly_summary_if_due(state: &mut App, report: &serde_json::Value, now: chrono::NaiveDateTime) {
    use chrono::{Datelike, Timelike, Weekday};
    if state.ephemeral || !state.prefs.notifications_enabled || !state.prefs.weekly_summary {
        return;
    }
    let due = match now.weekday() {
        Weekday::Mon => now.hour() >= 9,
        Weekday::Tue => true,
        _ => false,
    };
    if !due {
        return;
    }
    let Some(summary) = barista_core::usage::last_week_summary(report, now.date()) else {
        return;
    };
    let week = summary.week_start.format("%Y-%m-%d").to_string();
    if state.prefs.weekly_summary_sent.as_deref() == Some(week.as_str()) {
        return;
    }
    state.prefs.weekly_summary_sent = Some(week);
    state.save();
    platform::notify_with_action("Your week in Barista", &summary.text(), "Open history", "usage-report:history");
}

/// Shows one limit alert. On Windows its button opens the panel's usage view. On Linux the limits
/// are in the tray's menu, which can't be opened from here (and the usage report doesn't list
/// them), so the notice has no button.
fn show_limit_alert(alert: &barista_core::limit_alerts::Alert) {
    if cfg!(windows) {
        platform::notify_with_action(&alert.title, &alert.body, "Open usage", "panel:usage");
    } else {
        platform::notify(&alert.title, &alert.body);
    }
}

/// Notifies as a plan's limit windows fill, once per level and window, if Limit alerts and
/// notifications are on (docs/USAGE.md, "Limit alerts").
fn send_limit_alerts_if_due(state: &mut App, report: &serde_json::Value, now: f64) {
    if state.ephemeral || !state.prefs.notifications_enabled || !state.prefs.limit_alerts {
        return;
    }
    let (alerts, remembered) = barista_core::limit_alerts::evaluate(&report["limits"], &state.prefs.limit_alert_state, now);
    if remembered != state.prefs.limit_alert_state {
        state.prefs.limit_alert_state = remembered;
        state.save();
    }
    for alert in &alerts {
        show_limit_alert(alert);
    }
}

/// Writes the visual usage report (`barista report`) and opens it in the browser, off the main thread.
/// Only a failure shows up, as a notice.
pub fn open_usage_report(app: &Shared, period: barista_core::usage::Period) {
    let app = app.clone();
    std::thread::spawn(move || {
        let notice = match barista_core::usage::write_report(period, None, true) {
            Ok((_, true)) => return,
            Ok((path, false)) => format!("Couldn’t open a browser. The report is at {}", path.display()),
            Err(error) => format!("Couldn’t write the usage report: {error}"),
        };
        lock(&app).notice = Some(notice);
    });
}

/// An account Barista can ask for its plan's limits, read-only, with the login its agent saved.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum LimitsAccount {
    /// Your Claude account, with Claude Code's login.
    Claude,
    /// Your ChatGPT account, with Codex's login.
    ChatGpt,
}

impl LimitsAccount {
    pub const ALL: [Self; 2] = [Self::Claude, Self::ChatGpt];

    pub fn enabled(self, prefs: &Prefs) -> bool {
        match self {
            Self::Claude => prefs.claude_account_limits,
            Self::ChatGpt => prefs.codex_account_limits,
        }
    }

    /// Asks once and saves the limits; false when nothing new was saved.
    fn refresh(self) -> bool {
        match self {
            Self::Claude => barista_core::claude_account::refresh().is_ok(),
            Self::ChatGpt => barista_core::codex_account::refresh().is_ok(),
        }
    }

    fn forget(self) {
        let directory = barista_core::usage::data_dir();
        match self {
            Self::Claude => barista_core::claude_account::delete_saved(&directory),
            Self::ChatGpt => barista_core::codex_account::delete_saved(&directory),
        }
    }
}

/// Asks an account for its limits on a background thread: at most once every 5 minutes from the
/// last attempt (unless `now`, for turning the setting on), never two at once for one account.
pub fn fetch_account(app: &Shared, account: LimitsAccount, now: bool) {
    {
        let mut state = lock(app);
        let recent = state
            .account_last_attempt
            .get(&account)
            .is_some_and(|at| at.elapsed() < Duration::from_secs(300));
        if state.ephemeral || !account.enabled(&state.prefs) || state.account_fetching.contains(&account) || (recent && !now) {
            return;
        }
        state.account_fetching.insert(account);
        state.account_last_attempt.insert(account, std::time::Instant::now());
    }
    let app = app.clone();
    std::thread::spawn(move || {
        let refreshed = account.refresh();
        let still_enabled = {
            let mut state = lock(&app);
            state.account_fetching.remove(&account);
            account.enabled(&state.prefs)
        };
        if !still_enabled {
            // Turned off while asking: what was just saved is forgotten too.
            account.forget();
        }
        if refreshed {
            refresh_usage(&app, Duration::ZERO);
        }
    });
}

/// Turns reading limits from an account on or off. Off forgets what was saved.
pub fn set_account_limits(app: &Shared, account: LimitsAccount, enabled: bool) {
    {
        let mut state = lock(app);
        match account {
            LimitsAccount::Claude => state.prefs.claude_account_limits = enabled,
            LimitsAccount::ChatGpt => state.prefs.codex_account_limits = enabled,
        }
        state.save();
    }
    if enabled {
        fetch_account(app, account, true);
    } else {
        account.forget();
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

/// Tells baristatime.app that a copy of Barista is running, on a background thread: once per UTC
/// day while Help count active users is on, never two at once, and never from the test service
/// (docs/ACTIVE-USERS.md). Only a reply that got through is remembered; otherwise the next hourly
/// check tries again.
pub fn send_active_ping_if_due(app: &Shared) {
    let today = active_ping::today();
    let flags = {
        let mut state = lock(app);
        let Some(flags) = active_ping_due(&state.prefs, state.ephemeral, state.sending_active_ping, today) else {
            return;
        };
        state.sending_active_ping = true;
        flags
    };
    let app = app.clone();
    std::thread::spawn(move || {
        let sent = active_ping::send(flags);
        let mut state = lock(&app);
        state.sending_active_ping = false;
        if sent {
            state.prefs.last_active_ping = Some(active_ping::date_text(today));
            state.save();
        }
    });
}

/// What today's ping would say, or None when there is none to send: the setting is off, this is the
/// test service, one is already on its way, or today's got through.
fn active_ping_due(prefs: &Prefs, ephemeral: bool, sending: bool, today: chrono::NaiveDate) -> Option<active_ping::Flags> {
    if ephemeral || !prefs.share_active_count || sending {
        return None;
    }
    active_ping::due(prefs.last_active_ping.as_deref(), today)
}

struct Options {
    background: bool,
    headless: bool,
    serve: Option<String>,
    /// Set by the Windows installer when the user unchecked "Start Barista when I sign in":
    /// remember that choice instead of turning Launch at login on for the first run.
    login_choice_made: bool,
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
        login_choice_made: arguments.iter().any(|a| a == "--login-choice-made"),
    }
}

fn main() {
    let options = parse_options();
    if std::env::args().any(|a| a == "--version") {
        println!("barista-app {VERSION}");
        return;
    }
    let ephemeral = options.serve.is_some();
    // Windows: notifications and the taskbar show Barista's name and cup.
    #[cfg(windows)]
    if !ephemeral {
        platform::identity::register();
    }
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
        sending_active_ping: false,
        agents: Vec::new(),
        launch_at_login: false,
        ephemeral,
        usage: None,
        usage_panel: None,
        usage_week: None,
        usage_updated: None,
        usage_loading: false,
        account_last_attempt: HashMap::new(),
        account_fetching: HashSet::new(),
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
        if options.login_choice_made && !state.prefs.launch_at_login_choice_made {
            state.prefs.launch_at_login_choice_made = true;
            state.save();
        }
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
        // Usage: soon after launch, then every five minutes. Plan limits come from your Claude and
        // ChatGPT accounts first (read-only use of Claude Code's and Codex's logins), unless turned off.
        let usage_app = app.clone();
        std::thread::spawn(move || loop {
            for account in LimitsAccount::ALL {
                fetch_account(&usage_app, account, false);
            }
            refresh_usage(&usage_app, Duration::from_secs(60));
            std::thread::sleep(Duration::from_secs(300));
        });
        // Check hourly whether a day has passed, so sleep and restarts don't skip or repeat checks.
        // The active-user ping keeps the same hours, whether or not update checks are on.
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
                send_active_ping_if_due(&app);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_active_ping_waits_for_its_turn() {
        let today = chrono::NaiveDate::from_ymd_opt(2026, 11, 9).unwrap();
        let mut prefs = Prefs::default();
        // A new copy: on by default, and its first ping says so.
        let first = active_ping_due(&prefs, false, false, today).unwrap();
        assert!(first.new && first.weekly && first.monthly);
        // Never from the test service, never two at once, never with the setting off.
        assert_eq!(active_ping_due(&prefs, true, false, today), None);
        assert_eq!(active_ping_due(&prefs, false, true, today), None);
        prefs.share_active_count = false;
        assert_eq!(active_ping_due(&prefs, false, false, today), None);
        prefs.share_active_count = true;
        // Once today's got through, nothing until tomorrow, whether or not updates are checked.
        prefs.last_active_ping = Some(active_ping::date_text(today));
        assert_eq!(active_ping_due(&prefs, false, false, today), None);
        prefs.checks_for_updates = false;
        let tomorrow = active_ping_due(&prefs, false, false, today.succ_opt().unwrap()).unwrap();
        assert!(!tomorrow.new && !tomorrow.weekly && !tomorrow.monthly);
    }
}
