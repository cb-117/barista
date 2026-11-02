//! Claude Code, Codex, and Cursor lifecycle hooks: installing them, and running them.
use crate::connection::{detach, AgentConnection};
use crate::control::{ensure_private_directory, state_dir};
use crate::models::*;
use crate::system;
use serde_json::{json, Map, Value};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// Coding agents Barista can connect to. Claude Code and Codex use the same hook file shape:
/// event → [{matcher?, hooks: [{type: "command", command, timeout}]}]. Cursor's is flatter:
/// {"version": 1, "hooks": {event → [{command, timeout}]}}.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HookAgent {
    Claude,
    Codex,
    Cursor,
}

/// Cursor's events: a prompt starts work, tool calls and responses renew it, stop/sessionEnd release it.
pub const CURSOR_EVENTS: [&str; 5] = ["beforeSubmitPrompt", "postToolUse", "afterAgentResponse", "stop", "sessionEnd"];

impl HookAgent {
    pub const ALL: [HookAgent; 3] = [HookAgent::Claude, HookAgent::Codex, HookAgent::Cursor];

    pub fn parse(name: &str) -> Option<Self> {
        match name {
            "claude" => Some(Self::Claude),
            "codex" => Some(Self::Codex),
            "cursor" => Some(Self::Cursor),
            _ => None,
        }
    }
    pub fn name(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Codex => "codex",
            Self::Cursor => "cursor",
        }
    }
    pub fn display_name(self) -> &'static str {
        match self {
            Self::Claude => "Claude Code",
            Self::Codex => "Codex",
            Self::Cursor => "Cursor",
        }
    }
    /// The hook events Barista installs for this agent.
    pub fn events(self) -> &'static [&'static str] {
        match self {
            Self::Cursor => &CURSOR_EVENTS,
            _ => &EVENTS,
        }
    }
    /// Where the agent reads user-level hooks.
    pub fn default_settings(self) -> PathBuf {
        let home = dirs::home_dir().unwrap_or_default();
        match self {
            Self::Claude => home.join(".claude").join("settings.json"),
            Self::Codex => home.join(".codex").join("hooks.json"),
            Self::Cursor => home.join(".cursor").join("hooks.json"),
        }
    }
    /// The agent has been used by this account (its config folder exists).
    pub fn is_installed(self) -> bool {
        self.default_settings().parent().is_some_and(Path::is_dir)
    }
    /// Arguments after `barista` in the hook command.
    fn hook_arguments(self) -> &'static str {
        match self {
            Self::Claude => "hook",
            Self::Codex => "hook --agent codex",
            Self::Cursor => "hook --agent cursor",
        }
    }
    /// Codex titles each hook in its review list "N - <statusMessage>" (else just "Hook N"),
    /// and shows the message while the hook runs. Claude Code and Cursor don't use it.
    pub fn status_message(self, event: &str) -> Option<&'static str> {
        if self != Self::Codex {
            return None;
        }
        match event {
            "UserPromptSubmit" => Some("Barista: keep this computer awake while Codex works"),
            "PreToolUse" => Some("Barista: keep the computer awake during this tool call"),
            "PostToolUse" => Some("Barista: renew the awake session after this tool call"),
            "Stop" => Some("Barista: let the computer sleep now that Codex is done"),
            "SessionEnd" => Some("Barista: end this conversation\u{2019}s awake session"),
            _ => None,
        }
    }
    fn timeout(self, event: &str) -> i64 {
        // Codex caps SessionEnd hooks at 3 seconds.
        if self == Self::Codex && event == "SessionEnd" {
            3
        } else {
            10
        }
    }
}

/// Shown by Codex for the hooks file; set only when Barista creates the file.
pub const CODEX_FILE_DESCRIPTION: &str =
    "Barista keeps this computer awake while Codex works, then lets it sleep. https://github.com/cb-117/barista";

/// Events Barista listens to. Prompts start work, tool calls renew it, Stop/SessionEnd release it.
pub const EVENTS: [&str; 5] = ["UserPromptSubmit", "PreToolUse", "PostToolUse", "Stop", "SessionEnd"];
const TOOL_EVENTS: [&str; 2] = ["PreToolUse", "PostToolUse"];

/// The hook command for this CLI. POSIX shells get a single-quoted path; Windows gets a
/// double-quoted path with forward slashes, which both cmd.exe and Git Bash accept. Cursor runs
/// Windows hooks through PowerShell, which needs `&` to call a quoted path.
pub fn command(cli_path: &str, agent: HookAgent) -> String {
    if cfg!(windows) && agent == HookAgent::Cursor {
        // Cursor runs Windows hooks through PowerShell: a literal ('…') string expands nothing
        // ($, backticks); only an embedded apostrophe needs doubling.
        format!("& '{}' {}", cli_path.replace('\\', "/").replace('\'', "''"), agent.hook_arguments())
    } else if cfg!(windows) {
        format!("\"{}\" {}", cli_path.replace('\\', "/"), agent.hook_arguments())
    } else {
        format!("'{}' {}", cli_path.replace('\'', "'\\''"), agent.hook_arguments())
    }
}

/// True only for the exact shapes `command()` generates (or a bare `…/barista hook`),
/// so unrelated user hooks that merely end in "barista hook" are never touched.
pub fn is_ours(command: &str) -> bool {
    let Some(suffix) = HookAgent::ALL
        .iter()
        .map(|a| format!(" {}", a.hook_arguments()))
        .find(|s| command.ends_with(s.as_str()))
    else {
        return false;
    };
    is_barista_executable(&command[..command.len() - suffix.len()])
}

/// True for a quoted or bare path to this CLI, as `command()` writes it.
fn is_barista_executable(executable: &str) -> bool {
    // PowerShell's call operator, as Cursor's Windows commands use, with a literal ('…') path
    // where an apostrophe is written as ''.
    let powershell = executable.starts_with("& ");
    let executable = executable.strip_prefix("& ").unwrap_or(executable);
    let path = if let Some(body) = executable.strip_prefix('\'').filter(|_| powershell) {
        let Some(body) = body.strip_suffix('\'') else { return false };
        if body.replace("''", "").contains('\'') {
            return false;
        }
        body.replace("''", "'")
    } else if let Some(body) = executable.strip_prefix('\'') {
        // A single-quoted word where embedded quotes appear only as '\''.
        let Some(body) = body.strip_suffix('\'') else { return false };
        let parts: Vec<&str> = body.split("'\\''").collect();
        if parts.iter().any(|part| part.contains('\'')) {
            return false;
        }
        parts.join("'")
    } else if let Some(body) = executable.strip_prefix('"') {
        let Some(body) = body.strip_suffix('"') else { return false };
        if body.contains('"') {
            return false;
        }
        body.to_string()
    } else {
        let safe = |c: char| c.is_ascii_alphanumeric() || "/._-+@".contains(c);
        if executable.is_empty() || !executable.chars().all(safe) {
            return false;
        }
        executable.to_string()
    };
    let name = path.rsplit(['/', '\\']).next().unwrap_or("");
    let absolute = path.starts_with('/')
        || path.starts_with("\\\\")
        || (path.len() > 2
            && path.as_bytes()[0].is_ascii_alphabetic()
            && path.as_bytes()[1] == b':'
            && matches!(path.as_bytes()[2], b'/' | b'\\'));
    let bare = path == name;
    (name == "barista" || name == "barista.exe") && (bare || absolute)
}

/// Each entry's group (for Claude/Codex matchers) and command. Cursor's entries are their own group.
fn our_entries(hooks: &Map<String, Value>, event: &str) -> Vec<(Value, String)> {
    let mut result = Vec::new();
    for group in hooks.get(event).and_then(Value::as_array).into_iter().flatten() {
        let flat = std::slice::from_ref(group);
        let entries = group.get("hooks").and_then(Value::as_array).map(Vec::as_slice).unwrap_or(flat);
        for hook in entries {
            let kind = hook.get("type").and_then(Value::as_str).unwrap_or("command");
            if let (true, Some(command)) = (kind == "command", hook.get("command").and_then(Value::as_str)) {
                if is_ours(command) {
                    result.push((group.clone(), command.to_string()));
                }
            }
        }
    }
    result
}

/// Barista's hook entries within a group (Cursor's entries are their own group).
fn our_hooks(group: &Value) -> Vec<&Value> {
    let flat = std::slice::from_ref(group);
    let entries = group.get("hooks").and_then(Value::as_array).map(Vec::as_slice).unwrap_or(flat);
    entries
        .iter()
        .filter(|hook| is_ours(hook.get("command").and_then(Value::as_str).unwrap_or("")))
        .collect()
}

/// Commands Barista has installed, or an empty list when not connected.
pub fn installed_commands(agent: HookAgent, settings: Option<&Path>) -> Vec<String> {
    let url = settings.map(Path::to_path_buf).unwrap_or_else(|| agent.default_settings());
    let Ok(root) = read(&url) else { return Vec::new() };
    let Some(hooks) = root.get("hooks").and_then(Value::as_object) else {
        return Vec::new();
    };
    let mut found: Vec<String> = Vec::new();
    for event in agent.events() {
        for (_, command) in our_entries(hooks, event) {
            if !found.contains(&command) {
                found.push(command);
            }
        }
    }
    found
}

/// True when every event Barista needs runs exactly this CLI, with tool events matching all tools.
pub fn is_fully_installed(cli_path: &str, agent: HookAgent, settings: Option<&Path>) -> bool {
    let url = settings.map(Path::to_path_buf).unwrap_or_else(|| agent.default_settings());
    let Ok(root) = read(&url) else { return false };
    let Some(hooks) = root.get("hooks").and_then(Value::as_object) else {
        return false;
    };
    let expected = command(cli_path, agent);
    agent.events().iter().all(|event| {
        let entries = our_entries(hooks, event);
        !entries.is_empty()
            && entries.iter().all(|(group, command)| {
                *command == expected
                    && (!TOOL_EVENTS.contains(event) || group.get("matcher").and_then(Value::as_str) == Some("*"))
                    // Hooks from 0.0.7 have no label; they still work, so they still count as connected.
                    && our_hooks(group).iter().all(|hook| match hook.get("statusMessage") {
                        None => true,
                        Some(label) => label.as_str() == agent.status_message(event),
                    })
            })
    })
}

pub fn install(cli_path: &str, agent: HookAgent, settings: Option<&Path>) -> Result<()> {
    let url = settings.map(Path::to_path_buf).unwrap_or_else(|| agent.default_settings());
    edit(&url, |mut root| {
        let existing = root.get("hooks").and_then(Value::as_object).cloned().unwrap_or_default();
        let mut hooks = removing_ours(&existing);
        if agent == HookAgent::Cursor {
            for event in CURSOR_EVENTS {
                let mut groups = hooks.get(event).and_then(Value::as_array).cloned().unwrap_or_default();
                groups.push(json!({"command": command(cli_path, agent), "timeout": agent.timeout(event)}));
                hooks.insert(event.to_string(), Value::Array(groups));
            }
            root.entry("version").or_insert(json!(1));
            root.insert("hooks".into(), Value::Object(hooks));
            return Ok(Edit::Write(root));
        }
        for event in EVENTS {
            let mut entry = json!({"type": "command", "command": command(cli_path, agent), "timeout": agent.timeout(event)});
            if let Some(message) = agent.status_message(event) {
                entry["statusMessage"] = json!(message);
            }
            let mut group = json!({"hooks": [entry]});
            if TOOL_EVENTS.contains(&event) {
                group["matcher"] = json!("*");
            }
            let mut groups = hooks.get(event).and_then(Value::as_array).cloned().unwrap_or_default();
            groups.push(group);
            hooks.insert(event.to_string(), Value::Array(groups));
        }
        if agent == HookAgent::Codex && !root.contains_key("description") && root.keys().all(|key| key == "hooks") {
            root.insert("description".into(), json!(CODEX_FILE_DESCRIPTION));
        }
        root.insert("hooks".into(), Value::Object(hooks));
        Ok(Edit::Write(root))
    })
}

pub fn uninstall(agent: HookAgent, settings: Option<&Path>) -> Result<()> {
    let url = settings.map(Path::to_path_buf).unwrap_or_else(|| agent.default_settings());
    if !url.exists() {
        return Ok(());
    }
    let is_link = fs::symlink_metadata(&url).map(|m| m.file_type().is_symlink()).unwrap_or(false);
    let whole_file = owns_whole_file(&url);
    edit(&url, |mut root| {
        let Some(existing) = root.get("hooks").and_then(Value::as_object).cloned() else {
            return Ok(Edit::Keep);
        };
        let hooks = removing_ours(&existing);
        if hooks.is_empty() {
            root.remove("hooks");
        } else {
            root.insert("hooks".into(), Value::Object(hooks));
        }
        if root.get("description").and_then(Value::as_str) == Some(CODEX_FILE_DESCRIPTION) {
            root.remove("description");
        }
        // A hooks file that held only Barista's entries (as Codex's usually does) goes away entirely,
        // unless it's a symlink (a dotfiles repo's file would go with it): then it's emptied instead.
        // Cursor's file keeps a "version" key; with nothing else left it's empty too.
        let only_version = agent == HookAgent::Cursor && root.keys().all(|key| key == "version");
        Ok(if (root.is_empty() || only_version) && whole_file && !is_link {
            Edit::Delete
        } else {
            Edit::Write(root)
        })
    })
}

enum Edit {
    Keep,
    Write(Map<String, Value>),
    Delete,
}

/// The agent may save its settings while Barista edits them. If the file changed between reading
/// and writing, the edit is redone on the new contents, so the agent's change is never lost.
fn edit(url: &Path, change: impl Fn(Map<String, Value>) -> Result<Edit>) -> Result<()> {
    // Follow dotfile symlinks so the link itself survives.
    let url = resolve_symlinks(url);
    for _ in 0..3 {
        // A missing file starts empty; any other read error (permissions, a directory) is reported.
        let original = match fs::read(&url) {
            Ok(data) => Some(data),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => return Err(CoffeeError::msg(format!("Couldn’t read {}: {error}", url.display()))),
        };
        let result = change(parse(original.as_deref(), &url)?)?;
        if let Edit::Keep = result {
            return Ok(());
        }
        if fs::read(&url).ok() != original {
            continue;
        }
        match result {
            Edit::Keep => {}
            Edit::Delete => fs::remove_file(&url)?,
            Edit::Write(root) => write(&root, &url)?,
        }
        return Ok(());
    }
    Err(CoffeeError::msg(format!(
        "{} kept changing while Barista was editing it. Try again.",
        url.display()
    )))
}

// MARK: Status line

/// Each settings file's replaced status line lives in its own backup, `statusline-chain-<KEY>.json`,
/// so installing into one settings file never disturbs another's saved status line.
pub const STATUSLINE_CHAIN_PREFIX: &str = "statusline-chain-";

/// The first 16 hex digits of the FNV-1a 64-bit hash of `text`'s UTF-8 bytes.
pub fn fnv1a_key(text: &str) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in text.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    format!("{hash:016x}")
}

/// The settings file's resolved absolute path, as a string.
fn resolved_path(url: &Path) -> String {
    let resolved = resolve_symlinks(url);
    let absolute = fs::canonicalize(&resolved)
        .map(crate::connection::strip_verbatim)
        .unwrap_or_else(|_| {
            if resolved.is_absolute() {
                resolved.clone()
            } else {
                std::env::current_dir().unwrap_or_default().join(&resolved)
            }
        });
    absolute.to_string_lossy().into_owned()
}

/// The backup key for a settings file.
pub fn statusline_key(settings: &Path) -> String {
    fnv1a_key(&resolved_path(settings))
}

fn statusline_chain_path(key: &str) -> PathBuf {
    crate::usage::data_dir().join(format!("{STATUSLINE_CHAIN_PREFIX}{key}.json"))
}

/// The quoted CLI, as hook commands write it.
fn quoted_cli(cli_path: &str) -> String {
    if cfg!(windows) {
        format!("\"{}\"", cli_path.replace('\\', "/"))
    } else {
        format!("'{}'", cli_path.replace('\'', "'\\''"))
    }
}

/// Claude Code's status line command that records plan limits, chained to a saved one when `key` is set.
pub fn statusline_command(cli_path: &str, key: Option<&str>) -> String {
    match key {
        Some(key) => format!("{} statusline --chain {key}", quoted_cli(cli_path)),
        None => format!("{} statusline", quoted_cli(cli_path)),
    }
}

fn is_key(text: &str) -> bool {
    text.len() == 16 && text.chars().all(|c| c.is_ascii_hexdigit())
}

/// The command's `--chain` key when it's Barista's status line: Some(None) without a chain.
fn our_statusline(command: &str) -> Option<Option<String>> {
    if let Some(executable) = command.strip_suffix(" statusline") {
        return is_barista_executable(executable).then_some(None);
    }
    let (rest, key) = command.rsplit_once(' ')?;
    let executable = rest.strip_suffix(" statusline --chain")?;
    (is_key(key) && is_barista_executable(executable)).then(|| Some(key.to_string()))
}

/// True for a status line command Barista wrote, with or without `--chain KEY`.
pub fn is_our_statusline(command: &str) -> bool {
    our_statusline(command).is_some()
}

/// The status line object saved under `key`; Err when the backup is missing or unreadable.
pub fn statusline_chain(key: &str) -> std::result::Result<Value, String> {
    let data = fs::read(statusline_chain_path(key)).map_err(|e| e.to_string())?;
    let saved: Value = serde_json::from_slice(&data).map_err(|e| e.to_string())?;
    saved
        .get("statusLine")
        .cloned()
        .filter(Value::is_object)
        .ok_or_else(|| "it has no saved status line".to_string())
}

fn statusline_settings(settings: Option<&Path>) -> PathBuf {
    settings
        .map(Path::to_path_buf)
        .unwrap_or_else(|| HookAgent::Claude.default_settings())
}

fn current_statusline(root: &Map<String, Value>) -> Option<Option<String>> {
    root.get("statusLine")
        .and_then(|line| line.get("command"))
        .and_then(Value::as_str)
        .and_then(our_statusline)
}

/// Points Claude Code's status line at `barista statusline`, saving any status line it replaces.
pub fn statusline_install(cli_path: &str, settings: Option<&Path>) -> Result<()> {
    let url = statusline_settings(settings);
    edit(&url, |mut root| {
        let line = match (current_statusline(&root), root.get("statusLine")) {
            // Already Barista's: keep its chain (if any), refresh the CLI path.
            (Some(key), _) => json!({"type": "command", "command": statusline_command(cli_path, key.as_deref())}),
            (None, Some(existing)) if !existing.is_null() => {
                // Save theirs first: it keeps running behind Barista and comes back on uninstall.
                let path_text = resolved_path(&url);
                let key = fnv1a_key(&path_text);
                let path = statusline_chain_path(&key);
                if let Some(parent) = path.parent() {
                    fs::create_dir_all(parent)?;
                }
                let saved = serde_json::to_vec_pretty(&json!({"settings": path_text, "statusLine": existing}))
                    .map_err(|e| CoffeeError::msg(e.to_string()))?;
                write_atomically(&path, &saved)?;
                json!({"type": "command", "command": statusline_command(cli_path, Some(&key))})
            }
            _ => json!({"type": "command", "command": statusline_command(cli_path, None)}),
        };
        if root.get("statusLine") == Some(&line) {
            return Ok(Edit::Keep);
        }
        root.insert("statusLine".into(), line);
        Ok(Edit::Write(root))
    })
}

/// Restores the status line Barista replaced, or removes Barista's. Refuses, leaving the settings
/// unchanged, if a status line was saved but its backup is missing or unreadable.
pub fn statusline_uninstall(settings: Option<&Path>) -> Result<()> {
    let url = statusline_settings(settings);
    if !url.exists() {
        return Ok(());
    }
    let Some(key) = read(&url).ok().as_ref().and_then(current_statusline) else {
        return Ok(());
    };
    let restored = match &key {
        Some(key) => Some(statusline_chain(key).map_err(|error| {
            CoffeeError::msg(format!(
                "Barista’s saved copy of your own status line is missing or unreadable ({error}), so {} was left unchanged. Its .barista-backup copy still holds the original.",
                url.display()
            ))
        })?),
        None => None,
    };
    edit(&url, |mut root| {
        if current_statusline(&root).is_none() {
            return Ok(Edit::Keep);
        }
        match &restored {
            Some(line) => root.insert("statusLine".into(), line.clone()),
            None => root.remove("statusLine"),
        };
        Ok(Edit::Write(root))
    })?;
    if let Some(key) = key {
        let _ = fs::remove_file(statusline_chain_path(&key));
    }
    Ok(())
}

/// "Connected", "Connected (in front of your own status line)", or "Not connected".
pub fn statusline_status(settings: Option<&Path>) -> &'static str {
    match read(&statusline_settings(settings)).ok().as_ref().and_then(current_statusline) {
        None => "Not connected",
        Some(Some(_)) => "Connected (in front of your own status line)",
        Some(None) => "Connected",
    }
}

/// Only dedicated hook files may be deleted; a general settings file (Claude's) is kept.
fn owns_whole_file(url: &Path) -> bool {
    url.file_name().and_then(|n| n.to_str()) == Some("hooks.json")
}

fn removing_ours(hooks: &Map<String, Value>) -> Map<String, Value> {
    let mut result = hooks.clone();
    for (event, value) in hooks {
        let Some(groups) = value.as_array() else { continue };
        let kept: Vec<Value> = groups
            .iter()
            .filter_map(|group| {
                let Some(entries) = group.get("hooks").and_then(Value::as_array) else {
                    // A flat entry (Cursor's shape) is itself a hook.
                    let ours = group.get("command").and_then(Value::as_str).is_some_and(is_ours);
                    return (!ours).then(|| group.clone());
                };
                let remaining: Vec<Value> = entries
                    .iter()
                    .filter(|e| !is_ours(e.get("command").and_then(Value::as_str).unwrap_or("")))
                    .cloned()
                    .collect();
                if remaining.is_empty() {
                    return None;
                }
                let mut copy = group.clone();
                copy["hooks"] = Value::Array(remaining);
                Some(copy)
            })
            .collect();
        if kept.is_empty() {
            result.remove(event);
        } else {
            result.insert(event.clone(), Value::Array(kept));
        }
    }
    result
}

/// Follows symlinks (relative or absolute) without requiring the final file to exist.
pub fn resolve_symlinks(path: &Path) -> PathBuf {
    let mut current = path.to_path_buf();
    for _ in 0..16 {
        match fs::read_link(&current) {
            Ok(target) => {
                current = if target.is_absolute() {
                    target
                } else {
                    current.parent().unwrap_or(Path::new(".")).join(target)
                };
            }
            Err(_) => break,
        }
    }
    current
}

fn read(url: &Path) -> Result<Map<String, Value>> {
    let url = resolve_symlinks(url);
    if !url.exists() {
        return Ok(Map::new());
    }
    parse(Some(&fs::read(&url)?), &url)
}

fn parse(data: Option<&[u8]>, path: &Path) -> Result<Map<String, Value>> {
    let Some(data) = data.filter(|d| !d.iter().all(|b| matches!(b, 9 | 10 | 13 | 32))) else {
        return Ok(Map::new());
    };
    match serde_json::from_slice::<Value>(data) {
        Ok(Value::Object(object)) => Ok(object),
        _ => Err(CoffeeError::msg(format!(
            "{} isn’t valid JSON, so Barista left it unchanged.",
            path.display()
        ))),
    }
}

fn write(root: &Map<String, Value>, url: &Path) -> Result<()> {
    if let Some(parent) = url.parent() {
        fs::create_dir_all(parent)?;
    }
    if url.exists() {
        let mut backup = url.as_os_str().to_owned();
        backup.push(".barista-backup");
        let _ = fs::remove_file(&backup);
        fs::copy(url, &backup)?;
    }
    let mut text = serde_json::to_string_pretty(&Value::Object(root.clone())).map_err(|e| CoffeeError::msg(e.to_string()))?;
    text.push('\n');
    write_atomically(url, text.as_bytes())
}

pub fn write_atomically(url: &Path, data: &[u8]) -> Result<()> {
    let mut temporary = url.as_os_str().to_owned();
    temporary.push(format!(".tmp-{}", std::process::id()));
    let temporary = PathBuf::from(temporary);
    fs::write(&temporary, data)?;
    if let Err(error) = fs::rename(&temporary, url) {
        let _ = fs::remove_file(&temporary);
        return Err(error.into());
    }
    Ok(())
}

/// Remembers which lease belongs to which agent conversation, so separate hook processes can share it.
pub struct HookLeaseStore {
    pub directory: PathBuf,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SavedLease {
    pub id: String,
    pub turn_started: f64,
    pub stop_token: Option<String>,
}

impl HookLeaseStore {
    pub fn new(socket_path: &str) -> Self {
        Self {
            directory: state_dir(socket_path).join("hooks"),
        }
    }

    /// Accepts agent session IDs made of letters, digits, `-` and `_` only.
    pub fn key(raw: &str) -> Option<String> {
        let valid = (1..=128).contains(&raw.len()) && raw.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
        valid.then(|| raw.to_string())
    }

    /// Serializes concurrent hooks for one conversation (parallel tool calls fire hooks at once).
    pub fn with_lock<T>(&self, key: &str, body: impl FnOnce() -> Result<T>) -> Result<T> {
        // Create the private control directory too: hooks may run before Barista ever has.
        if let Some(parent) = self.directory.parent() {
            ensure_private_directory(parent)?;
        }
        ensure_private_directory(&self.directory)?;
        let mut options = fs::OpenOptions::new();
        options.create(true).truncate(false).read(true).write(true);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
        let file = options
            .open(self.directory.join(format!("{key}.lock")))
            .map_err(|_| CoffeeError::msg("Cannot lock agent session state."))?;
        file.lock().map_err(|_| CoffeeError::msg("Cannot lock agent session state."))?;
        let result = body();
        let _ = file.unlock();
        result
    }

    /// The conversation's lease, when its turn began (for the hard cap), and the app's stop token then.
    pub fn lease(&self, key: &str) -> Option<SavedLease> {
        let text = fs::read_to_string(self.directory.join(key)).ok()?;
        let parts: Vec<&str> = text.split_whitespace().collect();
        let id = parts.first().filter(|id| is_uuid(id))?.to_string();
        let started = parts.get(1).and_then(|s| s.parse::<f64>().ok()).unwrap_or_else(now);
        let stop_token = parts.get(2).filter(|t| **t != "-").map(|t| t.to_string());
        Some(SavedLease {
            id,
            turn_started: started,
            stop_token,
        })
    }

    pub fn save(&self, id: &str, turn_started: f64, stop_token: Option<&str>, key: &str) {
        let token = stop_token.filter(|t| is_uuid(t)).unwrap_or("-");
        let text = format!("{id} {} {token}", turn_started as i64);
        let _ = write_atomically(&self.directory.join(key), text.as_bytes());
    }

    pub fn remove(&self, key: &str) {
        let _ = fs::remove_file(self.directory.join(key));
    }
}

/// Handles a Claude Code or Codex hook event read from stdin.
pub struct AgentHook {
    pub socket_path: String,
    pub no_launch: bool,
    pub agent: HookAgent,
}

pub const HOOK_TTL: f64 = 1200.0;
pub const HOOK_HARD_CAP: f64 = 28800.0;

/// The last component of a path, ignoring trailing separators.
fn folder_name(path: &str) -> String {
    path.trim_end_matches(['/', '\\'])
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or("")
        .to_string()
}

impl AgentHook {
    pub fn run(&self) {
        // Read everything: a large tool input or output cut short would fail to parse and skip the renewal.
        let mut input = Vec::new();
        let _ = std::io::stdin().read_to_end(&mut input);
        let event: Option<Value> = serde_json::from_slice(&input).ok();
        let name = event
            .as_ref()
            .and_then(|e| e.get("hook_event_name"))
            .and_then(Value::as_str)
            .map(String::from);
        if self.agent == HookAgent::Cursor {
            self.handle_cursor(event.as_ref(), name.as_deref());
            // Cursor reads JSON from every hook; a prompt hook must say whether to continue.
            if name.as_deref() == Some("beforeSubmitPrompt") {
                println!("{{\"continue\":true}}");
            } else {
                println!("{{}}");
            }
            return;
        }
        self.handle(event.as_ref(), name.as_deref());
        // Codex requires JSON on stdout from Stop hooks; an empty object means "carry on".
        if self.agent == HookAgent::Codex && name.as_deref() == Some("Stop") {
            println!("{{}}");
        }
    }

    /// Cursor sends `conversation_id`, `generation_id`, `model`, and `workspace_roots`. Prompts and
    /// stops are recorded for usage tracking even when Barista isn't running.
    fn handle_cursor(&self, event: Option<&Value>, name: Option<&str>) {
        let (Some(event), Some(name)) = (event, name) else { return };
        let Some(conversation) = event.get("conversation_id").and_then(Value::as_str) else {
            return;
        };
        let text = |key: &str| event.get(key).and_then(Value::as_str).filter(|v| !v.is_empty());
        let generation = text("generation_id").unwrap_or("");
        let root = event
            .get("workspace_roots")
            .and_then(Value::as_array)
            .and_then(|roots| roots.first())
            .and_then(Value::as_str)
            .filter(|root| !root.is_empty());
        match name {
            "beforeSubmitPrompt" => crate::usage::record_cursor("prompt", conversation, generation, text("model"), root, None),
            "stop" => crate::usage::record_cursor("stop", conversation, generation, text("model"), root, text("status")),
            _ => {}
        }
        let Some(key) = HookLeaseStore::key(&format!("cursor-{conversation}")) else {
            return;
        };
        let store = HookLeaseStore::new(&self.socket_path);
        let folder = root.map(folder_name).unwrap_or_default();
        let title = if folder.is_empty() {
            "Cursor".to_string()
        } else {
            format!("Cursor · {}", folder.chars().take(80).collect::<String>())
        };
        match name {
            "beforeSubmitPrompt" => self.ensure(&key, &store, &title, true),
            "postToolUse" | "afterAgentResponse" => self.ensure(&key, &store, &title, false),
            "stop" | "sessionEnd" => self.release(&key, &store),
            _ => {}
        }
    }

    fn handle(&self, event: Option<&Value>, name: Option<&str>) {
        let (Some(event), Some(name)) = (event, name) else { return };
        let Some(session) = event.get("session_id").and_then(Value::as_str) else {
            return;
        };
        // Codex and Claude sessions never share a lease, even if their IDs collide.
        let raw = if self.agent == HookAgent::Claude {
            session.to_string()
        } else {
            format!("codex-{session}")
        };
        let Some(key) = HookLeaseStore::key(&raw) else { return };
        let store = HookLeaseStore::new(&self.socket_path);
        let folder = event.get("cwd").and_then(Value::as_str).map(folder_name).unwrap_or_default();
        let title = if folder.is_empty() {
            self.agent.display_name().to_string()
        } else {
            format!("{} · {}", self.agent.display_name(), folder.chars().take(80).collect::<String>())
        };
        match name {
            "UserPromptSubmit" => self.ensure(&key, &store, &title, true),
            "PreToolUse" | "PostToolUse" => self.ensure(&key, &store, &title, false),
            "Stop" => self.stop(&key, &store),
            "SessionEnd" => self.release(&key, &store),
            _ => {}
        }
    }

    fn connection(&self, launch: bool) -> AgentConnection {
        AgentConnection::new(&self.socket_path, launch && !self.no_launch)
    }

    fn ensure(&self, key: &str, store: &HookLeaseStore, title: &str, user_initiated: bool) {
        // Only a new prompt may launch the app or start a turn (with a fresh 8-hour cap). Tool calls
        // renew the turn's lease; they may replace it only if it lapsed from a missed renewal (a long
        // tool call) and the app confirms nothing was stopped since: never after a user Stop or
        // Stop all, battery cutoff, app restart, the agent's Stop, or the turn's hard cap.
        let connection = self.connection(user_initiated);
        let _ = store.with_lock(key, || {
            let mut turn_started = now();
            let mut stop_token: Option<String> = None;
            if let Some(saved) = store.lease(key) {
                let request = ControlRequest {
                    seconds: Some(HOOK_TTL),
                    message: Some("Working".into()),
                    max_seconds: user_initiated.then_some(HOOK_HARD_CAP),
                    ..ControlRequest::with_id(ControlAction::Heartbeat, &saved.id)
                };
                let reply = connection.send(&request)?;
                if reply.ok {
                    if user_initiated {
                        store.save(&saved.id, now(), reply.state.as_ref().and_then(|s| s.stop_token.as_deref()), key);
                    }
                    return Ok(());
                }
                store.remove(key);
                if !user_initiated {
                    let ended = reply.state.as_ref().and_then(|s| s.history.iter().find(|h| h.id == saved.id));
                    match (ended.map(|h| h.reason), saved.stop_token) {
                        (Some(EndReason::Expired), Some(token)) => {
                            turn_started = saved.turn_started;
                            stop_token = Some(token);
                        }
                        _ => return Ok(()),
                    }
                }
            } else if !user_initiated {
                return Ok(());
            }
            let remaining = HOOK_HARD_CAP - (now() - turn_started);
            if remaining < 1.0 {
                return Ok(());
            }
            let request = ControlRequest {
                title: Some(title.to_string()),
                seconds: Some(HOOK_TTL.min(remaining)),
                display: Some(false),
                process_id: system::agent_process_id(),
                max_seconds: Some(remaining),
                stop_token,
                ..ControlRequest::new(ControlAction::Acquire)
            };
            let reply = connection.send(&request)?;
            if let Some(lease) = reply.session {
                store.save(
                    &lease.id,
                    turn_started,
                    reply.state.as_ref().and_then(|s| s.stop_token.as_deref()),
                    key,
                );
            }
            Ok(())
        });
    }

    /// The agent finished its turn. If commands Claude started in the background are still running,
    /// keep the computer awake until they exit.
    fn stop(&self, key: &str, store: &HookLeaseStore) {
        // Background-command tracking relies on how Claude Code's Bash tool runs commands.
        if self.agent == HookAgent::Claude {
            if let Some(agent) = system::agent_process_id() {
                let pids = system::background_command_pids(agent, &system::ancestry());
                let kept = !pids.is_empty()
                    && store
                        .with_lock(key, || {
                            let Some(saved) = store.lease(key) else { return Ok(false) };
                            let request = ControlRequest {
                                seconds: Some(HookWaiter::GRACE),
                                message: Some(HookWaiter::WAITING_MESSAGE.into()),
                                ..ControlRequest::with_id(ControlAction::Heartbeat, &saved.id)
                            };
                            Ok(self.connection(false).send(&request)?.ok)
                        })
                        .unwrap_or(false);
                if kept && HookWaiter::spawn(&self.socket_path, key, &pids) {
                    return;
                }
            }
        }
        self.release(key, store);
    }

    fn release(&self, key: &str, store: &HookLeaseStore) {
        let _ = store.with_lock(key, || {
            if let Some(saved) = store.lease(key) {
                let _ = self
                    .connection(false)
                    .send(&ControlRequest::with_id(ControlAction::Release, &saved.id));
            }
            store.remove(key);
            Ok(())
        });
    }
}

/// Keeps a Claude session's lease alive while background commands it started are still running,
/// then lets it lapse within a couple of minutes. Gives up as soon as the lease is stopped,
/// replaced, or a new prompt starts a new turn (that turn's hooks take over).
pub struct HookWaiter {
    pub socket_path: String,
    pub key: String,
    pub pids: Vec<i32>,
}

impl HookWaiter {
    pub const GRACE: f64 = 120.0;
    pub const INTERVAL: u64 = 30;
    pub const WAITING_MESSAGE: &'static str = "Waiting on background work";

    pub fn run(&self) {
        let store = HookLeaseStore::new(&self.socket_path);
        let Some(start) = store.lease(&self.key) else { return };
        if self.pids.is_empty() {
            return;
        }
        let identities: Vec<(i32, Option<String>)> = self.pids.iter().map(|pid| (*pid, system::process_identity(*pid))).collect();
        let connection = AgentConnection::new(&self.socket_path, false);
        let give_up = now() + HOOK_HARD_CAP;
        while now() < give_up {
            std::thread::sleep(std::time::Duration::from_secs(Self::INTERVAL));
            let alive = identities
                .iter()
                .any(|(pid, identity)| identity.is_some() && system::process_identity(*pid) == *identity);
            if !alive {
                return; // The last heartbeat's short grace runs out on its own.
            }
            let keep_going = store
                .with_lock(&self.key, || {
                    let Some(current) = store.lease(&self.key) else { return Ok(false) };
                    if current.id != start.id || current.turn_started != start.turn_started {
                        return Ok(false);
                    }
                    // A tool call since Stop means Claude resumed this turn and renews the lease itself;
                    // a short "waiting" heartbeat now would cut its 20-minute lease to two minutes.
                    let status = connection.send(&ControlRequest::new(ControlAction::Status))?;
                    let waiting = status
                        .state
                        .as_ref()
                        .and_then(|s| s.sessions.iter().find(|l| l.id == start.id))
                        .and_then(|l| l.message.as_deref())
                        == Some(Self::WAITING_MESSAGE);
                    if !waiting {
                        return Ok(false);
                    }
                    let request = ControlRequest {
                        seconds: Some(Self::GRACE),
                        message: Some(Self::WAITING_MESSAGE.into()),
                        ..ControlRequest::with_id(ControlAction::Heartbeat, &start.id)
                    };
                    Ok(connection.send(&request)?.ok)
                })
                .unwrap_or(false);
            if !keep_going {
                return;
            }
        }
    }

    /// Starts `barista hook-wait` in its own session so it outlives the hook's timeout.
    pub fn spawn(socket_path: &str, key: &str, pids: &[i32]) -> bool {
        let Ok(executable) = std::env::current_exe() else { return false };
        let mut command = Command::new(executable);
        command.args(["hook-wait", "--socket", socket_path, key]);
        command.args(pids.iter().map(i32::to_string));
        command.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
        detach(&mut command);
        command.spawn().is_ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognizes_only_generated_commands() {
        assert!(is_ours(&command("/usr/local/bin/barista", HookAgent::Claude)));
        assert!(is_ours(&command("/home/o'neil/bin/barista", HookAgent::Codex)));
        assert!(is_ours("'/opt/My Apps/barista' hook"));
        assert!(is_ours("\"C:/Users/me/AppData/Local/Programs/Barista/barista.exe\" hook"));
        assert!(is_ours("\"C:\\Program Files\\Barista\\barista.exe\" hook --agent codex"));
        assert!(is_ours("barista hook"));
        assert!(is_ours("/usr/bin/barista hook"));
        assert!(is_ours(
            "& \"C:/Users/me/AppData/Local/Programs/Barista/barista.exe\" hook --agent cursor"
        ));
        assert!(is_ours(&command("/opt/barista", HookAgent::Cursor)));
        assert!(!is_ours("& echo barista hook --agent cursor"));
        assert!(!is_ours("echo barista hook"));
        assert!(!is_ours("say done"));
        assert!(!is_ours("bin/barista hook"), "relative paths aren't ours");
        assert!(!is_ours("'/a/barista' 'x' hook"));
        assert!(!is_ours("/usr/bin/not-barista hook"));
        assert!(!is_ours("\"C:/x/barista.exe\" \"y\" hook"));
    }

    #[test]
    fn install_and_uninstall_preserve_other_settings() {
        let directory = std::env::temp_dir().join(format!("barista-hooks-{}", new_id()));
        fs::create_dir_all(&directory).unwrap();
        let settings = directory.join("settings.json");
        let original = json!({"model": "opus", "hooks": {"Stop": [{"hooks": [{"type": "command", "command": "say done"}]}]}});
        fs::write(&settings, original.to_string()).unwrap();
        install("/opt/barista", HookAgent::Claude, Some(&settings)).unwrap();
        install("/opt/barista", HookAgent::Claude, Some(&settings)).unwrap();
        assert!(is_fully_installed("/opt/barista", HookAgent::Claude, Some(&settings)));
        assert!(!is_fully_installed("/elsewhere/barista", HookAgent::Claude, Some(&settings)));
        assert_eq!(
            installed_commands(HookAgent::Claude, Some(&settings)),
            vec![command("/opt/barista", HookAgent::Claude)]
        );
        uninstall(HookAgent::Claude, Some(&settings)).unwrap();
        let after: Value = serde_json::from_slice(&fs::read(&settings).unwrap()).unwrap();
        assert_eq!(after, original);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn codex_hooks_are_labeled_and_cleaned_up() {
        let directory = std::env::temp_dir().join(format!("barista-codex-{}", new_id()));
        fs::create_dir_all(&directory).unwrap();
        let settings = directory.join("hooks.json");
        install("/opt/barista", HookAgent::Codex, Some(&settings)).unwrap();
        let root: Value = serde_json::from_slice(&fs::read(&settings).unwrap()).unwrap();
        assert_eq!(root["description"], json!(CODEX_FILE_DESCRIPTION));
        assert_eq!(
            root["hooks"]["SessionEnd"][0]["hooks"][0]["statusMessage"],
            json!("Barista: end this conversation\u{2019}s awake session")
        );
        assert!(is_fully_installed("/opt/barista", HookAgent::Codex, Some(&settings)));
        // A 0.0.7 install without messages still works, so it still counts as connected...
        let mut old = root.clone();
        old["hooks"]["Stop"][0]["hooks"][0].as_object_mut().unwrap().remove("statusMessage");
        fs::write(&settings, old.to_string()).unwrap();
        assert!(is_fully_installed("/opt/barista", HookAgent::Codex, Some(&settings)));
        // ...but a different label means another copy wrote it.
        old["hooks"]["Stop"][0]["hooks"][0]["statusMessage"] = json!("something else");
        fs::write(&settings, old.to_string()).unwrap();
        assert!(!is_fully_installed("/opt/barista", HookAgent::Codex, Some(&settings)));
        // The user's own hook keeps the file, but Barista's description goes.
        let user = json!({"hooks": {"Stop": [{"hooks": [{"type": "command", "command": "notify-send done"}]}]}});
        fs::write(&settings, user.to_string()).unwrap();
        install("/opt/barista", HookAgent::Codex, Some(&settings)).unwrap();
        uninstall(HookAgent::Codex, Some(&settings)).unwrap();
        assert_eq!(serde_json::from_slice::<Value>(&fs::read(&settings).unwrap()).unwrap(), user);
        fs::remove_file(&settings).unwrap();
        install("/opt/barista", HookAgent::Codex, Some(&settings)).unwrap();
        uninstall(HookAgent::Codex, Some(&settings)).unwrap();
        assert!(!settings.exists());
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn powershell_commands_quote_literally() {
        // What `command()` writes for Cursor on Windows: nothing in a literal string expands.
        let path = "C:/Users/o'neil/$(evil)/`x/barista.exe";
        let quoted = format!("& '{}' hook --agent cursor", path.replace('\'', "''"));
        assert_eq!(quoted, "& 'C:/Users/o''neil/$(evil)/`x/barista.exe' hook --agent cursor");
        assert!(is_ours(&quoted));
        assert!(is_ours("& \"C:/Program Files/Barista/barista.exe\" hook --agent cursor"));
        assert!(
            !is_ours("& 'C:/a'b/barista.exe' hook --agent cursor"),
            "a lone apostrophe isn't a literal string"
        );
        assert!(!is_ours("& 'C:/x/not-barista.exe' hook --agent cursor"));
        if cfg!(windows) {
            assert_eq!(command(path, HookAgent::Cursor), quoted);
        }
    }

    #[test]
    fn status_line_commands_and_keys() {
        assert_eq!(fnv1a_key(""), "cbf29ce484222325");
        assert_eq!(fnv1a_key("a"), "af63dc4c8601ec8c");
        let plain = statusline_command("/opt/barista", None);
        let chained = statusline_command("/opt/barista", Some("0123456789abcdef"));
        assert!(is_our_statusline(&plain) && is_our_statusline(&chained));
        assert_eq!(our_statusline(&chained), Some(Some("0123456789abcdef".to_string())));
        assert_eq!(our_statusline(&plain), Some(None));
        assert!(!is_our_statusline("'/opt/barista' statusline --chain nothex"));
        assert!(!is_our_statusline("echo hi"));
    }

    #[test]
    fn lease_keys_are_safe_file_names() {
        assert_eq!(HookLeaseStore::key("abc-123_X"), Some("abc-123_X".into()));
        assert_eq!(HookLeaseStore::key("../escape"), None);
        assert_eq!(HookLeaseStore::key(""), None);
        assert_eq!(HookLeaseStore::key(&"a".repeat(129)), None);
    }
}
