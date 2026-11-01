//! Claude Code and Codex lifecycle hooks: installing them, and running them.
use crate::connection::{detach, AgentConnection};
use crate::control::{ensure_private_directory, state_dir};
use crate::models::*;
use crate::system;
use serde_json::{json, Map, Value};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// Coding agents Barista can connect to. Both use the same hook file shape:
/// event → [{matcher?, hooks: [{type: "command", command, timeout}]}].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HookAgent {
    Claude,
    Codex,
}

impl HookAgent {
    pub const ALL: [HookAgent; 2] = [HookAgent::Claude, HookAgent::Codex];

    pub fn parse(name: &str) -> Option<Self> {
        match name {
            "claude" => Some(Self::Claude),
            "codex" => Some(Self::Codex),
            _ => None,
        }
    }
    pub fn display_name(self) -> &'static str {
        match self {
            Self::Claude => "Claude Code",
            Self::Codex => "Codex",
        }
    }
    /// Where the agent reads user-level hooks.
    pub fn default_settings(self) -> PathBuf {
        let home = dirs::home_dir().unwrap_or_default();
        match self {
            Self::Claude => home.join(".claude").join("settings.json"),
            Self::Codex => home.join(".codex").join("hooks.json"),
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

/// Events Barista listens to. Prompts start work, tool calls renew it, Stop/SessionEnd release it.
pub const EVENTS: [&str; 5] = ["UserPromptSubmit", "PreToolUse", "PostToolUse", "Stop", "SessionEnd"];
const TOOL_EVENTS: [&str; 2] = ["PreToolUse", "PostToolUse"];

/// The hook command for this CLI. POSIX shells get a single-quoted path; Windows gets a
/// double-quoted path with forward slashes, which both cmd.exe and Git Bash accept.
pub fn command(cli_path: &str, agent: HookAgent) -> String {
    if cfg!(windows) {
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
    let executable = &command[..command.len() - suffix.len()];
    let path = if let Some(body) = executable.strip_prefix('\'') {
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

fn our_entries(hooks: &Map<String, Value>, event: &str) -> Vec<(Value, String)> {
    let mut result = Vec::new();
    for group in hooks.get(event).and_then(Value::as_array).into_iter().flatten() {
        for hook in group.get("hooks").and_then(Value::as_array).into_iter().flatten() {
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

/// Commands Barista has installed, or an empty list when not connected.
pub fn installed_commands(agent: HookAgent, settings: Option<&Path>) -> Vec<String> {
    let url = settings.map(Path::to_path_buf).unwrap_or_else(|| agent.default_settings());
    let Ok(root) = read(&url) else { return Vec::new() };
    let Some(hooks) = root.get("hooks").and_then(Value::as_object) else {
        return Vec::new();
    };
    let mut found: Vec<String> = Vec::new();
    for event in EVENTS {
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
    EVENTS.iter().all(|event| {
        let entries = our_entries(hooks, event);
        !entries.is_empty()
            && entries.iter().all(|(group, command)| {
                *command == expected && (!TOOL_EVENTS.contains(event) || group.get("matcher").and_then(Value::as_str) == Some("*"))
            })
    })
}

pub fn install(cli_path: &str, agent: HookAgent, settings: Option<&Path>) -> Result<()> {
    let url = settings.map(Path::to_path_buf).unwrap_or_else(|| agent.default_settings());
    edit(&url, |mut root| {
        let existing = root.get("hooks").and_then(Value::as_object).cloned().unwrap_or_default();
        let mut hooks = removing_ours(&existing);
        for event in EVENTS {
            let entry = json!({"type": "command", "command": command(cli_path, agent), "timeout": agent.timeout(event)});
            let mut group = json!({"hooks": [entry]});
            if TOOL_EVENTS.contains(&event) {
                group["matcher"] = json!("*");
            }
            let mut groups = hooks.get(event).and_then(Value::as_array).cloned().unwrap_or_default();
            groups.push(group);
            hooks.insert(event.to_string(), Value::Array(groups));
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
        // A hooks file that held only Barista's entries (as Codex's usually does) goes away entirely,
        // unless it's a symlink (a dotfiles repo's file would go with it): then it's emptied instead.
        Ok(if root.is_empty() && whole_file && !is_link {
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
                    return Some(group.clone());
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
        self.handle(event.as_ref(), name.as_deref());
        // Codex requires JSON on stdout from Stop hooks; an empty object means "carry on".
        if self.agent == HookAgent::Codex && name.as_deref() == Some("Stop") {
            println!("{{}}");
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
        let folder = event
            .get("cwd")
            .and_then(Value::as_str)
            .map(|cwd| {
                cwd.trim_end_matches(['/', '\\'])
                    .rsplit(['/', '\\'])
                    .next()
                    .unwrap_or("")
                    .to_string()
            })
            .unwrap_or_default();
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
    fn lease_keys_are_safe_file_names() {
        assert_eq!(HookLeaseStore::key("abc-123_X"), Some("abc-123_X".into()));
        assert_eq!(HookLeaseStore::key("../escape"), None);
        assert_eq!(HookLeaseStore::key(""), None);
        assert_eq!(HookLeaseStore::key(&"a".repeat(129)), None);
    }
}
