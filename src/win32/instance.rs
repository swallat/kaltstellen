//! L1: one tray instance per user session, enforced with a named mutex in
//! the session's `Local\` namespace.

use windows::Win32::Foundation::{CloseHandle, ERROR_ALREADY_EXISTS, GetLastError, HANDLE};
use windows::Win32::System::Threading::CreateMutexW;
use windows::core::PCWSTR;

use super::wide;
use crate::error::{Code, Error, Result};

/// Held while this process is the instance. The mutex is released when the
/// value is dropped, and by Windows if the process dies.
#[derive(Debug)]
pub struct Instance(HANDLE);

// The handle is only closed, from whichever thread drops the guard.
unsafe impl Send for Instance {}

impl Instance {
    /// `Ok(None)` if another process holds the mutex `name`.
    pub fn acquire(name: &str) -> Result<Option<Instance>> {
        let name = wide(name);
        // SAFETY: `name` is NUL-terminated and outlives the call.
        let handle = unsafe { CreateMutexW(None, true, PCWSTR(name.as_ptr())) }
            .map_err(|err| Error::new(Code::TrayInstance, format!("cannot create mutex: {err}")))?;
        // SAFETY: no call in between could have changed the thread's last
        // error; CreateMutexW sets it on success too.
        if unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
            // SAFETY: the handle was just returned and is closed once.
            let _ = unsafe { CloseHandle(handle) };
            return Ok(None);
        }
        Ok(Some(Instance(handle)))
    }
}

impl Drop for Instance {
    fn drop(&mut self) {
        // SAFETY: the handle is owned by this value and closed exactly once.
        let _ = unsafe { CloseHandle(self.0) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn l1_second_acquire_fails_until_the_first_is_dropped() {
        let name = format!("Local\\kaltstellen-test-{}", std::process::id());
        let first = Instance::acquire(&name).unwrap().expect("first instance");
        // The mutex is owned by this thread, but a second CreateMutexW still
        // reports that it exists, which is what a second process sees.
        assert!(Instance::acquire(&name).unwrap().is_none());
        drop(first);
        assert!(Instance::acquire(&name).unwrap().is_some());
    }
}
