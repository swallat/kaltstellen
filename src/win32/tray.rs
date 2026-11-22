//! The tray window: a hidden top-level window that owns the notification
//! area icon, shows the menu and runs the message loop.
//!
//! The window is top-level, not message-only, because only top-level
//! windows receive broadcasts such as `TaskbarCreated` now and
//! `WM_QUERYENDSESSION` later (WP8). It has no taskbar button and is never
//! shown. `WM_CLOSE` (for example from `taskkill` without `/F`) is passed
//! to the handler like the `Quit` menu item.

use std::cell::{Cell, RefCell};

use log::warn;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::SetProcessShutdownParameters;
use windows::Win32::UI::HiDpi::{
    AreDpiAwarenessContextsEqual, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
    DPI_AWARENESS_PER_MONITOR_AWARE, DPI_AWARENESS_SYSTEM_AWARE, DPI_AWARENESS_UNAWARE,
    GetAwarenessFromDpiAwarenessContext, GetThreadDpiAwarenessContext,
};
use windows::Win32::UI::Shell::{
    NIF_ICON, NIF_MESSAGE, NIF_SHOWTIP, NIF_TIP, NIM_ADD, NIM_DELETE, NIM_MODIFY, NIM_SETVERSION,
    NIN_SELECT, NINF_KEY, NOTIFYICON_VERSION_4, NOTIFYICONDATAW, Shell_NotifyIconW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreatePopupMenu, CreateWindowExW, DefWindowProcW, DestroyMenu, DestroyWindow,
    DispatchMessageW, GetMessageW, GetSystemMetrics, HICON, IDI_APPLICATION, IMAGE_ICON,
    LR_DEFAULTCOLOR, LoadIconW, LoadImageW, MF_CHECKED, MF_GRAYED, MF_SEPARATOR, MF_STRING, MSG,
    PostMessageW, PostQuitMessage, RegisterClassExW, RegisterWindowMessageW, SM_CXSMICON,
    SM_CYSMICON, SM_MENUDROPALIGNMENT, SetForegroundWindow, TPM_BOTTOMALIGN, TPM_LEFTALIGN,
    TPM_RIGHTALIGN, TPM_RIGHTBUTTON, TrackPopupMenuEx, TranslateMessage, WINDOW_EX_STYLE, WM_APP,
    WM_CLOSE, WM_COMMAND, WM_CONTEXTMENU, WM_DESTROY, WM_ENDSESSION, WM_NULL, WM_POWERBROADCAST,
    WM_QUERYENDSESSION, WNDCLASSEXW, WS_EX_TOOLWINDOW, WS_OVERLAPPED,
};
use windows::core::PCWSTR;

use super::wide;
use crate::error::{Code, Error, Result};
use crate::tray::model::{Icon, Item};

/// Window class; also how scripts find the window.
pub const CLASS: &str = "kaltstellen-tray";
const TITLE: &str = "kaltstellen";
/// Callback message of the notification icon.
const WM_TRAY: u32 = WM_APP + 1;
/// Posted by [`Waker::wake`] from other threads.
const WM_WAKE: u32 = WM_APP + 2;
const ICON_ID: u32 = 1;
const SHUTDOWN_LEVEL: u32 = 0x3FF;
/// `szTip` has 128 units; the last one stays NUL.
const TIP_UNITS: usize = 127;

/// Resource IDs of the tray icons, embedded by `build.rs`.
fn resource_id(icon: Icon) -> u16 {
    match icon {
        Icon::Neutral => 101,
        Icon::Active => 102,
        Icon::Error => 103,
    }
}

/// What the safe side of the tray does with window events. Called on the
/// window thread only; never re-entered.
pub trait Handler {
    /// The user opens the menu; returns what it shows.
    fn menu(&mut self) -> Vec<Item>;
    /// A menu item was chosen (or `WM_COMMAND` arrived from elsewhere).
    fn command(&mut self, id: u16);
    /// A [`Waker`] fired.
    fn wake(&mut self);
    /// `WM_CLOSE`: someone asks the program to end.
    fn close(&mut self);
    /// `WM_QUERYENDSESSION` (S5): the session is about to end. Returns
    /// whether it may end; called on the window thread, so it should return
    /// within a few seconds.
    fn query_end_session(&mut self) -> bool;
    /// `WM_ENDSESSION`: `ending` is false if the end was cancelled.
    fn end_session(&mut self, ending: bool);
    /// `WM_POWERBROADCAST` with its event (a `PBT_` constant).
    fn power(&mut self, event: u32);
}

/// Wakes the window thread from any thread.
#[derive(Debug, Clone)]
pub struct Waker(usize);

impl Waker {
    pub fn wake(&self) {
        // SAFETY: posting to a window that is gone fails harmlessly; the
        // value is only used as a window handle.
        let _ =
            unsafe { PostMessageW(Some(HWND(self.0 as *mut _)), WM_WAKE, WPARAM(0), LPARAM(0)) };
    }
}

struct Window {
    hwnd: HWND,
    icons: [HICON; 3],
    icon: Icon,
    tip: String,
    taskbar_created: u32,
}

thread_local! {
    static WINDOW: RefCell<Option<Window>> = const { RefCell::new(None) };
    static HANDLER: RefCell<Option<Box<dyn Handler>>> = RefCell::new(None);
    static QUITTING: Cell<bool> = const { Cell::new(false) };
}

/// The DPI awareness of this thread, from the manifest: `per-monitor v2`
/// when the manifest took effect.
pub fn dpi_awareness() -> &'static str {
    // SAFETY: plain queries of the calling thread's context.
    unsafe {
        let context = GetThreadDpiAwarenessContext();
        if AreDpiAwarenessContextsEqual(context, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2)
            .as_bool()
        {
            return "per-monitor v2";
        }
        match GetAwarenessFromDpiAwarenessContext(context) {
            DPI_AWARENESS_UNAWARE => "unaware",
            DPI_AWARENESS_SYSTEM_AWARE => "system",
            DPI_AWARENESS_PER_MONITOR_AWARE => "per-monitor",
            _ => "unknown",
        }
    }
}

/// Changes the icon and tooltip. Window thread only.
pub fn show(icon: Icon, tip: &str) {
    WINDOW.with_borrow_mut(|window| {
        if let Some(window) = window {
            window.icon = icon;
            window.tip = tip.to_owned();
            notify(window, NIM_MODIFY);
        }
    });
}

/// Ends the message loop after removing the icon. Window thread only.
pub fn exit() {
    QUITTING.set(true);
    let hwnd = WINDOW.with_borrow(|w| w.as_ref().map(|w| w.hwnd));
    if let Some(hwnd) = hwnd {
        // SAFETY: the window belongs to this thread.
        if unsafe { DestroyWindow(hwnd) }.is_err() {
            // SAFETY: plain call.
            unsafe { PostQuitMessage(0) };
        }
    }
}

fn notify_data(window: &Window) -> NOTIFYICONDATAW {
    let mut data = NOTIFYICONDATAW {
        cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
        hWnd: window.hwnd,
        uID: ICON_ID,
        uFlags: NIF_MESSAGE | NIF_ICON | NIF_TIP | NIF_SHOWTIP,
        uCallbackMessage: WM_TRAY,
        hIcon: window.icons[window.icon as usize],
        ..Default::default()
    };
    for (dst, src) in data
        .szTip
        .iter_mut()
        .take(TIP_UNITS)
        .zip(window.tip.encode_utf16())
    {
        *dst = src;
    }
    data
}

/// Adds, modifies or deletes the notification icon. Adding fails while
/// Explorer is not running yet; `TaskbarCreated` adds it later.
fn notify(window: &Window, message: windows::Win32::UI::Shell::NOTIFY_ICON_MESSAGE) -> bool {
    let mut data = notify_data(window);
    // SAFETY: `data` is fully initialised and lives across the call.
    let ok = unsafe { Shell_NotifyIconW(message, &data) }.as_bool();
    if ok && message == NIM_ADD {
        data.Anonymous.uVersion = NOTIFYICON_VERSION_4;
        // SAFETY: as above.
        let _ = unsafe { Shell_NotifyIconW(NIM_SETVERSION, &data) };
    }
    ok
}

fn load_icons() -> [HICON; 3] {
    // SAFETY: plain calls; a missing resource yields an error, not UB.
    unsafe {
        let module = GetModuleHandleW(None).ok();
        let cx = GetSystemMetrics(SM_CXSMICON);
        let cy = GetSystemMetrics(SM_CYSMICON);
        [Icon::Neutral, Icon::Active, Icon::Error].map(|icon| {
            let id = resource_id(icon);
            // MAKEINTRESOURCE: the ID in the low word of the pointer.
            match LoadImageW(
                module.map(Into::into),
                PCWSTR(id as usize as *const u16),
                IMAGE_ICON,
                cx,
                cy,
                LR_DEFAULTCOLOR,
            ) {
                Ok(handle) => HICON(handle.0),
                Err(err) => {
                    warn!("tray icon {id} not found ({err}); using the default icon");
                    LoadIconW(None, IDI_APPLICATION).unwrap_or_default()
                }
            }
        })
    }
}

/// Creates the window and the icon, hands a [`Waker`] to `make` to build
/// the handler, and runs the message loop until [`exit`].
pub fn run(icon: Icon, tip: &str, make: impl FnOnce(Waker) -> Box<dyn Handler>) -> Result<()> {
    let fail = |what: &str, err: windows::core::Error| {
        Error::new(Code::TrayWindow, format!("cannot {what}: {err}"))
    };
    let class = wide(CLASS);
    let title = wide(TITLE);
    // SAFETY: all pointers passed below point to live, NUL-terminated
    // buffers; the window procedure is a valid `extern "system"` function.
    let hwnd = unsafe {
        let instance = GetModuleHandleW(None).map_err(|e| fail("get the module handle", e))?;
        let wc = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(window_proc),
            hInstance: instance.into(),
            lpszClassName: PCWSTR(class.as_ptr()),
            ..Default::default()
        };
        if RegisterClassExW(&wc) == 0 {
            return Err(fail(
                "register the window class",
                windows::core::Error::from_thread(),
            ));
        }
        CreateWindowExW(
            WINDOW_EX_STYLE(WS_EX_TOOLWINDOW.0),
            PCWSTR(class.as_ptr()),
            PCWSTR(title.as_ptr()),
            WS_OVERLAPPED,
            0,
            0,
            0,
            0,
            None,
            None,
            Some(instance.into()),
            None,
        )
        .map_err(|e| fail("create the tray window", e))?
    };
    let taskbar = wide("TaskbarCreated");
    let window = Window {
        hwnd,
        icons: load_icons(),
        icon,
        tip: tip.to_owned(),
        // SAFETY: NUL-terminated buffer.
        taskbar_created: unsafe { RegisterWindowMessageW(PCWSTR(taskbar.as_ptr())) },
    };
    if !notify(&window, NIM_ADD) {
        warn!("notification area not available yet; the icon follows when Explorer starts");
    }
    WINDOW.set(Some(window));
    HANDLER.set(Some(make(Waker(hwnd.0 as usize))));
    // S5: be asked early at shutdown, before the programs it froze, so they
    // are running again when Windows asks them. 0x3FF is the highest level
    // for applications.
    // SAFETY: plain call.
    if let Err(err) = unsafe { SetProcessShutdownParameters(SHUTDOWN_LEVEL, 0) } {
        warn!("cannot raise the shutdown priority: {err}");
    }

    let mut msg = MSG::default();
    // SAFETY: standard message loop on this thread's queue.
    unsafe {
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
    HANDLER.set(None);
    WINDOW.set(None);
    Ok(())
}

/// Runs `f` on the handler unless it is busy (a nested message loop inside
/// a handler call); the event is then dropped.
fn with_handler<T>(f: impl FnOnce(&mut dyn Handler) -> T) -> Option<T> {
    HANDLER.with(|cell| {
        let mut guard = cell.try_borrow_mut().ok()?;
        guard.as_mut().map(|h| f(h.as_mut()))
    })
}

fn show_menu(hwnd: HWND, x: i32, y: i32) {
    let Some(items) = with_handler(|h| h.menu()) else {
        return;
    };
    // SAFETY: the menu is created, filled with live NUL-terminated labels,
    // tracked and destroyed within this block.
    unsafe {
        let Ok(menu) = CreatePopupMenu() else {
            warn!("cannot create the tray menu");
            return;
        };
        for item in &items {
            let result = match item {
                Item::Separator => AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null()),
                Item::Entry {
                    id,
                    label,
                    checked,
                    enabled,
                } => {
                    let mut flags = MF_STRING;
                    if *checked {
                        flags |= MF_CHECKED;
                    }
                    if !*enabled {
                        flags |= MF_GRAYED;
                    }
                    let label = wide(label);
                    AppendMenuW(menu, flags, usize::from(*id), PCWSTR(label.as_ptr()))
                }
            };
            if let Err(err) = result {
                warn!("cannot add a tray menu item: {err}");
            }
        }
        // Without this, the menu does not close when the user clicks
        // elsewhere (KB135788).
        let _ = SetForegroundWindow(hwnd);
        let align = if GetSystemMetrics(SM_MENUDROPALIGNMENT) != 0 {
            TPM_RIGHTALIGN
        } else {
            TPM_LEFTALIGN
        };
        // The chosen item arrives as WM_COMMAND after the menu closed, so
        // the handler is free again by then.
        let _ = TrackPopupMenuEx(
            menu,
            (align | TPM_BOTTOMALIGN | TPM_RIGHTBUTTON).0,
            x,
            y,
            hwnd,
            None,
        );
        let _ = PostMessageW(Some(hwnd), WM_NULL, WPARAM(0), LPARAM(0));
        let _ = DestroyMenu(menu);
    }
}

fn loword(value: usize) -> u32 {
    (value & 0xFFFF) as u32
}

/// Signed coordinate in the low or high word (GET_X_LPARAM, GET_Y_LPARAM).
fn coordinate(value: usize, high: bool) -> i32 {
    let word = if high {
        (value >> 16) & 0xFFFF
    } else {
        value & 0xFFFF
    };
    i32::from(word as u16 as i16)
}

extern "system" fn window_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_TRAY => {
            // NOTIFYICON_VERSION_4: event in the low word of lParam, anchor
            // point in wParam.
            let event = loword(lparam.0 as usize);
            if event == WM_CONTEXTMENU || event == NIN_SELECT || event == (NIN_SELECT | NINF_KEY) {
                show_menu(
                    hwnd,
                    coordinate(wparam.0, false),
                    coordinate(wparam.0, true),
                );
            }
            LRESULT(0)
        }
        WM_WAKE => {
            with_handler(|h| h.wake());
            LRESULT(0)
        }
        WM_COMMAND => {
            let id = loword(wparam.0) as u16;
            with_handler(|h| h.command(id));
            LRESULT(0)
        }
        WM_QUERYENDSESSION => {
            let allow = with_handler(|h| h.query_end_session()).unwrap_or(true);
            LRESULT(isize::from(allow))
        }
        WM_ENDSESSION => {
            with_handler(|h| h.end_session(wparam.0 != 0));
            LRESULT(0)
        }
        WM_POWERBROADCAST => {
            with_handler(|h| h.power(wparam.0 as u32));
            LRESULT(1)
        }
        WM_CLOSE => {
            if !QUITTING.get() {
                with_handler(|h| h.close());
            }
            LRESULT(0)
        }
        WM_DESTROY => {
            WINDOW.with_borrow(|window| {
                if let Some(window) = window {
                    notify(window, NIM_DELETE);
                }
            });
            // SAFETY: plain call.
            unsafe { PostQuitMessage(0) };
            LRESULT(0)
        }
        _ => {
            let taskbar_created = WINDOW.with_borrow(|w| w.as_ref().map(|w| w.taskbar_created));
            if taskbar_created == Some(msg) {
                // Explorer restarted: the icon is gone and must be added again.
                WINDOW.with_borrow(|window| {
                    if let Some(window) = window {
                        notify(window, NIM_ADD);
                    }
                });
                return LRESULT(0);
            }
            // SAFETY: forwarding the unchanged arguments.
            unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn coordinates_are_signed_words() {
        // x = -10 (0xFFF6), y = 300 (0x012C), as on a monitor left of the
        // primary one.
        let value = (0x012C << 16) | 0xFFF6;
        assert_eq!(coordinate(value, false), -10);
        assert_eq!(coordinate(value, true), 300);
        assert_eq!(loword(0x1234_0405), 0x0405);
    }
}
