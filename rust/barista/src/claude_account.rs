//! Claude plan limits from your Claude account: the numbers the Claude app's Settings → Usage shows.
//! Uses Claude Code's saved login read-only (never refreshed, written, logged, or put on a command
//! line). docs/USAGE.md, "Plan limits", source 0, is the specification.
use crate::hooks::write_atomically;
use crate::models::{format_date, now, parse_date, to_json, VERSION};
use crate::usage::{data_dir, ClaudeLimits};
use serde_json::{json, Value};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Stdio;

pub const ACCOUNT_LIMITS_FILE: &str = "claude-account-limits.json";
const USAGE_URL: &str = "https://api.anthropic.com/api/oauth/usage";

/// Results older than this aren't shown.
pub const MAX_AGE: f64 = 3600.0;

/// Why a refresh didn't save new limits.
#[derive(Debug, PartialEq, Eq)]
pub enum Skipped {
    NoLogin,
    Expired,
    /// curl couldn't complete the request.
    Unreachable(String),
    /// Anything but HTTP 200 with a JSON object.
    Status(String),
}

impl Skipped {
    /// The exact wording from docs/USAGE.md, shared with the Mac app.
    pub fn message(&self) -> String {
        match self {
            Self::NoLogin => "No Claude Code login found.".into(),
            Self::Expired => "Claude Code's login has expired. Open Claude Code to renew it.".into(),
            Self::Unreachable(detail) => format!("Couldn't reach Anthropic ({detail})."),
            Self::Status(status) => format!("Anthropic replied {status}."),
        }
    }
}

pub const UPDATED_MESSAGE: &str = "Claude account limits updated.";

/// The test override, if set.
fn credentials_override() -> Option<PathBuf> {
    std::env::var_os("BARISTA_CLAUDE_CREDENTIALS")
        .filter(|p| !p.is_empty())
        .map(PathBuf::from)
}

fn credentials_path() -> Option<PathBuf> {
    if let Some(path) = credentials_override() {
        return Some(path);
    }
    // macOS keeps Claude Code's login in the Keychain; the Mac app (Swift) reads it there.
    if cfg!(target_os = "macos") {
        return None;
    }
    let configured = std::env::var("CLAUDE_CONFIG_DIR")
        .ok()
        .and_then(|value| value.split(',').map(str::trim).find(|entry| !entry.is_empty()).map(PathBuf::from));
    let directory = configured.or_else(|| dirs::home_dir().map(|home| home.join(".claude")))?;
    Some(directory.join(".credentials.json"))
}

/// The endpoint. The override applies only with a test credentials file, so nothing can send a
/// real login somewhere else.
fn usage_url() -> String {
    if credentials_override().is_some() {
        if let Some(url) = std::env::var("BARISTA_CLAUDE_USAGE_URL").ok().filter(|u| !u.is_empty()) {
            return url;
        }
    }
    USAGE_URL.into()
}

/// Forgets saved account limits (when you turn the setting off).
pub fn delete_saved(directory: &Path) {
    let _ = fs::remove_file(directory.join(ACCOUNT_LIMITS_FILE));
}

/// The access token, if it stays valid for more than a minute.
pub fn usable_token(credentials: &Value, now: f64) -> Result<String, Skipped> {
    let oauth = credentials.get("claudeAiOauth").ok_or(Skipped::NoLogin)?;
    let token = oauth
        .get("accessToken")
        .and_then(Value::as_str)
        .filter(|t| !t.is_empty())
        .ok_or(Skipped::NoLogin)?;
    let expires = oauth.get("expiresAt").and_then(Value::as_f64).ok_or(Skipped::Expired)? / 1000.0;
    if expires <= now + 60.0 {
        return Err(Skipped::Expired);
    }
    Ok(token.to_string())
}

/// Why a request for an account's limits got no usable answer.
pub(crate) enum FetchError {
    /// curl couldn't complete the request.
    Unreachable(String),
    /// Anything but HTTP 200 with a JSON object.
    Status(String),
}

/// One credentialed GET with the system's curl: `headers` (one per line) go to curl on stdin
/// (`-H @-`), so a token never appears in the process list; redirects are never followed (no `-L`);
/// and no `.curlrc` applies (`curl -q`). Only HTTP 200 with a JSON object succeeds.
pub(crate) fn fetch(url: &str, headers: &str) -> Result<Value, FetchError> {
    let mut command = crate::update::curl();
    command
        .args(["-sS", "--max-time", "15", "-H", "@-", "-w", "\n%{http_code}"])
        .arg(url)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    let mut child = command.spawn().map_err(|error| FetchError::Unreachable(error.to_string()))?;
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(headers.as_bytes());
    }
    let output = child
        .wait_with_output()
        .map_err(|error| FetchError::Unreachable(error.to_string()))?;
    let text = String::from_utf8_lossy(&output.stdout);
    let (body, status) = text.rsplit_once('\n').unwrap_or(("", ""));
    let status = status.trim();
    // curl exits nonzero when the transfer failed, even if a status line and part of a body arrived
    // (a connection dropped mid-answer); what did arrive is not an answer.
    if !output.status.success() || status.is_empty() || status == "000" {
        let detail = output
            .status
            .code()
            .filter(|code| *code != 0)
            .map_or_else(|| "no response".to_string(), |code| format!("curl error {code}"));
        return Err(FetchError::Unreachable(detail));
    }
    if status != "200" {
        return Err(FetchError::Status(status.to_string()));
    }
    let usage: Value = serde_json::from_str(body).map_err(|_| FetchError::Status("200 without JSON".into()))?;
    if !usage.is_object() {
        return Err(FetchError::Status("200 without a JSON object".into()));
    }
    Ok(usage)
}

/// Asks Anthropic for the account's limits once and saves them. The previous result stays on any failure.
pub fn refresh() -> Result<(), Skipped> {
    let path = credentials_path().ok_or(Skipped::NoLogin)?;
    let credentials: Value = serde_json::from_slice(&fs::read(&path).map_err(|_| Skipped::NoLogin)?).map_err(|_| Skipped::NoLogin)?;
    let token = usable_token(&credentials, now())?;
    let headers = format!(
        "Authorization: Bearer {token}\nanthropic-beta: oauth-2025-04-20\nUser-Agent: Barista/{VERSION}\nAccept: application/json\n"
    );
    let usage = fetch(&usage_url(), &headers).map_err(|error| match error {
        FetchError::Unreachable(detail) => Skipped::Unreachable(detail),
        FetchError::Status(status) => Skipped::Status(status),
    })?;
    save(&data_dir(), &usage).map_err(Skipped::Unreachable)
}

fn save(directory: &Path, usage: &Value) -> Result<(), String> {
    fs::create_dir_all(directory).map_err(|e| e.to_string())?;
    let record = json!({"observedAt": format_date(now()), "usage": usage});
    write_atomically(&directory.join(ACCOUNT_LIMITS_FILE), to_json(&record).as_bytes()).map_err(|e| e.to_string())
}

/// The last saved account limits, unless older than an hour.
pub fn read(directory: &Path, now: f64) -> Option<ClaudeLimits> {
    let value: Value = serde_json::from_slice(&fs::read(directory.join(ACCOUNT_LIMITS_FILE)).ok()?).ok()?;
    let observed_at = value.get("observedAt").and_then(Value::as_str).and_then(parse_date)?;
    if now - observed_at > MAX_AGE {
        return None;
    }
    let usage = value.get("usage").filter(|u| u.is_object())?.clone();
    Some(ClaudeLimits {
        source: "Claude account",
        observed_at,
        rate_limits: usage,
    })
}

fn date(value: &Value) -> Option<f64> {
    value.as_str().and_then(parse_date)
}

/// One limit window from a usage response.
#[derive(Debug, PartialEq)]
pub struct AccountWindow {
    pub id: String,
    pub label: String,
    pub minutes: Option<u64>,
    pub used: f64,
    pub resets_at: Option<f64>,
    /// Extra usage: (used, limit) in US dollars.
    pub money: Option<(f64, f64)>,
}

fn window(id: impl Into<String>, label: impl Into<String>, minutes: Option<u64>, used: f64, resets_at: Option<f64>) -> AccountWindow {
    AccountWindow {
        id: id.into(),
        label: label.into(),
        minutes,
        used,
        resets_at,
        money: None,
    }
}

/// The windows in a usage response, per docs/USAGE.md.
pub fn windows(usage: &Value) -> Vec<AccountWindow> {
    let mut result = Vec::new();
    if let Some(limits) = usage.get("limits").and_then(Value::as_array) {
        for limit in limits {
            let Some(kind) = limit.get("kind").and_then(Value::as_str) else {
                continue;
            };
            let Some(percent) = limit.get("percent").and_then(Value::as_f64) else {
                continue;
            };
            let resets = date(&limit["resets_at"]);
            let (id, label, minutes) = match kind {
                // Same ids as Claude Code's status line (0.0.8), so scripts keep working.
                "session" => ("five_hour".to_string(), "Session".to_string(), Some(300)),
                "weekly_all" => ("seven_day".to_string(), "Weekly".to_string(), Some(10080)),
                "weekly_scoped" => {
                    let scope = &limit["scope"];
                    let name = scope["model"]["display_name"]
                        .as_str()
                        .or_else(|| scope["surface"].as_str())
                        .unwrap_or("Scoped")
                        .to_string();
                    (
                        format!("seven_day_{}", name.to_lowercase().replace(' ', "_")),
                        format!("Weekly · {name}"),
                        Some(10080),
                    )
                }
                other => (other.to_string(), other.replace('_', " "), None),
            };
            result.push(window(id, label, minutes, percent, resets));
        }
    } else {
        for (key, id, label, minutes) in [
            ("five_hour", "five_hour", "Session", 300u64),
            ("seven_day", "seven_day", "Weekly", 10080),
        ] {
            let entry = &usage[key];
            if let Some(used) = entry.get("utilization").and_then(Value::as_f64) {
                result.push(window(id, label, Some(minutes), used, date(&entry["resets_at"])));
            }
        }
    }
    let spend = &usage["spend"];
    if spend["enabled"].as_bool() == Some(true) {
        let money = |v: &Value| -> Option<f64> {
            let minor = v.get("amount_minor")?.as_f64()?;
            let exponent = v.get("exponent").and_then(Value::as_i64).unwrap_or(2);
            Some(minor / 10f64.powi(exponent as i32))
        };
        let used = money(&spend["used"]).unwrap_or(0.0);
        let limit = money(&spend["limit"]).unwrap_or(0.0);
        let percent = spend["percent"].as_f64().unwrap_or(0.0);
        result.push(AccountWindow {
            money: Some((used, limit)),
            ..window("extra_usage", "Extra usage", None, percent, None)
        });
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_are_used_only_while_valid() {
        let now = 1_000_000.0;
        let credentials = |expires_ms: f64| json!({"claudeAiOauth": {"accessToken": "t", "expiresAt": expires_ms}});
        assert_eq!(usable_token(&credentials((now + 3600.0) * 1000.0), now), Ok("t".into()));
        assert_eq!(usable_token(&credentials((now + 30.0) * 1000.0), now), Err(Skipped::Expired));
        assert_eq!(usable_token(&credentials((now - 1.0) * 1000.0), now), Err(Skipped::Expired));
        assert_eq!(usable_token(&json!({}), now), Err(Skipped::NoLogin));
    }

    /// A server on this computer that answers one request with `reply`, then hangs up.
    fn stand_in(reply: &'static str) -> String {
        use std::io::Read;
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/usage", listener.local_addr().unwrap());
        std::thread::spawn(move || {
            let Ok((mut stream, _)) = listener.accept() else { return };
            let mut request = Vec::new();
            let mut buffer = [0u8; 1024];
            while !request.windows(4).any(|window| window == b"\r\n\r\n") {
                match stream.read(&mut buffer) {
                    Ok(0) | Err(_) => break,
                    Ok(count) => request.extend_from_slice(&buffer[..count]),
                }
            }
            let _ = stream.write_all(reply.as_bytes());
        });
        url
    }

    /// Runs the system's `curl`, as the app does, against this computer only.
    #[test]
    fn an_answer_cut_short_is_a_failure_even_if_what_arrived_is_json() {
        let has_curl = crate::update::curl()
            .arg("--version")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|status| status.success());
        if !has_curl {
            return;
        }
        let whole = stand_in(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 20\r\nConnection: close\r\n\r\n{\"plan_type\":\"plus\"}",
        );
        assert_eq!(fetch(&whole, "Accept: application/json\n").ok(), Some(json!({"plan_type": "plus"})));
        // The same bytes, but the server promised 1000 and hung up: curl reports a failed transfer.
        let cut = stand_in("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 1000\r\nConnection: close\r\n\r\n{\"plan_type\":\"plus\"}");
        assert!(matches!(fetch(&cut, "Accept: application/json\n"), Err(FetchError::Unreachable(_))));
    }

    #[test]
    fn windows_follow_the_limits_list() {
        let usage = json!({
            "limits": [
                {"kind": "session", "percent": 24, "resets_at": "2100-01-01T23:39:59.555573+00:00"},
                {"kind": "weekly_all", "percent": 8, "resets_at": "2100-01-08T13:59:59+00:00"},
                {"kind": "weekly_scoped", "percent": 0, "resets_at": null, "scope": {"model": {"display_name": "Claude Fable"}}},
                {"kind": "daily_thing", "percent": 3}
            ],
            "spend": {"enabled": true, "percent": 5, "used": {"amount_minor": 1250, "exponent": 2}, "limit": {"amount_minor": 25000, "exponent": 2}}
        });
        let windows = windows(&usage);
        let ids: Vec<&str> = windows.iter().map(|w| w.id.as_str()).collect();
        assert_eq!(
            ids,
            ["five_hour", "seven_day", "seven_day_claude_fable", "daily_thing", "extra_usage"]
        );
        assert_eq!(windows[0].label, "Session");
        assert_eq!(windows[0].minutes, Some(300));
        assert_eq!(windows[0].used, 24.0);
        assert!(windows[0].resets_at.is_some());
        assert_eq!(windows[2].label, "Weekly · Claude Fable");
        assert_eq!(windows[3].label, "daily thing");
        assert_eq!(windows[3].minutes, None);
        assert_eq!(windows[4].money, Some((12.5, 250.0)));
    }

    #[test]
    fn windows_fall_back_to_utilization() {
        let usage = json!({"five_hour": {"utilization": 24.0, "resets_at": "2100-01-01T00:00:00Z"}, "seven_day": {"utilization": 8.0}, "spend": {"enabled": false}});
        let windows = windows(&usage);
        assert_eq!(windows.len(), 2);
        assert_eq!((windows[0].id.as_str(), windows[0].used), ("five_hour", 24.0));
        assert_eq!((windows[1].id.as_str(), windows[1].minutes), ("seven_day", Some(10080)));
    }

    #[test]
    fn unnamed_scopes_and_messages() {
        let usage = json!({"limits": [{"kind": "weekly_scoped", "percent": 1, "scope": {}}]});
        let windows = windows(&usage);
        assert_eq!(
            (windows[0].id.as_str(), windows[0].label.as_str()),
            ("seven_day_scoped", "Weekly · Scoped")
        );
        assert_eq!(Skipped::NoLogin.message(), "No Claude Code login found.");
        assert_eq!(
            Skipped::Expired.message(),
            "Claude Code's login has expired. Open Claude Code to renew it."
        );
        assert_eq!(Skipped::Status("500".into()).message(), "Anthropic replied 500.");
        assert_eq!(
            Skipped::Unreachable("curl error 7".into()).message(),
            "Couldn't reach Anthropic (curl error 7)."
        );
    }

    #[test]
    fn stale_results_are_ignored() {
        let directory = std::env::temp_dir().join(format!("barista-account-{}", crate::models::new_id()));
        save(&directory, &json!({"limits": []})).unwrap();
        let fresh = read(&directory, now()).expect("fresh result");
        assert!(read(&directory, fresh.observed_at + MAX_AGE + 1.0).is_none());
        delete_saved(&directory);
        assert!(read(&directory, now()).is_none());
        let _ = fs::remove_dir_all(directory);
    }
}
