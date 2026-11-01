//! `barista`: the command-line, MCP, and agent-hook interface to the Barista app.
use barista_core::connection::{current_executable, AgentConnection};
use barista_core::control::default_path;
use barista_core::hooks::{self, AgentHook, HookAgent, HookLeaseStore, HookWaiter};
use barista_core::mcp::McpServer;
use barista_core::session::AwakeSession;
use barista_core::usage;
use barista_core::*;
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::process::exit;

struct Options {
    command: String,
    values: BTreeMap<String, String>,
    flags: BTreeSet<String>,
    positionals: Vec<String>,
    child: Vec<String>,
}

impl Options {
    fn parse(mut arguments: Vec<String>) -> Result<Self> {
        let command = if arguments.is_empty() {
            "help".to_string()
        } else {
            arguments.remove(0)
        };
        let mut child = Vec::new();
        if let Some(separator) = arguments.iter().position(|a| a == "--") {
            child = arguments[separator + 1..].to_vec();
            arguments.truncate(separator);
        }
        const BOOLEAN: [&str; 4] = ["--json", "--display", "--all", "--no-launch"];
        const VALUES: [&str; 13] = [
            "--socket",
            "--name",
            "--minutes",
            "--seconds",
            "--ttl",
            "--max-awake",
            "--pid",
            "--progress",
            "--message",
            "--settings",
            "--agent",
            "--period",
            "--chain",
        ];
        let mut options = Options {
            command,
            values: BTreeMap::new(),
            flags: BTreeSet::new(),
            positionals: Vec::new(),
            child,
        };
        let mut index = 0;
        while index < arguments.len() {
            let token = &arguments[index];
            if BOOLEAN.contains(&token.as_str()) {
                if !options.flags.insert(token.clone()) {
                    return Err(CoffeeError::msg(format!("Duplicate option: {token}")));
                }
            } else if VALUES.contains(&token.as_str()) {
                index += 1;
                if index >= arguments.len() || options.values.contains_key(token) {
                    return Err(CoffeeError::msg(format!("Missing or duplicate value for {token}.")));
                }
                options.values.insert(token.clone(), arguments[index].clone());
            } else if token.starts_with("--") {
                return Err(CoffeeError::msg(format!("Unknown option: {token}")));
            } else {
                options.positionals.push(token.clone());
            }
            index += 1;
        }
        Ok(options)
    }

    fn validate(&self, values: &[&str], flags: &[&str], positional_count: usize) -> Result<()> {
        let bad_values = self.values.keys().any(|k| k != "--socket" && !values.contains(&k.as_str()));
        let bad_flags = self
            .flags
            .iter()
            .any(|f| f != "--json" && f != "--no-launch" && !flags.contains(&f.as_str()));
        if bad_values || bad_flags || self.positionals.len() != positional_count || (self.command != "run" && !self.child.is_empty()) {
            return Err(CoffeeError::msg(format!(
                "Unexpected arguments for {}. Run barista help for usage.",
                self.command
            )));
        }
        Ok(())
    }

    fn number(&self, key: &str, fallback: f64) -> Result<f64> {
        let Some(text) = self.values.get(key) else { return Ok(fallback) };
        match text.trim().parse::<f64>() {
            Ok(value) if value.is_finite() && text.trim() == text => Ok(value),
            _ => Err(CoffeeError::msg(format!("{key} must be a finite number."))),
        }
    }

    fn duration(&self, fallback: f64) -> Result<f64> {
        if self.values.contains_key("--minutes") && self.values.contains_key("--seconds") {
            return Err(CoffeeError::msg("Use --minutes or --seconds, not both."));
        }
        let value = if self.values.contains_key("--minutes") {
            self.number("--minutes", 0.0)? * 60.0
        } else {
            self.number("--seconds", fallback)?
        };
        if value <= 0.0 || value > 86400.0 {
            return Err(CoffeeError::msg("Choose a duration above zero and no longer than 24 hours."));
        }
        Ok(value)
    }

    fn socket(&self) -> String {
        self.values.get("--socket").cloned().unwrap_or_else(default_path)
    }
}

fn main() {
    #[cfg(unix)]
    unsafe {
        libc::signal(libc::SIGPIPE, libc::SIG_IGN);
    }
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    let json = arguments.iter().any(|a| a == "--json");
    match run(arguments) {
        Ok(code) => exit(code),
        Err(error) => {
            if json {
                println!("{}", to_json(&ControlReply::failure(error.to_string())));
            } else {
                eprintln!("barista: {error}");
            }
            exit(1)
        }
    }
}

fn cli_path() -> String {
    current_executable().to_string_lossy().into_owned()
}

fn run(arguments: Vec<String>) -> Result<i32> {
    let options = Options::parse(arguments)?;
    let connection = AgentConnection::new(options.socket(), !options.flags.contains("--no-launch"));
    match options.command.as_str() {
        "version" | "--version" => {
            println!("barista {VERSION}");
            return Ok(0);
        }
        "help" | "--help" | "-h" => {
            println!("{}", help());
            return Ok(0);
        }
        "mcp" => {
            options.validate(&[], &[], 0)?;
            McpServer { connection }.run();
            return Ok(0);
        }
        "hook-wait" => {
            // Spawned by the Stop hook; detached, silent, and never fails loudly.
            let _ = options.validate(&[], &[], options.positionals.len());
            if let Some(key) = options.positionals.first().and_then(|k| HookLeaseStore::key(k)) {
                let pids = options.positionals[1..]
                    .iter()
                    .filter_map(|p| p.parse::<i32>().ok())
                    .filter(|p| *p > 1)
                    .collect();
                HookWaiter {
                    socket_path: options.socket(),
                    key,
                    pids,
                }
                .run();
            }
            return Ok(0);
        }
        "hook" => {
            // Called by agent hooks: never fail the agent, never write to stdout (except Codex's Stop).
            let _ = options.validate(&["--agent"], &[], 0);
            let agent = options
                .values
                .get("--agent")
                .and_then(|a| HookAgent::parse(a))
                .unwrap_or(HookAgent::Claude);
            AgentHook {
                socket_path: options.socket(),
                no_launch: options.flags.contains("--no-launch"),
                agent,
            }
            .run();
            return Ok(0);
        }
        "setup" => {
            // One step for people and agents: connect every coding agent found, then start Barista.
            options.validate(&["--agent"], &[], 0)?;
            let agents: Vec<HookAgent> = match options.values.get("--agent") {
                Some(name) => vec![HookAgent::parse(name).ok_or_else(|| CoffeeError::msg("--agent must be claude, codex, or cursor."))?],
                None => HookAgent::ALL.into_iter().filter(|a| a.is_installed()).collect(),
            };
            for agent in &agents {
                hooks::install(&cli_path(), *agent, None)?;
                println!("Connected {} ({})", agent.display_name(), agent.default_settings().display());
                if *agent == HookAgent::Claude {
                    // Claude Code shares its plan limits only with its status line.
                    hooks::statusline_install(&cli_path(), None)?;
                    println!("Claude Code’s status line now shows your plan limits.");
                }
            }
            if agents.is_empty() {
                println!("No Claude Code, Codex, or Cursor setup found for this user. Run barista setup again after using one of them.");
            }
            let state = connection.send(&ControlRequest::new(ControlAction::Status))?.state;
            let version = state.map(|s| s.version).unwrap_or_else(|| VERSION.to_string());
            println!(
                "Barista {version} is running ({}).",
                if cfg!(windows) {
                    "the coffee cup in the notification area"
                } else {
                    "the coffee cup in the system tray"
                }
            );
            if agents.contains(&HookAgent::Codex) {
                println!("One step left for Codex: it runs new hooks only after you trust them. Open Codex, type /hooks, and trust Barista’s hooks.");
            }
            return Ok(0);
        }
        "statusline" => {
            if options.positionals.is_empty() {
                // Run by Claude Code after each reply: never fail, always print a line.
                let _ = options.validate(&["--chain"], &[], 0);
                statusline::run(options.values.get("--chain").map(String::as_str));
                return Ok(0);
            }
            options.validate(&["--settings"], &[], 1)?;
            let settings = options.values.get("--settings").map(PathBuf::from);
            let shown = settings.clone().unwrap_or_else(|| HookAgent::Claude.default_settings());
            match options.positionals[0].as_str() {
                "install" => {
                    hooks::statusline_install(&cli_path(), settings.as_deref())?;
                    println!("Claude Code’s status line now shows your plan limits: {}", shown.display());
                }
                "uninstall" => {
                    hooks::statusline_uninstall(settings.as_deref())?;
                    println!("Removed Barista’s status line: {}", shown.display());
                }
                "status" => println!("{}", hooks::statusline_status(settings.as_deref())),
                _ => {
                    return Err(CoffeeError::msg(
                        "Use: barista statusline [install|uninstall|status] [--settings PATH]",
                    ))
                }
            }
            return Ok(0);
        }
        "hooks" => {
            options.validate(&["--settings", "--agent"], &[], 1)?;
            let agent = HookAgent::parse(options.values.get("--agent").map_or("claude", String::as_str))
                .ok_or_else(|| CoffeeError::msg("--agent must be claude, codex, or cursor."))?;
            let settings = options.values.get("--settings").map(PathBuf::from);
            let url = settings.clone().unwrap_or_else(|| agent.default_settings());
            match options.positionals[0].as_str() {
                "install" => {
                    hooks::install(&cli_path(), agent, Some(&url))?;
                    println!("Connected {}: {}", agent.display_name(), url.display());
                    if agent == HookAgent::Codex {
                        println!("Codex runs new hooks only after you trust them: open Codex, run /hooks, and trust Barista’s hooks.");
                    }
                }
                "uninstall" => {
                    hooks::uninstall(agent, Some(&url))?;
                    println!("Disconnected {}: {}", agent.display_name(), url.display());
                }
                "status" => {
                    let commands = hooks::installed_commands(agent, Some(&url));
                    if commands.is_empty() {
                        println!("Not connected");
                    } else if hooks::is_fully_installed(&cli_path(), agent, Some(&url)) {
                        println!("Connected: {}", commands.join(", "));
                    } else {
                        println!(
                            "Needs repair (incomplete or another copy): run barista hooks install{}",
                            if agent == HookAgent::Claude {
                                String::new()
                            } else {
                                format!(" --agent {}", agent.name())
                            }
                        );
                    }
                }
                _ => {
                    return Err(CoffeeError::msg(
                        "Use: barista hooks install|uninstall|status [--agent claude|codex|cursor] [--settings PATH]",
                    ))
                }
            }
            return Ok(0);
        }
        "limits" => {
            // `barista limits refresh`: asks Anthropic for your Claude account's limits once.
            options.validate(&[], &[], 1)?;
            if options.positionals[0] != "refresh" {
                return Err(CoffeeError::msg("Use: barista limits refresh"));
            }
            let (ok, message) = match barista_core::claude_account::refresh() {
                Ok(()) => (true, barista_core::claude_account::UPDATED_MESSAGE.to_string()),
                Err(skipped) => (false, skipped.message()),
            };
            if options.flags.contains("--json") {
                println!("{}", to_json(&serde_json::json!({"ok": ok, "message": message})));
            } else {
                println!("{message}");
            }
            return Ok(if ok { 0 } else { 1 });
        }
        "usage" => {
            // Reads the agents' logs on this computer; never needs the app.
            options.validate(&["--period"], &[], 0)?;
            let periods = match options.values.get("--period") {
                Some(text) => {
                    vec![usage::Period::parse(text).ok_or_else(|| CoffeeError::msg("--period must be today, week, month, or all."))?]
                }
                None => vec![usage::Period::Today, usage::Period::Week, usage::Period::Month],
            };
            let report = usage::report(&periods);
            if options.flags.contains("--json") {
                println!("{}", to_json(&report));
            } else {
                print!("{}", usage::text(&report, options.values.contains_key("--period")));
            }
            return Ok(0);
        }
        "run" => {
            options.validate(&["--name", "--max-awake"], &["--display"], 0)?;
            return run_command(&options, &connection);
        }
        _ => {}
    }

    let request = match options.command.as_str() {
        "status" | "history" => {
            options.validate(&[], &[], 0)?;
            ControlRequest::new(if options.command == "status" {
                ControlAction::Status
            } else {
                ControlAction::History
            })
        }
        "start" | "acquire" => {
            options.validate(&["--name", "--minutes", "--seconds", "--pid", "--max-awake"], &["--display"], 0)?;
            let process_id = match options.values.get("--pid") {
                Some(text) => match text.parse::<i32>() {
                    Ok(pid) if pid > 0 => Some(pid),
                    _ => return Err(CoffeeError::msg("--pid must be a positive process ID.")),
                },
                None => None,
            };
            ControlRequest {
                title: Some(options.values.get("--name").cloned().unwrap_or_else(|| "Agent task".into())),
                seconds: Some(options.duration(300.0)?),
                display: Some(options.flags.contains("--display")),
                process_id,
                max_seconds: Some(options.number("--max-awake", 28800.0)?),
                ..ControlRequest::new(ControlAction::Acquire)
            }
        }
        "heartbeat" => {
            options.validate(&["--ttl", "--progress", "--message"], &[], 1)?;
            ControlRequest {
                seconds: Some(options.number("--ttl", 300.0)?),
                progress: if options.values.contains_key("--progress") {
                    Some(options.number("--progress", 0.0)?)
                } else {
                    None
                },
                message: options.values.get("--message").cloned(),
                ..ControlRequest::with_id(ControlAction::Heartbeat, options.positionals[0].clone())
            }
        }
        "extend" => {
            options.validate(&["--minutes", "--seconds"], &[], 1)?;
            ControlRequest {
                seconds: Some(options.duration(900.0)?),
                ..ControlRequest::with_id(ControlAction::Extend, options.positionals[0].clone())
            }
        }
        "stop" | "release" => {
            let all = options.flags.contains("--all");
            options.validate(&[], &["--all"], if all { 0 } else { 1 })?;
            ControlRequest {
                id: options.positionals.first().cloned(),
                ..ControlRequest::new(if all { ControlAction::StopAll } else { ControlAction::Release })
            }
        }
        other => return Err(CoffeeError::msg(format!("Unknown command: {other}. Run barista help."))),
    };
    let reply = connection.send(&request)?;
    if options.flags.contains("--json") {
        println!("{}", to_json(&reply));
    } else if !reply.ok {
        return Err(CoffeeError::msg(
            reply.error.clone().unwrap_or_else(|| "The request failed.".into()),
        ));
    } else if let Some(lease) = &reply.session {
        println!("{}", lease.id);
    } else if options.command == "history" {
        for event in reply.state.iter().flat_map(|s| s.history.iter()) {
            let ended = chrono::DateTime::from_timestamp(event.ended_at as i64, 0)
                .map(|d| d.with_timezone(&chrono::Local).format("%Y-%m-%d %H:%M").to_string())
                .unwrap_or_default();
            println!("{ended}  {} — {}", event.title, event.reason.label());
        }
    } else if let Some(state) = &reply.state {
        if state.awake {
            println!(
                "Awake · {} session(s) · display {}",
                state.sessions.len(),
                if state.display_awake { "on" } else { "allowed to sleep" }
            );
        } else {
            println!("Resting · no active sessions");
        }
        if state.battery_blocked {
            println!("Battery protection is active at {}%.", state.battery_threshold);
        }
        for lease in &state.sessions {
            let remaining = lease
                .expires_at
                .map_or("until stopped".to_string(), |end| AwakeSession::format_time(end - now()));
            println!("{}  {}  [{}]  {}", lease.id, lease.title, lease.source.as_str(), remaining);
        }
    }
    Ok(if reply.ok { 0 } else { 1 })
}

/// Runs a command while keeping the computer awake, renewing a short lease every ten seconds.
fn run_command(options: &Options, connection: &AgentConnection) -> Result<i32> {
    if options.child.is_empty() {
        return Err(CoffeeError::msg("Put a command after --, for example: barista run -- make build"));
    }
    let request = ControlRequest {
        title: Some(options.values.get("--name").cloned().unwrap_or_else(|| "Command task".into())),
        seconds: Some(45.0),
        display: Some(options.flags.contains("--display")),
        process_id: Some(system::current_pid()),
        source: Some(SessionSource::Command),
        max_seconds: Some(options.number("--max-awake", 28800.0)?),
        ..ControlRequest::new(ControlAction::Acquire)
    };
    let reply = connection.send(&request)?;
    let lease = match (reply.ok, reply.session) {
        (true, Some(lease)) => lease,
        _ => {
            return Err(CoffeeError::msg(
                reply.error.unwrap_or_else(|| "Couldn’t acquire a session.".into()),
            ))
        }
    };
    // Only the initial acquire may open Barista; maintenance never reopens an app the user quit.
    let connection = AgentConnection::new(&connection.path, false);
    let release = |connection: &AgentConnection| {
        let _ = connection.send(&ControlRequest::with_id(ControlAction::Release, &lease.id));
    };
    let status = child::run(&options.child, {
        let connection = connection.clone();
        let id = lease.id.clone();
        // Never reacquire a lease the user stopped, or defeat battery protection.
        // Once Barista reports the lease gone, stop renewing; the command keeps running.
        move || {
            let request = ControlRequest {
                seconds: Some(45.0),
                ..ControlRequest::with_id(ControlAction::Heartbeat, &id)
            };
            !matches!(connection.send(&request), Ok(reply) if !reply.ok)
        }
    });
    release(&connection);
    let outcome = status?;
    if outcome.interrupted {
        child::reraise_interrupt();
    }
    Ok(outcome.code)
}

/// `barista statusline`: Claude Code's status line command. Records plan limits for Usage & Limits.
mod statusline {
    use super::*;
    use serde_json::Value;
    use std::io::{Read, Write};
    use std::process::{Command, Stdio};

    pub fn run(chain: Option<&str>) {
        let mut input = Vec::new();
        let _ = std::io::stdin().read_to_end(&mut input);
        let status: Option<Value> = serde_json::from_slice(&input).ok();
        if let Some(limits) = status.as_ref().and_then(|s| s.get("rate_limits")).filter(|l| l.is_object()) {
            usage::record_claude_limits(limits);
        }
        // The user's own status line, if Barista replaced one, keeps running with the same input.
        let saved = chain.and_then(|key| hooks::statusline_chain(key).ok());
        if let Some(command) = saved.and_then(|line| line.get("command").and_then(Value::as_str).map(String::from)) {
            let mut shell = if cfg!(windows) {
                let mut c = Command::new("cmd");
                c.args(["/C", &command]);
                c
            } else {
                let mut c = Command::new("/bin/sh");
                c.args(["-c", &command]);
                c
            };
            shell.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::inherit());
            if let Ok(mut child) = shell.spawn() {
                if let Some(mut stdin) = child.stdin.take() {
                    let _ = stdin.write_all(&input);
                }
                if let Ok(output) = child.wait_with_output() {
                    let _ = std::io::stdout().write_all(&output.stdout);
                }
            }
            return;
        }
        if let Some(status) = status {
            println!("{}", line(&status, now()));
        }
    }

    /// "Opus · 5h 24% · 7d 41%", skipping windows that have reset.
    pub fn line(status: &Value, now: f64) -> String {
        let mut parts: Vec<String> = Vec::new();
        if let Some(name) = status.get("model").and_then(|m| m.get("display_name")).and_then(Value::as_str) {
            if !name.is_empty() {
                parts.push(name.to_string());
            }
        }
        for (key, label) in [("five_hour", "5h"), ("seven_day", "7d")] {
            let Some(window) = status.get("rate_limits").and_then(|l| l.get(key)) else {
                continue;
            };
            let Some(used) = window.get("used_percentage").and_then(Value::as_f64) else {
                continue;
            };
            if window.get("resets_at").and_then(Value::as_f64).is_some_and(|at| at <= now) {
                continue;
            }
            parts.push(format!("{label} {}%", used.round() as i64));
        }
        parts.join(" · ")
    }
}

mod child {
    use super::*;
    use std::process::Command;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;
    use std::time::Duration;

    pub struct Outcome {
        pub code: i32,
        /// The child died from Ctrl-C; this process should too, once its lease is released,
        /// so a calling shell script stops instead of carrying on.
        pub interrupted: bool,
    }

    /// Runs `arguments` in the foreground (same terminal, same process group, inherited stdio),
    /// calling `renew` every ten seconds until it returns false.
    pub fn run(arguments: &[String], renew: impl Fn() -> bool + Send + 'static) -> Result<Outcome> {
        let mut command = Command::new(&arguments[0]);
        command.args(&arguments[1..]);
        let signals = prepare(&mut command);
        let mut process = command
            .spawn()
            .map_err(|error| CoffeeError::msg(format!("Couldn’t run {}: {error}", arguments[0])))?;
        child_started(process.id());
        let finished = Arc::new(AtomicBool::new(false));
        let watcher = {
            let finished = finished.clone();
            std::thread::spawn(move || 'renewing: loop {
                for _ in 0..100 {
                    std::thread::sleep(Duration::from_millis(100));
                    if finished.load(Ordering::SeqCst) {
                        break 'renewing;
                    }
                }
                if !renew() {
                    break;
                }
            })
        };
        let status = process.wait()?;
        finished.store(true, Ordering::SeqCst);
        let _ = watcher.join();
        Ok(outcome(status, &signals))
    }

    #[cfg(unix)]
    pub struct Signals {
        /// Signals the caller chose to ignore (nohup ignores SIGHUP; shells ignore SIGINT for `&` jobs).
        ignored: Vec<libc::c_int>,
    }

    #[cfg(unix)]
    static CHILD: std::sync::atomic::AtomicI32 = std::sync::atomic::AtomicI32::new(0);
    /// Signals that arrived before the child existed, one bit per signal number.
    #[cfg(unix)]
    static PENDING: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

    /// Async-signal-safe: passes the signal to the child, or holds it until the child exists.
    /// Whichever of this handler and `child_started` clears the pending bit delivers it, so a
    /// signal is never lost or doubled around the spawn.
    #[cfg(unix)]
    extern "C" fn forward(signal: libc::c_int) {
        let bit = 1u64 << signal;
        PENDING.fetch_or(bit, Ordering::SeqCst);
        let pid = CHILD.load(Ordering::SeqCst);
        if pid > 0 && PENDING.fetch_and(!bit, Ordering::SeqCst) & bit != 0 {
            unsafe { libc::kill(pid, signal) };
        }
    }

    #[cfg(unix)]
    fn is_ignored(signal: libc::c_int) -> bool {
        unsafe {
            let mut current: libc::sigaction = std::mem::zeroed();
            libc::sigaction(signal, std::ptr::null(), &mut current) == 0 && current.sa_sigaction == libc::SIG_IGN
        }
    }

    /// In a terminal's foreground job, Ctrl-C and a hangup already reach the child (it shares this
    /// process's group), so forwarding them would deliver them twice. Without a terminal they can
    /// only arrive by PID, so they're forwarded. SIGTERM usually arrives by PID (`kill`, `timeout`,
    /// supervisors) and is always forwarded. This process never dies from them: it waits, so the
    /// session lasts until the child actually exits, even if it catches a signal and keeps going.
    #[cfg(unix)]
    fn prepare(command: &mut Command) -> Signals {
        use std::os::unix::process::CommandExt;
        let handled = [libc::SIGINT, libc::SIGHUP, libc::SIGTERM];
        let ignored: Vec<libc::c_int> = handled.into_iter().filter(|s| is_ignored(*s)).collect();
        let terminal_delivers = [0, 1, 2]
            .into_iter()
            .any(|fd| unsafe { libc::isatty(fd) == 1 && libc::tcgetpgrp(fd) == libc::getpgrp() });
        let forwarded: Vec<libc::c_int> = if terminal_delivers { vec![libc::SIGTERM] } else { handled.to_vec() };
        for signal in handled {
            let action = if ignored.contains(&signal) {
                libc::SIG_IGN
            } else if forwarded.contains(&signal) {
                forward as extern "C" fn(libc::c_int) as libc::sighandler_t
            } else {
                libc::SIG_IGN
            };
            unsafe { libc::signal(signal, action) };
        }
        // Handlers reset to the default across exec, but ignored signals stay ignored: give the
        // child back the defaults for what this process ignores, keeping what the caller ignored.
        // SIGPIPE is ignored by this CLI itself, so the child always gets the default.
        let restore: Vec<libc::c_int> = handled
            .into_iter()
            .filter(|s| !ignored.contains(s))
            .chain([libc::SIGPIPE])
            .collect();
        unsafe {
            command.pre_exec(move || {
                for signal in &restore {
                    libc::signal(*signal, libc::SIG_DFL);
                }
                Ok(())
            });
        }
        Signals { ignored }
    }

    #[cfg(unix)]
    fn child_started(pid: u32) {
        CHILD.store(pid as i32, Ordering::SeqCst);
        let pending = PENDING.swap(0, Ordering::SeqCst);
        for signal in 1..64 {
            if pending & (1u64 << signal) != 0 {
                unsafe { libc::kill(pid as i32, signal) };
            }
        }
    }

    #[cfg(unix)]
    fn outcome(status: std::process::ExitStatus, signals: &Signals) -> Outcome {
        use std::os::unix::process::ExitStatusExt;
        let signal = status.signal();
        Outcome {
            code: status.code().unwrap_or_else(|| 128 + signal.unwrap_or(0)),
            interrupted: signal == Some(libc::SIGINT) && !signals.ignored.contains(&libc::SIGINT),
        }
    }

    /// Dies from SIGINT, as the child did.
    #[cfg(unix)]
    pub fn reraise_interrupt() {
        unsafe {
            libc::signal(libc::SIGINT, libc::SIG_DFL);
            libc::kill(libc::getpid(), libc::SIGINT);
        }
    }

    #[cfg(windows)]
    pub struct Signals;

    #[cfg(windows)]
    unsafe extern "system" fn ignore_console_event(_event: u32) -> windows_sys::core::BOOL {
        1
    }

    /// Ctrl-C and Ctrl-Break reach every process on the console, including the child; this one
    /// ignores them and waits for the child to finish. (A handler isn't inherited, unlike
    /// "ignore Ctrl-C", so the child keeps the default behavior.)
    #[cfg(windows)]
    fn prepare(_command: &mut Command) -> Signals {
        unsafe { windows_sys::Win32::System::Console::SetConsoleCtrlHandler(Some(ignore_console_event), 1) };
        Signals
    }

    #[cfg(windows)]
    fn child_started(_pid: u32) {}

    #[cfg(windows)]
    fn outcome(status: std::process::ExitStatus, _signals: &Signals) -> Outcome {
        Outcome {
            code: status.code().unwrap_or(1),
            interrupted: false,
        }
    }

    #[cfg(windows)]
    pub fn reraise_interrupt() {}
}

fn help() -> String {
    let place = if cfg!(windows) { "notification area" } else { "system tray" };
    format!(
        "Barista {VERSION} · keeps your computer awake while work happens

barista status [--json]
barista start --name \"Build\" [--minutes 30 | --seconds 300] [--display] [--pid PID]
barista heartbeat SESSION_ID [--ttl 300] [--progress 0.5] [--message \"Running tests\"]
barista extend SESSION_ID [--minutes 15 | --seconds 900]
barista stop SESSION_ID
barista stop --all
barista history [--json]
barista run [--name \"Build\"] [--display] [--max-awake 28800] -- COMMAND [ARG ...]
barista usage [--period today|week|month|all] [--json]
barista statusline [install|uninstall|status] [--settings PATH]
barista limits refresh [--json]
barista mcp
barista hooks install|uninstall|status [--agent claude|codex|cursor] [--settings PATH]
barista setup [--agent claude|codex|cursor]
barista version

Agent sessions default to screen-off-friendly, expire after 5 minutes without renewal,
and have an 8-hour hard limit (--max-awake accepts seconds, up to 24 hours).
run renews automatically while its command runs, preserves output/exit code,
and releases its own session on completion. Other sessions remain active.
Stopping a session never terminates the underlying command.

setup connects every agent it finds (Claude Code, Codex, Cursor) and starts Barista.
hooks install connects Claude Code (or Codex with --agent codex, which then asks you to
trust the hooks once with /hooks, or Cursor with --agent cursor): prompts start a session,
tool calls renew it, and the session ends when the agent finishes its turn or exits.

usage reports sessions and tokens for Claude Code and Codex (from their local logs) and
Cursor turns (recorded by its hooks) for today, this week, and this month, with costs
estimated at API list prices. --period shows one period by model. It starts with plan limits:
Claude's (from your Claude account, which the Barista app checks every few minutes using Claude
Code's saved login, or Claude Code's status line) and ChatGPT's (from Codex's logs). limits refresh
checks your Claude account once. statusline is Claude Code's status line command that records
Claude's limits; setup installs it, keeping any status line you already had working behind it.

Global options: --json, --no-launch, --socket PATH. Barista (the coffee cup in the {place})
starts automatically when needed. MCP uses newline-delimited JSON-RPC over stdio; no TCP
port is opened."
    )
}
