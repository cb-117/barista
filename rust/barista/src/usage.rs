//! Token and session usage for Claude Code, Codex, and Cursor, read from files on this computer.
//! The specification is docs/USAGE.md; the Swift app implements the same rules.
use crate::hooks::write_atomically;
use crate::models::{format_date, now, parse_date, to_json, VERSION};
use chrono::{Datelike, Duration, Local, NaiveDate, TimeZone};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::fs;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

pub const PRICES_CHECKED: &str = env!("BARISTA_PRICES_CHECKED");
const CACHE_FILE: &str = "usage-cache-portable.json";
const CURSOR_FILE: &str = "cursor-usage.jsonl";
const CACHE_VERSION: u32 = 3;
const CLAUDE_LIMITS_FILE: &str = "claude-limits.json";
/// A Cursor turn's active time is capped, so a stop that never arrived can't count for days.
const MAX_TURN_SECONDS: f64 = 4.0 * 3600.0;

// MARK: Prices

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Price {
    pub model: String,
    pub input: f64,
    pub output: f64,
    pub cache_read: f64,
    pub cache_write5m: f64,
    pub cache_write1h: f64,
}

/// The price table from Sources/BaristaCore/ModelPrices.swift, extracted by build.rs.
pub fn prices() -> &'static [Price] {
    static PRICES: OnceLock<Vec<Price>> = OnceLock::new();
    PRICES.get_or_init(|| {
        serde_json::from_str(include_str!(concat!(env!("OUT_DIR"), "/model-prices.json"))).expect("ModelPrices.swift holds valid JSON")
    })
}

/// A model ID as reported: cut at the first `[` or `@` (`claude-haiku-5-5[1m]` → `claude-haiku-5-5`).
pub fn model_name(raw: &str) -> String {
    raw.split(['[', '@']).next().unwrap_or("").trim().to_string()
}

/// The longest price entry equal to the model or followed in it by `-`.
pub fn price_for(model: &str) -> Option<&'static Price> {
    let model = model_name(model);
    prices()
        .iter()
        .filter(|price| model == price.model || model.strip_prefix(price.model.as_str()).is_some_and(|rest| rest.starts_with('-')))
        .max_by_key(|price| price.model.len())
}

// MARK: Counts and cache

/// input, output, cache read, 5-minute writes, 1-hour writes, requests.
pub type Counts = [u64; 6];

fn add(into: &mut Counts, other: &Counts) {
    for (a, b) in into.iter_mut().zip(other) {
        *a += b;
    }
}

fn token_total(counts: &Counts) -> u64 {
    counts[..5].iter().sum()
}

fn cost(price: &Price, counts: &Counts) -> f64 {
    (counts[0] as f64 * price.input
        + counts[1] as f64 * price.output
        + counts[2] as f64 * price.cache_read
        + counts[3] as f64 * price.cache_write5m
        + counts[4] as f64 * price.cache_write1h)
        / 1_000_000.0
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct CodexState {
    /// The last counted `total_token_usage`: input, cached input, output, total.
    prev: Option<[u64; 4]>,
    model: Option<String>,
    session: Option<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct FileState {
    agent: String,
    offset: u64,
    #[serde(default)]
    keys: Vec<String>,
    #[serde(default)]
    codex: CodexState,
    /// "model\u{1f}session\u{1f}quarter-hour start" → counts.
    #[serde(default)]
    buckets: BTreeMap<String, Counts>,
    /// Codex: the newest plan limits this file reported.
    #[serde(default)]
    limits: Option<CodexLimits>,
}

/// The ChatGPT plan's limits from a Codex `token_count` event.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct CodexLimits {
    pub observed_at: f64,
    pub primary: Option<Value>,
    pub secondary: Option<Value>,
    pub plan: Option<String>,
    pub credits: Option<Value>,
}

/// The Claude plan's limits as last seen, in the status line's `rate_limits` shape.
#[derive(Clone, Debug, PartialEq)]
pub struct ClaudeLimits {
    /// "Claude account" (Anthropic's usage endpoint; `rate_limits` holds its response), "Claude Code"
    /// (status line), or "Claude app" (desktop app snapshot).
    pub source: &'static str,
    pub observed_at: f64,
    pub rate_limits: Value,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct Cache {
    version: u32,
    files: BTreeMap<String, FileState>,
}

/// Barista's data folder (usage cache, Cursor's usage log).
pub fn data_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("BARISTA_DATA_DIR").filter(|d| !d.is_empty()) {
        return PathBuf::from(dir);
    }
    let home = dirs::home_dir().unwrap_or_default();
    if cfg!(target_os = "macos") {
        return home.join("Library").join("Application Support").join("Barista");
    }
    if cfg!(windows) {
        return dirs::data_local_dir().unwrap_or(home).join("Barista");
    }
    match std::env::var_os("XDG_DATA_HOME").map(PathBuf::from).filter(|d| d.is_absolute()) {
        Some(base) => base.join("barista"),
        None => home.join(".local").join("share").join("barista"),
    }
}

/// Every `*.jsonl` file under `root`, without following directory symlinks.
fn jsonl_files(root: &Path, into: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(root) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(kind) = entry.file_type() else { continue };
        if kind.is_dir() {
            jsonl_files(&path, into);
        } else if path.extension().is_some_and(|e| e == "jsonl") && (kind.is_file() || path.is_file()) {
            into.push(path);
        }
    }
}

/// (agent, file) for every log, in ascending byte order of the full path.
fn log_files() -> Vec<(&'static str, PathBuf)> {
    let home = dirs::home_dir().unwrap_or_default();
    let claude_roots: Vec<PathBuf> = match std::env::var("CLAUDE_CONFIG_DIR") {
        Ok(dirs) if !dirs.trim().is_empty() => dirs
            .split(',')
            .map(str::trim)
            .filter(|d| !d.is_empty())
            .map(|d| PathBuf::from(d).join("projects"))
            .collect(),
        _ => vec![
            home.join(".claude").join("projects"),
            home.join(".config").join("claude").join("projects"),
        ],
    };
    let codex_home = std::env::var_os("CODEX_HOME")
        .filter(|d| !d.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".codex"));
    let mut files: Vec<(&'static str, PathBuf)> = Vec::new();
    for (agent, roots) in [
        ("claude", claude_roots),
        ("codex", vec![codex_home.join("sessions"), codex_home.join("archived_sessions")]),
    ] {
        let mut found = Vec::new();
        for root in roots {
            jsonl_files(&root, &mut found);
        }
        files.extend(found.into_iter().map(|path| (agent, path)));
    }
    files.sort_by(|a, b| a.1.as_os_str().as_encoded_bytes().cmp(b.1.as_os_str().as_encoded_bytes()));
    files.dedup_by(|a, b| a.1 == b.1);
    files
}

fn number(value: &Value, key: &str) -> u64 {
    value
        .get(key)
        .and_then(Value::as_f64)
        .filter(|n| n.is_finite() && *n > 0.0)
        .map_or(0, |n| n as u64)
}

/// The start of the 15-minute bucket holding `seconds`. Quarter hours line up with every time zone
/// (including +5:30 and +5:45), so period boundaries at local midnight stay exact.
fn hour_of(seconds: f64) -> i64 {
    (seconds / 900.0).floor() as i64 * 900
}

fn bucket_key(model: &str, session: &str, hour: i64) -> String {
    format!("{model}\u{1f}{session}\u{1f}{hour}")
}

fn file_stem(path: &Path) -> String {
    path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default()
}

/// Counts one Claude Code line, if it is a new assistant response with usage.
fn claude_line(line: &[u8], path: &Path, state: &mut FileState, seen: &mut HashSet<String>) {
    let text = String::from_utf8_lossy(line);
    if !text.contains("\"assistant\"") || !text.contains("\"usage\"") {
        return;
    }
    let Ok(value) = serde_json::from_str::<Value>(&text) else { return };
    if value.get("type").and_then(Value::as_str) != Some("assistant") {
        return;
    }
    let Some(message) = value.get("message") else { return };
    let Some(usage) = message.get("usage").filter(|u| u.is_object()) else {
        return;
    };
    let Some(model) = message.get("model").and_then(Value::as_str).filter(|m| *m != "<synthetic>") else {
        return;
    };
    let Some(time) = value.get("timestamp").and_then(Value::as_str).and_then(parse_date) else {
        return;
    };
    if let Some(id) = message.get("id").and_then(Value::as_str) {
        let key = format!("{id}:{}", value.get("requestId").and_then(Value::as_str).unwrap_or(""));
        if !seen.insert(key.clone()) {
            return;
        }
        state.keys.push(key);
    }
    let session = value
        .get("sessionId")
        .and_then(Value::as_str)
        .map(String::from)
        .unwrap_or_else(|| file_stem(path));
    let (write5m, write1h) = match usage.get("cache_creation").filter(|c| c.is_object()) {
        Some(creation) => (
            number(creation, "ephemeral_5m_input_tokens"),
            number(creation, "ephemeral_1h_input_tokens"),
        ),
        None => (number(usage, "cache_creation_input_tokens"), 0),
    };
    let counts = [
        number(usage, "input_tokens"),
        number(usage, "output_tokens"),
        number(usage, "cache_read_input_tokens"),
        write5m,
        write1h,
        1,
    ];
    add(
        state
            .buckets
            .entry(bucket_key(&model_name(model), &session, hour_of(time)))
            .or_default(),
        &counts,
    );
}

/// The Codex token delta for a `total_token_usage`, or None for a duplicate. Updates `prev`.
fn codex_delta(prev: &mut Option<[u64; 4]>, total: [u64; 4]) -> Option<[u64; 4]> {
    let delta = match *prev {
        Some(previous) if total[3] <= previous[3] => return None,
        Some(previous) => [0, 1, 2, 3].map(|i| total[i].saturating_sub(previous[i])),
        None => total,
    };
    *prev = Some(total);
    Some(delta)
}

/// Follows one Codex rollout line: session, model, and token counts.
fn codex_line(line: &[u8], path: &Path, state: &mut FileState) {
    let text = String::from_utf8_lossy(line);
    let kind = if text.contains("\"token_count\"") {
        "event_msg"
    } else if text.contains("\"turn_context\"") {
        "turn_context"
    } else if text.contains("\"session_meta\"") {
        "session_meta"
    } else {
        return;
    };
    let Ok(value) = serde_json::from_str::<Value>(&text) else { return };
    if value.get("type").and_then(Value::as_str) != Some(kind) {
        return;
    }
    let payload = value.get("payload").cloned().unwrap_or(Value::Null);
    match kind {
        "session_meta" => {
            let id = payload.get("id").or_else(|| payload.get("session_id")).and_then(Value::as_str);
            if let Some(id) = id {
                state.codex.session = Some(id.to_string());
            }
        }
        "turn_context" => {
            if let Some(model) = payload.get("model").and_then(Value::as_str) {
                state.codex.model = Some(model.to_string());
            }
        }
        _ => {
            if payload.get("type").and_then(Value::as_str) != Some("token_count") {
                return;
            }
            record_codex_limits(&value, &payload, state);
            let Some(total) = payload
                .get("info")
                .and_then(|i| i.get("total_token_usage"))
                .filter(|t| t.is_object())
            else {
                return;
            };
            let Some(time) = value.get("timestamp").and_then(Value::as_str).and_then(parse_date) else {
                return;
            };
            let total = [
                number(total, "input_tokens"),
                number(total, "cached_input_tokens"),
                number(total, "output_tokens"),
                number(total, "total_tokens"),
            ];
            let Some(delta) = codex_delta(&mut state.codex.prev, total) else {
                return;
            };
            let model = model_name(state.codex.model.as_deref().unwrap_or("unknown"));
            let session = state.codex.session.clone().unwrap_or_else(|| file_stem(path));
            let counts = [delta[0].saturating_sub(delta[1]), delta[2], delta[1], 0, 0, 1];
            add(
                state.buckets.entry(bucket_key(&model, &session, hour_of(time))).or_default(),
                &counts,
            );
        }
    }
}

/// Keeps the newest plan limits a `token_count` event reports, even when its tokens are a duplicate.
fn record_codex_limits(value: &Value, payload: &Value, state: &mut FileState) {
    let Some(limits) = payload.get("rate_limits").filter(|l| l.is_object()) else {
        return;
    };
    let window = |key: &str| limits.get(key).filter(|w| w.is_object()).cloned();
    let (primary, secondary) = (window("primary"), window("secondary"));
    if primary.is_none() && secondary.is_none() {
        return;
    }
    let Some(time) = value.get("timestamp").and_then(Value::as_str).and_then(parse_date) else {
        return;
    };
    if state.limits.as_ref().is_some_and(|l| l.observed_at > time) {
        return;
    }
    state.limits = Some(CodexLimits {
        observed_at: time,
        primary,
        secondary,
        plan: limits.get("plan_type").and_then(Value::as_str).map(String::from),
        credits: limits.get("credits").filter(|c| c.is_object()).cloned(),
    });
}

/// Reads whatever was appended to `path` since the cache last saw it.
fn read_file(agent: &str, path: &Path, state: &mut FileState, seen: &mut HashSet<String>) {
    let Ok(mut file) = fs::File::open(path) else { return };
    let size = file.metadata().map(|m| m.len()).unwrap_or(0);
    if size < state.offset {
        // Rewritten: drop what this file contributed and start over.
        for key in state.keys.drain(..) {
            seen.remove(&key);
        }
        *state = FileState {
            agent: agent.to_string(),
            ..FileState::default()
        };
    }
    if size == state.offset || file.seek(SeekFrom::Start(state.offset)).is_err() {
        return;
    }
    let mut data = Vec::new();
    if file.read_to_end(&mut data).is_err() {
        return;
    }
    // A line without its newline is still being written; leave it for the next run.
    let Some(end) = data.iter().rposition(|b| *b == b'\n') else {
        return;
    };
    for line in data[..end].split(|b| *b == b'\n') {
        let line = line.strip_suffix(b"\r").unwrap_or(line);
        if line.is_empty() {
            continue;
        }
        if agent == "claude" {
            claude_line(line, path, state, seen);
        } else {
            codex_line(line, path, state);
        }
    }
    state.offset += end as u64 + 1;
}

/// One 15-minute bucket of Claude or Codex usage.
#[derive(Clone, Debug)]
pub struct Entry {
    pub agent: String,
    pub model: String,
    pub session: String,
    pub hour: i64,
    pub counts: Counts,
}

/// One line of Barista's Cursor log.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct CursorRecord {
    pub conversation: String,
    pub event: String,
    #[serde(default)]
    pub generation: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    pub ts: String,
}

#[derive(Clone, Debug, Default)]
pub struct Snapshot {
    pub entries: Vec<Entry>,
    pub cursor: Vec<CursorRecord>,
    pub codex_limits: Option<CodexLimits>,
    pub claude_limits: Option<ClaudeLimits>,
}

/// Brings the cache up to date with the agents' logs, then returns every bucket and Cursor record.
pub fn collect() -> Snapshot {
    collect_in(&data_dir())
}

fn collect_in(directory: &Path) -> Snapshot {
    let cache_path = directory.join(CACHE_FILE);
    let mut cache: Cache = fs::read(&cache_path)
        .ok()
        .and_then(|data| serde_json::from_slice(&data).ok())
        .filter(|cache: &Cache| cache.version == CACHE_VERSION)
        .unwrap_or_default();
    cache.version = CACHE_VERSION;
    let before = serde_json::to_vec(&cache).unwrap_or_default();
    let mut seen: HashSet<String> = cache.files.values().flat_map(|f| f.keys.iter().cloned()).collect();
    let mut codex_names: HashSet<String> = HashSet::new();
    for (agent, path) in log_files() {
        // Codex moves a conversation's log into archived_sessions when it's archived; keyed by file
        // name, the same entry continues instead of counting the log twice.
        let key = if agent == "codex" {
            let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            if !codex_names.insert(name.clone()) {
                continue; // Two copies at once: only the first (in path order) counts.
            }
            format!("codex/{name}")
        } else {
            path.to_string_lossy().into_owned()
        };
        let state = cache.files.entry(key).or_insert_with(|| FileState {
            agent: agent.to_string(),
            ..FileState::default()
        });
        read_file(agent, &path, state, &mut seen);
    }
    if serde_json::to_vec(&cache).unwrap_or_default() != before && fs::create_dir_all(directory).is_ok() {
        if let Ok(data) = serde_json::to_vec(&cache) {
            let _ = write_atomically(&cache_path, &data);
        }
    }
    let mut entries = Vec::new();
    for state in cache.files.values() {
        for (key, counts) in &state.buckets {
            let mut parts = key.split('\u{1f}');
            let (Some(model), Some(session), Some(hour)) = (parts.next(), parts.next(), parts.next().and_then(|h| h.parse().ok())) else {
                continue;
            };
            entries.push(Entry {
                agent: state.agent.clone(),
                model: model.into(),
                session: session.into(),
                hour,
                counts: *counts,
            });
        }
    }
    let codex_limits = cache
        .files
        .values()
        .filter_map(|f| f.limits.clone())
        .max_by(|a, b| a.observed_at.total_cmp(&b.observed_at));
    Snapshot {
        entries,
        cursor: read_cursor(directory),
        codex_limits,
        claude_limits: newest_claude_limits([
            crate::claude_account::read(directory, now()),
            read_claude_status_limits(directory),
            read_claude_app_limits(now()),
        ]),
    }
}

// MARK: Plan limits

/// Saves the limits Claude Code's status line reported. Never fails loudly.
pub fn record_claude_limits(rate_limits: &Value) {
    let directory = data_dir();
    if fs::create_dir_all(&directory).is_err() {
        return;
    }
    let record = json!({"observedAt": format_date(now()), "rateLimits": rate_limits});
    let _ = write_atomically(&directory.join(CLAUDE_LIMITS_FILE), to_json(&record).as_bytes());
}

fn read_claude_status_limits(directory: &Path) -> Option<ClaudeLimits> {
    let value: Value = serde_json::from_slice(&fs::read(directory.join(CLAUDE_LIMITS_FILE)).ok()?).ok()?;
    let observed_at = value.get("observedAt").and_then(Value::as_str).and_then(parse_date)?;
    let rate_limits = value.get("rateLimits").filter(|l| l.is_object())?.clone();
    Some(ClaudeLimits {
        source: "Claude Code",
        observed_at,
        rate_limits,
    })
}

/// The Claude desktop app's folder: BARISTA_CLAUDE_APP_DIR, else its standard location.
fn claude_app_dir() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("BARISTA_CLAUDE_APP_DIR").filter(|d| !d.is_empty()) {
        return Some(PathBuf::from(dir));
    }
    if cfg!(windows) {
        return dirs::config_dir().map(|d| d.join("Claude"));
    }
    if cfg!(target_os = "macos") {
        return dirs::home_dir().map(|h| h.join("Library").join("Application Support").join("Claude"));
    }
    None
}

/// The newest sample in the desktop app's (undocumented, rarely updated) plan-usage-history.json,
/// if readable and no more than an hour old.
fn read_claude_app_limits(now: f64) -> Option<ClaudeLimits> {
    let data = fs::read(claude_app_dir()?.join("plan-usage-history.json")).ok()?;
    let value: Value = serde_json::from_slice(&data).ok()?;
    let sample = value
        .get("samples")?
        .as_array()?
        .iter()
        .filter(|s| s.get("t").and_then(Value::as_f64).is_some())
        .max_by(|a, b| a["t"].as_f64().unwrap_or(0.0).total_cmp(&b["t"].as_f64().unwrap_or(0.0)))?;
    let usage = sample.get("u")?;
    let mut rate_limits = serde_json::Map::new();
    for (key, id) in [("fh", "five_hour"), ("sd", "seven_day")] {
        if let Some(percent) = usage.get(key).and_then(Value::as_f64) {
            rate_limits.insert(id.into(), json!({"used_percentage": percent}));
        }
    }
    let observed_at = sample["t"].as_f64()? / 1000.0;
    if rate_limits.is_empty() || now - observed_at > 3600.0 {
        return None;
    }
    Some(ClaudeLimits {
        source: "Claude app",
        observed_at,
        rate_limits: Value::Object(rate_limits),
    })
}

/// The most recently observed Claude source (ties go to the earlier one in the list).
pub fn newest_claude_limits(sources: impl IntoIterator<Item = Option<ClaudeLimits>>) -> Option<ClaudeLimits> {
    sources
        .into_iter()
        .flatten()
        .fold(None, |best: Option<ClaudeLimits>, next| match best {
            Some(best) if best.observed_at >= next.observed_at => Some(best),
            _ => Some(next),
        })
}

/// "5-hour", "Weekly", "N-hour", or "N-day".
pub fn window_label(minutes: u64) -> String {
    match minutes {
        300 => "5-hour".into(),
        10080 => "Weekly".into(),
        m if m < 1440 => format!("{}-hour", (m as f64 / 60.0).round() as u64),
        m => format!("{}-day", (m as f64 / 1440.0).round() as u64),
    }
}

/// One limit window. A window whose reset time has passed reads 0% with `reset: true`.
fn window_json(id: &str, label: String, minutes: Option<u64>, used: f64, resets_at: Option<f64>, now: f64) -> Value {
    let reset = resets_at.is_some_and(|at| at <= now);
    json!({
        "id": id,
        "label": label,
        "windowMinutes": minutes,
        "usedPercent": if reset { 0.0 } else { used },
        "resetsAt": resets_at.map(format_date),
        "reset": reset,
    })
}

fn claude_limits_json(limits: Option<&ClaudeLimits>, now: f64) -> Value {
    let Some(limits) = limits else {
        return unavailable_limits("claude", "Claude plan");
    };
    let mut windows = Vec::new();
    if limits.source == "Claude account" {
        for window in crate::claude_account::windows(&limits.rate_limits) {
            let mut entry = window_json(&window.id, window.label, window.minutes, window.used, window.resets_at, now);
            if let Some((used_usd, limit_usd)) = window.money {
                entry["usedUSD"] = json!(used_usd);
                entry["limitUSD"] = json!(limit_usd);
            }
            windows.push(entry);
        }
        return json!({
            "agent": "claude", "name": "Claude plan", "available": true, "source": limits.source,
            "observedAt": format_date(limits.observed_at), "plan": null, "credits": null, "windows": windows,
        });
    }
    for (id, minutes) in [("five_hour", Some(300u64)), ("seven_day", Some(10080)), ("spend_limit", None)] {
        let Some(window) = limits.rate_limits.get(id).filter(|w| w.is_object()) else {
            continue;
        };
        let Some(used) = window.get("used_percentage").and_then(Value::as_f64) else {
            continue;
        };
        let resets_at = window.get("resets_at").and_then(Value::as_f64);
        let label = minutes.map(window_label).unwrap_or_else(|| "Spend limit".into());
        let mut entry = window_json(id, label, minutes, used, resets_at, now);
        if id == "spend_limit" {
            for (from, to) in [("used_usd", "usedUSD"), ("limit_usd", "limitUSD"), ("period", "period")] {
                if let Some(value) = window.get(from) {
                    entry[to] = value.clone();
                }
            }
        }
        windows.push(entry);
    }
    json!({
        "agent": "claude", "name": "Claude plan", "available": true, "source": limits.source,
        "observedAt": format_date(limits.observed_at), "plan": null, "credits": null, "windows": windows,
    })
}

fn codex_limits_json(limits: Option<&CodexLimits>, now: f64) -> Value {
    let Some(limits) = limits else {
        return unavailable_limits("codex", "ChatGPT plan");
    };
    let mut windows = Vec::new();
    for (id, window) in [("primary", &limits.primary), ("secondary", &limits.secondary)] {
        let Some(window) = window else { continue };
        let used = window.get("used_percent").and_then(Value::as_f64).unwrap_or(0.0);
        let minutes = window.get("window_minutes").and_then(Value::as_u64);
        let label = minutes.map(window_label).unwrap_or_else(|| "Limit".into());
        windows.push(window_json(
            id,
            label,
            minutes,
            used,
            window.get("resets_at").and_then(Value::as_f64),
            now,
        ));
    }
    let credits = limits.credits.as_ref().map(|c| {
        json!({"balance": c.get("balance").cloned().unwrap_or(Value::Null), "unlimited": c.get("unlimited").and_then(Value::as_bool).unwrap_or(false)})
    });
    json!({
        "agent": "codex", "name": "ChatGPT plan", "available": true, "source": "Codex",
        "observedAt": format_date(limits.observed_at), "plan": limits.plan, "credits": credits, "windows": windows,
    })
}

fn unavailable_limits(agent: &str, name: &str) -> Value {
    json!({"agent": agent, "name": name, "available": false, "source": null, "observedAt": null, "plan": null, "credits": null, "windows": []})
}

/// Claude, ChatGPT (Codex), and Cursor plan limits, in that order.
pub fn limits_json(snapshot: &Snapshot) -> Value {
    let now = now();
    json!([
        claude_limits_json(snapshot.claude_limits.as_ref(), now),
        codex_limits_json(snapshot.codex_limits.as_ref(), now),
        unavailable_limits("cursor", "Cursor"),
    ])
}

fn read_cursor(directory: &Path) -> Vec<CursorRecord> {
    let Ok(text) = fs::read_to_string(directory.join(CURSOR_FILE)) else {
        return Vec::new();
    };
    text.lines()
        .filter_map(|line| serde_json::from_str::<CursorRecord>(line).ok())
        .filter(|r| parse_date(&r.ts).is_some())
        .collect()
}

/// Appends one Cursor hook event to the usage log, in a single write.
pub fn record_cursor(event: &str, conversation: &str, generation: &str, model: Option<&str>, status: Option<&str>) {
    let record = CursorRecord {
        conversation: conversation.to_string(),
        event: event.to_string(),
        generation: generation.to_string(),
        model: model.map(String::from),
        status: status.map(String::from),
        ts: format_date(now()),
    };
    let directory = data_dir();
    if fs::create_dir_all(&directory).is_err() {
        return;
    }
    let mut line = to_json(&record);
    line.push('\n');
    if let Ok(mut file) = fs::OpenOptions::new().create(true).append(true).open(directory.join(CURSOR_FILE)) {
        let _ = file.write_all(line.as_bytes());
    }
}

// MARK: Periods

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Period {
    Today,
    Week,
    Month,
    All,
}

impl Period {
    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "today" => Some(Self::Today),
            "week" => Some(Self::Week),
            "month" => Some(Self::Month),
            "all" => Some(Self::All),
            _ => None,
        }
    }
    pub fn id(self) -> &'static str {
        match self {
            Self::Today => "today",
            Self::Week => "week",
            Self::Month => "month",
            Self::All => "all",
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Self::Today => "Today",
            Self::Week => "This week",
            Self::Month => "This month",
            Self::All => "All time",
        }
    }
    /// The local date this period starts on, for `today`; None for all time.
    pub fn start_date(self, today: NaiveDate) -> Option<NaiveDate> {
        match self {
            Self::Today => Some(today),
            Self::Week => Some(today - Duration::days(i64::from(today.weekday().num_days_from_monday()))),
            Self::Month => today.with_day(1),
            Self::All => None,
        }
    }
    /// Seconds since 1970 at local midnight of the start date.
    pub fn start(self) -> Option<f64> {
        let date = self.start_date(Local::now().date_naive())?;
        let midnight = date.and_hms_opt(0, 0, 0)?;
        let local = Local.from_local_datetime(&midnight).earliest()?;
        Some(local.timestamp() as f64)
    }
}

// MARK: Report

fn round6(value: f64) -> f64 {
    (value * 1_000_000.0).round() / 1_000_000.0
}

#[derive(Default)]
struct Tally {
    counts: Counts,
    cost: f64,
    unpriced: u64,
    sessions: BTreeSet<String>,
    turns: u64,
    active: f64,
}

impl Tally {
    fn add_tokens(&mut self, model: &str, session: &str, counts: &Counts) {
        add(&mut self.counts, counts);
        match price_for(model) {
            Some(price) => self.cost += cost(price, counts),
            None => self.unpriced += token_total(counts),
        }
        self.sessions.insert(session.to_string());
    }

    fn json(&self, extra: (&str, Value), cursor: bool) -> Value {
        let total = token_total(&self.counts);
        let estimated = if cursor || (total > 0 && self.unpriced == total) {
            Value::Null
        } else {
            json!(round6(self.cost))
        };
        let mut value = json!({
            "sessions": self.sessions.len(),
            "requests": self.counts[5],
            "turns": self.turns,
            "activeSeconds": self.active.round() as u64,
            "inputTokens": self.counts[0],
            "outputTokens": self.counts[1],
            "cacheReadTokens": self.counts[2],
            "cacheWriteTokens": self.counts[3] + self.counts[4],
            "totalTokens": total,
            "unpricedTokens": self.unpriced,
            "estimatedCost": estimated,
        });
        value[extra.0] = extra.1;
        value
    }
}

fn sorted_models(models: BTreeMap<String, Tally>, cursor: bool) -> Vec<Value> {
    let mut list: Vec<(String, Tally)> = models.into_iter().collect();
    list.sort_by(|a, b| {
        token_total(&b.1.counts)
            .cmp(&token_total(&a.1.counts))
            .then(b.1.turns.cmp(&a.1.turns))
            .then(a.0.cmp(&b.0))
    });
    list.into_iter()
        .map(|(name, tally)| tally.json(("model", json!(name)), cursor))
        .collect()
}

fn period_json(snapshot: &Snapshot, period: Period, start: Option<f64>) -> Value {
    let now = now();
    let inside = |time: f64| time <= now && start.is_none_or(|start| time >= start);
    let mut agents = Vec::new();
    let mut period_tokens = 0u64;
    let mut period_cost = 0.0;
    for (agent, name) in [("claude", "Claude Code"), ("codex", "Codex")] {
        let mut total = Tally::default();
        let mut models: BTreeMap<String, Tally> = BTreeMap::new();
        for entry in snapshot.entries.iter().filter(|e| e.agent == agent && inside(e.hour as f64)) {
            total.add_tokens(&entry.model, &entry.session, &entry.counts);
            models
                .entry(entry.model.clone())
                .or_default()
                .add_tokens(&entry.model, &entry.session, &entry.counts);
        }
        let mut value = total.json(("agent", json!(agent)), false);
        value["name"] = json!(name);
        value["models"] = Value::Array(sorted_models(models, false));
        period_tokens += token_total(&total.counts);
        if let Some(cost) = value["estimatedCost"].as_f64() {
            period_cost += cost;
        }
        agents.push(value);
    }
    agents.push(cursor_json(&snapshot.cursor, &inside));
    json!({
        "id": period.id(),
        "label": period.label(),
        "start": start.map(format_date),
        "totalTokens": period_tokens,
        "estimatedCost": round6(period_cost),
        "agents": agents,
    })
}

/// Cursor turns (one per generation with a stop), active time, sessions, and models.
fn cursor_json(records: &[CursorRecord], inside: &dyn Fn(f64) -> bool) -> Value {
    let mut prompts: BTreeMap<&str, (f64, Option<&str>)> = BTreeMap::new();
    for record in records.iter().filter(|r| r.event == "prompt") {
        let time = parse_date(&record.ts).unwrap_or(0.0);
        prompts.entry(record.generation.as_str()).or_insert((time, record.model.as_deref()));
    }
    let mut total = Tally::default();
    let mut models: BTreeMap<String, Tally> = BTreeMap::new();
    for record in records {
        if inside(parse_date(&record.ts).unwrap_or(0.0)) {
            total.sessions.insert(record.conversation.clone());
        }
    }
    let mut counted: HashSet<&str> = HashSet::new();
    for record in records.iter().filter(|r| r.event == "stop") {
        if !counted.insert(record.generation.as_str()) {
            continue;
        }
        let stopped = parse_date(&record.ts).unwrap_or(0.0);
        if !inside(stopped) {
            continue;
        }
        let prompt = prompts.get(record.generation.as_str());
        let active = prompt.map_or(0.0, |(started, _)| (stopped - started).clamp(0.0, MAX_TURN_SECONDS));
        let model = record.model.as_deref().or(prompt.and_then(|p| p.1)).unwrap_or("unknown");
        for tally in [&mut total, models.entry(model_name(model)).or_default()] {
            tally.turns += 1;
            tally.active += active;
            tally.sessions.insert(record.conversation.clone());
        }
    }
    let mut value = total.json(("agent", json!("cursor")), true);
    value["name"] = json!("Cursor");
    value["models"] = Value::Array(sorted_models(models, true));
    value
}

/// The usage report for `periods`, in the JSON shape of docs/USAGE.md.
pub fn build(snapshot: &Snapshot, periods: &[Period]) -> Value {
    json!({
        "estimateNote": format!("Costs are estimates at API list prices (checked {PRICES_CHECKED}), not what a subscription charges."),
        "generatedAt": format_date(now()),
        "limits": limits_json(snapshot),
        "periods": periods.iter().map(|p| period_json(snapshot, *p, p.start())).collect::<Vec<_>>(),
    })
}

/// Reads the logs and builds the report.
pub fn report(periods: &[Period]) -> Value {
    build(&collect(), periods)
}

// MARK: Text

pub fn format_tokens(tokens: u64) -> String {
    if tokens < 1000 {
        return tokens.to_string();
    }
    let mut value = tokens as f64;
    for unit in ["K", "M", "B"] {
        value /= 1000.0;
        if (value * 10.0).round() / 10.0 < 1000.0 || unit == "B" {
            return format!("{value:.1}{unit}");
        }
    }
    unreachable!()
}

/// `~$9.87`, `<$0.01`, `no price` (tokens but nothing priced), or `` (nothing to price).
pub fn format_cost(cost: Option<f64>, tokens: u64) -> String {
    match cost {
        None if tokens > 0 => "no price".into(),
        None => String::new(),
        Some(cost) if cost > 0.0 && cost < 0.01 => "<$0.01".into(),
        Some(cost) if cost > 0.0 => format!("~${cost:.2}"),
        Some(_) => String::new(),
    }
}

pub fn format_duration(seconds: u64) -> String {
    let minutes = (seconds as f64 / 60.0).round() as u64;
    match minutes {
        0 if seconds > 0 => "<1m".into(),
        m if m >= 60 => format!("{}h {:02}m", m / 60, m % 60),
        m => format!("{m}m"),
    }
}

fn plural(count: u64, word: &str) -> String {
    format!("{count} {word}{}", if count == 1 { "" } else { "s" })
}

/// One agent's (or model's) summary: "3 sessions · 12.4M tokens · ~$9.87", or Cursor's turns.
pub fn summary(value: &Value) -> String {
    let n = |key: &str| value.get(key).and_then(Value::as_u64).unwrap_or(0);
    let mut parts = Vec::new();
    if n("sessions") > 0 {
        parts.push(plural(n("sessions"), "session"));
    }
    if n("turns") > 0 {
        parts.push(plural(n("turns"), "turn"));
        if n("activeSeconds") > 0 {
            parts.push(format!("{} active", format_duration(n("activeSeconds"))));
        }
    }
    if n("totalTokens") > 0 {
        parts.push(format!("{} tokens", format_tokens(n("totalTokens"))));
        let cost = format_cost(value.get("estimatedCost").and_then(Value::as_f64), n("totalTokens"));
        if !cost.is_empty() {
            parts.push(cost);
        }
    }
    if parts.is_empty() {
        "—".into()
    } else {
        parts.join(" · ")
    }
}

fn period_heading(period: &Value) -> String {
    let label = period["label"].as_str().unwrap_or("");
    let start = period["start"]
        .as_str()
        .and_then(parse_date)
        .and_then(|s| chrono::DateTime::from_timestamp(s as i64, 0));
    match (period["id"].as_str(), start) {
        (Some("week"), Some(start)) => format!("{label} (since {})", start.with_timezone(&Local).format("%a %b %-d")),
        (Some("month"), Some(start)) => format!("{label} ({})", start.with_timezone(&Local).format("%B")),
        (Some("today"), Some(start)) => format!("{label} ({})", start.with_timezone(&Local).format("%a %b %-d")),
        _ => label.to_string(),
    }
}

fn local_time(seconds: f64) -> Option<chrono::DateTime<Local>> {
    chrono::DateTime::from_timestamp(seconds as i64, 0).map(|t| t.with_timezone(&Local))
}

/// One plan's limits on a line: "5-hour 24% (resets 3:10 PM) · Weekly 0% (reset) · prolite".
pub fn limit_summary(limit: &Value) -> String {
    if limit["available"].as_bool() != Some(true) {
        return if limit["agent"] == "cursor" {
            "not available".into()
        } else {
            "no data yet".into()
        };
    }
    let now = now();
    let mut parts: Vec<String> = Vec::new();
    for window in limit["windows"].as_array().into_iter().flatten() {
        let used = window["usedPercent"].as_f64().unwrap_or(0.0).round() as i64;
        let mut part = format!("{} {used}%", window["label"].as_str().unwrap_or(""));
        if window["reset"].as_bool() == Some(true) {
            part.push_str(" (reset)");
        } else if let Some(at) = window["resetsAt"].as_str().and_then(parse_date).and_then(local_time) {
            let format = if at.timestamp() as f64 - now < 86400.0 {
                "%-I:%M %p"
            } else {
                "%a %b %-d"
            };
            part.push_str(&format!(" (resets {})", at.format(format)));
        }
        if let (Some(used), Some(limit)) = (window["usedUSD"].as_f64(), window["limitUSD"].as_f64()) {
            part.push_str(&format!(" (${used:.2} of ${limit:.2})"));
        }
        parts.push(part);
    }
    if let Some(plan) = limit["plan"].as_str() {
        parts.push(plan.to_string());
    }
    if let Some(observed) = limit["observedAt"].as_str().and_then(parse_date) {
        if now - observed > 600.0 {
            if let Some(at) = local_time(observed) {
                parts.push(format!("as of {}", at.format("%b %-d %-I:%M %p")));
            }
        }
    }
    parts.join(" · ")
}

/// The report as text. `detailed` adds a line per model.
pub fn text(report: &Value, detailed: bool) -> String {
    let mut out = String::from("Barista usage · costs are estimates at API list prices, not your subscription bill\n");
    out.push_str("\nLimits\n");
    for limit in report["limits"].as_array().into_iter().flatten() {
        out.push_str(&format!(
            "  {:<12}  {}\n",
            limit["name"].as_str().unwrap_or(""),
            limit_summary(limit)
        ));
    }
    for period in report["periods"].as_array().into_iter().flatten() {
        out.push_str(&format!("\n{}\n", period_heading(period)));
        for agent in period["agents"].as_array().into_iter().flatten() {
            out.push_str(&format!("  {:<12}  {}\n", agent["name"].as_str().unwrap_or(""), summary(agent)));
            if detailed {
                for model in agent["models"].as_array().into_iter().flatten() {
                    out.push_str(&format!(
                        "      {:<28}  {}\n",
                        model["model"].as_str().unwrap_or(""),
                        summary(model)
                    ));
                }
            }
        }
        let tokens = period["totalTokens"].as_u64().unwrap_or(0);
        if tokens > 0 {
            let cost = format_cost(period["estimatedCost"].as_f64(), tokens);
            out.push_str(&format!(
                "  {:<12}  {} tokens{}\n",
                "Total",
                format_tokens(tokens),
                if cost.is_empty() { String::new() } else { format!(" · {cost}") }
            ));
        }
    }
    out.push_str(&format!(
        "\nCursor reports turns, not tokens. Prices checked {PRICES_CHECKED}; Barista {VERSION}.\n"
    ));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn limit_windows_are_labeled_by_length() {
        assert_eq!(window_label(300), "5-hour");
        assert_eq!(window_label(10080), "Weekly");
        assert_eq!(window_label(60), "1-hour");
        assert_eq!(window_label(720), "12-hour");
        assert_eq!(window_label(1440), "1-day");
        assert_eq!(window_label(43200), "30-day");
    }

    #[test]
    fn reset_windows_read_zero() {
        let now = 2_000_000_000.0;
        let open = window_json("primary", "Weekly".into(), Some(10080), 6.0, Some(now + 60.0), now);
        assert_eq!(open["usedPercent"], 6.0);
        assert_eq!(open["reset"], false);
        let reset = window_json("secondary", "5-hour".into(), Some(300), 55.0, Some(now - 1.0), now);
        assert_eq!(reset["usedPercent"], 0.0);
        assert_eq!(reset["reset"], true);
        let unknown = window_json("five_hour", "5-hour".into(), Some(300), 5.0, None, now);
        assert_eq!(unknown["resetsAt"], Value::Null);
        assert_eq!(unknown["reset"], false);
    }

    #[test]
    fn the_newest_claude_source_wins() {
        let status = ClaudeLimits {
            source: "Claude Code",
            observed_at: 100.0,
            rate_limits: json!({}),
        };
        let app = ClaudeLimits {
            source: "Claude app",
            observed_at: 200.0,
            rate_limits: json!({}),
        };
        assert_eq!(
            newest_claude_limits([None, Some(status.clone()), Some(app.clone())])
                .unwrap()
                .source,
            "Claude app"
        );
        let older_app = ClaudeLimits {
            observed_at: 50.0,
            ..app.clone()
        };
        assert_eq!(
            newest_claude_limits([Some(status.clone()), Some(older_app.clone())])
                .unwrap()
                .source,
            "Claude Code"
        );
        assert_eq!(newest_claude_limits([None, None, Some(older_app)]).unwrap().source, "Claude app");
        let account = ClaudeLimits {
            source: "Claude account",
            observed_at: 300.0,
            rate_limits: json!({}),
        };
        assert_eq!(
            newest_claude_limits([Some(account), Some(status), Some(app)]).unwrap().source,
            "Claude account"
        );
        assert!(newest_claude_limits([None, None, None]).is_none());
    }

    #[test]
    fn claude_limits_json_keeps_spend_details() {
        let limits = ClaudeLimits {
            source: "Claude Code",
            observed_at: 1_000.0,
            rate_limits: json!({
                "five_hour": {"used_percentage": 23.5, "resets_at": 4_102_444_800u64},
                "spend_limit": {"used_percentage": 62.8, "resets_at": 4_102_444_800u64, "used_usd": 314.12, "limit_usd": 500, "period": "monthly"}
            }),
        };
        let value = claude_limits_json(Some(&limits), 2_000.0);
        let windows = value["windows"].as_array().unwrap();
        assert_eq!(windows.len(), 2);
        assert_eq!(windows[0]["label"], "5-hour");
        assert_eq!(windows[1]["id"], "spend_limit");
        assert_eq!(windows[1]["limitUSD"], 500);
        assert_eq!(windows[1]["period"], "monthly");
        assert_eq!(claude_limits_json(None, 0.0)["available"], false);
    }

    #[test]
    fn codex_limits_survive_duplicate_token_counts() {
        let mut state = FileState {
            agent: "codex".into(),
            ..FileState::default()
        };
        let event = |ts: &str, used: f64| {
            format!(
                r#"{{"timestamp":"{ts}","type":"event_msg","payload":{{"type":"token_count","info":null,"rate_limits":{{"primary":{{"used_percent":{used},"window_minutes":10080,"resets_at":4102444800}},"secondary":null,"plan_type":"plus"}}}}}}"#
            )
        };
        codex_line(event("2026-10-30T12:00:00Z", 6.0).as_bytes(), Path::new("x.jsonl"), &mut state);
        codex_line(event("2026-10-30T11:00:00Z", 1.0).as_bytes(), Path::new("x.jsonl"), &mut state);
        let limits = state.limits.clone().unwrap();
        assert_eq!(
            limits.primary.unwrap()["used_percent"],
            6.0,
            "an older report never replaces a newer one"
        );
        assert_eq!(limits.plan.as_deref(), Some("plus"));
        assert!(state.buckets.is_empty(), "limits alone add no tokens");
    }

    #[test]
    fn prices_match_the_longest_entry() {
        assert_eq!(price_for("claude-opus-5-5").unwrap().model, "claude-opus-5-5");
        assert_eq!(price_for("claude-opus-5").unwrap().model, "claude-opus-5");
        assert_eq!(price_for("claude-opus-4-1-20250805").unwrap().model, "claude-opus-4-1");
        assert_eq!(price_for("claude-haiku-5-5[1m]").unwrap().model, "claude-haiku-5-5");
        assert_eq!(price_for("gpt-5.1-codex").unwrap().model, "gpt-5.1");
        assert_eq!(price_for("gpt-5-mini").unwrap().model, "gpt-5-mini");
        assert_eq!(price_for("gpt-5.5-pro").unwrap().model, "gpt-5.5-pro");
        assert!(price_for("gpt-5x").is_none());
        assert!(price_for("codex-auto-review").is_none());
        assert_eq!(model_name("claude-opus-5-5@20260101"), "claude-opus-5-5");
    }

    #[test]
    fn formats_tokens_and_costs() {
        assert_eq!(format_tokens(950), "950");
        assert_eq!(format_tokens(12_400), "12.4K");
        assert_eq!(format_tokens(3_100_000), "3.1M");
        assert_eq!(format_tokens(999_960), "1.0M");
        assert_eq!(format_tokens(1_200_000_000), "1.2B");
        assert_eq!(format_cost(Some(9.874), 10), "~$9.87");
        assert_eq!(format_cost(Some(0.004), 10), "<$0.01");
        assert_eq!(format_cost(None, 10), "no price");
        assert_eq!(format_cost(None, 0), "");
        assert_eq!(format_duration(330), "6m");
        assert_eq!(format_duration(3900), "1h 05m");
    }

    #[test]
    fn codex_deltas_skip_duplicates() {
        let mut prev = None;
        assert_eq!(codex_delta(&mut prev, [1000, 400, 50, 1050]), Some([1000, 400, 50, 1050]));
        assert_eq!(codex_delta(&mut prev, [1000, 400, 50, 1050]), None);
        assert_eq!(codex_delta(&mut prev, [3000, 2400, 150, 3150]), Some([2000, 2000, 100, 2100]));
        assert_eq!(codex_delta(&mut prev, [10, 0, 0, 10]), None, "a smaller total is a duplicate");
    }

    #[test]
    fn periods_start_on_midnight_monday_and_the_first() {
        let saturday = NaiveDate::from_ymd_opt(2026, 11, 7).unwrap();
        assert_eq!(Period::Today.start_date(saturday), Some(saturday));
        assert_eq!(Period::Week.start_date(saturday), NaiveDate::from_ymd_opt(2026, 11, 2));
        assert_eq!(Period::Month.start_date(saturday), NaiveDate::from_ymd_opt(2026, 11, 1));
        let monday = NaiveDate::from_ymd_opt(2026, 11, 2).unwrap();
        assert_eq!(Period::Week.start_date(monday), Some(monday));
        assert_eq!(Period::All.start_date(monday), None);
        assert!(Period::Week.start().unwrap() <= Period::Today.start().unwrap());
    }

    #[test]
    fn cursor_turns_use_their_prompt_and_stop() {
        let record = |event: &str, generation: &str, ts: &str, model: Option<&str>| CursorRecord {
            conversation: "c1".into(),
            event: event.into(),
            generation: generation.into(),
            model: model.map(String::from),
            status: None,
            ts: ts.into(),
        };
        let records = vec![
            record("prompt", "g1", "2026-10-30T13:00:00Z", Some("m")),
            record("stop", "g1", "2026-10-30T13:05:00Z", None),
            record("stop", "g1", "2026-10-30T13:06:00Z", None),
            record("stop", "g2", "2026-10-30T13:07:00Z", None),
        ];
        let value = cursor_json(&records, &|_| true);
        assert_eq!(value["turns"], 2);
        assert_eq!(value["activeSeconds"], 300);
        assert_eq!(value["models"][0]["model"], "m");
        assert_eq!(value["models"][1]["model"], "unknown");
        assert!(value["estimatedCost"].is_null());
    }
}
