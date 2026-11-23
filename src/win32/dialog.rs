//! The standard "open file" dialog, for choosing an executable in the main
//! window.

use std::path::PathBuf;

use windows::Win32::Foundation::HWND;
use windows::Win32::UI::Controls::Dialogs::{
    GetOpenFileNameW, OFN_FILEMUSTEXIST, OFN_NOCHANGEDIR, OFN_PATHMUSTEXIST, OPENFILENAMEW,
};
use windows::core::{PCWSTR, PWSTR};

/// A yes/no question in a message box of its own (no owner window), on the
/// calling thread. True for yes.
pub fn ask_yes_no(title: &str, question: &str) -> bool {
    use windows::Win32::UI::WindowsAndMessaging::{
        IDYES, MB_ICONQUESTION, MB_SETFOREGROUND, MB_YESNO, MessageBoxW,
    };
    let title: Vec<u16> = title.encode_utf16().chain(Some(0)).collect();
    let question: Vec<u16> = question.encode_utf16().chain(Some(0)).collect();
    // SAFETY: NUL-terminated texts that live across the modal call.
    let answer = unsafe {
        MessageBoxW(
            None,
            PCWSTR(question.as_ptr()),
            PCWSTR(title.as_ptr()),
            MB_YESNO | MB_ICONQUESTION | MB_SETFOREGROUND,
        )
    };
    answer == IDYES
}

/// Shows the dialog for `.exe` files, owned by the window `owner` (a value
/// from `webview::Waker::window`). `None` if the user cancels.
pub fn pick_executable(owner: usize) -> Option<PathBuf> {
    let mut buffer = vec![0u16; 1024];
    let filter: Vec<u16> = "Programs (*.exe)\0*.exe\0\0".encode_utf16().collect();
    let mut dialog = OPENFILENAMEW {
        lStructSize: std::mem::size_of::<OPENFILENAMEW>() as u32,
        hwndOwner: HWND(owner as *mut _),
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
