//! Console handling.
//!
//! - The executable is a console program, so Windows gives it a console
//!   window when it is started from Explorer or a shortcut. The tray lets go
//!   of that window; a console it shares with a terminal stays as it is.
//! - S5 for the command line: while an in-process switch runs, Ctrl+C,
//!   Ctrl+Break and closing the console window do not kill the process
//!   half-way; they are turned into an interrupt request the switch checks.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Condvar, Mutex};
use std::time::Duration;

use windows::Win32::System::Console::{
    CTRL_BREAK_EVENT, CTRL_C_EVENT, CTRL_CLOSE_EVENT, CTRL_LOGOFF_EVENT, CTRL_SHUTDOWN_EVENT,
    FreeConsole, GetConsoleProcessList, SetConsoleCtrlHandler,
};
use windows::core::BOOL;

use crate::error::{Code, Error, Result};

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

/// How long the handler of a closing console holds the process open while
/// the switch winds down. Windows ends the process about 5 s after the
/// close event in any case.
const CLOSE_GRACE: Duration = Duration::from_millis(4500);

static INTERRUPTED: AtomicBool = AtomicBool::new(false);
static FINISHED: Mutex<bool> = Mutex::new(false);
static FINISHED_CHANGED: Condvar = Condvar::new();

unsafe extern "system" fn handler(event: u32) -> BOOL {
    match event {
        CTRL_C_EVENT | CTRL_BREAK_EVENT => {
            INTERRUPTED.store(true, Ordering::SeqCst);
            // Handled: the process goes on and winds the switch down.
            true.into()
        }
        CTRL_CLOSE_EVENT | CTRL_LOGOFF_EVENT | CTRL_SHUTDOWN_EVENT => {
            INTERRUPTED.store(true, Ordering::SeqCst);
            // The process ends when this returns: wait for the wind-down.
            let finished = FINISHED.lock().unwrap_or_else(|e| e.into_inner());
            let _ = FINISHED_CHANGED.wait_timeout_while(finished, CLOSE_GRACE, |done| !*done);
            true.into()
        }
        _ => false.into(),
    }
}

/// Installed while alive: console control events become interrupt requests
/// instead of ending the process.
pub struct Interrupts(());

impl Interrupts {
    pub fn catch() -> Result<Interrupts> {
        INTERRUPTED.store(false, Ordering::SeqCst);
        *FINISHED.lock().unwrap_or_else(|e| e.into_inner()) = false;
        // SAFETY: `handler` is a valid handler routine for the whole process
        // lifetime; it only touches statics.
        unsafe { SetConsoleCtrlHandler(Some(handler), true) }.map_err(|e| {
            Error::new(
                Code::ProcessOs,
                format!("cannot install the console handler: {e}"),
            )
        })?;
        Ok(Interrupts(()))
    }

    /// Whether Ctrl+C, Ctrl+Break or a console close arrived.
    pub fn requested(&self) -> bool {
        INTERRUPTED.load(Ordering::SeqCst)
    }

    /// The switch has wound down: a waiting close handler may return.
    pub fn finished(&self) {
        *FINISHED.lock().unwrap_or_else(|e| e.into_inner()) = true;
        FINISHED_CHANGED.notify_all();
    }
}

impl Drop for Interrupts {
    fn drop(&mut self) {
        self.finished();
        // SAFETY: removes the handler installed in `catch`.
        let _ = unsafe { SetConsoleCtrlHandler(Some(handler), false) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn s5_handler_turns_ctrl_c_into_a_request() {
        let interrupts = Interrupts::catch().unwrap();
        assert!(!interrupts.requested());
        // SAFETY: calls the handler directly, as the console would.
        assert!(unsafe { handler(CTRL_C_EVENT) }.as_bool());
        assert!(interrupts.requested());
        // A close waits for the wind-down, then lets the process end.
        let closer = std::thread::spawn(|| unsafe { handler(CTRL_CLOSE_EVENT) }.as_bool());
        std::thread::sleep(Duration::from_millis(100));
        assert!(!closer.is_finished(), "the close handler waits");
        interrupts.finished();
        assert!(closer.join().unwrap());
        drop(interrupts);
    }
}
