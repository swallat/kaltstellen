//! The executable is a console program, so Windows gives it a console window
//! when it is started from Explorer or a shortcut. The tray lets go of that
//! window; a console it shares with a terminal stays as it is.

use windows::Win32::System::Console::{FreeConsole, GetConsoleProcessList};

/// Detaches from the console if this process is the only one attached to it,
/// which means Windows created the console for this process alone (Explorer,
/// shortcut, Run key). Returns whether it detached. The console window then
/// closes; it may have been visible for a moment.
pub fn detach_if_own() -> bool {
    let mut pids = [0u32; 4];
    // SAFETY: the buffer is valid for its whole length.
    let count = unsafe { GetConsoleProcessList(&mut pids) };
    if count != 1 {
        // 0: no console at all; more: a terminal or another program shares it.
        return false;
    }
    // SAFETY: plain call without arguments. Rust's standard streams treat
    // the now invalid handles as closed and drop further output.
    unsafe { FreeConsole() }.is_ok()
}
