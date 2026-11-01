//! Native "stay awake" requests. Every backend prevents *idle* sleep only: closing a lid or
//! choosing Sleep still works. All requests belong to this process, so the OS drops them if it exits.
use std::sync::{Arc, Mutex};

pub trait PowerAsserting: Send {
    /// Replaces any current request. On failure, the previous request must still be held.
    fn activate(&mut self, keep_display_awake: bool) -> Result<(), String>;
    fn deactivate(&mut self);
}

/// The platform's real implementation.
pub fn native() -> Box<dyn PowerAsserting> {
    #[cfg(windows)]
    return Box::new(windows::WindowsPower::default());
    #[cfg(target_os = "linux")]
    return Box::new(linux::LinuxPower::default());
    #[cfg(target_os = "macos")]
    return Box::new(macos::Caffeinate::default());
    #[allow(unreachable_code)]
    Box::new(FakePower::default())
}

/// Records requests without touching the OS; used by tests and `barista-app --serve`.
#[derive(Clone, Default)]
pub struct FakePower {
    inner: Arc<Mutex<(Option<bool>, bool)>>,
}

impl FakePower {
    /// None when released, otherwise whether the display is kept on.
    pub fn state(&self) -> Option<bool> {
        self.inner.lock().unwrap().0
    }
    pub fn fail(&self, fail: bool) {
        self.inner.lock().unwrap().1 = fail;
    }
}

impl PowerAsserting for FakePower {
    fn activate(&mut self, keep_display_awake: bool) -> Result<(), String> {
        let mut inner = self.inner.lock().unwrap();
        if inner.1 {
            return Err("The system couldn't start the awake session. Please try again.".into());
        }
        inner.0 = Some(keep_display_awake);
        Ok(())
    }
    fn deactivate(&mut self) {
        self.inner.lock().unwrap().0 = None;
    }
}

#[cfg(windows)]
mod windows {
    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Power::{
        PowerClearRequest, PowerCreateRequest, PowerRequestDisplayRequired, PowerRequestSystemRequired, PowerSetRequest, POWER_REQUEST_TYPE,
    };
    use windows_sys::Win32::System::Threading::{POWER_REQUEST_CONTEXT_SIMPLE_STRING, REASON_CONTEXT, REASON_CONTEXT_0};

    /// A power request object, like `powercfg /requests` lists. Not tied to a thread.
    struct Request {
        handle: HANDLE,
        types: Vec<POWER_REQUEST_TYPE>,
    }
    // SAFETY: power request handles may be used and closed from any thread.
    unsafe impl Send for Request {}

    impl Drop for Request {
        fn drop(&mut self) {
            unsafe {
                for kind in &self.types {
                    PowerClearRequest(self.handle, *kind);
                }
                CloseHandle(self.handle);
            }
        }
    }

    #[derive(Default)]
    pub struct WindowsPower {
        current: Option<Request>,
    }

    impl super::PowerAsserting for WindowsPower {
        fn activate(&mut self, keep_display_awake: bool) -> Result<(), String> {
            let mut reason: Vec<u16> = "Barista awake session".encode_utf16().chain([0]).collect();
            let context = REASON_CONTEXT {
                Version: 0, // POWER_REQUEST_CONTEXT_VERSION
                Flags: POWER_REQUEST_CONTEXT_SIMPLE_STRING,
                Reason: REASON_CONTEXT_0 {
                    SimpleReasonString: reason.as_mut_ptr(),
                },
            };
            let handle = unsafe { PowerCreateRequest(&context) };
            if handle.is_null() || handle == INVALID_HANDLE_VALUE {
                return Err(failure());
            }
            let mut request = Request { handle, types: Vec::new() };
            let mut types = vec![PowerRequestSystemRequired];
            if keep_display_awake {
                types.push(PowerRequestDisplayRequired);
            }
            for kind in types {
                if unsafe { PowerSetRequest(handle, kind) } == 0 {
                    return Err(failure()); // `request` drops, clearing what was set.
                }
                request.types.push(kind);
            }
            self.current = Some(request);
            Ok(())
        }
        fn deactivate(&mut self) {
            self.current = None;
        }
    }

    fn failure() -> String {
        format!(
            "Windows couldn't start the awake session ({}). Please try again.",
            std::io::Error::last_os_error()
        )
    }
}

#[cfg(target_os = "linux")]
mod linux {
    //! Desktop sessions decide when to idle-suspend, so Barista asks them first: GNOME's session
    //! manager, else the freedesktop PowerManagement and ScreenSaver services (KDE, Xfce, and
    //! others). It also takes a systemd-logind "idle" inhibitor, which covers logind's own
    //! IdleAction on minimal and headless systems.
    use zbus::blocking::Connection;
    use zbus::zvariant::OwnedFd;

    const APP: &str = "Barista";
    const REASON: &str = "Keeping the computer awake while work happens";
    const GNOME_SUSPEND: u32 = 4;
    const GNOME_IDLE: u32 = 8;

    enum Hold {
        /// Held while the descriptor is open.
        Logind(#[allow(dead_code)] OwnedFd),
        Gnome(u32),
        PowerManagement(u32),
        ScreenSaver(u32),
    }

    #[derive(Default)]
    pub struct LinuxPower {
        held: Vec<Hold>,
        // GNOME and ScreenSaver inhibitors end when the connection that made them closes,
        // so both connections live as long as Barista.
        session: Option<Connection>,
        system: Option<Connection>,
    }

    impl LinuxPower {
        fn session(&mut self) -> Option<&Connection> {
            if self.session.is_none() {
                self.session = Connection::session().ok();
            }
            self.session.as_ref()
        }
        fn system(&mut self) -> Option<&Connection> {
            if self.system.is_none() {
                self.system = Connection::system().ok();
            }
            self.system.as_ref()
        }

        fn release(&mut self, holds: Vec<Hold>) {
            for hold in holds {
                let call = |connection: Option<&Connection>, destination: &str, path: &str, interface: &str, method: &str, cookie: u32| {
                    if let Some(connection) = connection {
                        let _ = connection.call_method(Some(destination), path, Some(interface), method, &(cookie,));
                    }
                };
                match hold {
                    Hold::Logind(fd) => drop(fd),
                    Hold::Gnome(cookie) => call(
                        self.session.as_ref(),
                        "org.gnome.SessionManager",
                        "/org/gnome/SessionManager",
                        "org.gnome.SessionManager",
                        "Uninhibit",
                        cookie,
                    ),
                    Hold::PowerManagement(cookie) => call(
                        self.session.as_ref(),
                        "org.freedesktop.PowerManagement",
                        "/org/freedesktop/PowerManagement/Inhibit",
                        "org.freedesktop.PowerManagement.Inhibit",
                        "UnInhibit",
                        cookie,
                    ),
                    Hold::ScreenSaver(cookie) => call(
                        self.session.as_ref(),
                        "org.freedesktop.ScreenSaver",
                        "/org/freedesktop/ScreenSaver",
                        "org.freedesktop.ScreenSaver",
                        "UnInhibit",
                        cookie,
                    ),
                }
            }
        }
    }

    impl super::PowerAsserting for LinuxPower {
        fn activate(&mut self, keep_display_awake: bool) -> Result<(), String> {
            let mut holds = Vec::new();
            let mut system = false;
            let mut display = !keep_display_awake;
            if let Some(session) = self.session() {
                let flags = GNOME_SUSPEND | if keep_display_awake { GNOME_IDLE } else { 0 };
                let gnome = session
                    .call_method(
                        Some("org.gnome.SessionManager"),
                        "/org/gnome/SessionManager",
                        Some("org.gnome.SessionManager"),
                        "Inhibit",
                        &(APP, 0u32, REASON, flags),
                    )
                    .and_then(|reply| reply.body().deserialize::<u32>());
                if let Ok(cookie) = gnome {
                    holds.push(Hold::Gnome(cookie));
                    system = true;
                    display = true;
                } else {
                    let pm = session
                        .call_method(
                            Some("org.freedesktop.PowerManagement"),
                            "/org/freedesktop/PowerManagement/Inhibit",
                            Some("org.freedesktop.PowerManagement.Inhibit"),
                            "Inhibit",
                            &(APP, REASON),
                        )
                        .and_then(|reply| reply.body().deserialize::<u32>());
                    if let Ok(cookie) = pm {
                        holds.push(Hold::PowerManagement(cookie));
                        system = true;
                    }
                    if keep_display_awake {
                        let saver = session
                            .call_method(
                                Some("org.freedesktop.ScreenSaver"),
                                "/org/freedesktop/ScreenSaver",
                                Some("org.freedesktop.ScreenSaver"),
                                "Inhibit",
                                &(APP, REASON),
                            )
                            .and_then(|reply| reply.body().deserialize::<u32>());
                        if let Ok(cookie) = saver {
                            holds.push(Hold::ScreenSaver(cookie));
                            display = true;
                        }
                    }
                }
            }
            if let Some(bus) = self.system() {
                let logind = bus
                    .call_method(
                        Some("org.freedesktop.login1"),
                        "/org/freedesktop/login1",
                        Some("org.freedesktop.login1.Manager"),
                        "Inhibit",
                        &("idle", APP, REASON, "block"),
                    )
                    .and_then(|reply| reply.body().deserialize::<OwnedFd>());
                if let Ok(fd) = logind {
                    holds.push(Hold::Logind(fd));
                    system = true;
                }
            }
            if !system || !display {
                self.release(holds);
                return Err(if !system {
                    "Couldn't reach systemd-logind or the desktop's power service over D-Bus, so Linux can't be kept awake.".into()
                } else {
                    "This desktop doesn't offer a screensaver inhibitor over D-Bus, so the display can't be kept on.".into()
                });
            }
            let old = std::mem::replace(&mut self.held, holds);
            self.release(old);
            Ok(())
        }
        fn deactivate(&mut self) {
            let old = std::mem::take(&mut self.held);
            self.release(old);
        }
    }
}

#[cfg(target_os = "macos")]
mod macos {
    //! Development builds only; the Mac app is the Swift one. Uses the system's `caffeinate`,
    //! which exits on its own if this process dies (`-w`).
    use std::process::{Child, Command, Stdio};

    #[derive(Default)]
    pub struct Caffeinate {
        child: Option<Child>,
    }

    impl super::PowerAsserting for Caffeinate {
        fn activate(&mut self, keep_display_awake: bool) -> Result<(), String> {
            let mut command = Command::new("/usr/bin/caffeinate");
            command.arg(if keep_display_awake { "-id" } else { "-i" });
            command.args(["-w", &std::process::id().to_string()]);
            let child = command
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .map_err(|error| format!("macOS couldn't start the awake session ({error})."))?;
            self.deactivate();
            self.child = Some(child);
            Ok(())
        }
        fn deactivate(&mut self) {
            if let Some(mut child) = self.child.take() {
                let _ = child.kill();
                let _ = child.wait();
            }
        }
    }

    impl Drop for Caffeinate {
        fn drop(&mut self) {
            super::PowerAsserting::deactivate(self);
        }
    }
}
