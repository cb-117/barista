//! Limit alerts: which notifications to send as a plan's limit windows fill, and what to remember
//! so each is sent once. Pure: the app sends the notifications and saves the state.
//! docs/USAGE.md, "Limit alerts", is the specification.
use crate::models::parse_date;
use chrono::{DateTime, Local, TimeZone};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

/// A plan is looked at only if it was observed within this many seconds, and a reset is announced
/// only within this many seconds after it.
const RECENT: f64 = 3600.0;

/// What is remembered about one window.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Slot {
    /// The window's reset time (Unix seconds), as last reported.
    pub resets_at: f64,
    /// The highest level already notified: 0, 1 (pace), 80, 95, or 100.
    pub level: u8,
    /// The window got to 90% or more, so its reset is worth announcing.
    pub hot: bool,
    /// Whose window it is and its label, for the reset notice (the window may be gone by then).
    pub who: String,
    pub label: String,
}

/// What is remembered, per slot (`<agent>|<window id>`).
pub type State = BTreeMap<String, Slot>;

/// What a notification is about.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Notice {
    /// 1 (on pace to run out), 80, 95, or 100 (percent used).
    Level(u8),
    /// A window that got hot has reset.
    Reset,
}

/// One notification to send.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Alert {
    pub slot: String,
    pub notice: Notice,
    pub title: String,
    pub body: String,
}

/// "at 3:10 PM" when under 24 hours away, else a date ("on Nov 14").
fn when<Tz: TimeZone>(seconds: f64, now: f64, zone: &Tz) -> String
where
    Tz::Offset: std::fmt::Display,
{
    let at = DateTime::from_timestamp(seconds as i64, 0).unwrap_or_default().with_timezone(zone);
    if seconds - now < 86400.0 {
        format!("at {}", at.format("%-I:%M %p"))
    } else {
        format!("on {}", at.format("%b %-d"))
    }
}

/// A window's level: 100, 95, or 80 by how full it is, else 1 when it is on pace to run out.
fn level_of(used: f64, pace: Option<&str>, limit_at: Option<f64>) -> u8 {
    if used >= 99.5 {
        100
    } else if used >= 95.0 {
        95
    } else if used >= 80.0 {
        80
    } else if used >= 40.0 && pace == Some("over") && limit_at.is_some() {
        1
    } else {
        0
    }
}

/// The notifications due for the report's `limits` at `now` (local times), and what to remember.
pub fn evaluate(limits: &Value, state: &State, now: f64) -> (Vec<Alert>, State) {
    evaluate_in(limits, state, now, &Local)
}

/// `evaluate`, writing times in `zone`.
pub fn evaluate_in<Tz: TimeZone>(limits: &Value, state: &State, now: f64, zone: &Tz) -> (Vec<Alert>, State)
where
    Tz::Offset: std::fmt::Display,
{
    let mut alerts = Vec::new();
    // The windows still running in this report.
    struct Running<'a> {
        key: String,
        who: &'a str,
        window: &'a Value,
        minutes: f64,
        resets_at: f64,
    }
    let mut running = Vec::new();
    for plan in limits.as_array().into_iter().flatten() {
        if plan["available"].as_bool() != Some(true) {
            continue;
        }
        let Some(observed) = plan["observedAt"].as_str().and_then(parse_date) else {
            continue;
        };
        if now - observed > RECENT {
            continue;
        }
        let agent = plan["agent"].as_str().unwrap_or("");
        let who = match agent {
            "claude" => "Claude",
            "codex" => "ChatGPT",
            _ => plan["name"].as_str().unwrap_or(agent),
        };
        for window in plan["windows"].as_array().into_iter().flatten() {
            let (Some(id), Some(minutes), Some(resets_at)) = (
                window["id"].as_str(),
                window["windowMinutes"].as_f64().filter(|minutes| *minutes > 0.0),
                window["resetsAt"].as_str().and_then(parse_date),
            ) else {
                continue;
            };
            if window["reset"].as_bool() == Some(true) || resets_at <= now {
                continue;
            }
            let key = format!("{agent}|{id}");
            // One window per slot: a second with the same agent and id would fight the first over
            // what is remembered.
            if running.iter().any(|item: &Running| item.key == key) {
                continue;
            }
            running.push(Running {
                key,
                who,
                window,
                minutes,
                resets_at,
            });
        }
    }
    // The same window as remembered, unless its reset moved on by half a window or more (reset
    // times can jitter by a second between reports).
    let same_window = |slot: &Slot, item: &Running| item.resets_at - slot.resets_at < item.minutes * 30.0;

    let mut next = State::new();
    // Windows whose reset has passed: announce the ones that got hot, then forget them. One the
    // report still shows running (its reset time was corrected to a moment later) hasn't reset.
    for (key, slot) in state {
        if slot.resets_at > now || running.iter().any(|item| &item.key == key && same_window(slot, item)) {
            next.insert(key.clone(), slot.clone());
        } else if slot.hot && now - slot.resets_at <= RECENT {
            alerts.push(Alert {
                slot: key.clone(),
                notice: Notice::Reset,
                title: format!("{} {} limit reset", slot.who, slot.label),
                body: "A fresh window has started.".into(),
            });
        }
    }
    for item in &running {
        let (key, who, window, resets_at) = (item.key.clone(), item.who, item.window, item.resets_at);
        let label = window["label"].as_str().unwrap_or("");
        let used = window["usedPercent"].as_f64().unwrap_or(0.0);
        let limit_at = window["limitAt"].as_str().and_then(parse_date);
        let mut slot = match next.get(&key) {
            Some(slot) if same_window(slot, item) => slot.clone(),
            _ => Slot::default(),
        };
        slot.resets_at = resets_at;
        slot.who = who.to_string();
        slot.label = label.to_string();
        let level = level_of(used, window["pace"].as_str(), limit_at);
        if level > slot.level {
            slot.level = level;
            let percent = used.round() as i64;
            let resets = when(resets_at, now, zone);
            let runs_out = limit_at.map(|at| when(at, now, zone));
            let (title, body) = match (level, runs_out) {
                (100, _) => (format!("{who} {label} limit reached"), format!("It resets {resets}.")),
                (1, Some(out)) => (
                    format!("{who} {label} limit is running hot"),
                    format!("{percent}% used. At this pace it runs out {out}, before it resets {resets}."),
                ),
                (_, Some(out)) => (
                    format!("{who} {label} limit: {percent}% used"),
                    format!("At this pace it runs out {out}. It resets {resets}."),
                ),
                (_, None) => (format!("{who} {label} limit: {percent}% used"), format!("It resets {resets}.")),
            };
            alerts.push(Alert {
                slot: key.clone(),
                notice: Notice::Level(level),
                title,
                body,
            });
        }
        if used >= 90.0 {
            slot.hot = true;
        }
        next.insert(key, slot);
    }
    (alerts, next)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::format_date;
    use chrono::Utc;
    use serde_json::json;

    /// 2026-11-09 12:00:00 UTC.
    const NOW: f64 = 1_794_225_600.0;
    const HOUR: f64 = 3600.0;
    const DAY: f64 = 86400.0;

    fn window(id: &str, label: &str, minutes: u64, used: f64, resets_at: f64) -> Value {
        json!({"id": id, "label": label, "windowMinutes": minutes, "usedPercent": used,
               "resetsAt": format_date(resets_at), "reset": false,
               "elapsedPercent": null, "projectedPercent": null, "limitAt": null, "pace": null})
    }

    fn on_pace_to_run_out(mut window: Value, limit_at: f64) -> Value {
        window["pace"] = json!("over");
        window["limitAt"] = json!(format_date(limit_at));
        window
    }

    fn plan(agent: &str, observed_at: f64, windows: Vec<Value>) -> Value {
        let name = match agent {
            "claude" => "Claude plan",
            "codex" => "ChatGPT plan",
            other => other,
        };
        json!({"agent": agent, "name": name, "available": true, "source": "test",
               "observedAt": format_date(observed_at), "plan": null, "credits": null, "windows": windows})
    }

    fn session(used: f64, resets_at: f64) -> Value {
        json!([plan("claude", NOW, vec![window("five_hour", "5-hour", 300, used, resets_at)])])
    }

    fn check(limits: &Value, state: &State, now: f64) -> (Vec<Alert>, State) {
        evaluate_in(limits, state, now, &Utc)
    }

    fn notices(alerts: &[Alert]) -> Vec<Notice> {
        alerts.iter().map(|alert| alert.notice).collect()
    }

    #[test]
    fn each_level_is_sent_once_per_window() {
        let resets = NOW + 2.0 * HOUR;
        let mut state = State::new();
        let mut sent = Vec::new();
        for used in [10.0, 79.9, 80.0, 85.0, 94.9, 95.0, 97.0, 99.4, 99.5, 100.0, 100.0] {
            let (alerts, next) = check(&session(used, resets), &state, NOW);
            sent.extend(notices(&alerts));
            state = next;
        }
        assert_eq!(sent, [Notice::Level(80), Notice::Level(95), Notice::Level(100)]);
        let slot = &state["claude|five_hour"];
        assert_eq!((slot.level, slot.hot, slot.resets_at), (100, true, resets));
        // Falling back (a corrected report) never repeats a level.
        let (alerts, state) = check(&session(81.0, resets), &state, NOW);
        assert!(alerts.is_empty());
        assert_eq!(state["claude|five_hour"].level, 100);
    }

    #[test]
    fn notifications_use_the_specified_wording() {
        let resets = NOW + 2.0 * HOUR; // 2:00 PM
        let (alerts, _) = check(&session(83.6, resets), &State::new(), NOW);
        assert_eq!(alerts[0].slot, "claude|five_hour");
        assert_eq!(alerts[0].title, "Claude 5-hour limit: 84% used");
        assert_eq!(alerts[0].body, "It resets at 2:00 PM.");
        let paced = json!([plan(
            "codex",
            NOW,
            vec![on_pace_to_run_out(
                window("primary", "Weekly", 10080, 96.0, NOW + 3.0 * DAY),
                NOW + 5.0 * HOUR + 600.0
            )]
        )]);
        let (alerts, _) = check(&paced, &State::new(), NOW);
        assert_eq!(alerts[0].title, "ChatGPT Weekly limit: 96% used");
        assert_eq!(alerts[0].body, "At this pace it runs out at 5:10 PM. It resets on Nov 12.");
        let (alerts, _) = check(&session(99.7, resets), &State::new(), NOW);
        assert_eq!(
            (alerts[0].title.as_str(), alerts[0].body.as_str()),
            ("Claude 5-hour limit reached", "It resets at 2:00 PM.")
        );
        // Another agent goes by its plan's name.
        let other = json!([plan(
            "cursor",
            NOW,
            vec![window("monthly", "30-day", 43200, 80.0, NOW + 10.0 * DAY)]
        )]);
        let (alerts, state) = check(&other, &State::new(), NOW);
        assert_eq!(alerts[0].title, "cursor 30-day limit: 80% used");
        assert!(state.contains_key("cursor|monthly"));
    }

    #[test]
    fn a_jump_sends_only_the_highest_level() {
        let resets = NOW + 2.0 * HOUR;
        let (_, state) = check(&session(70.0, resets), &State::new(), NOW);
        assert_eq!(state["claude|five_hour"].level, 0);
        let (alerts, state) = check(&session(96.0, resets), &state, NOW);
        assert_eq!(notices(&alerts), [Notice::Level(95)]);
        assert_eq!(alerts[0].title, "Claude 5-hour limit: 96% used");
        let (alerts, _) = check(&session(96.0, resets), &state, NOW);
        assert!(alerts.is_empty());
        // Straight to full from nothing remembered: only "reached".
        let (alerts, _) = check(&session(100.0, resets), &State::new(), NOW);
        assert_eq!(notices(&alerts), [Notice::Level(100)]);
    }

    #[test]
    fn the_pace_alert_needs_forty_percent_and_a_run_out_time() {
        let resets = NOW + 2.5 * HOUR;
        let hot = |used: f64| {
            json!([plan(
                "claude",
                NOW,
                vec![on_pace_to_run_out(
                    window("five_hour", "5-hour", 300, used, resets),
                    NOW + 100.0 * 60.0
                )]
            )])
        };
        // Under 40% used, the pace alone says nothing yet.
        let (alerts, state) = check(&hot(39.0), &State::new(), NOW);
        assert!(alerts.is_empty());
        let (alerts, state) = check(&hot(60.0), &state, NOW);
        assert_eq!(notices(&alerts), [Notice::Level(1)]);
        assert_eq!(alerts[0].title, "Claude 5-hour limit is running hot");
        assert_eq!(
            alerts[0].body,
            "60% used. At this pace it runs out at 1:40 PM, before it resets at 2:30 PM."
        );
        // Once, then the percentage levels follow as it fills.
        let (alerts, state) = check(&hot(65.0), &state, NOW);
        assert!(alerts.is_empty());
        let (alerts, state) = check(&hot(82.0), &state, NOW);
        assert_eq!(notices(&alerts), [Notice::Level(80)]);
        assert_eq!(alerts[0].body, "At this pace it runs out at 1:40 PM. It resets at 2:30 PM.");
        // After a higher level, the pace alert is not sent late.
        let (alerts, _) = check(&hot(60.0), &state, NOW);
        assert!(alerts.is_empty());
        // Over pace without a run-out time (it runs out at the reset), or a calm pace: nothing.
        let mut no_time = window("five_hour", "5-hour", 300, 60.0, resets);
        no_time["pace"] = json!("over");
        let mut calm = window("seven_day", "Weekly", 10080, 60.0, NOW + 2.0 * DAY);
        calm["pace"] = json!("ok");
        let (alerts, _) = check(&json!([plan("claude", NOW, vec![no_time, calm])]), &State::new(), NOW);
        assert!(alerts.is_empty());
    }

    #[test]
    fn a_new_window_starts_again() {
        let resets = NOW + 2.0 * HOUR;
        let (alerts, state) = check(&session(96.0, resets), &State::new(), NOW);
        assert_eq!(notices(&alerts), [Notice::Level(95)]);
        // The same slot reports a reset time exactly half a window (150 minutes) later: a new window.
        let later = resets + 150.0 * 60.0;
        let (alerts, state) = check(&session(85.0, later), &state, NOW);
        assert_eq!(notices(&alerts), [Notice::Level(80)]);
        let slot = &state["claude|five_hour"];
        assert_eq!((slot.level, slot.hot, slot.resets_at), (80, false, later));
        // One second short of half a window is still the same window.
        let (_, state) = check(&session(96.0, resets), &State::new(), NOW);
        let (alerts, state) = check(&session(85.0, later - 1.0), &state, NOW);
        assert!(alerts.is_empty());
        assert_eq!((state["claude|five_hour"].level, state["claude|five_hour"].hot), (95, true));
    }

    #[test]
    fn reset_time_jitter_does_not_notify_again() {
        let resets = NOW + 2.0 * HOUR;
        let (alerts, mut state) = check(&session(96.0, resets), &State::new(), NOW);
        assert_eq!(alerts.len(), 1);
        for jitter in [1.0, -1.0, 2.0, 0.0, -2.0] {
            let (alerts, next) = check(&session(96.0, resets + jitter), &state, NOW + 60.0);
            assert!(alerts.is_empty(), "jitter {jitter}");
            assert_eq!(
                next["claude|five_hour"].resets_at,
                resets + jitter,
                "the current time is remembered"
            );
            assert_eq!((next["claude|five_hour"].level, next["claude|five_hour"].hot), (95, true));
            state = next;
        }
    }

    #[test]
    fn reset_time_jitter_across_the_reset_is_the_same_window() {
        let resets = NOW + 600.0;
        let (alerts, state) = check(&session(96.0, resets), &State::new(), NOW);
        assert_eq!(alerts.len(), 1);
        // The remembered reset time has just passed, and the report now says a second later.
        let limits = json!([plan("claude", resets, vec![window("five_hour", "5-hour", 300, 96.0, resets + 1.0)])]);
        let (alerts, state) = check(&limits, &state, resets);
        assert!(alerts.is_empty(), "{alerts:?}");
        assert_eq!((state["claude|five_hour"].level, state["claude|five_hour"].hot), (95, true));
        // The real reset is still announced, once, and the next window starts again.
        let later = resets + 300.0;
        let limits = json!([plan(
            "claude",
            later,
            vec![window("five_hour", "5-hour", 300, 2.0, later + 17700.0)]
        )]);
        let (alerts, state) = check(&limits, &state, later);
        assert_eq!(alerts.iter().map(|alert| alert.notice).collect::<Vec<_>>(), vec![Notice::Reset]);
        assert_eq!((state["claude|five_hour"].level, state["claude|five_hour"].hot), (0, false));
    }

    #[test]
    fn only_the_first_window_of_a_slot_counts() {
        let resets = NOW + 600.0;
        let (_, state) = check(&session(96.0, resets), &State::new(), NOW);
        // Two plans report the same agent and window id: the first is used, so the 95% notice
        // neither repeats nor is forgotten.
        let limits = json!([
            plan("claude", resets, vec![window("five_hour", "5-hour", 300, 96.0, resets + 1.0)]),
            plan("claude", resets, vec![window("five_hour", "5-hour", 300, 2.0, resets + 18000.0)]),
        ]);
        let (alerts, state) = check(&limits, &state, resets);
        assert!(alerts.is_empty(), "{alerts:?}");
        let (alerts, state) = check(&limits, &state, resets);
        assert!(alerts.is_empty(), "{alerts:?}");
        assert_eq!((state["claude|five_hour"].level, state["claude|five_hour"].hot), (95, true));
    }

    #[test]
    fn a_reset_is_announced_only_for_hot_windows_within_the_hour() {
        let resets = NOW + 2.0 * HOUR;
        let limits = json!([plan(
            "claude",
            NOW,
            vec![
                window("five_hour", "5-hour", 300, 92.0, resets),
                window("seven_day", "Weekly", 10080, 85.0, resets),
            ]
        )]);
        let (alerts, state) = check(&limits, &State::new(), NOW);
        assert_eq!(notices(&alerts), [Notice::Level(80), Notice::Level(80)]);
        assert!(state["claude|five_hour"].hot && !state["claude|seven_day"].hot);
        // Before the reset: nothing, and both stay remembered (even with no limits reported).
        let (alerts, kept) = check(&json!([]), &state, resets - 1.0);
        assert!(alerts.is_empty());
        assert_eq!(kept, state);
        // At the reset, and up to an hour after it: only the hot window; then both are forgotten.
        for after in [0.0, 60.0, HOUR] {
            let (alerts, next) = check(&json!([]), &state, resets + after);
            assert_eq!(notices(&alerts), [Notice::Reset], "{after}s after the reset");
            assert_eq!(alerts[0].slot, "claude|five_hour");
            assert_eq!(alerts[0].title, "Claude 5-hour limit reset");
            assert_eq!(alerts[0].body, "A fresh window has started.");
            assert!(next.is_empty());
        }
        // More than an hour after it (the computer was asleep): forgotten without a word.
        let (alerts, next) = check(&json!([]), &state, resets + HOUR + 1.0);
        assert!(alerts.is_empty() && next.is_empty());
        // The report still showing the passed window (as `reset`) doesn't bring it back, and the
        // fresh window that follows starts at level 0.
        let mut stale = window("five_hour", "5-hour", 300, 0.0, resets);
        stale["reset"] = json!(true);
        let (alerts, next) = check(&json!([plan("claude", resets, vec![stale])]), &state, resets + 60.0);
        assert_eq!(notices(&alerts), [Notice::Reset]);
        assert!(next.is_empty());
        let fresh = json!([plan(
            "claude",
            resets + 60.0,
            vec![window("five_hour", "5-hour", 300, 3.0, resets + 5.0 * HOUR)]
        )]);
        let (alerts, next) = check(&fresh, &state, resets + 60.0);
        assert_eq!(notices(&alerts), [Notice::Reset]);
        assert_eq!((next["claude|five_hour"].level, next["claude|five_hour"].hot), (0, false));
    }

    #[test]
    fn plans_observed_over_an_hour_ago_are_skipped() {
        let resets = NOW + 2.0 * HOUR;
        let observed = |age: f64| json!([plan("claude", NOW - age, vec![window("five_hour", "5-hour", 300, 96.0, resets)])]);
        let (alerts, state) = check(&observed(HOUR + 1.0), &State::new(), NOW);
        assert!(alerts.is_empty() && state.is_empty());
        let (alerts, state) = check(&observed(HOUR), &state, NOW);
        assert_eq!(notices(&alerts), [Notice::Level(95)]);
        assert_eq!(state.len(), 1);
        // Unavailable plans, and windows with no length, no reset time, or already reset, too.
        let mut unavailable = plan("codex", NOW, vec![window("primary", "Weekly", 10080, 96.0, resets)]);
        unavailable["available"] = json!(false);
        let mut no_length = window("extra_usage", "Extra usage", 0, 96.0, resets);
        no_length["windowMinutes"] = Value::Null;
        let mut no_reset = window("five_hour", "5-hour", 300, 96.0, resets);
        no_reset["resetsAt"] = Value::Null;
        let mut reset = window("seven_day", "Weekly", 10080, 96.0, NOW - 60.0);
        reset["reset"] = json!(true);
        let limits = json!([unavailable, plan("claude", NOW, vec![no_length, no_reset, reset])]);
        let (alerts, state) = check(&limits, &State::new(), NOW);
        assert!(alerts.is_empty() && state.is_empty());
    }

    #[test]
    fn times_are_a_clock_time_within_a_day_else_a_date() {
        assert_eq!(when(NOW + 3.0 * HOUR + 600.0, NOW, &Utc), "at 3:10 PM");
        assert_eq!(when(NOW + DAY - 1.0, NOW, &Utc), "at 11:59 AM");
        assert_eq!(when(NOW + DAY, NOW, &Utc), "on Nov 10");
        assert_eq!(when(NOW + 5.0 * DAY, NOW, &Utc), "on Nov 14");
    }

    #[test]
    fn the_remembered_state_survives_saving() {
        let (_, state) = check(&session(96.0, NOW + 2.0 * HOUR), &State::new(), NOW);
        let saved = serde_json::to_value(&state).unwrap();
        assert_eq!(
            saved,
            json!({"claude|five_hour": {"resetsAt": NOW + 2.0 * HOUR, "level": 95, "hot": true, "who": "Claude", "label": "5-hour"}})
        );
        assert_eq!(serde_json::from_value::<State>(saved).unwrap(), state);
        // Older or partial saved state still loads.
        let partial: State = serde_json::from_value(json!({"codex|primary": {"level": 80}})).unwrap();
        assert_eq!(partial["codex|primary"].level, 80);
    }
}
