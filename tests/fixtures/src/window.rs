//! A minimal top-level window with configurable close behaviour.

use std::path::PathBuf;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::Graphics::Gdi::{COLOR_WINDOW, HBRUSH};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::{
    CS_HREDRAW, CS_VREDRAW, CW_USEDEFAULT, CreateWindowExW, DefWindowProcW, DestroyWindow,
    DispatchMessageW, GetMessageW, IDYES, KillTimer, MB_ICONWARNING, MB_YESNO, MSG, MessageBoxW,
    PostQuitMessage, RegisterClassW, SW_SHOWNORMAL, SetTimer, ShowWindow, TranslateMessage,
    WINDOW_EX_STYLE, WM_CLOSE, WM_DESTROY, WM_TIMER, WNDCLASSW, WS_OVERLAPPEDWINDOW,
};
use windows::core::{HSTRING, PCWSTR, w};

/// What the window does when it receives `WM_CLOSE`.
#[derive(Debug, Clone, Copy)]
pub enum OnClose {
    /// Close immediately.
    Close,
    /// Close after the given delay in milliseconds (slow stop).
    Delay(u32),
    /// Ask "unsaved changes, close anyway?" with Yes/No; stays open unless
    /// someone answers Yes. The box has no Cancel button, so it ignores
    /// `WM_CLOSE` itself, like real unsaved-changes dialogs.
    Confirm,
    /// Close once the file in [`CLOSE_WHEN`] exists (the test decides when
    /// the stop ends), at the latest after [`GATE_LIMIT`], so a broken test
    /// cannot keep it open for ever.
    WhenFile,
}

/// The file of [`OnClose::WhenFile`].
pub static CLOSE_WHEN: OnceLock<PathBuf> = OnceLock::new();
/// Upper bound for [`OnClose::WhenFile`].
const GATE_LIMIT: Duration = Duration::from_secs(60);
static CLOSE_ASKED: OnceLock<Instant> = OnceLock::new();
const GATE_TIMER: usize = 2;

static ON_CLOSE: OnceLock<OnClose> = OnceLock::new();
const CLOSE_TIMER: usize = 1;

/// Creates the window and runs the message loop until it is destroyed.
pub fn run(title: &str, on_close: OnClose) {
    ON_CLOSE.set(on_close).expect("window::run called twice");
    // SAFETY: standard Win32 window creation; all pointers outlive the calls.
    unsafe {
        let instance = GetModuleHandleW(None).expect("GetModuleHandleW");
        let class = w!("ks_fixture_window");
        let wc = WNDCLASSW {
            style: CS_HREDRAW | CS_VREDRAW,
            lpfnWndProc: Some(wndproc),
            hInstance: instance.into(),
            lpszClassName: class,
            hbrBackground: HBRUSH((COLOR_WINDOW.0 + 1) as isize as *mut _),
            ..Default::default()
        };
        assert!(RegisterClassW(&wc) != 0, "RegisterClassW failed");
        let title = HSTRING::from(title);
        let hwnd = CreateWindowExW(
            WINDOW_EX_STYLE::default(),
            class,
            PCWSTR(title.as_ptr()),
            WS_OVERLAPPEDWINDOW,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            480,
            240,
            None,
            None,
            Some(instance.into()),
            None,
        )
        .expect("CreateWindowExW");
        let _ = ShowWindow(hwnd, SW_SHOWNORMAL);

        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}

extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    // SAFETY: called by Windows with a valid window handle.
    unsafe {
        match msg {
            WM_CLOSE => {
                match ON_CLOSE.get().copied().unwrap_or(OnClose::Close) {
                    OnClose::Close => {
                        let _ = DestroyWindow(hwnd);
                    }
                    OnClose::Delay(ms) => {
                        SetTimer(Some(hwnd), CLOSE_TIMER, ms, None);
                    }
                    OnClose::WhenFile => {
                        let _ = CLOSE_ASKED.set(Instant::now());
                        // Tells the test that the stop has begun.
                        if let Some(file) = CLOSE_WHEN.get() {
                            let _ = std::fs::write(closing_marker(file), b"");
                        }
                        SetTimer(Some(hwnd), GATE_TIMER, 50, None);
                    }
                    OnClose::Confirm => {
                        let answer = MessageBoxW(
                            Some(hwnd),
                            w!("There are unsaved changes. Close anyway?"),
                            w!("ks-dialog"),
                            MB_YESNO | MB_ICONWARNING,
                        );
                        if answer == IDYES {
                            let _ = DestroyWindow(hwnd);
                        }
                    }
                }
                LRESULT(0)
            }
            WM_TIMER if wparam.0 == GATE_TIMER => {
                let open = CLOSE_WHEN.get().is_some_and(|f| !f.exists())
                    && CLOSE_ASKED.get().is_some_and(|t| t.elapsed() < GATE_LIMIT);
                if !open {
                    let _ = KillTimer(Some(hwnd), GATE_TIMER);
                    let _ = DestroyWindow(hwnd);
                }
                LRESULT(0)
            }
            WM_TIMER if wparam.0 == CLOSE_TIMER => {
                let _ = KillTimer(Some(hwnd), CLOSE_TIMER);
                let _ = DestroyWindow(hwnd);
                LRESULT(0)
            }
            WM_DESTROY => {
                PostQuitMessage(0);
                LRESULT(0)
            }
            _ => DefWindowProcW(hwnd, msg, wparam, lparam),
        }
    }
}

/// The file [`OnClose::WhenFile`] writes when `WM_CLOSE` arrives: the gate
/// file's name with `.closing` appended.
pub fn closing_marker(gate: &std::path::Path) -> PathBuf {
    let mut name = gate.as_os_str().to_owned();
    name.push(".closing");
    PathBuf::from(name)
}
