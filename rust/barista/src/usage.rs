//! Token and session usage for Claude Code, Codex, and Cursor, read from files on this computer.
//! The specification is docs/USAGE.md; the Swift app implements the same rules.
use crate::hooks::write_atomically;
use crate::models::{format_date, now, parse_date, to_json, VERSION};
use chrono::{Datelike, Duration, Local, NaiveDate, TimeZone};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fs;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

pub const PRICES_CHECKED: &str = env!("BARISTA_PRICES_CHECKED");
const CACHE_FILE: &str = "usage-cache-portable.json";
const CURSOR_FILE: &str = "cursor-usage.jsonl";
const CACHE_VERSION: u32 = 4;
const CLAUDE_LIMITS_FILE: &str = "claude-limits.json";
/// A Cursor turn's active time is capped, so a stop that never arrived can't count for days.
const MAX_TURN_SECONDS: f64 = 4.0 * 3600.0;
/// Activity less than 15 minutes apart joins into one run of active time.
const JOIN_SECONDS: i64 = 900;
const REPORT_FILE: &str = "usage-report.html";
/// Sources/BaristaCore/ReportTemplate.swift's page, extracted by build.rs.
const REPORT_TEMPLATE: &str = include_str!(concat!(env!("OUT_DIR"), "/report-template.html"));
const REPORT_PLACEHOLDER: &str = "/*BARISTA_REPORT_DATA*/null";

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
    /// The first `turn_context` folder, used when no `session_meta` names one.
    #[serde(default)]
    context_folder: Option<String>,
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
    /// The folder the agent worked in (Claude: first `cwd`; Codex: first `session_meta` cwd).
    #[serde(default)]
    folder: Option<String>,
    /// Activity as `[first, last]` whole seconds, joined by the 15-minute rule as lines arrive.
    #[serde(default)]
    runs: Vec<[i64; 2]>,
}

impl FileState {
    /// The file's folder, if any line named one.
    fn folder(&self) -> Option<String> {
        self.folder.clone().or_else(|| self.codex.context_folder.clone())
    }

    /// Records one moment of activity (whole seconds since 1970).
    fn note_activity(&mut self, time: f64) {
        let second = time.floor() as i64;
        if let Some(last) = self.runs.last_mut() {
            if last[1] <= second && second <= last[1] + JOIN_SECONDS {
                last[1] = second;
                return;
            }
            if last[0] <= second && second <= last[1] {
                return;
            }
        }
        self.runs.push([second, second]);
    }
}

/// A string value that isn't empty.
fn non_empty<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    value.get(key).and_then(Value::as_str).filter(|s| !s.is_empty())
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

/// Follows one Claude Code line: the folder, activity, and tokens of a new assistant response.
fn claude_line(line: &[u8], path: &Path, state: &mut FileState, seen: &mut HashSet<String>) {
    let text = String::from_utf8_lossy(line);
    let wants_folder = state.folder.is_none() && text.contains("\"cwd\"");
    if !wants_folder && !text.contains("\"assistant\"") && !text.contains("\"user\"") {
        return;
    }
    let Ok(value) = serde_json::from_str::<Value>(&text) else { return };
    if state.folder.is_none() {
        state.folder = non_empty(&value, "cwd").map(String::from);
    }
    let kind = value.get("type").and_then(Value::as_str);
    if kind != Some("assistant") && kind != Some("user") {
        return;
    }
    let Some(time) = value.get("timestamp").and_then(Value::as_str).and_then(parse_date) else {
        return;
    };
    state.note_activity(time);
    if kind != Some("assistant") {
        return;
    }
    let Some(message) = value.get("message") else { return };
    let Some(usage) = message.get("usage").filter(|u| u.is_object()) else {
        return;
    };
    let Some(model) = message.get("model").and_then(Value::as_str).filter(|m| *m != "<synthetic>") else {
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

/// Follows one Codex rollout line: session, model, folder, activity, and token counts.
fn codex_line(line: &[u8], path: &Path, state: &mut FileState) {
    let text = String::from_utf8_lossy(line);
    if !["\"token_count\"", "\"user_message\"", "\"turn_context\"", "\"session_meta\""]
        .iter()
        .any(|kind| text.contains(kind))
    {
        return;
    }
    let Ok(value) = serde_json::from_str::<Value>(&text) else { return };
    let payload = value.get("payload").cloned().unwrap_or(Value::Null);
    match value.get("type").and_then(Value::as_str) {
        Some("session_meta") => {
            let id = payload.get("id").or_else(|| payload.get("session_id")).and_then(Value::as_str);
            if let Some(id) = id {
                state.codex.session = Some(id.to_string());
            }
            if state.folder.is_none() {
                state.folder = non_empty(&payload, "cwd").map(String::from);
            }
        }
        Some("turn_context") => {
            if let Some(model) = payload.get("model").and_then(Value::as_str) {
                state.codex.model = Some(model.to_string());
            }
            if state.codex.context_folder.is_none() {
                state.codex.context_folder = non_empty(&payload, "cwd").map(String::from);
            }
        }
        Some("event_msg") => {
            let event = payload.get("type").and_then(Value::as_str);
            if event == Some("user_message") || event == Some("token_count") {
                if let Some(time) = value.get("timestamp").and_then(Value::as_str).and_then(parse_date) {
                    state.note_activity(time);
                }
            }
            if event != Some("token_count") {
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
        _ => {}
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
    /// The folder of the log file this bucket came from.
    pub folder: Option<String>,
}

/// One log file's activity runs, for active time.
#[derive(Clone, Debug)]
pub struct Activity {
    pub agent: String,
    pub folder: Option<String>,
    pub runs: Vec<[i64; 2]>,
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
    /// The first of Cursor's workspace roots (records before 0.0.12 have none).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    pub ts: String,
}

#[derive(Clone, Debug, Default)]
pub struct Snapshot {
    pub entries: Vec<Entry>,
    pub activity: Vec<Activity>,
    pub cursor: Vec<CursorRecord>,
    pub codex_limits: Option<CodexLimits>,
    /// `codex_limits` came from your ChatGPT account rather than Codex's logs.
    pub codex_from_account: bool,
    pub claude_limits: Option<ClaudeLimits>,
    /// The Claude plan's name, from Claude Code's profile on this computer.
    pub claude_plan: Option<String>,
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
    let mut activity = Vec::new();
    for state in cache.files.values() {
        let folder = state.folder();
        if !state.runs.is_empty() {
            activity.push(Activity {
                agent: state.agent.clone(),
                folder: folder.clone(),
                runs: state.runs.clone(),
            });
        }
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
                folder: folder.clone(),
            });
        }
    }
    let logged = cache
        .files
        .values()
        .filter_map(|f| f.limits.clone())
        .max_by(|a, b| a.observed_at.total_cmp(&b.observed_at));
    let (codex_limits, codex_from_account) = newest_codex_limits(crate::codex_account::read(directory, now()), logged);
    Snapshot {
        entries,
        activity,
        cursor: read_cursor(directory),
        codex_limits,
        codex_from_account,
        claude_limits: newest_claude_limits([
            crate::claude_account::read(directory, now()),
            read_claude_status_limits(directory),
            read_claude_app_limits(now()),
        ]),
        claude_plan: read_claude_plan(),
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

/// The ChatGPT plan's limits observed most recently: your account's or Codex's logs' (your account
/// on a tie), and whether they are your account's.
pub fn newest_codex_limits(account: Option<CodexLimits>, logged: Option<CodexLimits>) -> (Option<CodexLimits>, bool) {
    match (account, logged) {
        (Some(account), Some(logged)) if logged.observed_at > account.observed_at => (Some(logged), false),
        (Some(account), _) => (Some(account), true),
        (None, logged) => (logged, false),
    }
}

/// The Claude plan's name from the profile Claude Code keeps on this computer (`oauthAccount` in
/// `.claude.json`), with no network request. None when the profile doesn't say.
fn read_claude_plan() -> Option<String> {
    let directory = std::env::var("CLAUDE_CONFIG_DIR")
        .ok()
        .and_then(|value| value.split(',').map(str::trim).find(|entry| !entry.is_empty()).map(PathBuf::from))
        .or_else(dirs::home_dir)?;
    let profile: Value = serde_json::from_slice(&fs::read(directory.join(".claude.json")).ok()?).ok()?;
    let account = profile.get("oauthAccount").filter(|a| a.is_object())?;
    let kind = non_empty(account, "organizationType").unwrap_or("");
    let tier = non_empty(account, "userRateLimitTier")
        .or_else(|| non_empty(account, "organizationRateLimitTier"))
        .unwrap_or("");
    claude_plan_name(kind, tier).map(String::from)
}

/// "Max 5x", "Team", "Pro"…, from the profile's organization type and rate-limit tier. The first
/// match wins (docs/USAGE.md, "Claude plan name").
pub fn claude_plan_name(kind: &str, tier: &str) -> Option<&'static str> {
    if kind.contains("enterprise") {
        Some("Enterprise")
    } else if kind.contains("team") {
        Some("Team")
    } else if tier.contains("max_20x") {
        Some("Max 20x")
    } else if tier.contains("max_5x") {
        Some("Max 5x")
    } else if tier.contains("max") || kind.contains("max") {
        Some("Max")
    } else if tier.contains("pro") || kind.contains("pro") {
        Some("Pro")
    } else {
        None
    }
}

/// Whether a window is on course to run out before it resets (docs/USAGE.md, "Pace"). Every part
/// is None when unknown.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Pace {
    /// How much of the window has passed: where even use would put the bar.
    pub elapsed_percent: Option<f64>,
    /// The percentage at the reset if the rate so far holds (it can pass 100).
    pub projected_percent: Option<f64>,
    /// When the window runs out at this rate, if that is before it resets.
    pub limit_at: Option<f64>,
    /// "over" (on course to run out before the reset) or "ok".
    pub pace: Option<&'static str>,
}

/// Rounded to one decimal, half away from zero.
fn round1(value: f64) -> f64 {
    (value * 10.0).round() / 10.0
}

/// The pace of a window of `minutes` that is `used` percent full and resets at `resets_at`.
pub fn pace(minutes: Option<u64>, used: f64, resets_at: Option<f64>, now: f64) -> Pace {
    let (Some(minutes), Some(resets_at)) = (minutes.filter(|m| *m > 0), resets_at.filter(|at| *at > now)) else {
        return Pace::default();
    };
    let window = minutes as f64 * 60.0;
    let passed = window - (resets_at - now);
    if passed <= 0.0 {
        // The reset is more than a whole window away.
        return Pace::default();
    }
    let elapsed = (passed / window).min(1.0);
    let mut pace = Pace {
        elapsed_percent: Some(round1(elapsed * 100.0)),
        ..Pace::default()
    };
    // Early in a window one burst projects nonsense.
    if elapsed < 0.08 || used < 1.0 {
        return pace;
    }
    let projected = used / elapsed;
    pace.projected_percent = Some(round1(projected));
    pace.pace = Some(if projected >= 100.0 { "over" } else { "ok" });
    if used < 100.0 {
        let limit_at = now + (100.0 - used) * (elapsed * window) / used;
        pace.limit_at = Some(limit_at).filter(|at| *at < resets_at);
    }
    pace
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
    let pace = pace(minutes, used, resets_at, now);
    json!({
        "id": id,
        "label": label,
        "windowMinutes": minutes,
        "usedPercent": if reset { 0.0 } else { used },
        "resetsAt": resets_at.map(format_date),
        "reset": reset,
        "elapsedPercent": pace.elapsed_percent,
        "projectedPercent": pace.projected_percent,
        "limitAt": pace.limit_at.map(format_date),
        "pace": pace.pace,
    })
}

fn claude_limits_json(limits: Option<&ClaudeLimits>, plan: Option<&str>, now: f64) -> Value {
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
            "observedAt": format_date(limits.observed_at), "plan": plan, "credits": null, "windows": windows,
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
        "observedAt": format_date(limits.observed_at), "plan": plan, "credits": null, "windows": windows,
    })
}

fn codex_limits_json(limits: Option<&CodexLimits>, from_account: bool, now: f64) -> Value {
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
        "agent": "codex", "name": "ChatGPT plan", "available": true,
        "source": if from_account { "ChatGPT account" } else { "Codex" },
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
        claude_limits_json(snapshot.claude_limits.as_ref(), snapshot.claude_plan.as_deref(), now),
        codex_limits_json(snapshot.codex_limits.as_ref(), snapshot.codex_from_account, now),
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
pub fn record_cursor(event: &str, conversation: &str, generation: &str, model: Option<&str>, project: Option<&str>, status: Option<&str>) {
    let record = CursorRecord {
        conversation: conversation.to_string(),
        event: event.to_string(),
        generation: generation.to_string(),
        model: model.map(String::from),
        project: project.filter(|p| !p.is_empty()).map(String::from),
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

fn period_json(snapshot: &Snapshot, period: Period, start: Option<f64>, roots: &mut Roots) -> Value {
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
    let (projects, active) = projects_json(snapshot, start, now, roots);
    json!({
        "id": period.id(),
        "label": period.label(),
        "start": start.map(format_date),
        "totalTokens": period_tokens,
        "estimatedCost": round6(period_cost),
        "activeSeconds": active,
        "agents": agents,
        "projects": projects,
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

// MARK: Projects

const AGENTS: [(&str, &str); 3] = [("claude", "Claude Code"), ("codex", "Codex"), ("cursor", "Cursor")];

fn is_separator(c: char) -> bool {
    c == '/' || (cfg!(windows) && c == '\\')
}

/// A project's name: its folder's last path component.
pub fn project_name(project: &str) -> String {
    let trimmed = project.trim_end_matches(is_separator);
    let name = trimmed.rsplit(is_separator).next().unwrap_or("");
    if name.is_empty() {
        project.to_string()
    } else {
        name.to_string()
    }
}

/// `path` with `.` and `..` components resolved lexically.
fn lexically_normal(path: &Path) -> PathBuf {
    use std::path::Component;
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if matches!(out.components().next_back(), Some(Component::Normal(_))) {
                    out.pop();
                } else if !out.has_root() {
                    out.push("..");
                }
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// For a `.git` file in `directory` pointing into a linked worktree's admin folder
/// (`…/.git/worktrees/NAME`), the main checkout: the directory holding that `.git`.
fn worktree_main(directory: &Path, git_file: &Path) -> Option<PathBuf> {
    let text = fs::read_to_string(git_file).ok()?;
    let first = text.lines().next()?;
    let target = first.strip_prefix("gitdir:")?.trim();
    if target.is_empty() {
        return None;
    }
    let target = lexically_normal(&directory.join(target));
    let parts: Vec<_> = target.components().collect();
    let index = parts
        .windows(2)
        .position(|pair| pair[0].as_os_str() == ".git" && pair[1].as_os_str() == "worktrees")?;
    let main: PathBuf = parts[..index].iter().map(|c| c.as_os_str()).collect();
    (!main.as_os_str().is_empty()).then_some(main)
}

/// The project a folder belongs to: the Git repository holding it (a linked worktree counts as its
/// main checkout), else the folder itself. Purely by path; symlinks are not resolved.
pub fn project_root(folder: &str) -> String {
    let mut path = folder.trim_end_matches(is_separator);
    if path.is_empty() || (cfg!(windows) && path.ends_with(':')) {
        // `/` (or `C:\`) keeps its one separator.
        path = &folder[..(path.len() + 1).min(folder.len())];
    }
    let found = (|| {
        if !Path::new(path).exists() {
            return None;
        }
        let mut directory = Some(Path::new(path));
        while let Some(current) = directory.filter(|d| !d.as_os_str().is_empty()) {
            let git = current.join(".git");
            if let Ok(meta) = fs::metadata(&git) {
                if meta.is_dir() {
                    return Some(current.to_path_buf());
                }
                if meta.is_file() {
                    return Some(worktree_main(current, &git).unwrap_or_else(|| current.to_path_buf()));
                }
            }
            directory = current.parent();
        }
        None
    })();
    let root = found.map_or_else(|| path.to_string(), |p| p.to_string_lossy().into_owned());
    if cfg!(windows) {
        root.replace('/', "\\")
    } else {
        root
    }
}

/// Folder → project, resolved once per report.
#[derive(Default)]
struct Roots(HashMap<String, Option<String>>);

impl Roots {
    /// The project of `folder`; None for activity with no folder (the `Unknown` project).
    fn project(&mut self, folder: Option<&str>) -> Option<String> {
        let folder = folder.filter(|f| !f.is_empty())?;
        self.0
            .entry(folder.to_string())
            .or_insert_with(|| Some(project_root(folder)))
            .clone()
    }
}

/// Sorts spans and joins any two whose gap is at most 15 minutes.
fn joined(mut spans: Vec<[i64; 2]>) -> Vec<[i64; 2]> {
    spans.sort_unstable();
    let mut runs: Vec<[i64; 2]> = Vec::new();
    for span in spans {
        match runs.last_mut() {
            Some(last) if span[0] - last[1] <= JOIN_SECONDS => last[1] = last[1].max(span[1]),
            _ => runs.push(span),
        }
    }
    runs
}

/// Total length of `runs` clipped to `from` … `to`.
fn clipped_seconds(runs: &[[i64; 2]], from: Option<i64>, to: i64) -> i64 {
    runs.iter()
        .map(|run| (run[1].min(to) - run[0].max(from.unwrap_or(i64::MIN))).max(0))
        .sum()
}

fn local_date(second: i64) -> NaiveDate {
    chrono::DateTime::from_timestamp(second, 0)
        .map(|t| t.with_timezone(&Local).date_naive())
        .unwrap_or_default()
}

/// Seconds since 1970 at the local midnight that ends `date`.
fn next_midnight(date: NaiveDate) -> Option<i64> {
    let next = date.succ_opt()?;
    // A time zone may skip midnight for daylight saving time; the day then starts at the first valid hour.
    (0..4).find_map(|hour| {
        Local
            .from_local_datetime(&next.and_hms_opt(hour, 0, 0)?)
            .earliest()
            .map(|t| t.timestamp())
    })
}

#[derive(Default)]
struct Day {
    active: i64,
    tokens: u64,
    cost: f64,
}

#[derive(Default)]
struct ProjectTally {
    /// Claude, Codex, Cursor.
    agents: [Tally; 3],
    spans: [Vec<[i64; 2]>; 3],
    /// (model, agent) → tally.
    models: BTreeMap<(String, &'static str), Tally>,
    days: BTreeMap<NaiveDate, Day>,
}

/// One agent's (or a project's) totals, with `estimatedCost` null when none of its tokens are priced.
fn project_tally_json(tally: &Tally, extra: (&str, Value), cursor: bool) -> Value {
    let mut value = tally.json(extra, cursor);
    if token_total(&tally.counts) == tally.unpriced {
        value["estimatedCost"] = Value::Null;
    }
    value
}

/// Every project with activity in the period, and the period's active seconds (all projects together).
fn projects_json(snapshot: &Snapshot, start: Option<f64>, now: f64, roots: &mut Roots) -> (Vec<Value>, i64) {
    let from = start.map(|s| s.floor() as i64);
    let to = now.floor() as i64;
    let inside = |time: f64| time <= now && start.is_none_or(|start| time >= start);
    let index = |agent: &str| AGENTS.iter().position(|(id, _)| *id == agent);
    let mut all: BTreeMap<Option<String>, ProjectTally> = BTreeMap::new();

    for entry in &snapshot.entries {
        let Some(agent) = index(&entry.agent) else { continue };
        if !inside(entry.hour as f64) {
            continue;
        }
        let project = all.entry(roots.project(entry.folder.as_deref())).or_default();
        project.agents[agent].add_tokens(&entry.model, &entry.session, &entry.counts);
        project
            .models
            .entry((entry.model.clone(), AGENTS[agent].0))
            .or_default()
            .add_tokens(&entry.model, &entry.session, &entry.counts);
        let day = project.days.entry(local_date(entry.hour)).or_default();
        day.tokens += token_total(&entry.counts);
        if let Some(price) = price_for(&entry.model) {
            day.cost += cost(price, &entry.counts);
        }
    }
    for activity in &snapshot.activity {
        let Some(agent) = index(&activity.agent) else { continue };
        let project = all.entry(roots.project(activity.folder.as_deref())).or_default();
        project.spans[agent].extend_from_slice(&activity.runs);
    }

    // Cursor: a conversation's folder is the `project` of its first record that has one.
    let records = &snapshot.cursor;
    let mut folders: HashMap<&str, &str> = HashMap::new();
    for record in records {
        if let Some(project) = record.project.as_deref().filter(|p| !p.is_empty()) {
            folders.entry(record.conversation.as_str()).or_insert(project);
        }
    }
    let conversation_project = |roots: &mut Roots, conversation: &str| roots.project(folders.get(conversation).copied());
    let mut prompts: HashMap<&str, (f64, Option<&str>)> = HashMap::new();
    for record in records {
        let time = parse_date(&record.ts).unwrap_or(0.0);
        if record.event == "prompt" {
            prompts.entry(record.generation.as_str()).or_insert((time, record.model.as_deref()));
        }
        let project = all.entry(conversation_project(roots, &record.conversation)).or_default();
        let second = time.floor() as i64;
        project.spans[2].push([second, second]);
        if inside(time) {
            project.agents[2].sessions.insert(record.conversation.clone());
        }
    }
    let mut counted: HashSet<&str> = HashSet::new();
    for record in records.iter().filter(|r| r.event == "stop") {
        if !counted.insert(record.generation.as_str()) {
            continue;
        }
        let stopped = parse_date(&record.ts).unwrap_or(0.0);
        let prompt = prompts.get(record.generation.as_str());
        let active = prompt.map_or(0.0, |(started, _)| (stopped - started).clamp(0.0, MAX_TURN_SECONDS));
        let project = all.entry(conversation_project(roots, &record.conversation)).or_default();
        if let Some((started, _)) = prompt {
            let first = started.floor() as i64;
            project.spans[2].push([first, first + active.floor() as i64]);
        }
        if !inside(stopped) {
            continue;
        }
        let model = model_name(record.model.as_deref().or(prompt.and_then(|p| p.1)).unwrap_or("unknown"));
        let cursor = &mut project.agents[2];
        cursor.turns += 1;
        cursor.sessions.insert(record.conversation.clone());
        let tally = project.models.entry((model, "cursor")).or_default();
        tally.turns += 1;
        tally.active += active;
        tally.sessions.insert(record.conversation.clone());
    }

    let mut everything: Vec<[i64; 2]> = Vec::new();
    let mut listed: Vec<(i64, u64, u64, String, Option<String>, Value)> = Vec::new();
    for (path, mut project) in all {
        let spans: Vec<[i64; 2]> = project.spans.iter().flatten().copied().collect();
        everything.extend_from_slice(&spans);
        let runs = joined(spans);
        let active = clipped_seconds(&runs, from, to);

        let mut total = Tally {
            active: active as f64,
            ..Tally::default()
        };
        let mut agents = Vec::new();
        let mut cost_sum = 0.0;
        for (index, (agent, name)) in AGENTS.iter().enumerate() {
            let tally = &mut project.agents[index];
            tally.active = clipped_seconds(&joined(std::mem::take(&mut project.spans[index])), from, to) as f64;
            if tally.sessions.is_empty() && tally.counts[5] == 0 && tally.turns == 0 && tally.active <= 0.0 {
                continue;
            }
            add(&mut total.counts, &tally.counts);
            total.unpriced += tally.unpriced;
            total.turns += tally.turns;
            total.sessions.extend(tally.sessions.iter().map(|s| format!("{agent}\u{1f}{s}")));
            let mut value = project_tally_json(tally, ("agent", json!(agent)), *agent == "cursor");
            value["name"] = json!(name);
            cost_sum += value["estimatedCost"].as_f64().unwrap_or(0.0);
            agents.push(value);
        }
        total.cost = cost_sum;
        if total.sessions.is_empty() && total.counts[5] == 0 && total.turns == 0 && active <= 0 {
            continue;
        }

        let mut models: Vec<((String, &str), Tally)> = project.models.into_iter().collect();
        models.sort_by(|a, b| {
            token_total(&b.1.counts)
                .cmp(&token_total(&a.1.counts))
                .then(b.1.turns.cmp(&a.1.turns))
                .then(a.0 .0.cmp(&b.0 .0))
                .then(a.0 .1.cmp(b.0 .1))
        });
        let models: Vec<Value> = models
            .into_iter()
            .map(|((model, agent), tally)| {
                let mut value = tally.json(("model", json!(model)), agent == "cursor");
                value["agent"] = json!(agent);
                value
            })
            .collect();

        // Days: active time split at local midnight, tokens by each bucket's local date.
        for run in &runs {
            let mut cursor = run[0].max(from.unwrap_or(i64::MIN));
            let end = run[1].min(to);
            while cursor < end {
                let date = local_date(cursor);
                let stop = end.min(next_midnight(date).unwrap_or(end).max(cursor + 1));
                project.days.entry(date).or_default().active += stop - cursor;
                cursor = stop;
            }
        }
        let days: Vec<Value> = project
            .days
            .iter()
            .filter(|(_, day)| day.active > 0 || day.tokens > 0)
            .map(|(date, day)| {
                json!({
                    "date": date.format("%Y-%m-%d").to_string(),
                    "activeSeconds": day.active,
                    "totalTokens": day.tokens,
                    "estimatedCost": round6(day.cost),
                })
            })
            .collect();

        let name = path.as_deref().map_or_else(|| "Unknown".to_string(), project_name);
        let mut value = project_tally_json(&total, ("project", json!(path)), false);
        value["name"] = json!(name);
        value["agents"] = Value::Array(agents);
        value["models"] = Value::Array(models);
        value["days"] = Value::Array(days);
        listed.push((active, token_total(&total.counts), total.turns, name, path, value));
    }
    listed.sort_by(|a, b| {
        a.4.is_none()
            .cmp(&b.4.is_none())
            .then(b.0.cmp(&a.0))
            .then(b.1.cmp(&a.1))
            .then(b.2.cmp(&a.2))
            .then(a.3.cmp(&b.3))
            .then(a.4.cmp(&b.4))
    });
    let active = clipped_seconds(&joined(everything), from, to);
    (listed.into_iter().map(|p| p.5).collect(), active)
}

/// The usage report for `periods`, in the JSON shape of docs/USAGE.md.
pub fn build(snapshot: &Snapshot, periods: &[Period]) -> Value {
    let mut roots = Roots::default();
    json!({
        "estimateNote": format!("Costs are estimates at API list prices (checked {PRICES_CHECKED}), not what a subscription charges."),
        "generatedAt": format_date(now()),
        "limits": limits_json(snapshot),
        "periods": periods.iter().map(|p| period_json(snapshot, *p, p.start(), &mut roots)).collect::<Vec<_>>(),
    })
}

/// Reads the logs and builds the report.
pub fn report(periods: &[Period]) -> Value {
    build(&collect(), periods)
}

// MARK: Visual report

/// The report page's template with no data, for Barista's own window (it sends the data live).
pub fn report_template() -> &'static str {
    REPORT_TEMPLATE
}

/// Active seconds for each of the `days` days ending on `today`, oldest first, from the report's
/// all-time period (docs/USAGE.md, "Weekly summary"). None without that period.
pub fn daily_active_seconds(report: &Value, days: u32, today: NaiveDate) -> Option<Vec<(NaiveDate, u64)>> {
    let all = report["periods"].as_array()?.iter().find(|period| period["id"] == "all")?;
    let mut totals: HashMap<String, u64> = HashMap::new();
    for project in all["projects"].as_array().into_iter().flatten() {
        for day in project["days"].as_array().into_iter().flatten() {
            if let Some(date) = day["date"].as_str() {
                *totals.entry(date.to_string()).or_default() += day["activeSeconds"].as_u64().unwrap_or(0);
            }
        }
    }
    Some(
        (0..i64::from(days))
            .rev()
            .map(|back| {
                let date = today - Duration::days(back);
                (date, totals.get(&date.format("%Y-%m-%d").to_string()).copied().unwrap_or(0))
            })
            .collect(),
    )
}

/// Last week's totals (Monday to Sunday before `today`'s week).
#[derive(Debug, Clone, PartialEq)]
pub struct WeeklySummary {
    pub week_start: NaiveDate,
    pub active_seconds: u64,
    pub projects: usize,
    pub estimated_cost: f64,
}

impl WeeklySummary {
    /// "Last week: 14h 5m across 5 projects, about $38 at API prices."
    pub fn text(&self) -> String {
        let place = if self.projects == 1 {
            "in 1 project".to_string()
        } else {
            format!("across {} projects", self.projects)
        };
        let cost = if self.estimated_cost >= 10.0 {
            format!(", about ${:.0} at API prices", self.estimated_cost.round())
        } else if self.estimated_cost >= 0.01 {
            format!(", about ${:.2} at API prices", self.estimated_cost)
        } else {
            String::new()
        };
        format!("Last week: {} {place}{cost}.", duration_text(self.active_seconds))
    }
}

fn duration_text(seconds: u64) -> String {
    if seconds >= 3600 {
        format!("{}h {}m", seconds / 3600, seconds % 3600 / 60)
    } else {
        format!("{}m", seconds / 60)
    }
}

/// Last week's summary from the report's all-time period, or None with under a minute of activity.
pub fn last_week_summary(report: &Value, today: NaiveDate) -> Option<WeeklySummary> {
    let all = report["periods"].as_array()?.iter().find(|period| period["id"] == "all")?;
    let this_week = Period::Week.start_date(today)?;
    let week_start = this_week - Duration::days(7);
    let dates: Vec<String> = (0..7)
        .map(|offset| (week_start + Duration::days(offset)).format("%Y-%m-%d").to_string())
        .collect();
    let (mut seconds, mut cost, mut projects) = (0u64, 0f64, 0usize);
    for project in all["projects"].as_array().into_iter().flatten() {
        let days: Vec<&Value> = project["days"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|day| day["date"].as_str().is_some_and(|date| dates.iter().any(|d| d == date)))
            .collect();
        let active: u64 = days.iter().map(|day| day["activeSeconds"].as_u64().unwrap_or(0)).sum();
        if active > 0 || days.iter().any(|day| day["totalTokens"].as_u64().unwrap_or(0) > 0) {
            projects += 1;
        }
        seconds += active;
        cost += days.iter().map(|day| day["estimatedCost"].as_f64().unwrap_or(0.0)).sum::<f64>();
    }
    (seconds >= 60).then_some(WeeklySummary {
        week_start,
        active_seconds: seconds,
        projects,
        estimated_cost: cost,
    })
}

/// The report page: the template with the data placeholder replaced by `report` plus `initialPeriod`,
/// with `<`, `>`, and `&` escaped so no text from the data can close the script.
pub fn report_page(report: &Value, initial: Period) -> String {
    let mut object = report.clone();
    object["initialPeriod"] = json!(initial.id());
    let data = to_json(&object)
        .replace('&', "\\u0026")
        .replace('<', "\\u003c")
        .replace('>', "\\u003e");
    REPORT_TEMPLATE.replacen(REPORT_PLACEHOLDER, &data, 1)
}

/// Where `barista report` writes by default: `usage-report.html` in the data folder.
pub fn default_report_path() -> PathBuf {
    data_dir().join(REPORT_FILE)
}

/// Writes the visual report with every period (`initial` shown first) to `output` (else the data
/// folder), atomically, then opens it in the default browser when `open`. Returns the file's path and
/// whether a browser was asked to open it.
pub fn write_report(initial: Period, output: Option<&Path>, open: bool) -> crate::Result<(PathBuf, bool)> {
    let path = output.map_or_else(default_report_path, Path::to_path_buf);
    let path = if path.is_relative() {
        std::env::current_dir().map(|d| d.join(&path)).unwrap_or(path)
    } else {
        path
    };
    let report = report(&[Period::Today, Period::Week, Period::Month, Period::All]);
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        fs::create_dir_all(parent)?;
    }
    write_atomically(&path, report_page(&report, initial).as_bytes())?;
    let opened = open && open_in_browser(&path);
    Ok((path, opened))
}

/// Opens a file in the default browser without waiting for it (`xdg-open`, `explorer.exe`, `open`).
pub fn open_in_browser(path: &Path) -> bool {
    use std::process::{Command, Stdio};
    let program = if cfg!(windows) {
        "explorer.exe"
    } else if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    let mut command = Command::new(program);
    command.arg(path).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    match command.spawn() {
        Ok(mut child) => {
            // Reap it in the background so a long-running app leaves no zombie behind.
            std::thread::spawn(move || child.wait());
            true
        }
        Err(_) => false,
    }
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

/// A time of day when under 24 hours away ("3:10 PM"), else a date ("Sat Nov 14").
fn near_time(at: chrono::DateTime<Local>, now: f64) -> String {
    let format = if at.timestamp() as f64 - now < 86400.0 {
        "%-I:%M %p"
    } else {
        "%a %b %-d"
    };
    at.format(format).to_string()
}

/// One plan's limits on a line: "5-hour 62% (resets 3:10 PM · runs out 1:40 PM) · Weekly 0% (reset)
/// · prolite".
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
            part.push_str(&format!(" (resets {}", near_time(at, now)));
            // Pace: when it runs out at this rate, else where it is heading by the reset.
            if let Some(out) = window["limitAt"].as_str().and_then(parse_date).and_then(local_time) {
                part.push_str(&format!(" · runs out {}", near_time(out, now)));
            } else if let Some(projected) = window["projectedPercent"].as_f64() {
                part.push_str(&format!(" · on pace for {}%", projected.round() as i64));
            }
            part.push(')');
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

/// The Projects block: "  app           19m active · 2 sessions · 6.7K tokens · ~$0.02".
fn project_lines(projects: &[Value]) -> String {
    let mut out = String::from("\nProjects\n");
    if projects.is_empty() {
        out.push_str("  —\n");
        return out;
    }
    let shown = &projects[..projects.len().min(10)];
    let name = |p: &Value| p["name"].as_str().unwrap_or("").to_string();
    let width = shown.iter().map(|p| name(p).chars().count()).max().unwrap_or(0).min(24);
    for project in shown {
        let n = |key: &str| project[key].as_u64().unwrap_or(0);
        let mut label = name(project);
        if label.chars().count() > width {
            label = label.chars().take(width.saturating_sub(1)).collect::<String>() + "…";
        }
        let mut parts = vec![
            format!("{} active", format_duration(n("activeSeconds"))),
            plural(n("sessions"), "session"),
        ];
        if n("turns") > 0 {
            parts.push(plural(n("turns"), "turn"));
        }
        if n("totalTokens") > 0 {
            parts.push(format!("{} tokens", format_tokens(n("totalTokens"))));
            let cost = format_cost(project["estimatedCost"].as_f64(), n("totalTokens"));
            if !cost.is_empty() {
                parts.push(cost);
            }
        }
        let pad = width.saturating_sub(label.chars().count());
        out.push_str(&format!("  {label}{}  {}\n", " ".repeat(pad), parts.join(" · ")));
    }
    if projects.len() > shown.len() {
        out.push_str(&format!("  … and {} more\n", projects.len() - shown.len()));
    }
    out
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
        if detailed {
            out.push_str(&project_lines(period["projects"].as_array().map_or(&[][..], Vec::as_slice)));
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
        let value = claude_limits_json(Some(&limits), None, 2_000.0);
        let windows = value["windows"].as_array().unwrap();
        assert_eq!(windows.len(), 2);
        assert_eq!(windows[0]["label"], "5-hour");
        assert_eq!(windows[1]["id"], "spend_limit");
        assert_eq!(windows[1]["limitUSD"], 500);
        assert_eq!(windows[1]["period"], "monthly");
        assert_eq!(claude_limits_json(None, None, 0.0)["available"], false);
        // The plan's name shows only with limits.
        assert_eq!(claude_limits_json(Some(&limits), Some("Max 5x"), 2_000.0)["plan"], "Max 5x");
        assert_eq!(claude_limits_json(None, Some("Max 5x"), 0.0)["plan"], Value::Null);
    }

    #[test]
    fn pace_projects_the_rate_so_far() {
        let now = 1_000_000.0;
        // Half of a 5-hour window gone with 60% used: on course for 120%, out in 100 minutes.
        let half = pace(Some(300), 60.0, Some(now + 9000.0), now);
        assert_eq!(half.elapsed_percent, Some(50.0));
        assert_eq!(half.projected_percent, Some(120.0));
        assert_eq!(half.pace, Some("over"));
        assert_eq!(half.limit_at, Some(now + 6000.0));
        // A day into a week with 10% used: on course for 70%, so it lasts.
        let week = pace(Some(10080), 10.0, Some(now + 6.0 * 86400.0), now);
        assert_eq!(week.elapsed_percent, Some(14.3));
        assert_eq!(week.projected_percent, Some(70.0));
        assert_eq!((week.pace, week.limit_at), (Some("ok"), None));
        // Exactly on course for 100% is over, but it runs out at the reset, not before it.
        let even = pace(Some(300), 50.0, Some(now + 9000.0), now);
        assert_eq!((even.pace, even.limit_at), (Some("over"), None));
        // Used up: over, with nothing left to run out.
        let full = pace(Some(300), 100.0, Some(now + 9000.0), now);
        assert_eq!(
            (full.projected_percent, full.pace, full.limit_at),
            (Some(200.0), Some("over"), None)
        );
    }

    #[test]
    fn pace_is_unknown_early_or_without_a_window() {
        let now = 1_000_000.0;
        // Under 8% of the window gone, or under 1% used: only where even use would be.
        let early = pace(Some(300), 5.0, Some(now + 17700.0), now);
        assert_eq!(early.elapsed_percent, Some(1.7));
        assert_eq!((early.projected_percent, early.limit_at, early.pace), (None, None, None));
        let unused = pace(Some(10080), 0.5, Some(now + 6.0 * 86400.0), now);
        assert_eq!((unused.elapsed_percent, unused.pace), (Some(14.3), None));
        assert_eq!(pace(Some(300), 4.0, Some(now + 16560.0), now).pace, Some("ok"), "8% gone is enough");
        // No length, no reset time, a reset that has passed, or one more than a window away.
        for (minutes, resets_at) in [
            (None, Some(now + 60.0)),
            (Some(0), Some(now + 60.0)),
            (Some(300), None),
            (Some(300), Some(now)),
            (Some(300), Some(now - 60.0)),
            (Some(300), Some(now + 18000.0)),
            (Some(300), Some(now + 86400.0)),
        ] {
            assert_eq!(pace(minutes, 50.0, resets_at, now), Pace::default(), "{minutes:?} {resets_at:?}");
        }
        // Every window carries the fields, null when unknown.
        let window = window_json("primary", "Weekly".into(), Some(10080), 6.0, Some(now + 30.0 * 86400.0), now);
        for key in ["elapsedPercent", "projectedPercent", "limitAt", "pace"] {
            assert_eq!(window.get(key), Some(&Value::Null), "{key}");
        }
        let window = window_json("five_hour", "5-hour".into(), Some(300), 60.0, Some(now + 9000.0), now);
        assert_eq!(
            (&window["elapsedPercent"], &window["projectedPercent"], &window["pace"]),
            (&json!(50.0), &json!(120.0), &json!("over"))
        );
        assert_eq!(window["limitAt"], format_date(now + 6000.0));
    }

    #[test]
    fn limit_summary_adds_the_pace() {
        let now = now();
        let plan = |windows: Vec<Value>| json!({"agent": "claude", "available": true, "windows": windows});
        let hot = window_json("five_hour", "5-hour".into(), Some(300), 60.0, Some(now + 9000.0), now);
        let calm = window_json("seven_day", "Weekly".into(), Some(10080), 10.0, Some(now + 6.0 * 86400.0), now);
        let early = window_json("five_hour", "5-hour".into(), Some(300), 5.0, Some(now + 17700.0), now);
        let text = limit_summary(&plan(vec![hot, calm, early]));
        let parts: Vec<&str> = text.split(" · ").collect();
        assert!(parts[0].starts_with("5-hour 60% (resets "), "{text}");
        assert!(parts[1].starts_with("runs out ") && parts[1].ends_with(')'), "{text}");
        assert!(parts[2].starts_with("Weekly 10% (resets "), "{text}");
        assert_eq!(parts[3], "on pace for 70%)", "{text}");
        assert!(parts[4].starts_with("5-hour 5% (resets ") && parts[4].ends_with(')'), "{text}");
        assert_eq!(parts.len(), 5, "{text}");
    }

    #[test]
    fn claude_plan_names_follow_type_then_tier() {
        assert_eq!(claude_plan_name("claude_max", "default_claude_max_5x"), Some("Max 5x"));
        assert_eq!(claude_plan_name("claude_max", "default_claude_max_20x"), Some("Max 20x"));
        assert_eq!(claude_plan_name("claude_max", "default_claude_ai"), Some("Max"));
        assert_eq!(claude_plan_name("", "default_claude_max"), Some("Max"));
        assert_eq!(claude_plan_name("claude_team", "default_claude_max_5x"), Some("Team"));
        assert_eq!(claude_plan_name("claude_enterprise", ""), Some("Enterprise"));
        assert_eq!(claude_plan_name("claude_pro", ""), Some("Pro"));
        assert_eq!(claude_plan_name("", "default_claude_pro"), Some("Pro"));
        assert_eq!(claude_plan_name("something", "default_raven"), None);
        assert_eq!(claude_plan_name("", ""), None);
    }

    #[test]
    fn chatgpt_limits_come_from_the_newest_source() {
        let limits = |observed_at: f64| CodexLimits {
            observed_at,
            primary: Some(json!({"used_percent": 20.0, "window_minutes": 300, "resets_at": 4_102_444_800u64})),
            ..CodexLimits::default()
        };
        let source = |account: Option<f64>, logged: Option<f64>| {
            let (limits, from_account) = newest_codex_limits(account.map(limits), logged.map(limits));
            (limits.map(|l| l.observed_at), from_account)
        };
        assert_eq!(source(Some(200.0), Some(100.0)), (Some(200.0), true));
        assert_eq!(source(Some(100.0), Some(200.0)), (Some(200.0), false));
        assert_eq!(source(Some(100.0), Some(100.0)), (Some(100.0), true), "your account on a tie");
        assert_eq!(source(Some(100.0), None), (Some(100.0), true));
        assert_eq!(source(None, Some(100.0)), (Some(100.0), false));
        assert_eq!(source(None, None), (None, false));
        assert_eq!(codex_limits_json(Some(&limits(100.0)), true, 1_000.0)["source"], "ChatGPT account");
        let logged = codex_limits_json(Some(&limits(100.0)), false, 1_000.0);
        assert_eq!(logged["source"], "Codex");
        assert_eq!(logged["windows"][0]["label"], "5-hour");
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
    fn activity_joins_within_fifteen_minutes() {
        let mut state = FileState::default();
        for time in [100.0, 101.9, 1000.0, 50.0, 500.0, 2000.0] {
            state.note_activity(time);
        }
        assert_eq!(state.runs, vec![[100, 1000], [50, 500], [2000, 2000]]);
        assert_eq!(joined(state.runs.clone()), vec![[50, 1000], [2000, 2000]]);
        assert_eq!(joined(vec![[0, 10], [911, 920]]), vec![[0, 10], [911, 920]]);
        assert_eq!(clipped_seconds(&[[50, 1000], [2000, 2000]], Some(500), 900), 400);
        assert_eq!(clipped_seconds(&[[50, 1000]], None, 2000), 950);
    }

    #[test]
    fn projects_follow_git_roots_and_worktrees() {
        let root = std::env::temp_dir().join(format!("barista-projects-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("main/.git/worktrees/wt")).unwrap();
        fs::create_dir_all(root.join("main/src/deep")).unwrap();
        fs::create_dir_all(root.join("wt/sub")).unwrap();
        fs::create_dir_all(root.join("module")).unwrap();
        fs::create_dir_all(root.join("plain")).unwrap();
        fs::write(root.join("wt/.git"), "gitdir: ../main/.git/worktrees/wt\n").unwrap();
        fs::write(root.join("module/.git"), "gitdir: ../main/.git/modules/module\n").unwrap();
        let text = |p: PathBuf| p.to_string_lossy().into_owned();
        let main = text(root.join("main"));
        assert_eq!(project_root(&text(root.join("main/src/deep"))), main);
        assert_eq!(project_root(&format!("{}/", text(root.join("main")))), main);
        assert_eq!(
            project_root(&text(root.join("wt/sub"))),
            main,
            "a linked worktree is its main checkout"
        );
        assert_eq!(
            project_root(&text(root.join("module"))),
            text(root.join("module")),
            "a submodule is its own project"
        );
        assert_eq!(project_root(&text(root.join("plain"))), text(root.join("plain")));
        // Joined part by part, so the expected path uses the platform's separator throughout.
        let gone = text(root.join("main").join("gone"));
        assert_eq!(project_root(&gone), gone, "a folder that no longer exists is its own project");
        assert_eq!(project_name(&main), "main");
        assert_eq!(project_name("/a/b/"), "b");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn last_week_and_the_last_seven_days_come_from_all_time() {
        let report = json!({"periods": [{"id": "all", "projects": [
            {"days": [
                {"date": "2026-10-26", "activeSeconds": 3600, "totalTokens": 10, "estimatedCost": 1.25},
                {"date": "2026-11-01", "activeSeconds": 1800, "totalTokens": 5, "estimatedCost": 2.5},
                {"date": "2026-11-02", "activeSeconds": 600, "totalTokens": 1, "estimatedCost": 9.0}
            ]},
            {"days": [{"date": "2026-10-30", "activeSeconds": 0, "totalTokens": 7, "estimatedCost": 0.25}]},
            {"days": [{"date": "2026-10-20", "activeSeconds": 900, "totalTokens": 1, "estimatedCost": 0.5}]}
        ]}]});
        // Monday, Nov 2: last week is Oct 26 to Nov 1.
        let today = NaiveDate::from_ymd_opt(2026, 11, 2).unwrap();
        let summary = last_week_summary(&report, today).unwrap();
        assert_eq!(summary.week_start, NaiveDate::from_ymd_opt(2026, 10, 26).unwrap());
        assert_eq!((summary.active_seconds, summary.projects), (5400, 2));
        assert!((summary.estimated_cost - 4.0).abs() < 1e-9);
        assert_eq!(summary.text(), "Last week: 1h 30m across 2 projects, about $4.00 at API prices.");
        let days = daily_active_seconds(&report, 7, today).unwrap();
        assert_eq!(days.len(), 7);
        assert_eq!(days[0], (NaiveDate::from_ymd_opt(2026, 10, 27).unwrap(), 0));
        assert_eq!(days[5].1, 1800);
        assert_eq!(days[6].1, 600);
        // Nothing last week: no summary.
        assert!(last_week_summary(&report, NaiveDate::from_ymd_opt(2026, 11, 20).unwrap()).is_none());
        assert!(last_week_summary(&json!({"periods": []}), today).is_none());
    }

    #[test]
    fn the_report_page_escapes_its_data() {
        assert_eq!(REPORT_TEMPLATE.matches(REPORT_PLACEHOLDER).count(), 1);
        let page = report_page(&json!({"periods": [], "name": "</script><b>&"}), Period::Month);
        assert!(!page.contains(REPORT_PLACEHOLDER));
        assert!(page.contains(r#""initialPeriod":"month""#));
        assert!(page.contains(r#"\u003c/script\u003e\u003cb\u003e\u0026"#));
        assert!(!page.contains("</script><b>"));
    }

    #[test]
    fn cursor_turns_use_their_prompt_and_stop() {
        let record = |event: &str, generation: &str, ts: &str, model: Option<&str>| CursorRecord {
            conversation: "c1".into(),
            event: event.into(),
            generation: generation.into(),
            model: model.map(String::from),
            project: None,
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
