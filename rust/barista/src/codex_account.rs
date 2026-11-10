//! ChatGPT plan limits from your ChatGPT account: the numbers Codex's `/status` shows.
//! Uses Codex's saved login read-only (never refreshed, written, logged, or put on a command
//! line). docs/USAGE.md, "Plan limits", ChatGPT plan, source 0, is the specification.
use crate::claude_account::{fetch, FetchError};
use crate::hooks::write_atomically;
use crate::models::{format_date, now, parse_date, to_json, VERSION};
use crate::usage::{data_dir, CodexLimits};
use serde_json::{json, Map, Value};
use std::fs;
use std::path::{Path, PathBuf};

pub const ACCOUNT_LIMITS_FILE: &str = "codex-account-limits.json";
const USAGE_URL: &str = "https://chatgpt.com/backend-api/wham/usage";

/// Results older than this aren't shown.
pub const MAX_AGE: f64 = 3600.0;

/// The only parts of the response that are saved: it also carries your email and account IDs.
const KEPT: [&str; 3] = ["plan_type", "rate_limit", "credits"];

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
            Self::NoLogin => "No Codex login found.".into(),
            Self::Expired => "Codex's login has expired. Open Codex to renew it.".into(),
            Self::Unreachable(detail) => format!("Couldn't reach ChatGPT ({detail})."),
            Self::Status(status) => format!("ChatGPT replied {status}."),
        }
    }
}

pub const UPDATED_MESSAGE: &str = "ChatGPT account limits updated.";

/// The test override, if set.
fn auth_override() -> Option<PathBuf> {
    std::env::var_os("BARISTA_CODEX_AUTH").filter(|p| !p.is_empty()).map(PathBuf::from)
}

/// Codex's saved login: `auth.json` in `CODEX_HOME` (default `~/.codex`).
fn auth_path() -> Option<PathBuf> {
    if let Some(path) = auth_override() {
        return Some(path);
    }
    let directory = std::env::var_os("CODEX_HOME")
        .filter(|d| !d.is_empty())
        .map(PathBuf::from)
        .or_else(|| dirs::home_dir().map(|home| home.join(".codex")))?;
    Some(directory.join("auth.json"))
}

/// The endpoint. The override applies only with a test login file, so nothing can send a real
/// login somewhere else.
fn usage_url() -> String {
    if auth_override().is_some() {
        if let Some(url) = std::env::var("BARISTA_CODEX_USAGE_URL").ok().filter(|u| !u.is_empty()) {
            return url;
        }
    }
    USAGE_URL.into()
}

/// Forgets saved account limits (when you turn the setting off).
pub fn delete_saved(directory: &Path) {
    let _ = fs::remove_file(directory.join(ACCOUNT_LIMITS_FILE));
}

/// Base64url (padding optional), as JWTs use.
fn base64url(text: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(text.len() * 3 / 4);
    let (mut buffer, mut bits) = (0u32, 0u32);
    for byte in text.trim_end_matches('=').bytes() {
        let value = match byte {
            b'A'..=b'Z' => byte - b'A',
            b'a'..=b'z' => byte - b'a' + 26,
            b'0'..=b'9' => byte - b'0' + 52,
            b'-' => 62,
            b'_' => 63,
            _ => return None,
        };
        buffer = (buffer << 6) | u32::from(value);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((buffer >> bits) as u8);
            buffer &= (1 << bits) - 1;
        }
    }
    Some(out)
}

/// When a JWT expires (Unix seconds): the numeric `exp` of its payload, if that is readable.
fn token_expiry(token: &str) -> Option<f64> {
    let payload = token.split('.').nth(1)?;
    let value: Value = serde_json::from_slice(&base64url(payload)?).ok()?;
    value.as_object()?.get("exp")?.as_f64()
}

/// A value that can go on a header line: not empty, with no control characters.
fn header_safe(value: &str) -> bool {
    !value.is_empty() && !value.chars().any(char::is_control)
}

/// The access token and account ID of a ChatGPT sign-in. A token whose expiry is readable is used
/// only while it stays valid for more than a minute; one without is sent as it is.
pub fn usable_login(auth: &Value, now: f64) -> Result<(String, Option<String>), Skipped> {
    let tokens = auth.get("tokens").ok_or(Skipped::NoLogin)?;
    let token = tokens
        .get("access_token")
        .and_then(Value::as_str)
        .filter(|t| header_safe(t))
        .ok_or(Skipped::NoLogin)?;
    if token_expiry(token).is_some_and(|expires| expires <= now + 60.0) {
        return Err(Skipped::Expired);
    }
    let account = tokens.get("account_id").and_then(Value::as_str).filter(|a| header_safe(a));
    Ok((token.to_string(), account.map(String::from)))
}

/// Asks ChatGPT for the account's limits once and saves them. The previous result stays on any failure.
pub fn refresh() -> Result<(), Skipped> {
    let path = auth_path().ok_or(Skipped::NoLogin)?;
    let auth: Value = serde_json::from_slice(&fs::read(&path).map_err(|_| Skipped::NoLogin)?).map_err(|_| Skipped::NoLogin)?;
    let (token, account) = usable_login(&auth, now())?;
    let mut headers = format!("Authorization: Bearer {token}\n");
    if let Some(account) = account {
        headers.push_str(&format!("ChatGPT-Account-Id: {account}\n"));
    }
    headers.push_str(&format!("User-Agent: Barista/{VERSION}\nAccept: application/json\n"));
    let usage = fetch(&usage_url(), &headers).map_err(|error| match error {
        FetchError::Unreachable(detail) => Skipped::Unreachable(detail),
        FetchError::Status(status) => Skipped::Status(status),
    })?;
    save(&data_dir(), &usage).map_err(Skipped::Unreachable)
}

/// Saves the limits in a response, and nothing else from it.
fn save(directory: &Path, usage: &Value) -> Result<(), String> {
    fs::create_dir_all(directory).map_err(|e| e.to_string())?;
    let mut kept = Map::new();
    for key in KEPT {
        if let Some(value) = usage.get(key) {
            kept.insert(key.into(), value.clone());
        }
    }
    let record = json!({"observedAt": format_date(now()), "usage": kept});
    write_atomically(&directory.join(ACCOUNT_LIMITS_FILE), to_json(&record).as_bytes()).map_err(|e| e.to_string())
}

/// The last saved account limits, unless older than an hour.
pub fn read(directory: &Path, now: f64) -> Option<CodexLimits> {
    let value: Value = serde_json::from_slice(&fs::read(directory.join(ACCOUNT_LIMITS_FILE)).ok()?).ok()?;
    let observed_at = value.get("observedAt").and_then(Value::as_str).and_then(parse_date)?;
    if now - observed_at > MAX_AGE {
        return None;
    }
    Some(limits(value.get("usage").filter(|u| u.is_object())?, observed_at))
}

/// A usage response in the shape Codex's logs report limits in, per docs/USAGE.md. The endpoint
/// is undocumented, so every field is optional.
pub fn limits(usage: &Value, observed_at: f64) -> CodexLimits {
    let window = |key: &str| {
        let window = usage.get("rate_limit")?.get(key).filter(|w| w.is_object())?;
        let used = window.get("used_percent").and_then(Value::as_f64)?;
        let minutes = window
            .get("limit_window_seconds")
            .and_then(Value::as_f64)
            .map(|seconds| (seconds / 60.0).round())
            .filter(|minutes| *minutes >= 1.0)
            .map(|minutes| minutes as u64);
        Some(json!({
            "used_percent": used,
            "window_minutes": minutes,
            "resets_at": window.get("reset_at").and_then(Value::as_f64),
        }))
    };
    CodexLimits {
        observed_at,
        primary: window("primary_window"),
        secondary: window("secondary_window"),
        plan: usage.get("plan_type").and_then(Value::as_str).map(String::from),
        credits: usage.get("credits").filter(|c| c.is_object()).cloned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A token shaped like a JWT whose payload is `payload`.
    fn jwt(payload: &Value) -> String {
        const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
        let encode = |bytes: &[u8]| -> String {
            let mut out = String::new();
            for chunk in bytes.chunks(3) {
                let n = chunk.iter().fold(0u32, |n, b| (n << 8) | u32::from(*b)) << (8 * (3 - chunk.len()));
                for index in 0..=chunk.len() {
                    out.push(ALPHABET[((n >> (18 - 6 * index)) & 63) as usize] as char);
                }
            }
            out
        };
        format!(
            "{}.{}.signature",
            encode(b"{\"alg\":\"none\"}"),
            encode(payload.to_string().as_bytes())
        )
    }

    #[test]
    fn base64url_decodes_with_and_without_padding() {
        assert_eq!(base64url("eyJhIjoxfQ").as_deref(), Some(&b"{\"a\":1}"[..]));
        assert_eq!(base64url("eyJhIjoxfQ==").as_deref(), Some(&b"{\"a\":1}"[..]));
        assert_eq!(base64url("-_8").as_deref(), Some(&[0xfb, 0xff][..]));
        assert_eq!(base64url("not base64!"), None);
    }

    #[test]
    fn tokens_are_used_only_while_valid() {
        let now = 1_000_000.0;
        let auth =
            |token: &str| json!({"auth_mode": "chatgpt", "tokens": {"access_token": token, "account_id": "acct-1", "refresh_token": "r"}});
        let fresh = jwt(&json!({"exp": now + 3600.0, "sub": "u"}));
        assert_eq!(usable_login(&auth(&fresh), now), Ok((fresh.clone(), Some("acct-1".into()))));
        assert_eq!(usable_login(&auth(&jwt(&json!({"exp": now + 30.0}))), now), Err(Skipped::Expired));
        assert_eq!(usable_login(&auth(&jwt(&json!({"exp": now - 1.0}))), now), Err(Skipped::Expired));
        // No readable expiry: sent as it is.
        for token in ["opaque-token", "a.b.c", &jwt(&json!({"exp": "soon"})), &jwt(&json!([1]))] {
            assert_eq!(usable_login(&auth(token), now).map(|(t, _)| t), Ok(token.to_string()));
        }
        // No account ID: the header is left out.
        assert_eq!(usable_login(&json!({"tokens": {"access_token": "t"}}), now), Ok(("t".into(), None)));
    }

    #[test]
    fn no_chatgpt_sign_in_is_no_login() {
        let now = 1_000_000.0;
        for auth in [
            json!({}),
            json!({"auth_mode": "apikey", "OPENAI_API_KEY": "sk-test", "tokens": null}),
            json!({"tokens": {"access_token": ""}}),
            json!({"tokens": {"access_token": 5}}),
            // A token that would add a header line of its own is never sent.
            json!({"tokens": {"access_token": "t\nX-Extra: 1"}}),
        ] {
            assert_eq!(usable_login(&auth, now), Err(Skipped::NoLogin), "{auth}");
        }
        assert_eq!(
            usable_login(&json!({"tokens": {"access_token": "t", "account_id": "a\r\nX-Extra: 1"}}), now),
            Ok(("t".into(), None))
        );
    }

    #[test]
    fn messages_match_the_specification() {
        assert_eq!(Skipped::NoLogin.message(), "No Codex login found.");
        assert_eq!(Skipped::Expired.message(), "Codex's login has expired. Open Codex to renew it.");
        assert_eq!(Skipped::Status("500".into()).message(), "ChatGPT replied 500.");
        assert_eq!(
            Skipped::Unreachable("curl error 7".into()).message(),
            "Couldn't reach ChatGPT (curl error 7)."
        );
        assert_eq!(UPDATED_MESSAGE, "ChatGPT account limits updated.");
    }

    #[test]
    fn windows_plan_and_credits_come_from_the_response() {
        let usage = json!({
            "plan_type": "plus",
            "rate_limit": {
                "primary_window": {"used_percent": 20, "limit_window_seconds": 18000, "reset_at": 4102444800u64},
                "secondary_window": {"used_percent": 41.5, "limit_window_seconds": 604800, "reset_at": 4102531200u64}
            },
            "credits": {"has_credits": true, "unlimited": false, "balance": "1200.0000000000"}
        });
        let limits = limits(&usage, 5.0);
        assert_eq!(limits.observed_at, 5.0);
        assert_eq!(limits.plan.as_deref(), Some("plus"));
        assert_eq!(
            limits.primary,
            Some(json!({"used_percent": 20.0, "window_minutes": 300, "resets_at": 4102444800.0}))
        );
        assert_eq!(limits.secondary.as_ref().unwrap()["window_minutes"], 10080);
        assert_eq!(limits.credits.as_ref().unwrap()["balance"], "1200.0000000000");
    }

    #[test]
    fn every_field_is_optional() {
        let usage = json!({"rate_limit": {
            "primary_window": {"used_percent": 7, "limit_window_seconds": 0},
            "secondary_window": {"limit_window_seconds": 604800, "reset_at": 1}
        }, "credits": "none"});
        let limits = super::limits(&usage, 0.0);
        assert_eq!(
            limits.primary,
            Some(json!({"used_percent": 7.0, "window_minutes": null, "resets_at": null}))
        );
        // A window without a numeric used_percent isn't one.
        assert_eq!((limits.secondary, limits.plan, limits.credits), (None, None, None));
        let empty = super::limits(&json!({}), 0.0);
        assert_eq!((empty.primary, empty.secondary), (None, None));
    }

    #[test]
    fn only_limits_are_saved_and_stale_results_are_ignored() {
        let directory = std::env::temp_dir().join(format!("barista-codex-account-{}", crate::models::new_id()));
        let response = json!({
            "user_id": "user-123", "account_id": "acct-1", "email": "someone@example.com", "plan_type": "plus",
            "rate_limit": {"primary_window": {"used_percent": 20, "limit_window_seconds": 18000, "reset_at": 4102444800u64}},
            "credits": {"balance": "5", "unlimited": false}, "promo": null
        });
        save(&directory, &response).unwrap();
        let saved = fs::read_to_string(directory.join(ACCOUNT_LIMITS_FILE)).unwrap();
        for private in ["user-123", "acct-1", "someone@example.com", "promo"] {
            assert!(!saved.contains(private), "{private} must not be saved");
        }
        let value: Value = serde_json::from_str(&saved).unwrap();
        let keys: Vec<&String> = value["usage"].as_object().unwrap().keys().collect();
        assert_eq!(keys, ["credits", "plan_type", "rate_limit"]);
        let fresh = read(&directory, now()).expect("fresh result");
        assert_eq!(fresh.plan.as_deref(), Some("plus"));
        assert!(read(&directory, fresh.observed_at + MAX_AGE + 1.0).is_none());
        delete_saved(&directory);
        assert!(read(&directory, now()).is_none());
        let _ = fs::remove_dir_all(directory);
    }
}
