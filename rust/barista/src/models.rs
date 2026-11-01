//! The wire format shared with the Mac app: same JSON keys, ISO 8601 dates, sorted keys.
use serde::{Deserialize, Serialize};
use std::fmt;

pub const VERSION: &str = env!("BARISTA_VERSION");

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SessionSource {
    Manual,
    Application,
    Agent,
    Command,
}

impl SessionSource {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Manual => "manual",
            Self::Application => "application",
            Self::Agent => "agent",
            Self::Command => "command",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CoffeeLease {
    pub id: String,
    pub title: String,
    pub source: SessionSource,
    #[serde(rename = "startedAt", with = "date")]
    pub started_at: f64,
    #[serde(rename = "expiresAt", with = "optional_date", default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<f64>,
    #[serde(rename = "hardDeadline", with = "optional_date", default, skip_serializing_if = "Option::is_none")]
    pub hard_deadline: Option<f64>,
    #[serde(rename = "keepDisplayAwake")]
    pub keep_display_awake: bool,
    #[serde(rename = "processID", default, skip_serializing_if = "Option::is_none")]
    pub process_id: Option<i32>,
    #[serde(rename = "processIdentity", default, skip_serializing_if = "Option::is_none")]
    pub process_identity: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub progress: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    #[serde(rename = "lastHeartbeat", with = "date")]
    pub last_heartbeat: f64,
}

impl CoffeeLease {
    pub fn is_task(&self) -> bool {
        matches!(self.source, SessionSource::Agent | SessionSource::Command)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum EndReason {
    Expired,
    Released,
    ProcessExited,
    BatteryLow,
    Stopped,
    AppQuit,
    PowerError,
}

impl EndReason {
    pub fn label(self) -> &'static str {
        match self {
            Self::Expired => "Timer finished",
            Self::Released => "Work finished",
            Self::ProcessExited => "Process finished",
            Self::BatteryLow => "Battery protection",
            Self::Stopped => "Stopped",
            Self::AppQuit => "App quit",
            Self::PowerError => "Power management error",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SessionCompletion {
    pub id: String,
    pub title: String,
    pub source: SessionSource,
    #[serde(rename = "startedAt", with = "date")]
    pub started_at: f64,
    #[serde(rename = "endedAt", with = "date")]
    pub ended_at: f64,
    pub reason: EndReason,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BatteryState {
    pub percent: i32,
    #[serde(rename = "onBattery")]
    pub on_battery: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SessionSnapshot {
    pub version: String,
    pub awake: bool,
    #[serde(rename = "displayAwake")]
    pub display_awake: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub battery: Option<BatteryState>,
    #[serde(rename = "batteryThreshold")]
    pub battery_threshold: i32,
    #[serde(rename = "batteryBlocked")]
    pub battery_blocked: bool,
    pub sessions: Vec<CoffeeLease>,
    pub history: Vec<SessionCompletion>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Changes whenever the user stops sessions, battery protection triggers, or the app restarts.
    /// Hooks pass it back to resume a lapsed turn only if nothing was stopped in between.
    #[serde(rename = "stopToken", default, skip_serializing_if = "Option::is_none")]
    pub stop_token: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ControlAction {
    Status,
    Acquire,
    Heartbeat,
    Release,
    Extend,
    StopAll,
    History,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ControlRequest {
    pub action: ControlAction,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seconds: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display: Option<bool>,
    #[serde(rename = "processID", default, skip_serializing_if = "Option::is_none")]
    pub process_id: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub progress: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<SessionSource>,
    /// acquire: hard cap. heartbeat: if set, restarts the hard cap from now (a new user turn).
    #[serde(rename = "maxSeconds", default, skip_serializing_if = "Option::is_none")]
    pub max_seconds: Option<f64>,
    /// acquire: refuse unless `SessionSnapshot.stop_token` still equals this value.
    #[serde(rename = "stopToken", default, skip_serializing_if = "Option::is_none")]
    pub stop_token: Option<String>,
}

impl ControlRequest {
    pub fn new(action: ControlAction) -> Self {
        Self {
            action,
            id: None,
            title: None,
            seconds: None,
            display: None,
            process_id: None,
            progress: None,
            message: None,
            source: None,
            max_seconds: None,
            stop_token: None,
        }
    }
    pub fn with_id(action: ControlAction, id: impl Into<String>) -> Self {
        Self {
            id: Some(id.into()),
            ..Self::new(action)
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ControlReply {
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<CoffeeLease>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<SessionSnapshot>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl ControlReply {
    pub fn failure(error: impl Into<String>) -> Self {
        Self {
            ok: false,
            session: None,
            state: None,
            error: Some(error.into()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CoffeeError {
    Message(String),
    /// Nothing is listening at the control endpoint (Barista isn't running).
    Unreachable(String),
}

impl CoffeeError {
    pub fn msg(text: impl Into<String>) -> Self {
        Self::Message(text.into())
    }
}

impl fmt::Display for CoffeeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Message(text) | Self::Unreachable(text) => f.write_str(text),
        }
    }
}

impl std::error::Error for CoffeeError {}

impl From<std::io::Error> for CoffeeError {
    fn from(error: std::io::Error) -> Self {
        Self::Message(error.to_string())
    }
}

pub type Result<T> = std::result::Result<T, CoffeeError>;

/// Compact JSON with sorted keys and unescaped slashes, like the Mac app's encoder.
pub fn to_json<T: Serialize>(value: &T) -> String {
    // serde_json's Map is a BTreeMap, so going through Value sorts every object's keys.
    let value = serde_json::to_value(value).expect("Barista values always encode");
    serde_json::to_string(&value).expect("JSON values always encode")
}

/// Strict, lowercase-or-uppercase hyphenated UUID text, as Foundation's `UUID(uuidString:)` accepts.
pub fn is_uuid(text: &str) -> bool {
    text.len() == 36
        && text.char_indices().all(|(index, c)| {
            if matches!(index, 8 | 13 | 18 | 23) {
                c == '-'
            } else {
                c.is_ascii_hexdigit()
            }
        })
}

pub fn new_id() -> String {
    uuid::Uuid::new_v4().to_string()
}

/// Seconds since 1970, with sub-second precision.
pub fn now() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

pub fn format_date(seconds: f64) -> String {
    let whole = seconds.floor() as i64;
    chrono::DateTime::from_timestamp(whole, 0)
        .unwrap_or_default()
        .format("%Y-%m-%dT%H:%M:%SZ")
        .to_string()
}

pub fn parse_date(text: &str) -> Option<f64> {
    let parsed = chrono::DateTime::parse_from_rfc3339(text).ok()?;
    Some(parsed.timestamp() as f64 + f64::from(parsed.timestamp_subsec_nanos()) / 1e9)
}

mod date {
    use serde::{Deserialize, Deserializer, Serializer};
    pub fn serialize<S: Serializer>(value: &f64, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&super::format_date(*value))
    }
    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<f64, D::Error> {
        let text = String::deserialize(deserializer)?;
        super::parse_date(&text).ok_or_else(|| serde::de::Error::custom("Expected an ISO 8601 date."))
    }
}

mod optional_date {
    use serde::{Deserialize, Deserializer, Serializer};
    pub fn serialize<S: Serializer>(value: &Option<f64>, serializer: S) -> Result<S::Ok, S::Error> {
        match value {
            Some(value) => serializer.serialize_str(&super::format_date(*value)),
            None => serializer.serialize_none(),
        }
    }
    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Option<f64>, D::Error> {
        match Option::<String>::deserialize(deserializer)? {
            Some(text) => super::parse_date(&text)
                .map(Some)
                .ok_or_else(|| serde::de::Error::custom("Expected an ISO 8601 date.")),
            None => Ok(None),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_keys_are_sorted_and_dates_are_iso8601() {
        let lease = CoffeeLease {
            id: "x".into(),
            title: "Build/test".into(),
            source: SessionSource::Command,
            started_at: 1_700_000_000.75,
            expires_at: None,
            hard_deadline: Some(1_700_000_100.0),
            keep_display_awake: false,
            process_id: Some(42),
            process_identity: None,
            progress: None,
            message: None,
            last_heartbeat: 1_700_000_000.0,
        };
        assert_eq!(
            to_json(&lease),
            r#"{"hardDeadline":"2023-11-14T22:15:00Z","id":"x","keepDisplayAwake":false,"lastHeartbeat":"2023-11-14T22:13:20Z","processID":42,"source":"command","startedAt":"2023-11-14T22:13:20Z","title":"Build/test"}"#
        );
        let decoded: CoffeeLease = serde_json::from_str(&to_json(&lease)).unwrap();
        assert_eq!(decoded.process_id, Some(42));
        assert_eq!(decoded.started_at, 1_700_000_000.0);
    }

    #[test]
    fn requests_need_an_action() {
        assert!(serde_json::from_str::<ControlRequest>(r#"{"broken":true}"#).is_err());
        let request: ControlRequest = serde_json::from_str(r#"{"action":"stopAll","extra":1}"#).unwrap();
        assert_eq!(request.action, ControlAction::StopAll);
        assert!(serde_json::from_str::<ControlRequest>(r#"{"action":"acquire","processID":1.5}"#).is_err());
    }

    #[test]
    fn uuid_text_is_strict() {
        assert!(is_uuid(&new_id()));
        assert!(is_uuid("E621E1F8-C36C-495A-93FC-0C247A3E6E5F"));
        assert!(!is_uuid("e621e1f8c36c495a93fc0c247a3e6e5f"));
        assert!(!is_uuid("{e621e1f8-c36c-495a-93fc-0c247a3e6e5f}"));
    }
}
