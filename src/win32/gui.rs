//! A small native form for the main window (WP10): a top-level window with
//! standard controls (buttons, list box, list views, edit, combo box,
//! labels) at fixed positions scaled to the monitor's DPI. The safe side
//! describes the controls and reacts to [`Event`]s through [`Form`]; all
//! decisions live in `crate::gui`.

use std::cell::RefCell;
use std::path::PathBuf;

use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{CreateFontIndirectW, DeleteObject, HFONT, HGDIOBJ};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Controls::Dialogs::{
    GetOpenFileNameW, OFN_FILEMUSTEXIST, OFN_NOCHANGEDIR, OFN_PATHMUSTEXIST, OPENFILENAMEW,
};
use windows::Win32::UI::Controls::{
    ICC_LISTVIEW_CLASSES, ICC_STANDARD_CLASSES, INITCOMMONCONTROLSEX, InitCommonControlsEx,
    LVCF_TEXT, LVCF_WIDTH, LVCOLUMNW, LVIF_TEXT, LVITEMW, LVM_DELETEALLITEMS, LVM_GETNEXTITEM,
    LVM_INSERTCOLUMNW, LVM_INSERTITEMW, LVM_SETEXTENDEDLISTVIEWSTYLE, LVM_SETITEMTEXTW,
    LVNI_SELECTED, LVS_EX_FULLROWSELECT, LVS_REPORT, LVS_SHOWSELALWAYS, LVS_SINGLESEL,
};
use windows::Win32::UI::HiDpi::{GetDpiForWindow, SystemParametersInfoForDpi};
use windows::Win32::UI::WindowsAndMessaging::{
    BS_PUSHBUTTON, CB_ADDSTRING, CB_GETCURSEL, CB_RESETCONTENT, CB_SETCURSEL, CBS_DROPDOWNLIST,
    CW_USEDEFAULT, CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW,
    ES_AUTOHSCROLL, GetDlgItem, GetMessageW, GetWindowTextLengthW, GetWindowTextW, HMENU, IDYES,
    IsDialogMessageW, LB_ADDSTRING, LB_GETCURSEL, LB_RESETCONTENT, LB_SETCURSEL, LBN_SELCHANGE,
    LBS_NOTIFY, MB_ICONQUESTION, MB_ICONWARNING, MB_OK, MB_YESNO, MSG, MessageBoxW,
    NONCLIENTMETRICSW, PostMessageW, PostQuitMessage, RegisterClassExW, SPI_GETNONCLIENTMETRICS,
    SW_SHOW, SWP_NOACTIVATE, SWP_NOZORDER, SendMessageW, SetWindowPos, SetWindowTextW, ShowWindow,
    TranslateMessage, WINDOW_EX_STYLE, WINDOW_STYLE, WM_APP, WM_CLOSE, WM_COMMAND, WM_DESTROY,
    WM_DPICHANGED, WM_SETFONT, WNDCLASSEXW, WS_BORDER, WS_CAPTION, WS_CHILD, WS_EX_CLIENTEDGE,
    WS_MINIMIZEBOX, WS_OVERLAPPED, WS_SYSMENU, WS_TABSTOP, WS_VISIBLE, WS_VSCROLL,
};
use windows::core::{PCWSTR, PWSTR};

use super::wide;
use crate::error::{Code, Error, Result};

/// Window class of the main window; also how tests find it.
pub const CLASS: &str = "kaltstellen-main";
const WM_WAKE: u32 = WM_APP + 2;

/// Kinds of controls.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Kind {
    Label,
    Button,
    Edit,
    /// Single selection; sends [`Event::Selected`].
    List,
    /// Report-style list view with these columns (title, width).
    Table(Vec<(String, i32)>),
    /// Drop-down list.
    Choice,
}

/// One control: ID, kind, text, and its place at 96 DPI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Control {
    pub id: u16,
    pub kind: Kind,
    pub text: String,
    pub rect: (i32, i32, i32, i32),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Event {
    Clicked(u16),
    Selected(u16),
    /// A [`Waker`] fired.
    Woken,
    /// The user closes the window.
    Closing,
}

/// The safe side of the window.
pub trait Form {
    fn event(&mut self, ui: &Ui, event: Event);
}

/// Wakes the window thread from another thread.
#[derive(Debug, Clone)]
pub struct Waker(usize);

impl Waker {
    pub fn wake(&self) {
        // SAFETY: posting to a window that is gone fails harmlessly.
        let _ =
            unsafe { PostMessageW(Some(HWND(self.0 as *mut _)), WM_WAKE, WPARAM(0), LPARAM(0)) };
    }
}

/// Access to the controls, for the form, on the window thread.
pub struct Ui {
    hwnd: HWND,
}

impl Ui {
    fn item(&self, id: u16) -> Option<HWND> {
        // SAFETY: plain lookup of a child of our window.
        unsafe { GetDlgItem(Some(self.hwnd), i32::from(id)) }.ok()
    }

    fn send(&self, id: u16, msg: u32, wparam: usize, lparam: isize) -> isize {
        match self.item(id) {
            // SAFETY: a control of this thread's window; pointers passed in
            // `lparam` by the callers below live across the call.
            Some(hwnd) => {
                unsafe { SendMessageW(hwnd, msg, Some(WPARAM(wparam)), Some(LPARAM(lparam))) }.0
            }
            None => -1,
        }
    }

    pub fn waker(&self) -> Waker {
        Waker(self.hwnd.0 as usize)
    }

    pub fn text(&self, id: u16) -> String {
        let Some(hwnd) = self.item(id) else {
            return String::new();
        };
        // SAFETY: the buffer is sized from the length query.
        unsafe {
            let len = GetWindowTextLengthW(hwnd).max(0) as usize;
            let mut buffer = vec![0u16; len + 1];
            let n = GetWindowTextW(hwnd, &mut buffer).max(0) as usize;
            String::from_utf16_lossy(&buffer[..n])
        }
    }

    pub fn set_text(&self, id: u16, text: &str) {
        if let Some(hwnd) = self.item(id) {
            let text = wide(text);
            // SAFETY: NUL-terminated text.
            let _ = unsafe { SetWindowTextW(hwnd, PCWSTR(text.as_ptr())) };
        }
    }

    /// Fills a list box and selects `selected`.
    pub fn set_list(&self, id: u16, items: &[String], selected: Option<usize>) {
        self.send(id, LB_RESETCONTENT, 0, 0);
        for item in items {
            let text = wide(item);
            self.send(id, LB_ADDSTRING, 0, text.as_ptr() as isize);
        }
        self.send(id, LB_SETCURSEL, selected.unwrap_or(usize::MAX), 0);
    }

    pub fn list_selection(&self, id: u16) -> Option<usize> {
        usize::try_from(self.send(id, LB_GETCURSEL, 0, 0)).ok()
    }

    /// Fills a drop-down list and selects `selected`.
    pub fn set_choices(&self, id: u16, items: &[&str], selected: Option<usize>) {
        self.send(id, CB_RESETCONTENT, 0, 0);
        for item in items {
            let text = wide(item);
            self.send(id, CB_ADDSTRING, 0, text.as_ptr() as isize);
        }
        self.send(id, CB_SETCURSEL, selected.unwrap_or(usize::MAX), 0);
    }

    pub fn choice(&self, id: u16) -> Option<usize> {
        usize::try_from(self.send(id, CB_GETCURSEL, 0, 0)).ok()
    }

    /// Replaces the rows of a table.
    pub fn set_rows(&self, id: u16, rows: &[Vec<String>]) {
        self.send(id, LVM_DELETEALLITEMS, 0, 0);
        for (n, row) in rows.iter().enumerate() {
            for (column, cell) in row.iter().enumerate() {
                let mut text = wide(cell);
                let mut item = LVITEMW {
                    mask: LVIF_TEXT,
                    iItem: n as i32,
                    iSubItem: column as i32,
                    pszText: PWSTR(text.as_mut_ptr()),
                    ..Default::default()
                };
                let msg = if column == 0 {
                    LVM_INSERTITEMW
                } else {
                    LVM_SETITEMTEXTW
                };
                self.send(id, msg, n, &raw mut item as isize);
            }
        }
    }

    pub fn row_selection(&self, id: u16) -> Option<usize> {
        usize::try_from(self.send(id, LVM_GETNEXTITEM, usize::MAX, LVNI_SELECTED as isize)).ok()
    }

    /// A yes/no question; true for yes.
    pub fn ask(&self, title: &str, question: &str) -> bool {
        let (text, title) = (wide(question), wide(title));
        // SAFETY: NUL-terminated texts; modal on our window.
        let answer = unsafe {
            MessageBoxW(
                Some(self.hwnd),
                PCWSTR(text.as_ptr()),
                PCWSTR(title.as_ptr()),
                MB_YESNO | MB_ICONQUESTION,
            )
        };
        answer == IDYES
    }

    pub fn tell(&self, title: &str, message: &str) {
        let (text, title) = (wide(message), wide(title));
        // SAFETY: as in `ask`.
        let _ = unsafe {
            MessageBoxW(
                Some(self.hwnd),
                PCWSTR(text.as_ptr()),
                PCWSTR(title.as_ptr()),
                MB_OK | MB_ICONWARNING,
            )
        };
    }

    /// The standard "open file" dialog for executables.
    pub fn pick_executable(&self) -> Option<PathBuf> {
        let mut buffer = vec![0u16; 1024];
        let filter: Vec<u16> = "Programs (*.exe)\0*.exe\0\0".encode_utf16().collect();
        let mut dialog = OPENFILENAMEW {
            lStructSize: std::mem::size_of::<OPENFILENAMEW>() as u32,
            hwndOwner: self.hwnd,
            lpstrFilter: PCWSTR(filter.as_ptr()),
            lpstrFile: PWSTR(buffer.as_mut_ptr()),
            nMaxFile: buffer.len() as u32,
            Flags: OFN_FILEMUSTEXIST | OFN_PATHMUSTEXIST | OFN_NOCHANGEDIR,
            ..Default::default()
        };
        // SAFETY: all buffers live across the modal call.
        if !unsafe { GetOpenFileNameW(&mut dialog) }.as_bool() {
            return None;
        }
        let end = buffer.iter().position(|&u| u == 0).unwrap_or(buffer.len());
        Some(PathBuf::from(String::from_utf16_lossy(&buffer[..end])))
    }

    /// Closes the window; the message loop ends.
    pub fn close(&self) {
        // SAFETY: our own window, on its thread.
        let _ = unsafe { DestroyWindow(self.hwnd) };
    }
}

struct State {
    form: Box<dyn Form>,
    controls: Vec<Control>,
    font: HFONT,
}

thread_local! {
    static STATE: RefCell<Option<State>> = const { RefCell::new(None) };
}

fn scale(value: i32, dpi: u32) -> i32 {
    value * dpi as i32 / 96
}

/// Message font of the system at `dpi`.
fn message_font(dpi: u32) -> HFONT {
    let mut metrics = NONCLIENTMETRICSW {
        cbSize: std::mem::size_of::<NONCLIENTMETRICSW>() as u32,
        ..Default::default()
    };
    // SAFETY: the struct is sized; the font handle is owned by the caller.
    unsafe {
        let _ = SystemParametersInfoForDpi(
            SPI_GETNONCLIENTMETRICS.0,
            metrics.cbSize,
            Some((&raw mut metrics).cast()),
            0,
            dpi,
        );
        CreateFontIndirectW(&metrics.lfMessageFont)
    }
}

/// Places every control for `dpi` and gives it the message font.
fn layout(hwnd: HWND, controls: &[Control], font: HFONT, dpi: u32) {
    for control in controls {
        // SAFETY: children of our window.
        unsafe {
            if let Ok(child) = GetDlgItem(Some(hwnd), i32::from(control.id)) {
                let (x, y, w, h) = control.rect;
                let _ = SetWindowPos(
                    child,
                    None,
                    scale(x, dpi),
                    scale(y, dpi),
                    scale(w, dpi),
                    scale(h, dpi),
                    SWP_NOZORDER | SWP_NOACTIVATE,
                );
                SendMessageW(
                    child,
                    WM_SETFONT,
                    Some(WPARAM(font.0 as usize)),
                    Some(LPARAM(1)),
                );
            }
        }
    }
}

/// Creates the window with `controls`, client size `size` at 96 DPI, hands
/// a [`Ui`] to `make`, and runs the message loop until the window closes.
pub fn run(
    title: &str,
    size: (i32, i32),
    controls: Vec<Control>,
    make: impl FnOnce(&Ui) -> Box<dyn Form>,
) -> Result<()> {
    let fail = |what: &str, err: windows::core::Error| {
        Error::new(Code::GuiWindow, format!("cannot {what}: {err}"))
    };
    let class = wide(CLASS);
    let title = wide(title);
    // SAFETY: pointers passed below point to live NUL-terminated buffers.
    let hwnd = unsafe {
        let icc = INITCOMMONCONTROLSEX {
            dwSize: std::mem::size_of::<INITCOMMONCONTROLSEX>() as u32,
            dwICC: ICC_STANDARD_CLASSES | ICC_LISTVIEW_CLASSES,
        };
        let _ = InitCommonControlsEx(&icc);
        let instance = GetModuleHandleW(None).map_err(|e| fail("get the module handle", e))?;
        let wc = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(window_proc),
            hInstance: instance.into(),
            lpszClassName: PCWSTR(class.as_ptr()),
            // COLOR_BTNFACE + 1: the dialog background.
            hbrBackground: windows::Win32::Graphics::Gdi::HBRUSH(16 as *mut _),
            ..Default::default()
        };
        if RegisterClassExW(&wc) == 0 {
            return Err(fail(
                "register the window class",
                windows::core::Error::from_thread(),
            ));
        }
        let hwnd = CreateWindowExW(
            WINDOW_EX_STYLE(0),
            PCWSTR(class.as_ptr()),
            PCWSTR(title.as_ptr()),
            WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU | WS_MINIMIZEBOX,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            size.0,
            size.1,
            None,
            None,
            Some(instance.into()),
            None,
        )
        .map_err(|e| fail("create the main window", e))?;
        for control in &controls {
            let (class, style, ex) = match &control.kind {
                Kind::Label => ("STATIC", WINDOW_STYLE(0), WINDOW_EX_STYLE(0)),
                Kind::Button => (
                    "BUTTON",
                    WS_TABSTOP | WINDOW_STYLE(BS_PUSHBUTTON as u32),
                    WINDOW_EX_STYLE(0),
                ),
                Kind::Edit => (
                    "EDIT",
                    WS_TABSTOP | WS_BORDER | WINDOW_STYLE(ES_AUTOHSCROLL as u32),
                    WINDOW_EX_STYLE(0),
                ),
                Kind::List => (
                    "LISTBOX",
                    WS_TABSTOP | WS_VSCROLL | WS_BORDER | WINDOW_STYLE(LBS_NOTIFY as u32),
                    WINDOW_EX_STYLE(0),
                ),
                Kind::Table(_) => (
                    "SysListView32",
                    WS_TABSTOP | WINDOW_STYLE(LVS_REPORT | LVS_SINGLESEL | LVS_SHOWSELALWAYS),
                    WS_EX_CLIENTEDGE,
                ),
                Kind::Choice => (
                    "COMBOBOX",
                    WS_TABSTOP | WS_VSCROLL | WINDOW_STYLE(CBS_DROPDOWNLIST as u32),
                    WINDOW_EX_STYLE(0),
                ),
            };
            let class = wide(class);
            let text = wide(&control.text);
            let (_, _, w, h) = control.rect;
            // Drop-down lists need room for the open list.
            let h = if control.kind == Kind::Choice {
                h * 8
            } else {
                h
            };
            let child = CreateWindowExW(
                ex,
                PCWSTR(class.as_ptr()),
                PCWSTR(text.as_ptr()),
                WS_CHILD | WS_VISIBLE | style,
                0,
                0,
                w,
                h,
                Some(hwnd),
                Some(HMENU(usize::from(control.id) as *mut _)),
                Some(instance.into()),
                None,
            )
            .map_err(|e| fail("create a control", e))?;
            if let Kind::Table(columns) = &control.kind {
                SendMessageW(
                    child,
                    LVM_SETEXTENDEDLISTVIEWSTYLE,
                    Some(WPARAM(LVS_EX_FULLROWSELECT as usize)),
                    Some(LPARAM(LVS_EX_FULLROWSELECT as isize)),
                );
                let dpi = GetDpiForWindow(hwnd);
                for (n, (title, width)) in columns.iter().enumerate() {
                    let mut text = wide(title);
                    let column = LVCOLUMNW {
                        mask: LVCF_TEXT | LVCF_WIDTH,
                        cx: scale(*width, dpi),
                        pszText: PWSTR(text.as_mut_ptr()),
                        ..Default::default()
                    };
                    SendMessageW(
                        child,
                        LVM_INSERTCOLUMNW,
                        Some(WPARAM(n)),
                        Some(LPARAM(&raw const column as isize)),
                    );
                }
            }
        }
        hwnd
    };
    // SAFETY: our window.
    let dpi = unsafe { GetDpiForWindow(hwnd) };
    let font = message_font(dpi);
    layout(hwnd, &controls, font, dpi);
    // Client size for this DPI.
    resize(hwnd, size, dpi);
    let ui = Ui { hwnd };
    let form = make(&ui);
    STATE.set(Some(State {
        form,
        controls,
        font,
    }));
    // SAFETY: our window; standard message loop with dialog keyboard
    // navigation (Tab between controls).
    unsafe {
        let _ = ShowWindow(hwnd, SW_SHOW);
        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            if !IsDialogMessageW(hwnd, &msg).as_bool() {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
    }
    if let Some(state) = STATE.take() {
        // SAFETY: the font was created above and is no longer in use.
        let _ = unsafe { DeleteObject(HGDIOBJ(state.font.0)) };
    }
    Ok(())
}

fn resize(hwnd: HWND, size: (i32, i32), dpi: u32) {
    let mut rect = RECT {
        left: 0,
        top: 0,
        right: scale(size.0, dpi),
        bottom: scale(size.1, dpi),
    };
    // SAFETY: valid rect; our window.
    unsafe {
        let style = WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU | WS_MINIMIZEBOX;
        let _ = windows::Win32::UI::HiDpi::AdjustWindowRectExForDpi(
            &mut rect,
            style,
            false,
            WINDOW_EX_STYLE(0),
            dpi,
        );
        let _ = SetWindowPos(
            hwnd,
            None,
            0,
            0,
            rect.right - rect.left,
            rect.bottom - rect.top,
            windows::Win32::UI::WindowsAndMessaging::SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE,
        );
    }
}

fn dispatch(hwnd: HWND, event: Event) {
    // The form is taken out while it runs, so a nested message loop (a
    // message box) cannot reach it twice; such events are dropped.
    let Some(mut state) = STATE.with_borrow_mut(Option::take) else {
        return;
    };
    state.form.event(&Ui { hwnd }, event);
    STATE.with_borrow_mut(|slot| {
        if slot.is_none() {
            *slot = Some(state);
        }
    });
}

extern "system" fn window_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_COMMAND => {
            let id = (wparam.0 & 0xFFFF) as u16;
            let code = ((wparam.0 >> 16) & 0xFFFF) as u32;
            if lparam.0 != 0 {
                if code == 0 {
                    dispatch(hwnd, Event::Clicked(id));
                } else if code == LBN_SELCHANGE {
                    dispatch(hwnd, Event::Selected(id));
                }
            }
            LRESULT(0)
        }
        WM_WAKE => {
            dispatch(hwnd, Event::Woken);
            LRESULT(0)
        }
        WM_DPICHANGED => {
            // Moved to a monitor with another scaling: new font, new places.
            let dpi = (wparam.0 & 0xFFFF) as u32;
            STATE.with_borrow_mut(|state| {
                if let Some(state) = state {
                    let old = state.font;
                    state.font = message_font(dpi);
                    layout(hwnd, &state.controls, state.font, dpi);
                    // SAFETY: the old font is no longer selected anywhere.
                    let _ = unsafe { DeleteObject(HGDIOBJ(old.0)) };
                }
            });
            // SAFETY: lParam points to the suggested rectangle.
            let rect = unsafe { &*(lparam.0 as *const RECT) };
            // SAFETY: our window.
            let _ = unsafe {
                SetWindowPos(
                    hwnd,
                    None,
                    rect.left,
                    rect.top,
                    rect.right - rect.left,
                    rect.bottom - rect.top,
                    SWP_NOZORDER | SWP_NOACTIVATE,
                )
            };
            LRESULT(0)
        }
        WM_CLOSE => {
            dispatch(hwnd, Event::Closing);
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
