//! Saved preferences and session history, in the user's config folder.
use barista_core::SessionCompletion;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Prefs {
    pub keep_display_awake: bool,
    pub battery_threshold: i32,
    pub notifications_enabled: bool,
    pub agent_access: bool,
    pub checks_for_updates: bool,
    pub show_countdown: bool,
    pub last_update_check: f64,
    pub dismissed_update: Option<String>,
    pub launch_at_login_choice_made: bool,
    /// Your last timer, in seconds; -1 means "until stopped".
    pub last_session_seconds: f64,
    /// Keep awake while the Claude app (Cowork) or the ChatGPT app is busy.
    pub watch_cowork: bool,
    pub watch_chatgpt: bool,
    /// Ask Anthropic for the Claude plan's limits with Claude Code's saved login.
    pub claude_account_limits: bool,
    /// Ask ChatGPT for the ChatGPT plan's limits with Codex's saved login.
    pub codex_account_limits: bool,
    /// Notifications as a plan's limit windows fill (docs/USAGE.md, "Limit alerts").
    pub limit_alerts: bool,
    /// What limit alerts remember per window, so each is sent once.
    pub limit_alert_state: barista_core::limit_alerts::State,
    /// A notification on Mondays with last week's active time, projects, and estimated cost.
    pub weekly_summary: bool,
    /// The Monday (YYYY-MM-DD) of the last week summarized, so each week is sent once.
    pub weekly_summary_sent: Option<String>,
    /// Help count active users: one anonymous ping a day (docs/ACTIVE-USERS.md).
    pub share_active_count: bool,
    /// The UTC date (YYYY-MM-DD) of the last ping that got through, so each day is counted once.
    pub last_active_ping: Option<String>,
}

impl Default for Prefs {
    fn default() -> Self {
        Self {
            keep_display_awake: true,
            battery_threshold: 20,
            notifications_enabled: false,
            agent_access: true,
            checks_for_updates: true,
            show_countdown: true,
            last_update_check: 0.0,
            dismissed_update: None,
            launch_at_login_choice_made: false,
            last_session_seconds: 1800.0,
            watch_cowork: true,
            watch_chatgpt: true,
            claude_account_limits: true,
            codex_account_limits: true,
            limit_alerts: true,
            limit_alert_state: Default::default(),
            weekly_summary: true,
            weekly_summary_sent: None,
            share_active_count: true,
            last_active_ping: None,
        }
    }
}

pub fn directory() -> PathBuf {
    let base = dirs::config_dir().unwrap_or_else(std::env::temp_dir);
    base.join(if cfg!(windows) { "Barista" } else { "barista" })
}

fn read<T: for<'de> Deserialize<'de>>(name: &str) -> Option<T> {
    serde_json::from_slice(&std::fs::read(directory().join(name)).ok()?).ok()
}

fn write<T: Serialize>(name: &str, value: &T) {
    let directory = directory();
    let _ = std::fs::create_dir_all(&directory);
    if let Ok(text) = serde_json::to_vec_pretty(value) {
        let _ = barista_core::hooks::write_atomically(&directory.join(name), &text);
    }
}

impl Prefs {
    pub fn load() -> Self {
        read("settings.json").unwrap_or_default()
    }
    pub fn save(&self) {
        write("settings.json", self);
    }
}

pub fn load_history() -> Vec<SessionCompletion> {
    read("history.json").unwrap_or_default()
}

pub fn save_history(history: &[SessionCompletion]) {
    write("history.json", &history);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_saved_by_older_versions_still_load() {
        // Before the active-user count: neither of its keys is in the file.
        let old: Prefs = serde_json::from_str(r#"{"checksForUpdates": false, "batteryThreshold": 30}"#).unwrap();
        assert!(!old.checks_for_updates);
        assert_eq!(old.battery_threshold, 30);
        assert!(old.share_active_count);
        assert_eq!(old.last_active_ping, None);
    }

    #[test]
    fn the_active_count_settings_keep_their_documented_names() {
        let prefs = Prefs {
            share_active_count: false,
            last_active_ping: Some("2026-11-09".into()),
            ..Prefs::default()
        };
        let saved = serde_json::to_value(&prefs).unwrap();
        assert_eq!(saved["shareActiveCount"], false);
        assert_eq!(saved["lastActivePing"], "2026-11-09");
        assert_eq!(serde_json::from_value::<Prefs>(saved).unwrap(), prefs);
    }
}
