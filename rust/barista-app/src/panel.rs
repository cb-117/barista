//! Windows: the panel that opens when you left-click the cup. A borderless popup above the
//! taskbar hosting WebView2 (built into Windows 11 and current Windows 10), showing
//! assets/panel.html. The page sends menu command ids (see `menu::handle`); Barista sends back a
//! JSON snapshot of its state (`menu::panel_state`).
//!
//! WebView2 is driven through its COM API (webview2-com) rather than wry, because wry's Linux
//! backend would put GTK and glib back into Cargo.lock. Everything here runs on the tray thread,
//! whose message pump in tray.rs also delivers WebView2's callbacks.
use crate::tray::UserEvent;
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::mpsc::Sender;
use std::time::{Duration, Instant};
use webview2_com::Microsoft::Web::WebView2::Win32::*;
use webview2_com::*;
use windows::core::{w, Interface, PCWSTR, PWSTR};
use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Dwm::{DwmSetWindowAttribute, DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND, DWM_WINDOW_CORNER_PREFERENCE};
use windows::Win32::Graphics::Gdi::{
    CreateSolidBrush, DeleteObject, GetMonitorInfoW, InvalidateRect, MonitorFromPoint, HBRUSH, HGDIOBJ, MONITORINFO,
    MONITOR_DEFAULTTONEAREST,
};
use windows::Win32::System::Com::{CoInitializeEx, COINIT_APARTMENTTHREADED};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
use windows::Win32::UI::WindowsAndMessaging::*;

const PAGE: &str = include_str!("../assets/panel.html");
/// Logical size; scaled by the monitor's DPI. The height follows the page's content (it reports it),
/// within these bounds.
const WIDTH: f64 = 340.0;
const HEIGHT: f64 = 560.0;
const MIN_HEIGHT: f64 = 240.0;
const MAX_HEIGHT: f64 = 660.0;
/// A tray click right after the panel hid itself (the click took focus away) means "close", not
/// "open again".
const REOPEN_GRACE: Duration = Duration::from_millis(400);

/// The tray icon's rectangle in physical pixels, when the click reported one.
#[derive(Clone, Copy, Debug)]
pub struct Anchor {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

thread_local! {
    /// The window procedure can't hold a reference to the panel, so what it needs lives here.
    static CONTROLLER: RefCell<Option<ICoreWebView2Controller>> = const { RefCell::new(None) };
    static HIDDEN_AT: Cell<Option<Instant>> = const { Cell::new(None) };
}

struct View {
    controller: ICoreWebView2Controller,
    webview: ICoreWebView2,
}

#[derive(Default)]
struct Shared {
    view: Option<View>,
    /// The page has loaded and listens for state.
    ready: bool,
    /// WebView2 couldn't start; the tray falls back to its full menu.
    failed: bool,
    creating: bool,
    /// Set just before loading the panel page: the one navigation the view may make.
    expect_navigation: bool,
    /// The panel page's URI, the only source whose messages count.
    page_source: Option<String>,
}

/// Commands the page may send: what its buttons do, and nothing else (`menu::handle` also accepts
/// ids the page has no reason to send, like opening files).
const PAGE_COMMANDS: [&str; 14] = [
    "quit",
    "dismiss-notice",
    "dismiss-update",
    "open-update",
    "stop-manual",
    "stop-all",
    "check-updates",
    "clear-history",
    "usage-refresh",
    "usage-report",
    "statusline-install",
    "panel:hide",
    "panel:ready",
    "start",
];
/// Commands with an argument: `start:900`, `cancel:<id>`, `toggle:display`, and so on.
const PAGE_COMMAND_PREFIXES: [&str; 11] = [
    "start:",
    "add:",
    "cancel:",
    "toggle:",
    "battery:",
    "connect:",
    "disconnect:",
    "copy:",
    "panel:height:",
    "usage-report:",
    "support:",
];

fn page_may_send(command: &str) -> bool {
    PAGE_COMMANDS.contains(&command) || PAGE_COMMAND_PREFIXES.iter().any(|prefix| command.starts_with(prefix))
}

pub struct Panel {
    hwnd: HWND,
    shared: Rc<RefCell<Shared>>,
    sender: Sender<UserEvent>,
    /// The content height the page last asked for (logical px), and where the panel last opened.
    height: Cell<f64>,
    anchor: Cell<Option<Anchor>>,
    /// The last state posted to the page, so an unchanged one isn't sent again.
    posted: RefCell<String>,
    /// A view ("usage") the page should switch to, sent with the next state once the page is ready.
    go: Cell<Option<&'static str>>,
}

/// Light or dark, from the Windows app theme, so the window's first frame matches the page.
fn dark_theme() -> bool {
    use winreg::enums::HKEY_CURRENT_USER;
    winreg::RegKey::predef(HKEY_CURRENT_USER)
        .open_subkey(r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize")
        .and_then(|key| key.get_value::<u32, _>("AppsUseLightTheme"))
        .map(|light| light == 0)
        .unwrap_or(false)
}

fn brush(color: COREWEBVIEW2_COLOR) -> HBRUSH {
    unsafe { CreateSolidBrush(COLORREF(u32::from(color.R) | u32::from(color.G) << 8 | u32::from(color.B) << 16)) }
}

/// Windows switched between light and dark: repaint the window and WebView2's background (the page
/// follows by itself through prefers-color-scheme).
fn apply_theme(hwnd: HWND) {
    let color = background();
    unsafe {
        let old = SetClassLongPtrW(hwnd, GCLP_HBRBACKGROUND, brush(color).0 as isize);
        if old != 0 {
            let _ = DeleteObject(HGDIOBJ(old as *mut std::ffi::c_void));
        }
        CONTROLLER.with(|controller| {
            if let Some(controller2) = controller.borrow().as_ref().and_then(|c| c.cast::<ICoreWebView2Controller2>().ok()) {
                let _ = controller2.SetDefaultBackgroundColor(color);
            }
        });
        let _ = InvalidateRect(Some(hwnd), None, true);
    }
}

/// WM_SETTINGCHANGE's lParam names what changed; "ImmersiveColorSet" is the app theme.
fn is_theme_change(lparam: LPARAM) -> bool {
    if lparam.0 == 0 {
        return false;
    }
    let name = unsafe { PCWSTR(lparam.0 as *const u16).to_string() };
    name.is_ok_and(|name| name == "ImmersiveColorSet")
}

fn background() -> COREWEBVIEW2_COLOR {
    if dark_theme() {
        COREWEBVIEW2_COLOR {
            A: 255,
            R: 32,
            G: 32,
            B: 32,
        }
    } else {
        COREWEBVIEW2_COLOR {
            A: 255,
            R: 249,
            G: 249,
            B: 249,
        }
    }
}

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain([0]).collect()
}

fn take_string(value: PWSTR) -> String {
    CoTaskMemPWSTR::from(value).to_string()
}

extern "system" fn window_proc(hwnd: HWND, message: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    unsafe {
        match message {
            // Clicking anywhere else closes the panel, like a flyout.
            WM_ACTIVATE if (wparam.0 & 0xFFFF) as u32 == WA_INACTIVE => {
                let _ = ShowWindow(hwnd, SW_HIDE);
                HIDDEN_AT.with(|at| at.set(Some(Instant::now())));
                CONTROLLER.with(|controller| {
                    if let Some(controller) = controller.borrow().as_ref() {
                        let _ = controller.SetIsVisible(false);
                    }
                });
                LRESULT(0)
            }
            WM_SIZE => {
                CONTROLLER.with(|controller| {
                    if let Some(controller) = controller.borrow().as_ref() {
                        let mut rect = RECT::default();
                        let _ = GetClientRect(hwnd, &mut rect);
                        let _ = controller.SetBounds(rect);
                    }
                });
                LRESULT(0)
            }
            WM_CLOSE => {
                let _ = ShowWindow(hwnd, SW_HIDE);
                LRESULT(0)
            }
            WM_SETTINGCHANGE => {
                if is_theme_change(lparam) {
                    apply_theme(hwnd);
                }
                DefWindowProcW(hwnd, message, wparam, lparam)
            }
            _ => DefWindowProcW(hwnd, message, wparam, lparam),
        }
    }
}

impl Panel {
    /// Creates the (hidden) window. WebView2 itself starts the first time the panel opens.
    pub fn new(sender: Sender<UserEvent>) -> Option<Panel> {
        unsafe {
            // WebView2 needs a single-threaded COM apartment on the thread that owns its windows.
            let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
            let instance = GetModuleHandleW(None).ok()?;
            let class = WNDCLASSW {
                lpfnWndProc: Some(window_proc),
                hInstance: instance.into(),
                lpszClassName: w!("BaristaPanel"),
                hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
                hbrBackground: brush(background()),
                ..Default::default()
            };
            RegisterClassW(&class);
            let hwnd = CreateWindowExW(
                WS_EX_TOOLWINDOW | WS_EX_TOPMOST,
                w!("BaristaPanel"),
                w!("Barista"),
                WS_POPUP,
                0,
                0,
                WIDTH as i32,
                HEIGHT as i32,
                None,
                None,
                Some(instance.into()),
                None,
            )
            .ok()?;
            // Rounded corners on Windows 11 (ignored on Windows 10).
            let corners: DWM_WINDOW_CORNER_PREFERENCE = DWMWCP_ROUND;
            let _ = DwmSetWindowAttribute(
                hwnd,
                DWMWA_WINDOW_CORNER_PREFERENCE,
                &corners as *const _ as *const std::ffi::c_void,
                std::mem::size_of::<DWM_WINDOW_CORNER_PREFERENCE>() as u32,
            );
            Some(Panel {
                hwnd,
                shared: Rc::new(RefCell::new(Shared::default())),
                sender,
                height: Cell::new(HEIGHT),
                anchor: Cell::new(None),
                posted: RefCell::new(String::new()),
                go: Cell::new(None),
            })
        }
    }

    /// WebView2 couldn't start (for example, the runtime isn't installed).
    pub fn failed(&self) -> bool {
        self.shared.borrow().failed
    }

    pub fn visible(&self) -> bool {
        unsafe { IsWindowVisible(self.hwnd).as_bool() }
    }

    /// Opens the panel, or closes it if it's open (or just closed itself because this click took
    /// focus away from it).
    pub fn toggle(&self, anchor: Option<Anchor>) {
        let just_hidden = HIDDEN_AT.with(|at| at.get().is_some_and(|when| when.elapsed() < REOPEN_GRACE));
        if self.visible() {
            self.hide();
        } else if !just_hidden {
            self.show(anchor);
        }
    }

    pub fn hide(&self) {
        unsafe {
            let _ = ShowWindow(self.hwnd, SW_HIDE);
        }
        if let Some(view) = self.shared.borrow().view.as_ref() {
            unsafe {
                let _ = view.controller.SetIsVisible(false);
            }
        }
    }

    /// The page reports its content height; resize to fit, keeping the panel against the taskbar.
    pub fn set_height(&self, logical: f64) {
        let height = logical.clamp(MIN_HEIGHT, MAX_HEIGHT);
        if (height - self.height.get()).abs() < 1.0 {
            return;
        }
        self.height.set(height);
        if self.visible() {
            self.place(self.anchor.get());
        }
    }

    pub fn show(&self, anchor: Option<Anchor>) {
        if self.failed() {
            return;
        }
        if anchor.is_some() {
            self.anchor.set(anchor);
        }
        self.place(self.anchor.get());
        unsafe {
            let _ = ShowWindow(self.hwnd, SW_SHOW);
            let _ = SetForegroundWindow(self.hwnd);
        }
        let shared = self.shared.borrow();
        if let Some(view) = shared.view.as_ref() {
            unsafe {
                let mut rect = RECT::default();
                let _ = GetClientRect(self.hwnd, &mut rect);
                let _ = view.controller.SetBounds(rect);
                let _ = view.controller.SetIsVisible(true);
                let _ = view.controller.MoveFocus(COREWEBVIEW2_MOVE_FOCUS_REASON_PROGRAMMATIC);
            }
        } else if !shared.creating {
            drop(shared);
            self.create_view();
        }
    }

    /// Asks the page to show one of its views ("main", "usage", "settings") when it next gets state.
    pub fn open_view(&self, view: &'static str) {
        self.go.set(Some(view));
    }

    /// Sends the page Barista's state, if it changed since the last one sent.
    pub fn post(&self, state: &serde_json::Value) {
        let shared = self.shared.borrow();
        let (Some(view), true) = (shared.view.as_ref(), shared.ready) else {
            return;
        };
        if let Some(go) = self.go.take() {
            let message = wide(&serde_json::json!({ "go": go }).to_string());
            unsafe {
                let _ = view.webview.PostWebMessageAsJson(PCWSTR(message.as_ptr()));
            }
        }
        let json = state.to_string();
        if *self.posted.borrow() == json {
            return;
        }
        let message = wide(&json);
        unsafe {
            if view.webview.PostWebMessageAsJson(PCWSTR(message.as_ptr())).is_ok() {
                *self.posted.borrow_mut() = json;
            }
        }
    }

    /// The page (re)loaded and has nothing yet: the next post always goes through.
    pub fn page_ready(&self) {
        self.posted.borrow_mut().clear();
    }

    /// WebView2's browser or page process ended (crashed, killed, or updated): drop this view so
    /// the next open starts a new one.
    pub fn reset(&self) {
        self.hide();
        let view = {
            let mut shared = self.shared.borrow_mut();
            shared.ready = false;
            shared.creating = false;
            shared.expect_navigation = false;
            shared.page_source = None;
            shared.view.take()
        };
        CONTROLLER.with(|slot| *slot.borrow_mut() = None);
        self.posted.borrow_mut().clear();
        if let Some(view) = view {
            unsafe {
                let _ = view.controller.Close();
            }
        }
    }

    /// Above the tray icon, inside the monitor's work area (wherever the taskbar is).
    fn place(&self, anchor: Option<Anchor>) {
        unsafe {
            let point = match anchor {
                Some(a) => POINT {
                    x: a.x + a.width / 2,
                    y: a.y + a.height / 2,
                },
                None => {
                    let mut cursor = POINT::default();
                    let _ = GetCursorPos(&mut cursor);
                    cursor
                }
            };
            let monitor = MonitorFromPoint(point, MONITOR_DEFAULTTONEAREST);
            let mut info = MONITORINFO {
                cbSize: std::mem::size_of::<MONITORINFO>() as u32,
                ..Default::default()
            };
            let _ = GetMonitorInfoW(monitor, &mut info);
            let (mut dpi_x, mut dpi_y) = (96u32, 96u32);
            let _ = GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &mut dpi_x, &mut dpi_y);
            let scale = f64::from(dpi_x.max(96)) / 96.0;
            let gap = (12.0 * scale).round() as i32;
            let work = info.rcWork;
            // Never larger than the work area (a small screen, or a large scale factor).
            let width = ((WIDTH * scale).round() as i32).min(work.right - work.left - 2 * gap).max(1);
            let height = ((self.height.get() * scale).round() as i32)
                .min(work.bottom - work.top - 2 * gap)
                .max(1);
            let (x, y) = position(anchor, point, work, info.rcMonitor, width, height, gap);
            let _ = SetWindowPos(self.hwnd, Some(HWND_TOPMOST), x, y, width, height, SWP_NOACTIVATE);
        }
        // WebView2 places its popups (like a select's list) from this.
        CONTROLLER.with(|controller| {
            if let Some(controller) = controller.borrow().as_ref() {
                unsafe {
                    let _ = controller.NotifyParentWindowPositionChanged();
                }
            }
        });
    }

    /// Starts WebView2 without blocking: its environment and controller arrive as callbacks
    /// through the tray thread's message pump.
    fn create_view(&self) {
        self.shared.borrow_mut().creating = true;
        let folder = dirs::data_local_dir()
            .unwrap_or_else(std::env::temp_dir)
            .join("Barista")
            .join("WebView2");
        let _ = std::fs::create_dir_all(&folder);
        let folder = wide(&folder.to_string_lossy());
        let hwnd = self.hwnd;
        let shared = self.shared.clone();
        let sender = self.sender.clone();
        let on_environment = CreateCoreWebView2EnvironmentCompletedHandler::create(Box::new(move |result, environment| {
            let environment = match (result, environment) {
                (Ok(()), Some(environment)) => environment,
                (Err(error), _) => return fail(&shared, &sender, &format!("{error}")),
                (Ok(()), None) => return fail(&shared, &sender, "no environment"),
            };
            let controller_shared = shared.clone();
            let controller_sender = sender.clone();
            let on_controller =
                CreateCoreWebView2ControllerCompletedHandler::create(Box::new(move |result, controller| match (result, controller) {
                    (Ok(()), Some(controller)) => {
                        if let Err(error) = configure(hwnd, controller, &controller_shared, &controller_sender) {
                            return fail(&controller_shared, &controller_sender, &format!("{error}"));
                        }
                        Ok(())
                    }
                    (Err(error), _) => fail(&controller_shared, &controller_sender, &format!("{error}")),
                    (Ok(()), None) => fail(&controller_shared, &controller_sender, "no controller"),
                }));
            unsafe {
                if let Err(error) = environment.CreateCoreWebView2Controller(hwnd, &on_controller) {
                    return fail(&shared, &sender, &format!("{error}"));
                }
            }
            Ok(())
        }));
        let started = unsafe {
            CreateCoreWebView2EnvironmentWithOptions(
                PCWSTR::null(),
                PCWSTR(folder.as_ptr()),
                None::<&ICoreWebView2EnvironmentOptions>,
                &on_environment,
            )
        };
        if let Err(error) = started {
            let _ = fail(&self.shared, &self.sender, &format!("{error}"));
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Edge {
    Bottom,
    Top,
    Left,
    Right,
}

/// The taskbar's edge. It gives up that edge of the work area; when it hides itself (the work area
/// is the whole monitor), the edge nearest the icon is the one it slides out from.
fn taskbar_edge(point: POINT, work: RECT, screen: RECT) -> Edge {
    if work.bottom < screen.bottom {
        Edge::Bottom
    } else if work.top > screen.top {
        Edge::Top
    } else if work.left > screen.left {
        Edge::Left
    } else if work.right < screen.right {
        Edge::Right
    } else {
        [
            (screen.bottom - point.y, Edge::Bottom),
            (point.y - screen.top, Edge::Top),
            (point.x - screen.left, Edge::Left),
            (screen.right - point.x, Edge::Right),
        ]
        .into_iter()
        .min_by_key(|(distance, _)| *distance)
        .map_or(Edge::Bottom, |(_, edge)| edge)
    }
}

/// The panel's top-left corner: beside the icon, off the taskbar, inside the work area. `width` and
/// `height` already fit the work area.
fn position(anchor: Option<Anchor>, point: POINT, work: RECT, screen: RECT, width: i32, height: i32, gap: i32) -> (i32, i32) {
    // The icon's rectangle, or just the point (the cursor) when the click didn't report one.
    let icon = anchor.map_or(
        RECT {
            left: point.x,
            top: point.y,
            right: point.x,
            bottom: point.y,
        },
        |a| RECT {
            left: a.x,
            top: a.y,
            right: a.x + a.width,
            bottom: a.y + a.height,
        },
    );
    let (x, y) = match taskbar_edge(point, work, screen) {
        Edge::Bottom => (point.x - width / 2, icon.top.min(work.bottom) - height - gap),
        Edge::Top => (point.x - width / 2, icon.bottom.max(work.top) + gap),
        Edge::Left => (icon.right.max(work.left) + gap, point.y - height / 2),
        Edge::Right => (icon.left.min(work.right) - width - gap, point.y - height / 2),
    };
    (
        x.clamp(work.left + gap, (work.right - width - gap).max(work.left + gap)),
        y.clamp(work.top + gap, (work.bottom - height - gap).max(work.top + gap)),
    )
}

/// Whether a message came from the panel page. The page is loaded with NavigateToString:
/// NavigationStarting reports it as a `data:` URI, but its messages report `about:blank` as their
/// source, so the two never match. Every other navigation is cancelled, so the only page that can be
/// showing is the panel; this still refuses messages before it loaded and from any web or file page.
pub(crate) fn from_panel_page(loaded: bool, source: &str) -> bool {
    loaded && (source == "about:blank" || source.starts_with("data:"))
}

/// WebView2 is unavailable: say so once, hide, and let the tray fall back to its full menu.
fn fail(shared: &Rc<RefCell<Shared>>, sender: &Sender<UserEvent>, reason: &str) -> windows::core::Result<()> {
    let mut state = shared.borrow_mut();
    if !state.failed {
        eprintln!("barista-app: WebView2 isn't available ({reason}); using the tray menu instead.");
        state.failed = true;
        state.creating = false;
        let _ = sender.send(UserEvent::Menu("panel:failed".into()));
    }
    Ok(())
}

/// Locks the page down, wires up messages, and loads the panel.
fn configure(
    hwnd: HWND,
    controller: ICoreWebView2Controller,
    shared: &Rc<RefCell<Shared>>,
    sender: &Sender<UserEvent>,
) -> windows::core::Result<()> {
    unsafe {
        if let Ok(controller2) = controller.cast::<ICoreWebView2Controller2>() {
            let _ = controller2.SetDefaultBackgroundColor(background());
        }
        let webview = controller.CoreWebView2()?;
        let settings = webview.Settings()?;
        settings.SetAreDevToolsEnabled(cfg!(debug_assertions))?;
        settings.SetAreDefaultContextMenusEnabled(false)?;
        settings.SetIsStatusBarEnabled(false)?;
        settings.SetIsZoomControlEnabled(false)?;
        if let Ok(settings3) = settings.cast::<ICoreWebView2Settings3>() {
            let _ = settings3.SetAreBrowserAcceleratorKeysEnabled(false);
        }

        let mut token = 0i64;
        // The panel is one local page: allow loading it, once, and cancel every other navigation
        // (links, reloads, anything a page script tries). No new windows either.
        let navigation_shared = shared.clone();
        webview.add_NavigationStarting(
            &NavigationStartingEventHandler::create(Box::new(move |_, args| {
                let Some(args) = args else { return Ok(()) };
                let mut shared = navigation_shared.borrow_mut();
                if std::mem::take(&mut shared.expect_navigation) {
                    let mut uri = PWSTR::null();
                    args.Uri(&mut uri)?;
                    shared.page_source = Some(take_string(uri));
                } else {
                    args.SetCancel(true)?;
                }
                Ok(())
            })),
            &mut token,
        )?;
        webview.add_NewWindowRequested(
            &NewWindowRequestedEventHandler::create(Box::new(|_, args| {
                if let Some(args) = args {
                    args.SetHandled(true)?;
                }
                Ok(())
            })),
            &mut token,
        )?;
        // The page sends {"cmd": "<menu command id>"}. Only the panel page's messages count, and
        // only commands its buttons send.
        let message_shared = shared.clone();
        let message_sender = sender.clone();
        webview.add_WebMessageReceived(
            &WebMessageReceivedEventHandler::create(Box::new(move |_, args| {
                let Some(args) = args else { return Ok(()) };
                let mut source = PWSTR::null();
                args.Source(&mut source)?;
                let source = take_string(source);
                if !from_panel_page(message_shared.borrow().page_source.is_some(), &source) {
                    return Ok(());
                }
                let mut json = PWSTR::null();
                args.WebMessageAsJson(&mut json)?;
                let json = take_string(json);
                let command = serde_json::from_str::<serde_json::Value>(&json)
                    .ok()
                    .and_then(|value| value.get("cmd").and_then(|cmd| cmd.as_str()).map(String::from));
                if let Some(command) = command.filter(|cmd| cmd.len() <= 200 && page_may_send(cmd)) {
                    if command == "panel:ready" {
                        message_shared.borrow_mut().ready = true;
                    }
                    let _ = message_sender.send(UserEvent::Menu(command));
                }
                Ok(())
            })),
            &mut token,
        )?;

        // WebView2's processes can end (a crash, Task Manager, a runtime update): start over on the
        // next open.
        let failed_sender = sender.clone();
        webview.add_ProcessFailed(
            &ProcessFailedEventHandler::create(Box::new(move |_, _| {
                let _ = failed_sender.send(UserEvent::Menu("panel:process-failed".into()));
                Ok(())
            })),
            &mut token,
        )?;

        let mut rect = RECT::default();
        let _ = GetClientRect(hwnd, &mut rect);
        controller.SetBounds(rect)?;
        let visible = IsWindowVisible(hwnd).as_bool();
        controller.SetIsVisible(visible)?;
        if visible {
            let _ = controller.MoveFocus(COREWEBVIEW2_MOVE_FOCUS_REASON_PROGRAMMATIC);
        }
        let page = wide(PAGE);
        shared.borrow_mut().expect_navigation = true;
        webview.NavigateToString(PCWSTR(page.as_ptr()))?;

        CONTROLLER.with(|slot| *slot.borrow_mut() = Some(controller.clone()));
        let mut state = shared.borrow_mut();
        state.view = Some(View { controller, webview });
        state.creating = false;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const SCREEN: RECT = RECT {
        left: 0,
        top: 0,
        right: 1920,
        bottom: 1080,
    };

    fn icon(x: i32, y: i32) -> Option<Anchor> {
        Some(Anchor {
            x,
            y,
            width: 24,
            height: 24,
        })
    }

    #[test]
    fn sits_above_a_bottom_taskbar() {
        let work = RECT { bottom: 1032, ..SCREEN };
        let (x, y) = position(icon(1800, 1044), POINT { x: 1812, y: 1056 }, work, SCREEN, 340, 560, 12);
        assert_eq!((x, y), (1920 - 340 - 12, 1032 - 560 - 12));
    }

    #[test]
    fn auto_hide_taskbar_uses_the_nearest_edge() {
        // Work area = whole monitor; the icon is at the left edge, halfway down.
        let point = POINT { x: 20, y: 540 };
        assert_eq!(taskbar_edge(point, SCREEN, SCREEN), Edge::Left);
        let (x, _) = position(icon(8, 528), point, SCREEN, SCREEN, 340, 560, 12);
        assert_eq!(x, 8 + 24 + 12);
        assert_eq!(taskbar_edge(POINT { x: 1812, y: 1070 }, SCREEN, SCREEN), Edge::Bottom);
    }

    #[test]
    fn stays_inside_the_work_area() {
        let work = RECT { bottom: 1032, ..SCREEN };
        let (x, y) = position(icon(0, 1044), POINT { x: 12, y: 1056 }, work, SCREEN, 340, 1008, 12);
        assert_eq!((x, y), (12, 12));
    }

    #[test]
    fn messages_count_only_from_the_loaded_panel_page() {
        assert!(from_panel_page(true, "about:blank"));
        assert!(from_panel_page(true, "data:text/html;charset=utf-8;base64,PCFET0NUWVBF"));
        assert!(!from_panel_page(false, "about:blank"));
        assert!(!from_panel_page(true, "https://example.com/"));
        assert!(!from_panel_page(true, "file:///C:/page.html"));
    }

    #[test]
    fn the_page_may_send_only_its_own_commands() {
        assert!(page_may_send("start:900"));
        assert!(page_may_send("toggle:display"));
        assert!(page_may_send("panel:height:420"));
        assert!(page_may_send("usage-report:week"));
        assert!(!page_may_send("panel:failed"));
        assert!(!page_may_send("panel:usage"));
        assert!(!page_may_send("panel:process-failed"));
        assert!(!page_may_send("open-log"));
    }
}
