//! A client that opens Barista when it isn't running yet.
use crate::control::{default_path, ControlClient};
use crate::models::*;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

#[derive(Clone)]
pub struct AgentConnection {
    pub path: String,
    pub auto_launch: bool,
}

impl AgentConnection {
    pub fn new(path: impl Into<String>, auto_launch: bool) -> Self {
        Self {
            path: path.into(),
            auto_launch,
        }
    }

    pub fn send(&self, request: &ControlRequest) -> Result<ControlReply> {
        let client = ControlClient::new(&self.path);
        match client.send(request) {
            Err(CoffeeError::Unreachable(_)) => {
                if !self.auto_launch || self.path != default_path() {
                    return Err(CoffeeError::msg("Barista is not running. Open Barista and try again."));
                }
                launch_app()?;
                // Connect only: no mutation is retried after a request has been sent.
                let deadline = Instant::now() + Duration::from_secs(6);
                while Instant::now() < deadline {
                    match client.send(&ControlRequest::new(ControlAction::Status)) {
                        Ok(_) => return client.send(request),
                        Err(CoffeeError::Unreachable(_)) => std::thread::sleep(Duration::from_millis(100)),
                        Err(error) => return Err(error),
                    }
                }
                Err(CoffeeError::msg(
                    "Barista did not start its control service. Quit any older copy and open the current app.",
                ))
            }
            other => other,
        }
    }
}

pub const APP_NAME: &str = if cfg!(windows) { "barista-app.exe" } else { "barista-app" };

/// This executable, with symlinks resolved.
pub fn current_executable() -> PathBuf {
    let path = std::env::current_exe().unwrap_or_else(|_| PathBuf::from(std::env::args().next().unwrap_or_default()));
    std::fs::canonicalize(&path).map(strip_verbatim).unwrap_or(path)
}

/// `canonicalize` returns `\\?\C:\…` on Windows, which hook commands and shells don't expect.
pub fn strip_verbatim(path: PathBuf) -> PathBuf {
    #[cfg(windows)]
    if let Some(text) = path.to_str() {
        if let Some(rest) = text.strip_prefix(r"\\?\") {
            if !rest.starts_with("UNC\\") {
                return PathBuf::from(rest);
            }
        }
    }
    path
}

/// The tray app installed next to this CLI (or named by BARISTA_APP, or on PATH).
pub fn find_app() -> Option<PathBuf> {
    let mut candidates = Vec::new();
    if let Some(path) = std::env::var_os("BARISTA_APP") {
        candidates.push(PathBuf::from(path));
    }
    if let Some(directory) = current_executable().parent() {
        candidates.push(directory.join(APP_NAME));
    }
    if let Some(paths) = std::env::var_os("PATH") {
        candidates.extend(std::env::split_paths(&paths).map(|directory| directory.join(APP_NAME)));
    }
    candidates.into_iter().find(|path| path.is_file())
}

fn launch_app() -> Result<()> {
    let app = find_app().ok_or_else(|| {
        CoffeeError::msg(format!(
            "Cannot locate {APP_NAME}. Open Barista manually, or set BARISTA_APP to its full path."
        ))
    })?;
    let mut command = Command::new(&app);
    command
        .arg("--background")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    detach(&mut command);
    command
        .spawn()
        .map_err(|error| CoffeeError::msg(format!("Couldn't open Barista: {error}")))?;
    Ok(())
}

/// Runs the child in its own session/process group, without a console window, so it outlives us.
pub fn detach(command: &mut Command) {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        unsafe {
            command.pre_exec(|| {
                libc::setsid();
                Ok(())
            });
        }
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const DETACHED_PROCESS: u32 = 0x0000_0008;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP | CREATE_NO_WINDOW);
    }
}
