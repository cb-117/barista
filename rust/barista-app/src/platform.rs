//! Launch at login, opening links, and desktop notifications.
use std::path::Path;
use std::process::{Command, Stdio};

/// Development builds run from Cargo's target folder; registering those paths would break.
pub fn is_development_build(executable: &Path) -> bool {
    executable.components().any(|c| c.as_os_str() == "target")
}

#[cfg(target_os = "linux")]
mod login {
    use std::path::{Path, PathBuf};

    fn entry() -> PathBuf {
        dirs::config_dir().unwrap_or_default().join("autostart").join("barista.desktop")
    }
    pub fn is_enabled() -> bool {
        entry().is_file()
    }
    pub fn set(enabled: bool, executable: &Path) -> std::io::Result<()> {
        let path = entry();
        if !enabled {
            return match std::fs::remove_file(&path) {
                Err(error) if error.kind() != std::io::ErrorKind::NotFound => Err(error),
                _ => Ok(()),
            };
        }
        std::fs::create_dir_all(path.parent().unwrap())?;
        // Desktop Entry Exec quoting: wrap in double quotes, escaping ", `, $, and \.
        let quoted: String = executable
            .to_string_lossy()
            .chars()
            .flat_map(|c| {
                if matches!(c, '"' | '`' | '$' | '\\') {
                    vec!['\\', c]
                } else {
                    vec![c]
                }
            })
            .collect();
        let text = format!(
            "[Desktop Entry]\nType=Application\nName=Barista\nComment=Keeps your computer awake while work happens\nExec=\"{quoted}\" --background\nIcon=barista\nTerminal=false\nX-GNOME-Autostart-enabled=true\n"
        );
        std::fs::write(path, text)
    }
}

#[cfg(windows)]
mod login {
    use std::path::Path;
    use winreg::enums::{HKEY_CURRENT_USER, KEY_READ, KEY_WRITE};
    use winreg::RegKey;

    const RUN: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";

    pub fn is_enabled() -> bool {
        RegKey::predef(HKEY_CURRENT_USER)
            .open_subkey_with_flags(RUN, KEY_READ)
            .and_then(|key| key.get_value::<String, _>("Barista"))
            .is_ok()
    }
    pub fn set(enabled: bool, executable: &Path) -> std::io::Result<()> {
        let (key, _) = RegKey::predef(HKEY_CURRENT_USER).create_subkey_with_flags(RUN, KEY_WRITE)?;
        if enabled {
            key.set_value("Barista", &format!("\"{}\" --background", executable.display()))
        } else {
            match key.delete_value("Barista") {
                Err(error) if error.kind() != std::io::ErrorKind::NotFound => Err(error),
                _ => Ok(()),
            }
        }
    }
}

#[cfg(not(any(target_os = "linux", windows)))]
mod login {
    use std::path::Path;
    pub fn is_enabled() -> bool {
        false
    }
    pub fn set(_enabled: bool, _executable: &Path) -> std::io::Result<()> {
        Err(std::io::Error::other("Use the Mac app for launch at login."))
    }
}

pub use login::{is_enabled as launches_at_login, set as set_launch_at_login};

pub fn open_url(url: &str) {
    let mut command = if cfg!(windows) {
        let mut command = Command::new("rundll32");
        command.args(["url.dll,FileProtocolHandler", url]);
        command
    } else if cfg!(target_os = "macos") {
        let mut command = Command::new("open");
        command.arg(url);
        command
    } else {
        let mut command = Command::new("xdg-open");
        command.arg(url);
        command
    };
    let _ = command.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn();
}

/// Shows a desktop notification without blocking the caller.
pub fn notify(title: &str, body: &str) {
    let (title, body) = (title.to_string(), body.to_string());
    std::thread::spawn(move || {
        let _ = notify_rust::Notification::new()
            .appname("Barista")
            .summary(&title)
            .body(&body)
            .show();
    });
}
