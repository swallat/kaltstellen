//! The main window as a WebView2 host (WP10, spec M6). A top-level window
//! with one WebView2 controller that shows only the content embedded in the
//! executable, served from a host name that can never resolve. Everything
//! the page wants goes through [`Page::message`]; decisions are made by
//! safe code in `crate::gui`.
//!
//! Lockdown: navigation away from the embedded origin is cancelled, new
//! windows are refused, every web request outside the origin is answered
//! with 403 (no network at all), permission requests are denied, default
//! script dialogs are off, and in release builds dev tools, the context
//! menu and browser accelerator keys are off. A file dropped onto the
//! window (M2) never navigates to it: the page handles every drop, and the
//! navigation guard would cancel the `file:` URL anyway.

use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::sync::mpsc;

use webview2_com::Microsoft::Web::WebView2::Win32::*;
use webview2_com::*;
use windows::Win32::Foundation::HANDLE;
use windows::Win32::Foundation::{E_POINTER, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::System::Com::{
    COINIT_APARTMENTTHREADED, CoInitializeEx, CoTaskMemFree, IStream, STGM_CREATE, STGM_WRITE,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::Shell::{SHCreateMemStream, SHCreateStreamOnFileEx};
use windows::Win32::UI::WindowsAndMessaging::{
    CW_USEDEFAULT, CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetClientRect,
    GetMessageW, HWND_TOPMOST, LoadIconW, MSG, PostMessageW, PostQuitMessage, RegisterClassExW,
    SW_SHOW, SW_SHOWNOACTIVATE, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER, SetWindowPos,
    ShowWindow, TranslateMessage, WINDOW_EX_STYLE, WM_APP, WM_CLOSE, WM_DESTROY, WM_SETFOCUS,
    WM_SIZE, WNDCLASSEXW, WS_OVERLAPPEDWINDOW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetClassNameW, GetPropW, IsIconic, SW_RESTORE, SetForegroundWindow, SetPropW,
};
use windows::core::{PCWSTR, PWSTR};

use super::wide;
use crate::error::{Code, Error, Result};

/// Window class of the main window; also how tests find it.
pub const CLASS: &str = "kaltstellen-main";
/// The origin the embedded content is served from. `.invalid` never
/// resolves (RFC 2606), so nothing can be fetched from there by accident.
pub const ORIGIN: &str = "https://ui.kaltstellen.invalid/";
const WM_WAKE: u32 = WM_APP + 2;
const WM_CAPTURE: u32 = WM_APP + 3;
/// MAKEINTRESOURCE of the program icon (resource 1, see build.rs).
const APP_ICON: usize = 1;

/// Content Security Policy sent with every embedded file: only own scripts
/// and styles, no inline script, no network, no frames, no forms.
pub const CSP: &str = "default-src 'none'; script-src 'self'; style-src 'self'; img-src 'self' data:; \
     font-src 'self'; connect-src 'none'; base-uri 'none'; form-action 'none'; frame-ancestors 'none'";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Theme {
    Light,
    Dark,
}

/// How the window runs.
#[derive(Debug, Clone)]
pub struct Options {
    pub title: String,
    /// Client size at 96 DPI.
    pub size: (i32, i32),
    /// WebView2 user data folder (cache, cookies of the embedded page).
    pub user_data: PathBuf,
    pub dev_tools: bool,
    /// Forces the page's colour scheme (`prefers-color-scheme`).
    pub theme: Option<Theme>,
    /// Query string for `index.html`, without `?`.
    pub query: String,
    /// Show without taking the keyboard focus (screenshots, tests).
    pub no_activate: bool,
    /// Marks the window as the one of this instance (see [`bring_to_front`]).
    pub instance: String,
}

/// What the safe side wants done after a message from the page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Answer {
    Nothing,
    /// Post this JSON to the page.
    Reply(String),
    /// Run this script in the page (test driver).
    Script(String),
    /// Capture the page as PNG into the file, then close.
    CaptureAndClose(PathBuf),
    Close,
}

/// The safe side of the window.
pub trait Page {
    /// A message from the page, as JSON text, with the paths of the files
    /// it attached (M2: files dropped from Explorer; usually none).
    fn message(&mut self, json: &str, files: Vec<PathBuf>) -> Answer;
    /// A [`Waker`] fired; returns what to do (events for the page, close).
    fn wake(&mut self) -> Vec<Answer>;
    /// The user closes the window; true to let it close.
    fn closing(&mut self) -> bool;
}

/// Wakes the window thread from another thread.
#[derive(Debug, Clone)]
pub struct Waker(usize);

impl Waker {
    /// The window, as a value for owner windows of dialogs.
    pub fn window(&self) -> usize {
        self.0
    }

    pub fn wake(&self) {
        // SAFETY: posting to a window that is gone fails harmlessly.
        let _ =
            unsafe { PostMessageW(Some(HWND(self.0 as *mut _)), WM_WAKE, WPARAM(0), LPARAM(0)) };
    }
}

/// Name of the window property that marks the instance.
const INSTANCE_PROPERTY: &str = "kaltstellen.instance";

/// A non-zero number for an instance name (FNV-1a, 31 bits).
fn instance_tag(instance: &str) -> usize {
    let mut hash: u32 = 0x811c_9dc5;
    for byte in instance.bytes() {
        hash ^= u32::from(byte);
        hash = hash.wrapping_mul(0x0100_0193);
    }
    (hash & 0x7FFF_FFFF).max(1) as usize
}

/// M6: brings the main window of `instance` to the front, restored if it
/// was minimised. False if no such window was found.
pub fn bring_to_front(instance: &str) -> bool {
    struct Search {
        class: Vec<u16>,
        tag: usize,
        property: Vec<u16>,
        found: Option<HWND>,
    }
    unsafe extern "system" fn visit(hwnd: HWND, param: LPARAM) -> windows::core::BOOL {
        // SAFETY: `param` points to the `Search` below for the whole call.
        let search = unsafe { &mut *(param.0 as *mut Search) };
        let mut class = [0u16; 64];
        // SAFETY: valid buffer; a stale handle makes the calls fail.
        let len = unsafe { GetClassNameW(hwnd, &mut class) } as usize;
        if class[..len] == search.class[..search.class.len() - 1] {
            // SAFETY: as above.
            let tag = unsafe { GetPropW(hwnd, PCWSTR(search.property.as_ptr())) };
            if tag.0 as usize == search.tag {
                search.found = Some(hwnd);
                return false.into();
            }
        }
        true.into()
    }
    let mut search = Search {
        class: wide(CLASS),
        tag: instance_tag(instance),
        property: wide(INSTANCE_PROPERTY),
        found: None,
    };
    // SAFETY: the callback only uses `search`, which outlives the call.
    let _ = unsafe { EnumWindows(Some(visit), LPARAM(&raw mut search as isize)) };
    let Some(hwnd) = search.found else {
        return false;
    };
    // SAFETY: a window of another process of ours; failures are harmless.
    unsafe {
        if IsIconic(hwnd).as_bool() {
            let _ = ShowWindow(hwnd, SW_RESTORE);
        }
        let _ = SetForegroundWindow(hwnd);
    }
    true
}

/// The installed Evergreen WebView2 runtime version, `None` if there is
/// none. Honours `WEBVIEW2_BROWSER_EXECUTABLE_FOLDER` like WebView2 does.
pub fn runtime_version() -> Option<String> {
    let mut version = PWSTR::null();
    // SAFETY: the returned string is freed with CoTaskMemFree.
    unsafe {
        GetAvailableCoreWebView2BrowserVersionString(PCWSTR::null(), &mut version).ok()?;
        if version.is_null() {
            return None;
        }
        let text = version.to_string().ok();
        CoTaskMemFree(Some(version.0 as *const _));
        text.filter(|t| !t.is_empty())
    }
}

/// Embedded content: path below the origin to (bytes, MIME type).
pub type Content = fn(&str) -> Option<(&'static [u8], &'static str)>;

struct Window {
    controller: ICoreWebView2Controller,
    webview: ICoreWebView2,
}

thread_local! {
    static WINDOW: RefCell<Option<Window>> = const { RefCell::new(None) };
    static PAGE: RefCell<Option<Box<dyn Page>>> = RefCell::new(None);
    static HWND_OF: Cell<usize> = const { Cell::new(0) };
    static CAPTURE: RefCell<Option<PathBuf>> = const { RefCell::new(None) };
    /// WM_CLOSE arrived while the WebView was still being created.
    static CLOSE_EARLY: Cell<bool> = const { Cell::new(false) };
    /// The window was closed; later failures of the start-up do not count.
    static CLOSED: Cell<bool> = const { Cell::new(false) };
}

/// Runs `f` on the page. The page is taken out while it runs, so a nested
/// message loop (the file dialog) cannot reach it twice; such a message is
/// dropped.
fn with_page<T>(f: impl FnOnce(&mut dyn Page) -> T) -> Option<T> {
    let mut page = PAGE.with_borrow_mut(Option::take)?;
    let result = f(page.as_mut());
    PAGE.with_borrow_mut(|slot| {
        if slot.is_none() {
            *slot = Some(page);
        }
    });
    Some(result)
}

fn fail(what: &str, err: impl std::fmt::Display) -> Error {
    Error::new(Code::GuiWindow, format!("cannot {what}: {err}"))
}

fn take_string(text: PWSTR) -> String {
    // SAFETY: strings returned by WebView2 are CoTaskMem allocations.
    unsafe {
        let value = text.to_string().unwrap_or_default();
        CoTaskMemFree(Some(text.0 as *const _));
        value
    }
}

fn post_json(webview: &ICoreWebView2, json: &str) {
    let json = wide(json);
    // SAFETY: NUL-terminated JSON.
    let _ = unsafe { webview.PostWebMessageAsJson(PCWSTR(json.as_ptr())) };
}

/// A response for an embedded file, or 403.
fn respond(
    env: &ICoreWebView2Environment,
    content: Content,
    uri: &str,
) -> Option<ICoreWebView2WebResourceResponse> {
    let path = uri
        .strip_prefix(ORIGIN)
        .map(|p| p.split(['?', '#']).next().unwrap_or(""));
    let found = path.and_then(|p| content(if p.is_empty() { "index.html" } else { p }));
    // SAFETY: the stream copies the bytes; strings are NUL-terminated.
    unsafe {
        match found {
            Some((bytes, mime)) => {
                let stream: Option<IStream> = SHCreateMemStream(Some(bytes));
                let headers = wide(&format!(
                    "Content-Type: {mime}\r\nContent-Security-Policy: {CSP}\r\nX-Content-Type-Options: nosniff\r\nCache-Control: no-store"
                ));
                let ok = wide("OK");
                env.CreateWebResourceResponse(
                    stream.as_ref(),
                    200,
                    PCWSTR(ok.as_ptr()),
                    PCWSTR(headers.as_ptr()),
                )
                .ok()
            }
            None => {
                let reason = wide("Forbidden");
                let headers = wide("Content-Type: text/plain");
                env.CreateWebResourceResponse(
                    None::<&IStream>,
                    403,
                    PCWSTR(reason.as_ptr()),
                    PCWSTR(headers.as_ptr()),
                )
                .ok()
            }
        }
    }
}

fn scale(value: i32, dpi: u32) -> i32 {
    value * dpi as i32 / 96
}

fn fit(hwnd: HWND) {
    WINDOW.with_borrow(|window| {
        if let Some(window) = window {
            let mut rect = RECT::default();
            // SAFETY: our window; the controller belongs to this thread.
            unsafe {
                let _ = GetClientRect(hwnd, &mut rect);
                let _ = window.controller.SetBounds(rect);
            }
        }
    });
}

fn act(hwnd: HWND, answer: Answer) {
    let webview = WINDOW.with_borrow(|w| w.as_ref().map(|w| w.webview.clone()));
    let Some(webview) = webview else {
        return;
    };
    match answer {
        Answer::Nothing => {}
        Answer::Reply(json) => post_json(&webview, &json),
        Answer::Script(script) => {
            let script = wide(&script);
            let handler = ExecuteScriptCompletedHandler::create(Box::new(|_, _| Ok(())));
            // SAFETY: NUL-terminated script.
            let _ = unsafe { webview.ExecuteScript(PCWSTR(script.as_ptr()), &handler) };
        }
        Answer::CaptureAndClose(path) => {
            // Outside the event handler that asked for it, from the loop.
            CAPTURE.set(Some(path));
            // SAFETY: our window.
            let _ = unsafe { PostMessageW(Some(hwnd), WM_CAPTURE, WPARAM(0), LPARAM(0)) };
        }
        Answer::Close => close(hwnd),
    }
}

fn capture(hwnd: HWND, webview: &ICoreWebView2, path: &Path) {
    let file = wide(&path.to_string_lossy());
    // SAFETY: the stream is kept alive by the completion closure.
    let stream = unsafe {
        SHCreateStreamOnFileEx(
            PCWSTR(file.as_ptr()),
            (STGM_CREATE | STGM_WRITE).0,
            0x80,
            true,
            None,
        )
    };
    let Ok(stream) = stream else {
        log::warn!("gui: cannot create {}", path.display());
        close(hwnd);
        return;
    };
    log::info!("gui: capturing {}", path.display());
    let keep = stream.clone();
    let window = hwnd.0 as usize;
    let handler = CapturePreviewCompletedHandler::create(Box::new(move |result| {
        drop(keep);
        log::info!("gui: capture done: {result:?}");
        if let Err(err) = result {
            log::warn!("gui: capture failed: {err}");
        }
        close(HWND(window as *mut _));
        Ok(())
    }));
    // SAFETY: valid stream and handler.
    if let Err(err) = unsafe {
        webview.CapturePreview(
            COREWEBVIEW2_CAPTURE_PREVIEW_IMAGE_FORMAT_PNG,
            &stream,
            &handler,
        )
    } {
        log::warn!("gui: capture failed: {err}");
        close(hwnd);
    }
}

fn close(hwnd: HWND) {
    CLOSED.set(true);
    if let Some(window) = WINDOW.take() {
        // SAFETY: closing the controller ends the WebView on this thread.
        let _ = unsafe { window.controller.Close() };
    }
    // SAFETY: our window, on its thread.
    let _ = unsafe { DestroyWindow(hwnd) };
}

/// Creates the window and the WebView, loads `index.html` from `content`,
/// hands a [`Waker`] to `make` and runs the message loop until the window
/// closes.
pub fn run(
    options: Options,
    content: Content,
    make: impl FnOnce(Waker) -> Box<dyn Page>,
) -> Result<()> {
    let class = wide(CLASS);
    let title = wide(&options.title);
    // SAFETY: COM for this thread; pointers below point to live buffers.
    let hwnd = unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        let instance = GetModuleHandleW(None).map_err(|e| fail("get the module handle", e))?;
        let wc = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(window_proc),
            hInstance: instance.into(),
            lpszClassName: PCWSTR(class.as_ptr()),
            // The program icon (resource 1, embedded by build.rs).
            hIcon: LoadIconW(Some(instance.into()), PCWSTR(APP_ICON as *const u16))
                .unwrap_or_default(),
            ..Default::default()
        };
        if RegisterClassExW(&wc) == 0 {
            return Err(fail(
                "register the window class",
                windows::core::Error::from_thread(),
            ));
        }
        CreateWindowExW(
            WINDOW_EX_STYLE(0),
            PCWSTR(class.as_ptr()),
            PCWSTR(title.as_ptr()),
            WS_OVERLAPPEDWINDOW,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            options.size.0,
            options.size.1,
            None,
            None,
            Some(instance.into()),
            None,
        )
        .map_err(|e| fail("create the main window", e))?
    };
    // SAFETY: our window.
    unsafe {
        let dpi = GetDpiForWindow(hwnd);
        let _ = SetWindowPos(
            hwnd,
            None,
            0,
            0,
            scale(options.size.0, dpi),
            scale(options.size.1, dpi),
            SWP_NOMOVE | SWP_NOZORDER,
        );
    }

    // Window property naming this instance, read by a second start.
    let property = wide(INSTANCE_PROPERTY);
    // SAFETY: our window; the value is a number, not a pointer.
    let _ = unsafe {
        SetPropW(
            hwnd,
            PCWSTR(property.as_ptr()),
            Some(HANDLE(instance_tag(&options.instance) as *mut _)),
        )
    };
    let environment = create_environment(&options.user_data)?;
    let controller = create_controller(&environment, hwnd)?;
    // SAFETY: COM calls on this thread's objects.
    let webview = unsafe {
        let webview = controller
            .CoreWebView2()
            .map_err(|e| fail("get the WebView", e))?;
        let settings = webview
            .Settings()
            .map_err(|e| fail("get the WebView settings", e))?;
        let _ = settings.SetAreDevToolsEnabled(options.dev_tools);
        let _ = settings.SetAreDefaultContextMenusEnabled(options.dev_tools);
        let _ = settings.SetAreDefaultScriptDialogsEnabled(false);
        let _ = settings.SetIsStatusBarEnabled(false);
        if let Ok(settings3) = windows::core::Interface::cast::<ICoreWebView2Settings3>(&settings) {
            let _ = settings3.SetAreBrowserAcceleratorKeysEnabled(options.dev_tools);
        }
        if let Some(theme) = options.theme
            && let Ok(webview13) = windows::core::Interface::cast::<ICoreWebView2_13>(&webview)
            && let Ok(profile) = webview13.Profile()
        {
            let _ = profile.SetPreferredColorScheme(match theme {
                Theme::Light => COREWEBVIEW2_PREFERRED_COLOR_SCHEME_LIGHT,
                Theme::Dark => COREWEBVIEW2_PREFERRED_COLOR_SCHEME_DARK,
            });
        }
        install_handlers(&webview, &environment, content)?;
        webview
    };

    HWND_OF.set(hwnd.0 as usize);
    WINDOW.set(Some(Window {
        controller,
        webview: webview.clone(),
    }));
    PAGE.set(Some(make(Waker(hwnd.0 as usize))));
    if CLOSE_EARLY.get() {
        close(hwnd);
        WINDOW.set(None);
        PAGE.set(None);
        return Ok(());
    }
    fit(hwnd);
    let mut url = format!("{ORIGIN}index.html");
    if !options.query.is_empty() {
        url.push('?');
        url.push_str(&options.query);
    }
    let url = wide(&url);
    // SAFETY: our window and WebView; standard message loop.
    unsafe {
        if let Err(err) = webview.Navigate(PCWSTR(url.as_ptr())) {
            // A close during start-up (COM calls pump messages) ends the
            // WebView before it navigated; that is a normal end.
            if !CLOSED.get() {
                return Err(fail("load the page", err));
            }
        }
        // The first ShowWindow of a process may take the start-up flag of
        // its parent (for example "hidden"); the second one counts.
        let show = if options.no_activate {
            SW_SHOWNOACTIVATE
        } else {
            SW_SHOW
        };
        let _ = ShowWindow(hwnd, show);
        let _ = ShowWindow(hwnd, show);
        // The WebView of a window that was hidden at creation stays
        // invisible until told otherwise.
        WINDOW.with_borrow(|w| {
            if let Some(w) = w {
                let _ = w.controller.SetIsVisible(true);
            }
        });
        fit(hwnd);
        if options.no_activate {
            // Review runs: on top without the focus, so the page renders
            // even when other windows would cover it.
            let _ = SetWindowPos(
                hwnd,
                Some(HWND_TOPMOST),
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
            );
        }
        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
    WINDOW.set(None);
    PAGE.set(None);
    Ok(())
}

fn create_environment(user_data: &Path) -> Result<ICoreWebView2Environment> {
    let folder = wide(&user_data.to_string_lossy());
    let (tx, rx) = mpsc::channel();
    let options = CoreWebView2EnvironmentOptions::default();
    // SAFETY: the folder string lives until the call completes (we wait).
    CreateCoreWebView2EnvironmentCompletedHandler::wait_for_async_operation(
        Box::new(move |handler| unsafe {
            let options: ICoreWebView2EnvironmentOptions = options.into();
            CreateCoreWebView2EnvironmentWithOptions(
                PCWSTR::null(),
                PCWSTR(folder.as_ptr()),
                &options,
                &handler,
            )
            .map_err(webview2_com::Error::WindowsError)
        }),
        Box::new(move |result, environment| {
            result?;
            tx.send(environment.ok_or_else(|| windows::core::Error::from(E_POINTER)))
                .expect("receiver alive");
            Ok(())
        }),
    )
    .map_err(|e| fail("start WebView2", format!("{e:?}")))?;
    rx.recv()
        .map_err(|e| fail("start WebView2", e))?
        .map_err(|e| fail("start WebView2", e))
}

fn create_controller(
    environment: &ICoreWebView2Environment,
    hwnd: HWND,
) -> Result<ICoreWebView2Controller> {
    let (tx, rx) = mpsc::channel();
    let environment = environment.clone();
    CreateCoreWebView2ControllerCompletedHandler::wait_for_async_operation(
        // SAFETY: our window; the handler is called on this thread.
        Box::new(move |handler| unsafe {
            environment
                .CreateCoreWebView2Controller(hwnd, &handler)
                .map_err(webview2_com::Error::WindowsError)
        }),
        Box::new(move |result, controller| {
            result?;
            tx.send(controller.ok_or_else(|| windows::core::Error::from(E_POINTER)))
                .expect("receiver alive");
            Ok(())
        }),
    )
    .map_err(|e| fail("create the WebView", format!("{e:?}")))?;
    rx.recv()
        .map_err(|e| fail("create the WebView", e))?
        .map_err(|e| fail("create the WebView", e))
}

/// Most attached objects read from one message; the bridge allows fewer.
const MAX_ATTACHED: u32 = 1024;

/// M2: the paths of the files the page attached to a message with
/// `chrome.webview.postMessageWithAdditionalObjects` (files dropped from
/// Explorer: the page itself only sees their names). An object that is not
/// a file, or a file without a path, gives an empty path, which the bridge
/// refuses. Needs a runtime with `ICoreWebView2WebMessageReceivedEventArgs2`;
/// an older one gives none.
fn attached_files(args: &ICoreWebView2WebMessageReceivedEventArgs) -> Vec<PathBuf> {
    use windows::core::Interface;
    let Ok(args) = args.cast::<ICoreWebView2WebMessageReceivedEventArgs2>() else {
        return Vec::new();
    };
    let mut files = Vec::new();
    // SAFETY: COM calls on objects of this thread; strings from WebView2
    // are freed by `take_string`.
    unsafe {
        let Ok(objects) = args.AdditionalObjects() else {
            return files;
        };
        let mut count = 0u32;
        if objects.Count(&mut count).is_err() {
            return files;
        }
        for index in 0..count.min(MAX_ATTACHED) {
            let path = objects
                .GetValueAtIndex(index)
                .ok()
                .and_then(|object| object.cast::<ICoreWebView2File>().ok())
                .and_then(|file| {
                    let mut path = PWSTR::null();
                    file.Path(&mut path).ok()?;
                    (!path.is_null()).then(|| take_string(path))
                })
                .unwrap_or_default();
            files.push(PathBuf::from(path));
        }
    }
    files
}

/// # Safety
/// COM objects of this thread.
unsafe fn install_handlers(
    webview: &ICoreWebView2,
    environment: &ICoreWebView2Environment,
    content: Content,
) -> Result<()> {
    let mut token = 0i64;
    unsafe {
        // Every request is seen here: embedded files are served, anything
        // else gets 403, so the page has no network at all.
        let all = wide("*");
        webview
            .AddWebResourceRequestedFilter(
                PCWSTR(all.as_ptr()),
                COREWEBVIEW2_WEB_RESOURCE_CONTEXT_ALL,
            )
            .map_err(|e| fail("filter requests", e))?;
        let env = environment.clone();
        let requested = WebResourceRequestedEventHandler::create(Box::new(move |_, args| {
            let Some(args) = args else { return Ok(()) };
            let mut uri = PWSTR::null();
            args.Request()?.Uri(&mut uri)?;
            let uri = take_string(uri);
            if cfg!(debug_assertions) {
                log::info!("gui: request {uri}");
            }
            if let Some(response) = respond(&env, content, &uri) {
                args.SetResponse(&response)?;
            }
            Ok(())
        }));
        webview
            .add_WebResourceRequested(&requested, &mut token)
            .map_err(|e| fail("serve the page", e))?;

        let navigation = NavigationStartingEventHandler::create(Box::new(|_, args| {
            let Some(args) = args else { return Ok(()) };
            let mut uri = PWSTR::null();
            args.Uri(&mut uri)?;
            if !take_string(uri).starts_with(ORIGIN) {
                args.SetCancel(true)?;
            }
            Ok(())
        }));
        webview
            .add_NavigationStarting(&navigation, &mut token)
            .map_err(|e| fail("guard navigation", e))?;

        let new_window = NewWindowRequestedEventHandler::create(Box::new(|_, args| {
            if let Some(args) = args {
                args.SetHandled(true)?;
            }
            Ok(())
        }));
        webview
            .add_NewWindowRequested(&new_window, &mut token)
            .map_err(|e| fail("refuse new windows", e))?;

        let permission = PermissionRequestedEventHandler::create(Box::new(|_, args| {
            if let Some(args) = args {
                args.SetState(COREWEBVIEW2_PERMISSION_STATE_DENY)?;
            }
            Ok(())
        }));
        webview
            .add_PermissionRequested(&permission, &mut token)
            .map_err(|e| fail("refuse permissions", e))?;

        let received = WebMessageReceivedEventHandler::create(Box::new(|_, args| {
            let Some(args) = args else { return Ok(()) };
            // Only messages from the embedded page count.
            let mut source = PWSTR::null();
            args.Source(&mut source)?;
            if !take_string(source).starts_with(ORIGIN) {
                return Ok(());
            }
            let mut json = PWSTR::null();
            args.WebMessageAsJson(&mut json)?;
            let json = take_string(json);
            if cfg!(debug_assertions) {
                log::info!(
                    "gui: message {}",
                    json.chars().take(200).collect::<String>()
                );
            }
            let files = attached_files(&args);
            if let Some(answer) = with_page(|page| page.message(&json, files)) {
                act(HWND(HWND_OF.get() as *mut _), answer);
            }
            Ok(())
        }));
        webview
            .add_WebMessageReceived(&received, &mut token)
            .map_err(|e| fail("listen to the page", e))?;
    }
    Ok(())
}

extern "system" fn window_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_SIZE => {
            fit(hwnd);
            LRESULT(0)
        }
        WM_SETFOCUS => {
            WINDOW.with_borrow(|w| {
                if let Some(w) = w {
                    // SAFETY: controller of this thread.
                    let _ = unsafe {
                        w.controller
                            .MoveFocus(COREWEBVIEW2_MOVE_FOCUS_REASON_PROGRAMMATIC)
                    };
                }
            });
            LRESULT(0)
        }
        WM_CAPTURE => {
            let webview = WINDOW.with_borrow(|w| w.as_ref().map(|w| w.webview.clone()));
            if let (Some(webview), Some(path)) = (webview, CAPTURE.take()) {
                capture(hwnd, &webview, &path);
            }
            LRESULT(0)
        }
        WM_WAKE => {
            let events = with_page(|page| page.wake());
            for answer in events.unwrap_or_default() {
                act(hwnd, answer);
            }
            LRESULT(0)
        }
        WM_CLOSE => {
            if WINDOW.with_borrow(Option::is_none) {
                // Still starting: close once the WebView exists, so its
                // creation is not cut off half-way.
                CLOSE_EARLY.set(true);
                return LRESULT(0);
            }
            let allow = with_page(|page| page.closing()).unwrap_or(true);
            if allow {
                close(hwnd);
            }
            LRESULT(0)
        }
        WM_DESTROY => {
            // SAFETY: plain call.
            unsafe { PostQuitMessage(0) };
            LRESULT(0)
        }
        // SAFETY: forwarding the unchanged arguments.
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}
