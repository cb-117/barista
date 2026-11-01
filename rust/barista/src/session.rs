//! Independent, expiring sessions share one native power request. A port of AwakeSession.swift.
use crate::models::*;
use crate::power::PowerAsserting;
use crate::system;

type Clock = Box<dyn Fn() -> f64 + Send>;
type Identity = Box<dyn Fn(i32) -> Option<String> + Send>;
type Battery = Box<dyn Fn() -> Option<BatteryState> + Send>;
type CompletionCallback = Box<dyn FnMut(&SessionCompletion) + Send>;
type HistoryCallback = Box<dyn FnMut(&[SessionCompletion]) + Send>;

pub struct AwakeSession {
    sessions: Vec<CoffeeLease>,
    history: Vec<SessionCompletion>,
    pub error: Option<String>,
    battery: Option<BatteryState>,
    battery_threshold: i32,
    keep_display_awake: bool,
    /// Called for every session that ends, after history is updated.
    pub on_completion: Option<CompletionCallback>,
    /// Called whenever history changes, so the host can save it.
    pub on_history_change: Option<HistoryCallback>,
    power: Box<dyn PowerAsserting>,
    now: Clock,
    process_identity: Identity,
    battery_provider: Battery,
    /// The time every session decision uses; always current after `refresh()`.
    clock: f64,
    assertion_display: Option<bool>,
    power_retry_pending: bool,
    was_battery_blocked: bool,
    /// Random per app run; replaced on every user stop, battery cutoff, or quit.
    stop_token: String,
}

pub const MAX_SESSIONS: usize = 64;
pub const MAX_SECONDS: f64 = 86400.0;

impl AwakeSession {
    pub fn new(power: Box<dyn PowerAsserting>) -> Self {
        Self::with_providers(power, Box::new(now), Box::new(system::process_identity), Box::new(system::battery))
    }

    pub fn with_providers(power: Box<dyn PowerAsserting>, now: Clock, process_identity: Identity, battery_provider: Battery) -> Self {
        let clock = now();
        let battery = battery_provider();
        Self {
            sessions: Vec::new(),
            history: Vec::new(),
            error: None,
            battery,
            battery_threshold: 20,
            keep_display_awake: true,
            on_completion: None,
            on_history_change: None,
            power,
            now,
            process_identity,
            battery_provider,
            clock,
            assertion_display: None,
            power_retry_pending: false,
            was_battery_blocked: false,
            stop_token: new_id(),
        }
    }

    pub fn set_history(&mut self, history: Vec<SessionCompletion>) {
        self.history = history.into_iter().take(20).collect();
    }

    pub fn sessions(&self) -> &[CoffeeLease] {
        &self.sessions
    }
    pub fn history(&self) -> &[SessionCompletion] {
        &self.history
    }
    pub fn battery(&self) -> Option<BatteryState> {
        self.battery
    }
    pub fn stop_token(&self) -> &str {
        &self.stop_token
    }
    pub fn clock(&self) -> f64 {
        self.clock
    }
    pub fn is_active(&self) -> bool {
        !self.sessions.is_empty()
    }
    pub fn display_awake(&self) -> bool {
        self.assertion_display == Some(true)
    }
    pub fn manual_session(&self) -> Option<&CoffeeLease> {
        self.sessions.iter().find(|s| s.source == SessionSource::Manual)
    }
    pub fn battery_threshold(&self) -> i32 {
        self.battery_threshold
    }
    pub fn set_battery_threshold(&mut self, value: i32) {
        self.battery_threshold = value;
        self.refresh();
    }
    pub fn keep_display_awake(&self) -> bool {
        self.keep_display_awake
    }
    pub fn set_keep_display_awake(&mut self, value: bool) {
        if self.keep_display_awake == value {
            return;
        }
        self.keep_display_awake = value;
        if let Some(manual) = self.sessions.iter_mut().find(|s| s.source == SessionSource::Manual) {
            manual.keep_display_awake = value;
            self.reconcile_power();
        }
    }
    pub fn battery_blocked(&self) -> bool {
        match self.battery {
            Some(battery) if self.battery_threshold > 0 => battery.on_battery && battery.percent <= self.battery_threshold,
            _ => false,
        }
    }

    pub fn format_time(interval: f64) -> String {
        let seconds = interval.clamp(0.0, 31_536_000.0).ceil() as i64;
        if seconds >= 3600 {
            format!("{}:{:02}:{:02}", seconds / 3600, seconds % 3600 / 60, seconds % 60)
        } else {
            format!("{:02}:{:02}", seconds / 60, seconds % 60)
        }
    }

    /// Your own timer shows its time left. Commands and agents renew a short lease while they
    /// work, so their expiry means nothing to a person; show how many are running instead.
    pub fn countdown_label(&self) -> String {
        if let Some(manual) = self.manual_session() {
            let Some(end) = manual.expires_at else { return "∞".into() };
            let minutes = ((end - (self.now)()).max(0.0) / 60.0).ceil() as i64;
            return if minutes >= 60 {
                format!("{}h {}m", minutes / 60, minutes % 60)
            } else {
                format!("{minutes}m")
            };
        }
        if self.sessions.len() == 1 {
            "1 task".into()
        } else {
            format!("{} tasks", self.sessions.len())
        }
    }

    pub fn start(&mut self, duration: Option<f64>) {
        let display = self.keep_display_awake;
        if let Err(error) = self.acquire("My coffee break", SessionSource::Manual, duration, display, None, None, true) {
            self.error = Some(error.to_string());
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn acquire(
        &mut self,
        title: &str,
        source: SessionSource,
        duration: Option<f64>,
        display: bool,
        process_id: Option<i32>,
        max_duration: Option<f64>,
        replacing_manual: bool,
    ) -> Result<CoffeeLease> {
        self.refresh();
        let title = title.trim();
        let length = title.chars().count();
        if length == 0 || length > 100 || title.chars().any(char::is_control) {
            return Err(CoffeeError::msg("Use a session name of 1–100 characters without line breaks."));
        }
        if let Some(duration) = duration {
            validate_duration(duration)?;
        }
        if let Some(max) = max_duration {
            validate_duration(max)?;
        }
        if matches!(source, SessionSource::Agent | SessionSource::Command) && duration.is_none() {
            return Err(CoffeeError::msg("Agent sessions need an expiry. Send heartbeats to renew them."));
        }
        if self.battery_blocked() {
            return Err(CoffeeError::msg(format!(
                "Battery protection is on. Connect power or charge above {}%.",
                self.battery_threshold
            )));
        }
        if self.sessions.len() >= MAX_SESSIONS && !(replacing_manual && self.manual_session().is_some()) {
            return Err(CoffeeError::msg("There are already 64 active sessions."));
        }
        let identity = process_id.and_then(|pid| (self.process_identity)(pid));
        if process_id.is_some() && identity.is_none() {
            return Err(CoffeeError::msg("That process is no longer running or cannot be inspected."));
        }
        let start = self.clock;
        let deadline = max_duration.map(|max| start + max);
        let mut expiry = duration.map(|d| start + d);
        if let Some(deadline) = deadline {
            expiry = Some(expiry.unwrap_or(deadline).min(deadline));
        }
        let lease = CoffeeLease {
            id: new_id(),
            title: title.to_string(),
            source,
            started_at: start,
            expires_at: expiry,
            hard_deadline: deadline,
            keep_display_awake: display,
            process_id,
            process_identity: identity,
            progress: None,
            message: None,
            last_heartbeat: start,
        };
        let mut existing: Vec<CoffeeLease> = if replacing_manual {
            self.sessions
                .iter()
                .filter(|s| s.source != SessionSource::Manual)
                .cloned()
                .collect()
        } else {
            self.sessions.clone()
        };
        let desired_display = display || existing.iter().any(|s| s.keep_display_awake);
        // Create the replacement power request before changing visible ownership.
        if self.assertion_display != Some(desired_display) {
            self.power.activate(desired_display).map_err(CoffeeError::Message)?;
            self.assertion_display = Some(desired_display);
        }
        // A replaced manual timer is simply superseded; it isn't recorded as ended.
        existing.push(lease.clone());
        self.sessions = existing;
        self.error = None;
        Ok(lease)
    }

    /// Revokes every agent turn's right to resume a lapsed lease, active or not.
    pub fn revoke_agent_turns(&mut self) {
        self.stop_token = new_id();
    }

    pub fn stop(&mut self, reason: EndReason) {
        // Revokes lapsed agent turns too, even when nothing is visibly active.
        self.revoke_agent_turns();
        let ids: Vec<String> = self.sessions.iter().map(|s| s.id.clone()).collect();
        self.finish(&ids, reason);
        self.error = None;
    }
    pub fn stop_manual(&mut self) {
        let ids: Vec<String> = self
            .sessions
            .iter()
            .filter(|s| s.source == SessionSource::Manual)
            .map(|s| s.id.clone())
            .collect();
        self.finish(&ids, EndReason::Stopped);
    }
    pub fn release(&mut self, id: &str) {
        self.finish(&[id.to_string()], EndReason::Released);
    }
    pub fn cancel(&mut self, id: &str) {
        self.finish(&[id.to_string()], EndReason::Stopped);
    }
    pub fn clear_history(&mut self) {
        self.history.clear();
        self.history_changed();
    }
    pub fn add_time(&mut self, seconds: f64) {
        let Some(id) = self.manual_session().map(|s| s.id.clone()) else {
            return;
        };
        if let Err(error) = self.extend(&id, seconds) {
            self.error = Some(error.to_string());
        }
    }

    pub fn extend(&mut self, id: &str, seconds: f64) -> Result<()> {
        self.refresh();
        validate_duration(seconds)?;
        let clock = self.clock;
        let Some(lease) = self.sessions.iter_mut().find(|s| s.id == id) else {
            return Err(CoffeeError::msg("Session has ended or has no timer."));
        };
        let Some(expiry) = lease.expires_at else {
            return Err(CoffeeError::msg("Session has ended or has no timer."));
        };
        let candidate = expiry + seconds;
        if candidate - clock > MAX_SECONDS {
            return Err(CoffeeError::msg("A session can have at most 24 hours remaining."));
        }
        lease.expires_at = Some(lease.hard_deadline.map_or(candidate, |deadline| deadline.min(candidate)));
        Ok(())
    }

    pub fn heartbeat(
        &mut self,
        id: &str,
        seconds: f64,
        progress: Option<f64>,
        message: Option<String>,
        restarting_cap: Option<f64>,
    ) -> Result<CoffeeLease> {
        self.refresh();
        validate_duration(seconds)?;
        if let Some(cap) = restarting_cap {
            validate_duration(cap)?;
        }
        if let Some(progress) = progress {
            if !progress.is_finite() || !(0.0..=1.0).contains(&progress) {
                return Err(CoffeeError::msg("Progress must be between 0 and 1."));
            }
        }
        if let Some(message) = &message {
            if message.chars().count() > 200 || message.chars().any(char::is_control) {
                return Err(CoffeeError::msg(
                    "Status messages must fit on one line and be at most 200 characters.",
                ));
            }
        }
        let clock = self.clock;
        let Some(lease) = self.sessions.iter_mut().find(|s| s.id == id && s.is_task()) else {
            return Err(CoffeeError::msg(
                "Agent session not found. It may have expired; acquire a new session.",
            ));
        };
        if let Some(cap) = restarting_cap {
            lease.hard_deadline = Some(clock + cap);
        }
        let candidate = clock + seconds;
        lease.expires_at = Some(lease.hard_deadline.map_or(candidate, |deadline| deadline.min(candidate)));
        lease.last_heartbeat = clock;
        if progress.is_some() {
            lease.progress = progress;
        }
        if message.is_some() {
            lease.message = message;
        }
        Ok(lease.clone())
    }

    pub fn refresh(&mut self) {
        self.clock = (self.now)();
        self.battery = (self.battery_provider)();
        if self.battery_blocked() {
            // Entering protection revokes lapsed turns even when no session is active.
            if !self.was_battery_blocked {
                self.revoke_agent_turns();
            }
            self.was_battery_blocked = true;
            let ids: Vec<String> = self.sessions.iter().map(|s| s.id.clone()).collect();
            self.finish(&ids, EndReason::BatteryLow);
            return;
        }
        self.was_battery_blocked = false;
        let exited: Vec<String> = self
            .sessions
            .iter()
            .filter(|lease| {
                lease
                    .process_id
                    .is_some_and(|pid| (self.process_identity)(pid) != lease.process_identity)
            })
            .map(|s| s.id.clone())
            .collect();
        self.finish(&exited, EndReason::ProcessExited);
        let clock = self.clock;
        let expired: Vec<String> = self
            .sessions
            .iter()
            .filter(|s| s.expires_at.is_some_and(|end| end <= clock))
            .map(|s| s.id.clone())
            .collect();
        self.finish(&expired, EndReason::Expired);
        self.reconcile_power();
    }

    pub fn snapshot(&mut self) -> SessionSnapshot {
        self.refresh();
        SessionSnapshot {
            version: VERSION.to_string(),
            awake: self.is_active(),
            display_awake: self.display_awake(),
            battery: self.battery,
            battery_threshold: self.battery_threshold,
            battery_blocked: self.battery_blocked(),
            sessions: self.sessions.clone(),
            history: self.history.clone(),
            error: self.error.clone(),
            stop_token: Some(self.stop_token.clone()),
        }
    }

    pub fn handle(&mut self, request: ControlRequest) -> ControlReply {
        match self.perform(request) {
            Ok(session) => ControlReply {
                ok: true,
                session,
                state: Some(self.snapshot()),
                error: None,
            },
            Err(error) => ControlReply {
                ok: false,
                session: None,
                state: Some(self.snapshot()),
                error: Some(error.to_string()),
            },
        }
    }

    fn perform(&mut self, request: ControlRequest) -> Result<Option<CoffeeLease>> {
        Ok(match request.action {
            ControlAction::Status | ControlAction::History => None,
            ControlAction::Acquire => {
                let source = if request.source == Some(SessionSource::Command) {
                    SessionSource::Command
                } else {
                    SessionSource::Agent
                };
                if let Some(token) = &request.stop_token {
                    if *token != self.stop_token {
                        return Err(CoffeeError::msg(
                            "Sessions were stopped since this turn began. Waiting for new user-initiated work.",
                        ));
                    }
                }
                Some(self.acquire(
                    request.title.as_deref().unwrap_or("Agent task"),
                    source,
                    Some(request.seconds.unwrap_or(300.0)),
                    request.display.unwrap_or(false),
                    request.process_id,
                    Some(request.max_seconds.unwrap_or(28800.0)),
                    false,
                )?)
            }
            ControlAction::Heartbeat => {
                let id = required_id(request.id.as_deref())?;
                Some(self.heartbeat(
                    &id,
                    request.seconds.unwrap_or(300.0),
                    request.progress,
                    request.message,
                    request.max_seconds,
                )?)
            }
            ControlAction::Release => {
                let id = required_id(request.id.as_deref())?;
                self.release(&id);
                None
            }
            ControlAction::Extend => {
                let Some(seconds) = request.seconds else {
                    return Err(CoffeeError::msg("Specify how many seconds to add."));
                };
                let id = required_id(request.id.as_deref())?;
                self.extend(&id, seconds)?;
                self.sessions.iter().find(|s| s.id == id).cloned()
            }
            ControlAction::StopAll => {
                self.stop(EndReason::Stopped);
                None
            }
        })
    }

    fn finish(&mut self, ids: &[String], reason: EndReason) {
        if ids.is_empty() {
            return;
        }
        let finished: Vec<CoffeeLease> = self.sessions.iter().filter(|s| ids.contains(&s.id)).cloned().collect();
        if finished.is_empty() {
            return;
        }
        if matches!(
            reason,
            EndReason::Stopped | EndReason::BatteryLow | EndReason::AppQuit | EndReason::PowerError
        ) {
            self.stop_token = new_id();
        }
        self.sessions.retain(|s| !ids.contains(&s.id));
        for lease in finished {
            let completion = SessionCompletion {
                id: lease.id,
                title: lease.title,
                source: lease.source,
                started_at: lease.started_at,
                ended_at: self.clock,
                reason,
            };
            self.history.insert(0, completion.clone());
            if let Some(callback) = self.on_completion.as_mut() {
                callback(&completion);
            }
        }
        self.history.truncate(20);
        self.history_changed();
        self.reconcile_power();
    }

    fn history_changed(&mut self) {
        if let Some(callback) = self.on_history_change.as_mut() {
            callback(&self.history);
        }
    }

    fn reconcile_power(&mut self) {
        if self.sessions.is_empty() {
            if self.assertion_display.is_some() {
                self.power.deactivate();
            }
            self.assertion_display = None;
            return;
        }
        let display = self.sessions.iter().any(|s| s.keep_display_awake);
        if self.assertion_display == Some(display) {
            return;
        }
        match self.power.activate(display) {
            Ok(()) => {
                self.assertion_display = Some(display);
                if self.power_retry_pending {
                    self.error = None;
                    self.power_retry_pending = false;
                }
            }
            Err(error) => {
                // The previous request is still held: keep it (and every session) rather than
                // dropping all protection, and retry on the next refresh.
                if self.assertion_display.is_some() {
                    self.error = Some(error);
                    self.power_retry_pending = true;
                    return;
                }
                // Nothing is held: end every session the normal way (history, notifications,
                // stop token), which also leaves power released.
                self.power.deactivate();
                self.assertion_display = None;
                let ids: Vec<String> = self.sessions.iter().map(|s| s.id.clone()).collect();
                self.finish(&ids, EndReason::PowerError);
                self.error = Some(error);
            }
        }
    }
}

fn validate_duration(duration: f64) -> Result<()> {
    if duration.is_finite() && duration > 0.0 && duration <= MAX_SECONDS {
        Ok(())
    } else {
        Err(CoffeeError::msg(
            "Duration must be greater than zero and at most 86,400 seconds (24 hours).",
        ))
    }
}

fn required_id(id: Option<&str>) -> Result<String> {
    match id {
        Some(id) if is_uuid(id) => Ok(id.to_string()),
        _ => Err(CoffeeError::msg("Provide the session ID returned by acquire.")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::power::FakePower;
    use std::sync::{Arc, Mutex};

    struct Harness {
        time: Arc<Mutex<f64>>,
        battery: Arc<Mutex<Option<BatteryState>>>,
        alive: Arc<Mutex<Vec<i32>>>,
        power: FakePower,
    }

    fn session() -> (AwakeSession, Harness) {
        let time = Arc::new(Mutex::new(1_000_000.0));
        let battery = Arc::new(Mutex::new(None));
        let alive = Arc::new(Mutex::new(vec![100]));
        let power = FakePower::default();
        let (t, b, a) = (time.clone(), battery.clone(), alive.clone());
        let session = AwakeSession::with_providers(
            Box::new(power.clone()),
            Box::new(move || *t.lock().unwrap()),
            Box::new(move |pid| a.lock().unwrap().contains(&pid).then(|| format!("start-{pid}"))),
            Box::new(move || *b.lock().unwrap()),
        );
        (
            session,
            Harness {
                time,
                battery,
                alive,
                power,
            },
        )
    }

    impl Harness {
        fn advance(&self, seconds: f64) {
            *self.time.lock().unwrap() += seconds;
        }
    }

    fn agent(session: &mut AwakeSession, title: &str, seconds: f64) -> CoffeeLease {
        session
            .acquire(title, SessionSource::Agent, Some(seconds), false, None, Some(28800.0), false)
            .unwrap()
    }

    #[test]
    fn timers_expire_and_release_power() {
        let (mut session, harness) = session();
        session.start(Some(60.0));
        assert!(session.is_active());
        assert_eq!(harness.power.state(), Some(true));
        harness.advance(61.0);
        session.refresh();
        assert!(!session.is_active());
        assert_eq!(harness.power.state(), None);
        assert_eq!(session.history()[0].reason, EndReason::Expired);
    }

    #[test]
    fn independent_owners_and_display_aggregation() {
        let (mut session, harness) = session();
        let build = agent(&mut session, "Build", 300.0);
        let show = session
            .acquire("Slides", SessionSource::Agent, Some(300.0), true, None, None, false)
            .unwrap();
        assert_eq!(harness.power.state(), Some(true));
        session.release(&show.id);
        assert_eq!(harness.power.state(), Some(false));
        assert_eq!(session.sessions().len(), 1);
        assert_eq!(session.sessions()[0].id, build.id);
    }

    #[test]
    fn heartbeats_renew_within_the_hard_cap() {
        let (mut session, harness) = session();
        let lease = session
            .acquire("Agent", SessionSource::Agent, Some(100.0), false, None, Some(150.0), false)
            .unwrap();
        harness.advance(90.0);
        let renewed = session
            .heartbeat(&lease.id, 100.0, Some(0.5), Some("Halfway".into()), None)
            .unwrap();
        assert_eq!(renewed.expires_at, lease.hard_deadline);
        assert_eq!(renewed.progress, Some(0.5));
        harness.advance(61.0);
        session.refresh();
        assert!(!session.is_active(), "the hard cap ends the session");
        assert!(
            session.heartbeat(&lease.id, 100.0, None, None, None).is_err(),
            "expired leases can't be revived"
        );
        assert!(session.heartbeat(&lease.id, 100.0, Some(2.0), None, None).is_err());
    }

    #[test]
    fn restarting_the_cap_moves_the_deadline() {
        let (mut session, harness) = session();
        let lease = session
            .acquire("Agent", SessionSource::Agent, Some(100.0), false, None, Some(150.0), false)
            .unwrap();
        harness.advance(90.0);
        let renewed = session.heartbeat(&lease.id, 100.0, None, None, Some(1000.0)).unwrap();
        assert_eq!(
            renewed.expires_at,
            Some(session.clock() + 100.0),
            "a new turn's cap allows the full lease"
        );
        assert_eq!(renewed.hard_deadline, Some(session.clock() + 1000.0));
    }

    #[test]
    fn watched_process_exit_ends_its_session() {
        let (mut session, harness) = session();
        let lease = session
            .acquire("Watched", SessionSource::Agent, Some(300.0), false, Some(100), None, false)
            .unwrap();
        assert!(session
            .acquire("Gone", SessionSource::Agent, Some(300.0), false, Some(7), None, false)
            .is_err());
        harness.alive.lock().unwrap().clear();
        session.refresh();
        assert_eq!(session.history()[0].id, lease.id);
        assert_eq!(session.history()[0].reason, EndReason::ProcessExited);
    }

    #[test]
    fn battery_protection_stops_everything_and_revokes_turns() {
        let (mut session, harness) = session();
        agent(&mut session, "Agent", 300.0);
        let token = session.stop_token().to_string();
        *harness.battery.lock().unwrap() = Some(BatteryState {
            percent: 15,
            on_battery: true,
        });
        session.refresh();
        assert!(!session.is_active());
        assert_ne!(session.stop_token(), token);
        assert!(session
            .acquire("Again", SessionSource::Agent, Some(60.0), false, None, None, false)
            .is_err());
        *harness.battery.lock().unwrap() = Some(BatteryState {
            percent: 15,
            on_battery: false,
        });
        assert!(session
            .acquire("Plugged in", SessionSource::Agent, Some(60.0), false, None, None, false)
            .is_ok());
    }

    #[test]
    fn stop_tokens_guard_resumed_turns() {
        let (mut session, _harness) = session();
        let token = session.stop_token().to_string();
        let request = |token: &str| ControlRequest {
            stop_token: Some(token.into()),
            title: Some("Turn".into()),
            ..ControlRequest::new(ControlAction::Acquire)
        };
        assert!(session.handle(request(&token)).ok);
        session.stop(EndReason::Stopped);
        assert!(!session.handle(request(&token)).ok, "a user stop revokes the turn");
    }

    #[test]
    fn validation() {
        let (mut session, _harness) = session();
        assert!(session
            .acquire("", SessionSource::Agent, Some(1.0), false, None, None, false)
            .is_err());
        assert!(session
            .acquire("a\nb", SessionSource::Agent, Some(1.0), false, None, None, false)
            .is_err());
        assert!(session
            .acquire(&"x".repeat(101), SessionSource::Agent, Some(1.0), false, None, None, false)
            .is_err());
        assert!(session
            .acquire("Agent", SessionSource::Agent, None, false, None, None, false)
            .is_err());
        assert!(session
            .acquire("Agent", SessionSource::Agent, Some(-1.0), false, None, None, false)
            .is_err());
        assert!(session
            .acquire("Agent", SessionSource::Agent, Some(86401.0), false, None, None, false)
            .is_err());
        assert!(!session.handle(ControlRequest::with_id(ControlAction::Release, "not-a-uuid")).ok);
        for index in 0..64 {
            agent(&mut session, &format!("Agent {index}"), 60.0);
        }
        assert!(session
            .acquire("One too many", SessionSource::Agent, Some(60.0), false, None, None, false)
            .is_err());
    }

    #[test]
    fn power_failure_ends_sessions_cleanly() {
        let (mut session, harness) = session();
        harness.power.fail(true);
        session.start(Some(60.0));
        assert!(!session.is_active());
        assert!(session.error.is_some());
        harness.power.fail(false);
        let lease = agent(&mut session, "Build", 300.0);
        harness.power.fail(true);
        // Switching to display-on fails, but the system request already held is kept.
        session
            .acquire("Slides", SessionSource::Agent, Some(300.0), true, None, None, false)
            .unwrap_err();
        assert_eq!(session.sessions()[0].id, lease.id);
        assert_eq!(harness.power.state(), Some(false));
    }

    #[test]
    fn manual_timer_extends_and_formats() {
        let (mut session, harness) = session();
        session.start(Some(600.0));
        session.add_time(900.0);
        assert_eq!(session.countdown_label(), "25m");
        harness.advance(1.0);
        session.start(None);
        assert_eq!(session.countdown_label(), "∞");
        assert_eq!(session.sessions().len(), 1, "a new timer replaces your old one");
        assert_eq!(AwakeSession::format_time(3725.0), "1:02:05");
        assert_eq!(AwakeSession::format_time(65.2), "01:06");
    }
}
