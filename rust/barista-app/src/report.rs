//! Windows: the usage window (Projects and History). A normal, resizable window hosting WebView2
//! with the shared report page (Sources/BaristaCore/ReportTemplate.swift, the same page
//! `barista report` writes). Barista sends the page live data; the page asks Barista to copy text,
//! save a CSV, web page, or PDF, or show a project's folder (docs/USAGE.md, "Visual report").
//!
//! Like the panel, everything runs on the tray thread, whose message pump delivers WebView2's
//! callbacks. The page loads once from a string; every other navigation is cancelled.
use crate::tray::UserEvent;
use crate::{lock, Shared as AppShared};
use barista_core::usage::Period;
use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::mpsc::Sender;
use webview2_com::Microsoft::Web::WebView2::Win32::*;
use webview2_com::*;
use windows::core::{w, Interface, BOOL, PCWSTR, PWSTR};
use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Dwm::{DwmSetWindowAttribute, DWMWA_USE_IMMERSIVE_DARK_MODE};
use windows::Win32::Graphics::Gdi::{CreateSolidBrush, GetMonitorInfoW, MonitorFromPoint, HBRUSH, MONITORINFO, MONITOR_DEFAULTTONEAREST};
use windows::Win32::System::Com::{CoCreateInstance, CLSCTX_INPROC_SERVER};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::HiDpi::{GetDpiForMonitor, GetDpiForWindow, MDT_EFFECTIVE_DPI};
use windows::Win32::UI::Shell::Common::COMDLG_FILTERSPEC;
use windows::Win32::UI::Shell::{FileSaveDialog, IFileSaveDialog, SIGDN_FILESYSPATH};
use windows::Win32::UI::WindowsAndMessaging::*;

/// Logical size when it first opens, and the smallest it can be made.
const WIDTH: f64 = 1000.0;
const HEIGHT: f64 = 780.0;
const MIN_WIDTH: f64 = 560.0;
const MIN_HEIGHT: f64 = 420.0;

thread_local! {
    static CONTROLLER: RefCell<Option<ICoreWebView2Controller>> = const { RefCell::new(None) };
}

struct View {
    controller: ICoreWebView2Controller,
    webview: ICoreWebView2,
    environment: ICoreWebView2Environment,
}

#[derive(Default)]
struct State {
    view: Option<View>,
    ready: bool,
    creating: bool,
    failed: bool,
    expect_navigation: bool,
    loaded: bool,
}

pub struct ReportWindow {
    hwnd: HWND,
    state: Rc<RefCell<State>>,
    sender: Sender<UserEvent>,
    /// The page's view ("projects" or "history") and period, so a reopen shows the same.
    view: RefCell<String>,
    period: Cell<Period>,
    /// When the data last sent was computed, so unchanged data isn't serialized again.
    sent: Cell<Option<std::time::Instant>>,
}

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain([0]).collect()
}

fn take_string(value: PWSTR) -> String {
    CoTaskMemPWSTR::from(value).to_string()
}

fn dark_theme() -> bool {
    use winreg::enums::HKEY_CURRENT_USER;
    winreg::RegKey::predef(HKEY_CURRENT_USER)
        .open_subkey(r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize")
        .and_then(|key| key.get_value::<u32, _>("AppsUseLightTheme"))
        .map(|light| light == 0)
        .unwrap_or(false)
}

fn background() -> COREWEBVIEW2_COLOR {
    let (r, g, b) = if dark_theme() { (32, 32, 32) } else { (249, 249, 249) };
    COREWEBVIEW2_COLOR { A: 255, R: r, G: g, B: b }
}

fn brush(color: COREWEBVIEW2_COLOR) -> HBRUSH {
    unsafe { CreateSolidBrush(COLORREF(u32::from(color.R) | u32::from(color.G) << 8 | u32::from(color.B) << 16)) }
}

/// A dark title bar when Windows uses dark mode for apps (Windows 10 2004 and later; ignored before).
fn apply_title_bar(hwnd: HWND) {
    let dark = BOOL::from(dark_theme());
    unsafe {
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_USE_IMMERSIVE_DARK_MODE,
            &dark as *const _ as *const std::ffi::c_void,
            std::mem::size_of::<BOOL>() as u32,
        );
    }
    CONTROLLER.with(|controller| {
        if let Some(controller2) = controller.borrow().as_ref().and_then(|c| c.cast::<ICoreWebView2Controller2>().ok()) {
            unsafe {
                let _ = controller2.SetDefaultBackgroundColor(background());
            }
        }
    });
}

extern "system" fn window_proc(hwnd: HWND, message: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    unsafe {
        match message {
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
            WM_MOVE | WM_MOVING => {
                CONTROLLER.with(|controller| {
                    if let Some(controller) = controller.borrow().as_ref() {
                        let _ = controller.NotifyParentWindowPositionChanged();
                    }
                });
                DefWindowProcW(hwnd, message, wparam, lparam)
            }
            WM_GETMINMAXINFO => {
                let scale = f64::from(GetDpiForWindow(hwnd).max(96)) / 96.0;
                let info = &mut *(lparam.0 as *mut MINMAXINFO);
                info.ptMinTrackSize = POINT {
                    x: (MIN_WIDTH * scale) as i32,
                    y: (MIN_HEIGHT * scale) as i32,
                };
                LRESULT(0)
            }
            WM_DPICHANGED => {
                // Windows suggests the new size for the new monitor's scale.
                let rect = &*(lparam.0 as *const RECT);
                let _ = SetWindowPos(
                    hwnd,
                    None,
                    rect.left,
                    rect.top,
                    rect.right - rect.left,
                    rect.bottom - rect.top,
                    SWP_NOZORDER | SWP_NOACTIVATE,
                );
                LRESULT(0)
            }
            WM_SETFOCUS => {
                CONTROLLER.with(|controller| {
                    if let Some(controller) = controller.borrow().as_ref() {
                        let _ = controller.MoveFocus(COREWEBVIEW2_MOVE_FOCUS_REASON_PROGRAMMATIC);
                    }
                });
                LRESULT(0)
            }
            WM_CLOSE => {
                // Hidden, not destroyed: it reopens instantly, where it was.
                let _ = ShowWindow(hwnd, SW_HIDE);
                LRESULT(0)
            }
            WM_SETTINGCHANGE => {
                apply_title_bar(hwnd);
                DefWindowProcW(hwnd, message, wparam, lparam)
            }
            _ => DefWindowProcW(hwnd, message, wparam, lparam),
        }
    }
}

impl ReportWindow {
    /// Creates the (hidden) window. WebView2 starts the first time it's shown.
    pub fn new(sender: Sender<UserEvent>) -> Option<ReportWindow> {
        unsafe {
            let instance = GetModuleHandleW(None).ok()?;
            // Barista's cup, from the icon resource embedded in barista-app.exe (build.rs).
            let icon = LoadIconW(
                Some(instance.into()),
                PCWSTR(std::ptr::without_provenance::<u16>(1)), /* MAKEINTRESOURCE(1) */
            )
            .unwrap_or_default();
            let class = WNDCLASSW {
                lpfnWndProc: Some(window_proc),
                hInstance: instance.into(),
                lpszClassName: w!("BaristaUsage"),
                hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
                hbrBackground: brush(background()),
                hIcon: icon,
                ..Default::default()
            };
            RegisterClassW(&class);
            let hwnd = CreateWindowExW(
                WINDOW_EX_STYLE::default(),
                w!("BaristaUsage"),
                w!("Barista · Usage"),
                WS_OVERLAPPEDWINDOW,
                CW_USEDEFAULT,
                CW_USEDEFAULT,
                WIDTH as i32,
                HEIGHT as i32,
                None,
                None,
                Some(instance.into()),
                None,
            )
            .ok()?;
            apply_title_bar(hwnd);
            Some(ReportWindow {
                hwnd,
                state: Rc::new(RefCell::new(State::default())),
                sender,
                view: RefCell::new("projects".into()),
                period: Cell::new(Period::Week),
                sent: Cell::new(None),
            })
        }
    }

    pub fn visible(&self) -> bool {
        unsafe { IsWindowVisible(self.hwnd).as_bool() }
    }

    /// WebView2 couldn't start: callers open the report in the browser instead.
    pub fn failed(&self) -> bool {
        self.state.borrow().failed
    }

    /// Opens (or brings forward) the window on `view` ("projects" or "history") and, if given, a period.
    pub fn show(&self, view: Option<&str>, period: Option<Period>) {
        if let Some(view) = view.filter(|v| matches!(*v, "projects" | "history")) {
            *self.view.borrow_mut() = view.to_string();
        }
        if let Some(period) = period {
            self.period.set(period);
        }
        unsafe {
            if !self.visible() {
                self.center_first_time();
                let _ = ShowWindow(self.hwnd, SW_SHOW);
            }
            if IsIconic(self.hwnd).as_bool() {
                let _ = ShowWindow(self.hwnd, SW_RESTORE);
            }
            let _ = SetForegroundWindow(self.hwnd);
        }
        let (has_view, creating) = {
            let state = self.state.borrow();
            (state.view.is_some(), state.creating)
        };
        if has_view {
            if let Some(view) = self.state.borrow().view.as_ref() {
                unsafe {
                    let mut rect = RECT::default();
                    let _ = GetClientRect(self.hwnd, &mut rect);
                    let _ = view.controller.SetBounds(rect);
                    let _ = view.controller.SetIsVisible(true);
                }
            }
            self.post_settings();
        } else if !creating {
            self.create_view();
        }
    }

    /// The first time it opens: centered in the work area of the monitor under the cursor, sized for it.
    fn center_first_time(&self) {
        thread_local!(static PLACED: Cell<bool> = const { Cell::new(false) });
        if PLACED.with(|placed| placed.replace(true)) {
            return;
        }
        unsafe {
            let mut cursor = POINT::default();
            let _ = GetCursorPos(&mut cursor);
            let monitor = MonitorFromPoint(cursor, MONITOR_DEFAULTTONEAREST);
            let mut info = MONITORINFO {
                cbSize: std::mem::size_of::<MONITORINFO>() as u32,
                ..Default::default()
            };
            let _ = GetMonitorInfoW(monitor, &mut info);
            let (mut dpi_x, mut dpi_y) = (96u32, 96u32);
            let _ = GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &mut dpi_x, &mut dpi_y);
            let scale = f64::from(dpi_x.max(96)) / 96.0;
            let work = info.rcWork;
            let width = ((WIDTH * scale) as i32).min(work.right - work.left - 40);
            let height = ((HEIGHT * scale) as i32).min(work.bottom - work.top - 40);
            let x = work.left + (work.right - work.left - width) / 2;
            let y = work.top + (work.bottom - work.top - height) / 2;
            let _ = SetWindowPos(self.hwnd, None, x, y, width, height, SWP_NOZORDER | SWP_NOACTIVATE);
        }
    }

    fn post(&self, message: &serde_json::Value) {
        let state = self.state.borrow();
        let (Some(view), true) = (state.view.as_ref(), state.ready) else {
            return;
        };
        let text = wide(&message.to_string());
        unsafe {
            let _ = view.webview.PostWebMessageAsJson(PCWSTR(text.as_ptr()));
        }
    }

    fn post_settings(&self) {
        self.post(&serde_json::json!({"type": "view", "id": *self.view.borrow()}));
        self.post(&serde_json::json!({"type": "period", "id": self.period.get().id()}));
    }

    /// Sends the latest usage if it's newer than what the page has. Call it from the event loop.
    pub fn update(&self, app: &AppShared) {
        if !self.visible() || !self.state.borrow().ready {
            return;
        }
        let message = {
            let state = lock(app);
            let (Some(usage), Some(updated)) = (state.usage.as_ref(), state.usage_updated) else {
                return;
            };
            if self.sent.get() == Some(updated) {
                return;
            }
            self.sent.set(Some(updated));
            serde_json::json!({"type": "data", "report": usage, "period": self.period.get().id()})
        };
        self.post(&message);
    }

    /// A message from the page (already checked to come from it): see the page's `post` calls.
    pub fn handle(&self, app: &AppShared, message: serde_json::Value) {
        match message["cmd"].as_str().unwrap_or("") {
            "ready" => {
                self.state.borrow_mut().ready = true;
                self.sent.set(None);
                self.post_settings();
                self.update(app);
            }
            "period" => {
                if let Some(period) = message["id"].as_str().and_then(Period::parse) {
                    self.period.set(period);
                }
            }
            "view" => {
                if let Some(view) = message["id"].as_str().filter(|v| matches!(*v, "projects" | "history")) {
                    *self.view.borrow_mut() = view.to_string();
                }
            }
            "copy" => {
                let text = message["text"].as_str().unwrap_or("");
                if !text.is_empty() && text.len() <= 10_000 && crate::menu::copy(text).is_ok() {
                    self.post(&serde_json::json!({"type": "copied"}));
                }
            }
            "open-folder" => {
                // Only a folder the report itself lists, and only if it's still there.
                let Some(path) = message["path"].as_str() else { return };
                let listed = lock(app).usage.as_ref().is_some_and(|usage| {
                    usage["periods"].as_array().into_iter().flatten().any(|period| {
                        period["projects"]
                            .as_array()
                            .into_iter()
                            .flatten()
                            .any(|project| project["project"].as_str() == Some(path))
                    })
                });
                if listed && std::path::Path::new(path).is_dir() {
                    let _ = std::process::Command::new("explorer.exe").arg(path).spawn();
                }
            }
            "export" => self.export(app, &message),
            _ => {}
        }
    }

    fn export(&self, app: &AppShared, message: &serde_json::Value) {
        let period = message["period"].as_str().and_then(Period::parse).unwrap_or(self.period.get());
        let stem = format!(
            "Barista usage {} {}",
            period.label().to_lowercase(),
            chrono::Local::now().format("%Y-%m-%d")
        );
        match message["format"].as_str().unwrap_or("") {
            "csv" => {
                let Some(text) = message["text"].as_str().filter(|text| text.len() <= 20_000_000) else {
                    return;
                };
                let name = message["filename"]
                    .as_str()
                    .filter(|name| name.ends_with(".csv") && !name.contains(['/', '\\', ':']))
                    .map(String::from)
                    .unwrap_or_else(|| format!("{stem}.csv"));
                if let Some(path) = save_dialog(self.hwnd, &name, "CSV (comma-separated values)", "*.csv", "csv") {
                    self.saved(std::fs::write(&path, text));
                }
            }
            "html" => {
                let page = lock(app)
                    .usage
                    .as_ref()
                    .map(|usage| barista_core::usage::report_page(usage, period));
                let Some(page) = page else { return };
                if let Some(path) = save_dialog(self.hwnd, &format!("{stem}.html"), "Web page", "*.html", "html") {
                    self.saved(std::fs::write(&path, page));
                }
            }
            "pdf" => {
                if let Some(path) = save_dialog(self.hwnd, &format!("{stem}.pdf"), "PDF", "*.pdf", "pdf") {
                    self.print_pdf(path);
                }
            }
            "browser" => crate::open_usage_report(app, period),
            _ => {}
        }
    }

    fn saved(&self, result: std::io::Result<()>) {
        let text = match result {
            Ok(()) => "Saved".to_string(),
            Err(error) => format!("Couldn’t save: {error}"),
        };
        self.post(&serde_json::json!({"type": "saved", "text": text}));
    }

    /// The whole report (both views, light colors) as a PDF, through WebView2's own printing.
    fn print_pdf(&self, path: PathBuf) {
        let (webview, environment) = {
            let state = self.state.borrow();
            let Some(view) = state.view.as_ref() else { return };
            (view.webview.clone(), view.environment.clone())
        };
        let Ok(webview7) = webview.cast::<ICoreWebView2_7>() else {
            self.post(&serde_json::json!({"type": "saved", "text": "Saving a PDF needs a newer WebView2. Use Export → Open in browser and print from there."}));
            return;
        };
        self.post(&serde_json::json!({"type": "exporting", "on": true}));
        let settings = environment.cast::<ICoreWebView2Environment6>().ok().and_then(|environment| unsafe {
            let settings = environment.CreatePrintSettings().ok()?;
            let _ = settings.SetShouldPrintBackgrounds(true);
            let _ = settings.SetShouldPrintHeaderAndFooter(false);
            Some(settings)
        });
        let sender = self.sender.clone();
        let handler = PrintToPdfCompletedHandler::create(Box::new(move |result, ok| {
            let text = if result.is_ok() && ok { "Saved" } else { "Couldn’t save the PDF" };
            let _ = sender.send(UserEvent::Report(serde_json::json!({"cmd": "printed", "text": text})));
            Ok(())
        }));
        let target = wide(&path.to_string_lossy());
        unsafe {
            if webview7.PrintToPdf(PCWSTR(target.as_ptr()), settings.as_ref(), &handler).is_err() {
                self.post(&serde_json::json!({"type": "exporting", "on": false}));
                self.post(&serde_json::json!({"type": "saved", "text": "Couldn’t save the PDF"}));
            }
        }
    }

    /// PrintToPdf finished (sent back through the event loop).
    pub fn printed(&self, text: &str) {
        self.post(&serde_json::json!({"type": "exporting", "on": false}));
        self.post(&serde_json::json!({"type": "saved", "text": text}));
    }

    /// WebView2's process ended: start over the next time the window opens.
    pub fn reset(&self) {
        unsafe {
            let _ = ShowWindow(self.hwnd, SW_HIDE);
        }
        let view = {
            let mut state = self.state.borrow_mut();
            state.ready = false;
            state.creating = false;
            state.loaded = false;
            state.expect_navigation = false;
            state.view.take()
        };
        CONTROLLER.with(|slot| *slot.borrow_mut() = None);
        self.sent.set(None);
        if let Some(view) = view {
            unsafe {
                let _ = view.controller.Close();
            }
        }
    }

    fn create_view(&self) {
        self.state.borrow_mut().creating = true;
        let folder = dirs::data_local_dir()
            .unwrap_or_else(std::env::temp_dir)
            .join("Barista")
            .join("WebView2");
        let _ = std::fs::create_dir_all(&folder);
        let folder = wide(&folder.to_string_lossy());
        let hwnd = self.hwnd;
        let state = self.state.clone();
        let sender = self.sender.clone();
        let on_environment = CreateCoreWebView2EnvironmentCompletedHandler::create(Box::new(move |result, environment| {
            let environment = match (result, environment) {
                (Ok(()), Some(environment)) => environment,
                _ => return fail(&state, &sender),
            };
            let controller_state = state.clone();
            let controller_sender = sender.clone();
            let controller_environment = environment.clone();
            let on_controller =
                CreateCoreWebView2ControllerCompletedHandler::create(Box::new(move |result, controller| match (result, controller) {
                    (Ok(()), Some(controller)) => {
                        if configure(
                            hwnd,
                            controller,
                            controller_environment.clone(),
                            &controller_state,
                            &controller_sender,
                        )
                        .is_err()
                        {
                            return fail(&controller_state, &controller_sender);
                        }
                        Ok(())
                    }
                    _ => fail(&controller_state, &controller_sender),
                }));
            unsafe {
                if environment.CreateCoreWebView2Controller(hwnd, &on_controller).is_err() {
                    return fail(&state, &sender);
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
        if started.is_err() {
            let _ = fail(&self.state, &self.sender);
        }
    }
}

/// WebView2 is unavailable: hide, and let the event loop open the report in the browser instead.
fn fail(state: &Rc<RefCell<State>>, sender: &Sender<UserEvent>) -> windows::core::Result<()> {
    let mut state = state.borrow_mut();
    if !state.failed {
        state.failed = true;
        state.creating = false;
        let _ = sender.send(UserEvent::Report(serde_json::json!({"cmd": "failed"})));
    }
    Ok(())
}

fn configure(
    hwnd: HWND,
    controller: ICoreWebView2Controller,
    environment: ICoreWebView2Environment,
    state: &Rc<RefCell<State>>,
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
        settings.SetIsZoomControlEnabled(true)?;
        if let Ok(settings3) = settings.cast::<ICoreWebView2Settings3>() {
            let _ = settings3.SetAreBrowserAcceleratorKeysEnabled(false);
        }

        let mut token = 0i64;
        let navigation_state = state.clone();
        webview.add_NavigationStarting(
            &NavigationStartingEventHandler::create(Box::new(move |_, args| {
                let Some(args) = args else { return Ok(()) };
                let mut state = navigation_state.borrow_mut();
                if std::mem::take(&mut state.expect_navigation) {
                    state.loaded = true;
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
        let message_state = state.clone();
        let message_sender = sender.clone();
        webview.add_WebMessageReceived(
            &WebMessageReceivedEventHandler::create(Box::new(move |_, args| {
                let Some(args) = args else { return Ok(()) };
                let mut source = PWSTR::null();
                args.Source(&mut source)?;
                if !crate::panel::from_panel_page(message_state.borrow().loaded, &take_string(source)) {
                    return Ok(());
                }
                let mut json = PWSTR::null();
                args.WebMessageAsJson(&mut json)?;
                let json = take_string(json);
                if json.len() > 25_000_000 {
                    return Ok(());
                }
                if let Ok(message) = serde_json::from_str::<serde_json::Value>(&json) {
                    if matches!(
                        message["cmd"].as_str(),
                        Some("ready" | "period" | "view" | "copy" | "open-folder" | "export")
                    ) {
                        let _ = message_sender.send(UserEvent::Report(message));
                    }
                }
                Ok(())
            })),
            &mut token,
        )?;
        let failed_sender = sender.clone();
        webview.add_ProcessFailed(
            &ProcessFailedEventHandler::create(Box::new(move |_, _| {
                let _ = failed_sender.send(UserEvent::Report(serde_json::json!({"cmd": "process-failed"})));
                Ok(())
            })),
            &mut token,
        )?;

        let mut rect = RECT::default();
        let _ = GetClientRect(hwnd, &mut rect);
        controller.SetBounds(rect)?;
        controller.SetIsVisible(IsWindowVisible(hwnd).as_bool())?;
        let _ = controller.MoveFocus(COREWEBVIEW2_MOVE_FOCUS_REASON_PROGRAMMATIC);
        let page = wide(barista_core::usage::report_template());
        state.borrow_mut().expect_navigation = true;
        webview.NavigateToString(PCWSTR(page.as_ptr()))?;

        CONTROLLER.with(|slot| *slot.borrow_mut() = Some(controller.clone()));
        let mut state = state.borrow_mut();
        state.view = Some(View {
            controller,
            webview,
            environment,
        });
        state.creating = false;
    }
    Ok(())
}

/// Windows' Save dialog; None if cancelled.
fn save_dialog(owner: HWND, name: &str, kind: &str, pattern: &str, extension: &str) -> Option<PathBuf> {
    unsafe {
        let dialog: IFileSaveDialog = CoCreateInstance(&FileSaveDialog, None, CLSCTX_INPROC_SERVER).ok()?;
        let (kind, pattern, extension, name) = (wide(kind), wide(pattern), wide(extension), wide(name));
        let types = [COMDLG_FILTERSPEC {
            pszName: PCWSTR(kind.as_ptr()),
            pszSpec: PCWSTR(pattern.as_ptr()),
        }];
        dialog.SetFileTypes(&types).ok()?;
        dialog.SetDefaultExtension(PCWSTR(extension.as_ptr())).ok()?;
        dialog.SetFileName(PCWSTR(name.as_ptr())).ok()?;
        dialog.Show(Some(owner)).ok()?;
        let item = dialog.GetResult().ok()?;
        let path = item.GetDisplayName(SIGDN_FILESYSPATH).ok()?;
        Some(PathBuf::from(take_string(path)))
    }
}
