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

/// What a click on a notification's button runs: a menu command id, sent to the tray's event loop.
type ActionHandler = Box<dyn Fn(String) + Send + Sync>;
static ACTION_HANDLER: std::sync::OnceLock<ActionHandler> = std::sync::OnceLock::new();

// macOS development builds have no tray, so nothing registers a handler there.
#[cfg_attr(target_os = "macos", allow(dead_code))]
pub fn on_notification_action(handler: impl Fn(String) + Send + Sync + 'static) {
    let _ = ACTION_HANDLER.set(Box::new(handler));
}

fn run_action(command: &str) {
    if let Some(handler) = ACTION_HANDLER.get() {
        handler(command.to_string());
    }
}

/// Shows a desktop notification without blocking the caller.
pub fn notify(title: &str, body: &str) {
    show_notification(title, body, None);
}

/// A notification with one button that runs `command` (a menu command id) when clicked.
pub fn notify_with_action(title: &str, body: &str, button: &str, command: &str) {
    show_notification(title, body, Some((button.to_string(), command.to_string())));
}

#[cfg(not(windows))]
fn show_notification(title: &str, body: &str, action: Option<(String, String)>) {
    let (title, body) = (title.to_string(), body.to_string());
    std::thread::spawn(move || {
        let mut notification = notify_rust::Notification::new();
        notification.appname("Barista").icon("barista").summary(&title).body(&body);
        if let Some((label, command)) = &action {
            notification.action(command, label);
        }
        let Ok(handle) = notification.show() else { return };
        #[cfg(all(unix, not(target_os = "macos")))]
        if action.is_some() {
            // Waits on this thread for a click (or for the notification to close).
            handle.wait_for_action(|chosen| {
                if let Some((_, command)) = &action {
                    if chosen == command || chosen == "default" {
                        run_action(command);
                    }
                }
            });
        }
        #[cfg(not(all(unix, not(target_os = "macos"))))]
        let _ = (handle, &action, run_action as fn(&str));
    });
}

/// Windows: a toast from Barista itself (its name and cup, not "Windows PowerShell"), with an
/// optional button. Windows delivers a click only while the toast object is alive, so this thread
/// holds it for the time the banner shows.
#[cfg(windows)]
fn show_notification(title: &str, body: &str, action: Option<(String, String)>) {
    use tauri_winrt_notification::{IconCrop, Toast};
    let (title, body) = (title.to_string(), body.to_string());
    std::thread::spawn(move || {
        let mut toast = Toast::new(identity::APP_ID).title(&title).text1(&body);
        if let Some(icon) = identity::icon_file() {
            toast = toast.icon(&icon, IconCrop::Square, "Barista");
        }
        if let Some((label, command)) = &action {
            let target = command.clone();
            toast = toast.add_button(label, command).on_activated(move |chosen| {
                // The button sends its command; clicking the toast itself sends none.
                if chosen.is_none() || chosen.as_deref() == Some(target.as_str()) {
                    run_action(&target);
                }
                Ok(())
            });
        }
        if toast.show().is_ok() && action.is_some() {
            std::thread::sleep(std::time::Duration::from_secs(30));
        }
    });
}

/// Windows: Barista's identity for notifications and the taskbar. An unpackaged app gets its toast
/// name and icon from HKCU\Software\Classes\AppUserModelId\<id>; the installer's Start menu
/// shortcut carries the same id.
#[cfg(windows)]
pub mod identity {
    use std::path::PathBuf;

    pub const APP_ID: &str = "Barista.App";
    const ICON: &[u8] = include_bytes!("../assets/barista.png");

    /// The cup, written once to Barista's local data folder (toasts need a file path).
    pub fn icon_file() -> Option<PathBuf> {
        let path = dirs::data_local_dir()?.join("Barista").join("barista-256.png");
        if std::fs::read(&path).ok().as_deref() != Some(ICON) {
            std::fs::create_dir_all(path.parent()?).ok()?;
            std::fs::write(&path, ICON).ok()?;
        }
        Some(path)
    }

    pub fn register() {
        use winreg::enums::HKEY_CURRENT_USER;
        let icon = icon_file();
        if let Ok((key, _)) = winreg::RegKey::predef(HKEY_CURRENT_USER).create_subkey(format!(r"Software\Classes\AppUserModelId\{APP_ID}"))
        {
            let _ = key.set_value("DisplayName", &"Barista");
            if let Some(icon) = &icon {
                let _ = key.set_value("IconUri", &icon.to_string_lossy().to_string());
            }
        }
        let id: Vec<u16> = APP_ID.encode_utf16().chain([0]).collect();
        unsafe {
            let _ = windows::Win32::UI::Shell::SetCurrentProcessExplicitAppUserModelID(windows::core::PCWSTR(id.as_ptr()));
        }
    }
}
