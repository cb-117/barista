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
