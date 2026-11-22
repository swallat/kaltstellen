//! Thin safe wrapper around the Win32 process APIs kaltstellen needs.
//!
//! All `unsafe` in the crate lives in this module.

use anyhow::{Context, Result, bail};
use windows::Win32::Foundation::{CloseHandle, HANDLE, NTSTATUS};
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS,
};
use windows::Win32::System::ProcessStatus::EmptyWorkingSet;
use windows::Win32::System::Threading::{
    OpenProcess, PROCESS_ACCESS_RIGHTS, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SET_QUOTA,
    PROCESS_SUSPEND_RESUME,
};

// Not exposed by the Win32 metadata, but stable since Windows XP and what
// Process Explorer / PsSuspend use as well.
#[link(name = "ntdll")]
unsafe extern "system" {
    fn NtSuspendProcess(process: HANDLE) -> NTSTATUS;
    fn NtResumeProcess(process: HANDLE) -> NTSTATUS;
}

/// Processes that must never be frozen: doing so hangs the session or the
/// whole machine.
const PROTECTED: &[&str] = &[
    "system",
    "registry",
    "smss.exe",
    "csrss.exe",
    "wininit.exe",
    "winlogon.exe",
    "services.exe",
    "lsass.exe",
    "svchost.exe",
    "fontdrvhost.exe",
    "dwm.exe",
    "explorer.exe",
    "conhost.exe",
    "openconsole.exe",
    "werfault.exe",
];

#[derive(Debug, Clone)]
pub struct ProcessInfo {
    pub pid: u32,
    pub parent_pid: u32,
    pub name: String,
}

/// Closes the wrapped handle on drop.
struct Handle(HANDLE);

impl Drop for Handle {
    fn drop(&mut self) {
        // SAFETY: the handle was returned by a successful Win32 call and is
        // closed exactly once.
        let _ = unsafe { CloseHandle(self.0) };
    }
}

fn open(pid: u32, access: PROCESS_ACCESS_RIGHTS) -> Result<Handle> {
    // SAFETY: plain FFI call without pointer arguments.
    let handle = unsafe { OpenProcess(access, false, pid) }
        .with_context(|| format!("cannot open process {pid}"))?;
    Ok(Handle(handle))
}

/// Snapshot of all running processes.
pub fn list() -> Result<Vec<ProcessInfo>> {
    // SAFETY: plain FFI call without pointer arguments.
    let snapshot = Handle(
        unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) }
            .context("cannot take process snapshot")?,
    );
    let mut entry = PROCESSENTRY32W {
        dwSize: size_of::<PROCESSENTRY32W>() as u32,
        ..Default::default()
    };
    let mut processes = Vec::new();

    // SAFETY: `entry` is a valid PROCESSENTRY32W with dwSize set.
    unsafe { Process32FirstW(snapshot.0, &mut entry) }.context("cannot read process snapshot")?;
    loop {
        let len = entry
            .szExeFile
            .iter()
            .position(|&c| c == 0)
            .unwrap_or(entry.szExeFile.len());
        processes.push(ProcessInfo {
            pid: entry.th32ProcessID,
            parent_pid: entry.th32ParentProcessID,
            name: String::from_utf16_lossy(&entry.szExeFile[..len]),
        });
        // SAFETY: as above. An error here means "no more entries".
        if unsafe { Process32NextW(snapshot.0, &mut entry) }.is_err() {
            break;
        }
    }
    Ok(processes)
}

/// All processes whose executable name equals `name` (case-insensitive, the
/// `.exe` suffix is optional), excluding kaltstellen itself.
pub fn find(name: &str) -> Result<Vec<ProcessInfo>> {
    let wanted = if name.to_ascii_lowercase().ends_with(".exe") {
        name.to_owned()
    } else {
        format!("{name}.exe")
    };
    if is_hard_protected(name) || is_hard_protected(&wanted) {
        bail!("{name} is a protected system process and will not be touched");
    }
    let own_pid = std::process::id();
    Ok(list()?
        .into_iter()
        .filter(|p| p.pid != own_pid && p.name.eq_ignore_ascii_case(&wanted))
        .collect())
}

/// Whether the executable name is on the hard-protected list (spec P6).
/// Expects the name with its extension; the comparison ignores case.
pub fn is_hard_protected(name: &str) -> bool {
    PROTECTED.iter().any(|p| p.eq_ignore_ascii_case(name))
}

/// Freezes every thread of the process. Each call raises the suspend count,
/// so it must be paired with exactly one [`resume`].
pub fn suspend(pid: u32) -> Result<()> {
    let handle = open(pid, PROCESS_SUSPEND_RESUME)?;
    // SAFETY: `handle` is a live process handle with PROCESS_SUSPEND_RESUME.
    unsafe { NtSuspendProcess(handle.0) }
        .ok()
        .with_context(|| format!("cannot suspend process {pid}"))
}

/// Undoes one [`suspend`].
pub fn resume(pid: u32) -> Result<()> {
    let handle = open(pid, PROCESS_SUSPEND_RESUME)?;
    // SAFETY: `handle` is a live process handle with PROCESS_SUSPEND_RESUME.
    unsafe { NtResumeProcess(handle.0) }
        .ok()
        .with_context(|| format!("cannot resume process {pid}"))
}

/// Asks Windows to move the process's pages out of physical memory. On a
/// suspended process they stay out, because nothing touches them again.
pub fn trim_working_set(pid: u32) -> Result<()> {
    let handle = open(pid, PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SET_QUOTA)?;
    // SAFETY: `handle` is a live process handle with the required rights.
    unsafe { EmptyWorkingSet(handle.0) }
        .with_context(|| format!("cannot trim working set of process {pid}"))
}
