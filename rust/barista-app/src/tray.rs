//! The coffee cup in the notification area (Windows) or system tray (Linux). What the menu shows and
//! does lives in `menu.rs`; this file only draws it. No GUI toolkit is involved: Windows uses
//! tray-icon's native backend with a small message pump, and Linux serves a StatusNotifierItem over
//! D-Bus with ksni, so there's no GTK (or glib) anywhere.
use crate::menu::{self, Entry, Icons, Model};
use crate::{lock, platform, refresh_usage, Shared};
use barista_core::control::ControlServer;
use std::sync::mpsc::{Receiver, Sender};
use std::time::{Duration, Instant};

/// What the platform's tray reports to the loop.
pub enum UserEvent {
    /// A menu entry was chosen; the id is a command for `menu::handle`.
    Menu(String),
    /// The menu is about to show, so usage in it should be fresh.
    Opened,
}

/// Runs the tray until Quit. `wait` blocks for up to a second and returns the next tray event while
/// keeping the platform's tray machinery running.
fn event_loop(app: Shared, server: ControlServer, background: bool, mut wait: impl FnMut(&Receiver<UserEvent>) -> Option<UserEvent>) -> ! {
    let (sender, events) = std::sync::mpsc::channel::<UserEvent>();
    let icons = menu::icons();
    let model = menu::build(&app);
    let tray = backend::Tray::create(&model, &icons, sender);
    if tray.is_none() {
        eprintln!("barista-app: couldn't show the tray icon. Barista keeps running.");
    }
    if !background {
        let place = if cfg!(windows) {
            "notification area (you may need to click ^ to see it)"
        } else {
            "system tray"
        };
        platform::notify("Barista is running", &format!("Find the coffee cup in the {place}."));
    }
    let mut shown = model.signature;
    let mut last_agent_refresh = Instant::now();
    loop {
        match wait(&events) {
            // Usage shown in a menu that's opening should be at most a minute old.
            Some(UserEvent::Opened) => refresh_usage(&app, Duration::from_secs(60)),
            Some(UserEvent::Menu(id)) => {
                if menu::handle(&app, &id) {
                    lock(&app).shutdown();
                    server.stop();
                    drop(tray);
                    std::process::exit(0);
                }
                shown.clear(); // Redraw right away.
            }
            None => {}
        }
        if last_agent_refresh.elapsed() > Duration::from_secs(30) {
            lock(&app).refresh_agents();
            last_agent_refresh = Instant::now();
        }
        if let Some(tray) = &tray {
            let model = menu::build(&app);
            if model.signature != shown {
                tray.show(&model, &icons);
                shown = model.signature;
            }
        }
    }
}

/// Linux: a StatusNotifierItem served by ksni on its own D-Bus thread, so the loop only waits.
#[cfg(target_os = "linux")]
pub fn run(app: Shared, server: ControlServer, background: bool) -> ! {
    event_loop(app, server, background, |events| events.recv_timeout(Duration::from_secs(1)).ok())
}

/// Windows: the tray and its menu deliver clicks as window messages to this thread, so pump them,
/// waking at least every 100 ms to pass on menu events.
#[cfg(windows)]
pub fn run(app: Shared, server: ControlServer, background: bool) -> ! {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        DispatchMessageW, MsgWaitForMultipleObjects, PeekMessageW, TranslateMessage, MSG, PM_REMOVE, QS_ALLINPUT, WM_QUIT,
    };
    backend::prepare_process();
    event_loop(app, server, background, |events| {
        let deadline = Instant::now() + Duration::from_secs(1);
        loop {
            unsafe {
                let mut message: MSG = std::mem::zeroed();
                while PeekMessageW(&mut message, std::ptr::null_mut(), 0, 0, PM_REMOVE) != 0 {
                    if message.message == WM_QUIT {
                        // Asked to exit (e.g. PostQuitMessage): quit the normal way.
                        return Some(UserEvent::Menu("quit".into()));
                    }
                    TranslateMessage(&message);
                    DispatchMessageW(&message);
                }
            }
            if let Ok(event) = events.try_recv() {
                return Some(event);
            }
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return None;
            }
            let wait = left.min(Duration::from_millis(100)).as_millis() as u32;
            unsafe { MsgWaitForMultipleObjects(0, std::ptr::null(), 0, wait, QS_ALLINPUT) };
        }
    })
}

#[cfg(windows)]
mod backend {
    use super::{Entry, Icons, Model, Sender, UserEvent};
    use tray_icon::menu::{CheckMenuItem, IsMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem, Submenu};
    use tray_icon::{Icon, TrayIcon, TrayIconBuilder, TrayIconEvent};

    /// Per-monitor DPI awareness (so menus aren't blurry on scaled displays) and dark popup menus in
    /// dark mode, as tao used to set up. Every function is looked up at run time, with the same
    /// fallbacks and version checks tao used, so older Windows versions simply skip what they lack.
    pub fn prepare_process() {
        use windows_sys::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};
        unsafe fn function(library: &str, name: &[u8]) -> Option<unsafe extern "system" fn() -> isize> {
            let wide: Vec<u16> = library.encode_utf16().chain([0]).collect();
            let module = unsafe { LoadLibraryW(wide.as_ptr()) };
            if module.is_null() {
                return None;
            }
            unsafe { GetProcAddress(module, name.as_ptr()) }
        }
        unsafe fn ordinal(library: &str, number: usize) -> Option<unsafe extern "system" fn() -> isize> {
            let wide: Vec<u16> = library.encode_utf16().chain([0]).collect();
            let module = unsafe { LoadLibraryW(wide.as_ptr()) };
            if module.is_null() {
                return None;
            }
            unsafe { GetProcAddress(module, number as *const u8) }
        }
        unsafe {
            // DPI: Windows 10 1703+ per-monitor v2, else 8.1+ per-monitor, else Vista+ system DPI.
            const PER_MONITOR_AWARE_V2: isize = -4;
            if let Some(f) = function("user32.dll", b"SetProcessDpiAwarenessContext\0") {
                let f: unsafe extern "system" fn(isize) -> i32 = std::mem::transmute(f);
                f(PER_MONITOR_AWARE_V2);
            } else if let Some(f) = function("shcore.dll", b"SetProcessDpiAwareness\0") {
                let f: unsafe extern "system" fn(i32) -> i32 = std::mem::transmute(f);
                f(2); // PROCESS_PER_MONITOR_DPI_AWARE
            } else if let Some(f) = function("user32.dll", b"SetProcessDPIAware\0") {
                let f: unsafe extern "system" fn() -> i32 = std::mem::transmute(f);
                f();
            }

            // Dark menus use undocumented uxtheme ordinals, so check the build first, like tao:
            // 1809 (17763) has AllowDarkModeForApp(BOOL) at 135; 1903 (18362)+ has
            // SetPreferredAppMode(mode) there (1 = allow dark) and FlushMenuThemes at 136.
            let build = windows_build();
            if build < 17763 {
                return;
            }
            if let Some(f) = ordinal("uxtheme.dll", 135) {
                let f: unsafe extern "system" fn(i32) -> i32 = std::mem::transmute(f);
                f(1); // TRUE for AllowDarkModeForApp, AllowDark for SetPreferredAppMode.
            }
            if build >= 18362 {
                if let Some(f) = ordinal("uxtheme.dll", 136) {
                    let f: unsafe extern "system" fn() = std::mem::transmute(f);
                    f();
                }
            }
        }
    }

    /// The Windows build number from ntdll's RtlGetVersion (GetVersionEx lies without a manifest), or 0.
    fn windows_build() -> u32 {
        use windows_sys::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};
        #[repr(C)]
        struct OsVersionInfo {
            size: u32,
            major: u32,
            minor: u32,
            build: u32,
            platform: u32,
            service_pack: [u16; 128],
        }
        unsafe {
            let wide: Vec<u16> = "ntdll.dll".encode_utf16().chain([0]).collect();
            let module = LoadLibraryW(wide.as_ptr());
            if module.is_null() {
                return 0;
            }
            let Some(f) = GetProcAddress(module, c"RtlGetVersion".as_ptr().cast()) else {
                return 0;
            };
            let f: unsafe extern "system" fn(*mut OsVersionInfo) -> i32 = std::mem::transmute(f);
            let mut info: OsVersionInfo = std::mem::zeroed();
            info.size = std::mem::size_of::<OsVersionInfo>() as u32;
            if f(&mut info) == 0 {
                info.build
            } else {
                0
            }
        }
    }

    pub struct Tray(TrayIcon);

    fn icon(icons: &Icons, active: bool) -> Option<Icon> {
        let pixels = if active { &icons.active } else { &icons.resting };
        Icon::from_rgba(pixels.clone(), icons.width, icons.height).ok()
    }

    fn append(entries: &[Entry], add: &mut dyn FnMut(&dyn IsMenuItem)) {
        for entry in entries {
            match entry {
                Entry::Item { id, text, enabled } => add(&MenuItem::with_id(id.as_str(), text, *enabled, None)),
                Entry::Check { id, text, checked } => add(&CheckMenuItem::with_id(id.as_str(), text, true, *checked, None)),
                Entry::Label(text) => add(&MenuItem::new(text, false, None)),
                Entry::Separator => add(&PredefinedMenuItem::separator()),
                Entry::Submenu { text, enabled, children } => {
                    let submenu = Submenu::new(text, *enabled);
                    append(children, &mut |child| {
                        let _ = submenu.append(child);
                    });
                    add(&submenu);
                }
            }
        }
    }

    fn menu(entries: &[Entry]) -> Menu {
        let menu = Menu::new();
        append(entries, &mut |entry| {
            let _ = menu.append(entry);
        });
        menu
    }

    impl Tray {
        pub fn create(model: &Model, icons: &Icons, sender: Sender<UserEvent>) -> Option<Self> {
            let menu_sender = sender.clone();
            MenuEvent::set_event_handler(Some(move |event: MenuEvent| {
                let _ = menu_sender.send(UserEvent::Menu(event.id.0));
            }));
            TrayIconEvent::set_event_handler(Some(move |_| {
                let _ = sender.send(UserEvent::Opened);
            }));
            let mut builder = TrayIconBuilder::new()
                .with_id("barista")
                .with_menu(Box::new(menu(&model.entries)))
                .with_tooltip(&model.tooltip)
                .with_title(&model.title);
            if let Some(icon) = icon(icons, model.active) {
                builder = builder.with_icon(icon);
            }
            builder.build().ok().map(Tray)
        }

        pub fn show(&self, model: &Model, icons: &Icons) {
            self.0.set_menu(Some(Box::new(menu(&model.entries))));
            let _ = self.0.set_tooltip(Some(&model.tooltip));
            let _ = self.0.set_icon(icon(icons, model.active));
            self.0.set_title(Some(&model.title));
        }
    }
}

#[cfg(target_os = "linux")]
mod backend {
    use super::{Entry, Icons, Model, Sender, UserEvent};
    use ksni::blocking::{Handle, TrayMethods};
    use ksni::menu::{CheckmarkItem, StandardItem, SubMenu};
    use ksni::MenuItem;

    /// What ksni serves. The host draws it; ksni calls back here on clicks.
    struct Cup {
        entries: Vec<Entry>,
        tooltip: String,
        title: String,
        icon: ksni::Icon,
        sender: Sender<UserEvent>,
    }

    /// ksni wants ARGB32 in network byte order; the bundled icon is RGBA.
    fn argb(icons: &Icons, active: bool) -> ksni::Icon {
        let pixels = if active { &icons.active } else { &icons.resting };
        let data = pixels.as_chunks::<4>().0.iter().flat_map(|p| [p[3], p[0], p[1], p[2]]).collect();
        ksni::Icon {
            width: icons.width as i32,
            height: icons.height as i32,
            data,
        }
    }

    /// Without GTK there's no label beside the icon, so the countdown goes in the title (shown by the
    /// host's tooltip and its list of tray items).
    fn title(model: &Model) -> String {
        if model.title.is_empty() {
            "Barista".into()
        } else {
            format!("Barista · {}", model.title)
        }
    }

    fn items(entries: &[Entry]) -> Vec<MenuItem<Cup>> {
        entries
            .iter()
            .map(|entry| match entry {
                Entry::Item { id, text, enabled } => {
                    let id = id.clone();
                    StandardItem {
                        label: text.clone(),
                        enabled: *enabled,
                        activate: Box::new(move |cup: &mut Cup| {
                            let _ = cup.sender.send(UserEvent::Menu(id.clone()));
                        }),
                        ..Default::default()
                    }
                    .into()
                }
                Entry::Check { id, text, checked } => {
                    let id = id.clone();
                    CheckmarkItem {
                        label: text.clone(),
                        checked: *checked,
                        activate: Box::new(move |cup: &mut Cup| {
                            let _ = cup.sender.send(UserEvent::Menu(id.clone()));
                        }),
                        ..Default::default()
                    }
                    .into()
                }
                Entry::Label(text) => StandardItem {
                    label: text.clone(),
                    enabled: false,
                    ..Default::default()
                }
                .into(),
                Entry::Separator => MenuItem::Separator,
                Entry::Submenu { text, enabled, children } => SubMenu {
                    label: text.clone(),
                    enabled: *enabled,
                    submenu: items(children),
                    ..Default::default()
                }
                .into(),
            })
            .collect()
    }

    impl ksni::Tray for Cup {
        /// Left-click opens the menu, as it did with AppIndicator.
        const MENU_ON_ACTIVATE: bool = true;

        /// Fixed, so desktops remember whether the icon is shown or hidden.
        fn id(&self) -> String {
            "barista".into()
        }
        fn title(&self) -> String {
            self.title.clone()
        }
        fn icon_pixmap(&self) -> Vec<ksni::Icon> {
            vec![self.icon.clone()]
        }
        fn tool_tip(&self) -> ksni::ToolTip {
            ksni::ToolTip {
                title: self.title.clone(),
                description: self.tooltip.clone(),
                ..Default::default()
            }
        }
        fn menu(&self) -> Vec<MenuItem<Self>> {
            items(&self.entries)
        }
        fn menu_about_to_show(&mut self) {
            let _ = self.sender.send(UserEvent::Opened);
        }
    }

    pub struct Tray(Handle<Cup>);

    impl Tray {
        pub fn create(model: &Model, icons: &Icons, sender: Sender<UserEvent>) -> Option<Self> {
            let cup = Cup {
                entries: model.entries.clone(),
                tooltip: model.tooltip.clone(),
                title: title(model),
                icon: argb(icons, model.active),
                sender,
            };
            // The panel may start after Barista (login): wait for it rather than giving up.
            match cup.assume_sni_available(true).spawn() {
                Ok(handle) => Some(Tray(handle)),
                Err(error) => {
                    eprintln!("barista-app: no system tray available ({error}).");
                    None
                }
            }
        }

        pub fn show(&self, model: &Model, icons: &Icons) {
            let (entries, tooltip, title, icon) = (
                model.entries.clone(),
                model.tooltip.clone(),
                title(model),
                argb(icons, model.active),
            );
            self.0.update(move |cup| {
                cup.entries = entries;
                cup.tooltip = tooltip;
                cup.title = title;
                cup.icon = icon;
            });
        }
    }

    impl Drop for Tray {
        fn drop(&mut self) {
            self.0.shutdown().wait();
        }
    }
}
