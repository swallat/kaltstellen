//! Safe wrapper around the Win32 process APIs kaltstellen needs (WP2).
//!
//! All `unsafe` in the crate lives in this module (spec Q4).
//!
//! Least access (A7): taking a [`Snapshot`] opens no process at all. A
//! process is opened only through [`Target::open`] or one of the functions
//! that take the processes of a matched program.

use std::collections::HashSet;
use std::ffi::{OsStr, c_void};
use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use windows::Win32::Foundation::{
    CloseHandle, ERROR_ACCESS_DENIED, ERROR_ELEVATION_REQUIRED, ERROR_INVALID_PARAMETER, FILETIME,
    HANDLE, HWND, LPARAM, NTSTATUS, WAIT_OBJECT_0, WPARAM,
};
use windows::Win32::System::ProcessStatus::EmptyWorkingSet;
use windows::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
use windows::Win32::System::Threading::{
    CREATE_NEW_CONSOLE, CreateProcessW, GetExitCodeProcess, GetProcessInformation, GetProcessTimes,
    OpenProcess, PROCESS_ACCESS_RIGHTS, PROCESS_INFORMATION, PROCESS_NAME_WIN32,
    PROCESS_PROTECTION_LEVEL_INFORMATION, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SET_QUOTA,
    PROCESS_SUSPEND_RESUME, PROCESS_SYNCHRONIZE, PROCESS_TERMINATE, ProcessProtectionLevelInfo,
    QueryFullProcessImageNameW, STARTUPINFOW, TerminateProcess, WaitForSingleObject,
};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetWindowThreadProcessId, IsIconic, IsWindowVisible, PostMessageW, SW_MINIMIZE,
    ShowWindowAsync, WM_CLOSE,
};
use windows::core::{BOOL, PCWSTR, PWSTR};

use crate::config::OnTimeout;
use crate::error::{Code, Error, Result, Step};
use crate::protection;

// Not exposed by the Win32 metadata, but stable since Windows XP and used by
// well-known system tools (spec R6).
#[link(name = "ntdll")]
unsafe extern "system" {
    fn NtSuspendProcess(process: HANDLE) -> NTSTATUS;
    fn NtResumeProcess(process: HANDLE) -> NTSTATUS;
    fn NtQuerySystemInformation(
        class: u32,
        info: *mut c_void,
        length: u32,
        return_length: *mut u32,
    ) -> NTSTATUS;
}

const SYSTEM_PROCESS_INFORMATION: u32 = 5;
const STATUS_INFO_LENGTH_MISMATCH: NTSTATUS = NTSTATUS(0xC000_0004_u32 as i32);
const STILL_ACTIVE: u32 = 259;
const PROTECTION_LEVEL_NONE: u32 = 0xFFFF_FFFE;

/// Head of one `SYSTEM_PROCESS_INFORMATION` record on 64-bit Windows.
#[repr(C)]
struct RawProcess {
    next_entry_offset: u32,
    number_of_threads: u32,
    working_set_private_size: i64,
    hard_fault_count: u32,
    number_of_threads_high_watermark: u32,
    cycle_time: u64,
    create_time: i64,
    user_time: i64,
    kernel_time: i64,
    image_name_length: u16,
    image_name_maximum_length: u16,
    image_name_buffer: *const u16,
    base_priority: i32,
    unique_process_id: usize,
    inherited_from_unique_process_id: usize,
    handle_count: u32,
    session_id: u32,
}

const _: () = {
    assert!(std::mem::offset_of!(RawProcess, create_time) == 32);
    assert!(std::mem::offset_of!(RawProcess, image_name_buffer) == 64);
    assert!(std::mem::offset_of!(RawProcess, unique_process_id) == 80);
    assert!(std::mem::offset_of!(RawProcess, session_id) == 100);
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessInfo {
    pub pid: u32,
    pub parent_pid: u32,
    /// Executable file name as the kernel reports it, without folder.
    pub name: String,
    pub session_id: u32,
    /// Creation time in 100 ns units since 1601 (`FILETIME`). Together with
    /// the PID it identifies a process for good (S6).
    pub created: u64,
}

/// All processes at one moment. Taking it opens no process.
#[derive(Debug, Clone)]
pub struct Snapshot {
    processes: Vec<ProcessInfo>,
    own_pid: u32,
}

impl Snapshot {
    pub fn take() -> Result<Self> {
        Ok(Self {
            processes: query_processes()?,
            own_pid: std::process::id(),
        })
    }

    /// Builds a snapshot from known data, as if `own_pid` were kaltstellen.
    pub fn from_parts(processes: Vec<ProcessInfo>, own_pid: u32) -> Self {
        Self { processes, own_pid }
    }

    pub fn processes(&self) -> &[ProcessInfo] {
        &self.processes
    }

    pub fn get(&self, pid: u32) -> Option<&ProcessInfo> {
        self.processes.iter().find(|p| p.pid == pid)
    }

    /// The process with this PID and creation time, if it is still the same
    /// one (S6).
    pub fn find(&self, pid: u32, created: u64) -> Option<&ProcessInfo> {
        self.get(pid).filter(|p| p.created == created)
    }

    /// The session kaltstellen runs in; only its processes are considered.
    /// A value that is the same for every snapshot of one Windows boot and
    /// differs after a reboot: the creation time of the `System` process
    /// (S10). Zero if unknown.
    pub fn boot_marker(&self) -> u64 {
        self.get(4).map_or(0, |system| system.created)
    }

    pub fn own_session(&self) -> Option<u32> {
        self.get(self.own_pid).map(|p| p.session_id)
    }

    /// A7: all processes of the program `exe` in the current session, with
    /// their child processes. kaltstellen itself, its ancestors and
    /// hard-protected processes are never part of the result.
    pub fn program(&self, exe: &str) -> Vec<ProcessInfo> {
        let exe = with_extension(exe);
        self.tree(|p| p.name.eq_ignore_ascii_case(&exe))
    }

    /// Like [`Snapshot::program`], but only processes whose image is `path`
    /// count as the program. This opens the processes named `exe` to read
    /// their path; the second value lists those that could not be read.
    pub fn program_at(&self, exe: &str, path: &Path) -> (Vec<ProcessInfo>, Vec<Error>) {
        let exe = with_extension(exe);
        let wanted = normalize_path(path);
        let mut roots = HashSet::new();
        let mut errors = Vec::new();
        for info in self.candidates(|p| p.name.eq_ignore_ascii_case(&exe)) {
            match Target::open(info, Access::Query).and_then(|t| match t {
                Some(target) => target.image_path().map(Some),
                None => Ok(None),
            }) {
                Ok(Some(image)) if normalize_path(&image) == wanted => {
                    roots.insert(info.pid);
                }
                Ok(_) => {}
                Err(err) => errors.push(err),
            }
        }
        (self.tree(|p| roots.contains(&p.pid)), errors)
    }

    /// Processes of the current session that `is_root` accepts and that may
    /// be touched at all.
    fn candidates<'a>(
        &'a self,
        is_root: impl Fn(&ProcessInfo) -> bool + 'a,
    ) -> impl Iterator<Item = &'a ProcessInfo> + 'a {
        let session = self.own_session();
        self.processes.iter().filter(move |p| {
            Some(p.session_id) == session
                && p.pid != 0
                && !protection::is_hard_protected(&p.name)
                && is_root(p)
        })
    }

    /// kaltstellen's ancestors: parent, grandparent, ... A parent only counts
    /// if it was created before its child, otherwise the PID was reused.
    fn ancestors(&self) -> HashSet<u32> {
        let mut found = HashSet::new();
        let mut current = self.get(self.own_pid);
        while let Some(child) = current {
            let parent = self
                .get(child.parent_pid)
                .filter(|p| p.pid != child.pid && p.pid != 0 && p.created <= child.created);
            match parent {
                Some(parent) if found.insert(parent.pid) => current = Some(parent),
                _ => break,
            }
        }
        found
    }

    fn tree(&self, is_root: impl Fn(&ProcessInfo) -> bool) -> Vec<ProcessInfo> {
        let ancestors = self.ancestors();
        let session = self.own_session();
        let mut pending: Vec<&ProcessInfo> = self.candidates(is_root).collect();
        pending.reverse();
        let mut visited = HashSet::new();
        let mut result = Vec::new();
        while let Some(node) = pending.pop() {
            // Self-protection: kaltstellen and everything it started stay out.
            if node.pid == self.own_pid || !visited.insert(node.pid) {
                continue;
            }
            // An ancestor is skipped itself, but its other children belong
            // to the program.
            if !ancestors.contains(&node.pid) {
                result.push(node.clone());
            }
            pending.extend(self.processes.iter().filter(|child| {
                child.parent_pid == node.pid
                    && child.pid != node.pid
                    && child.pid != 0
                    && child.created >= node.created
                    && Some(child.session_id) == session
                    && !protection::is_hard_protected(&child.name)
            }));
        }
        result
    }
}

fn with_extension(exe: &str) -> String {
    let exe = exe.trim();
    if exe.contains('.') {
        exe.to_owned()
    } else {
        format!("{exe}.exe")
    }
}

/// Whether two paths name the same file, ignoring case and slash direction.
pub fn same_path(a: &Path, b: &Path) -> bool {
    normalize_path(a) == normalize_path(b)
}

fn normalize_path(path: &Path) -> String {
    path.to_string_lossy().replace('/', "\\").to_lowercase()
}

fn query_processes() -> Result<Vec<ProcessInfo>> {
    // u64 elements keep the buffer aligned for the records.
    let mut buffer: Vec<u64> = vec![0; 64 * 1024];
    loop {
        let bytes = (buffer.len() * 8) as u32;
        let mut needed = 0u32;
        // SAFETY: the buffer is writable for `bytes` bytes and `needed` is a
        // valid out pointer.
        let status = unsafe {
            NtQuerySystemInformation(
                SYSTEM_PROCESS_INFORMATION,
                buffer.as_mut_ptr().cast(),
                bytes,
                &mut needed,
            )
        };
        if status == STATUS_INFO_LENGTH_MISMATCH {
            let wanted = (needed as usize).max(bytes as usize) / 8 + 16 * 1024;
            buffer = vec![0; wanted];
            continue;
        }
        status
            .ok()
            .map_err(|err| Error::new(Code::Snapshot, err.message()))?;
        break;
    }

    let base = buffer.as_ptr().cast::<u8>();
    let len = buffer.len() * 8;
    let mut processes = Vec::new();
    let mut offset = 0usize;
    while offset + size_of::<RawProcess>() <= len {
        // SAFETY: the record lies inside the buffer (checked above) and the
        // kernel aligns records to 8 bytes; `read_unaligned` does not rely on
        // that.
        let raw = unsafe { base.add(offset).cast::<RawProcess>().read_unaligned() };
        let name_bytes = raw.image_name_buffer as usize;
        let name_len = usize::from(raw.image_name_length) / 2;
        let name = if raw.image_name_buffer.is_null() {
            "System Idle Process".to_owned()
        } else if name_bytes >= base as usize && name_bytes + name_len * 2 <= base as usize + len {
            // SAFETY: the kernel stores the name inside the same buffer; the
            // range was just checked to lie within it.
            let units = unsafe {
                std::slice::from_raw_parts(raw.image_name_buffer.cast::<[u8; 2]>(), name_len)
            };
            let units: Vec<u16> = units.iter().map(|u| u16::from_ne_bytes(*u)).collect();
            String::from_utf16_lossy(&units)
        } else {
            String::new()
        };
        processes.push(ProcessInfo {
            pid: raw.unique_process_id as u32,
            parent_pid: raw.inherited_from_unique_process_id as u32,
            name,
            session_id: raw.session_id,
            created: raw.create_time as u64,
        });
        if raw.next_entry_offset == 0 {
            break;
        }
        offset += raw.next_entry_offset as usize;
    }
    Ok(processes)
}

/// Closes the wrapped handle on drop.
struct Handle(HANDLE);

impl Drop for Handle {
    fn drop(&mut self) {
        // SAFETY: the handle came from a successful Win32 call and is closed
        // exactly once.
        let _ = unsafe { CloseHandle(self.0) };
    }
}

/// What a process is opened for. Each asks for the fewest rights that do it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Access {
    /// Read the image path.
    Query,
    /// Suspend, resume and trim.
    Suspend,
    /// Resume, wait for exit and terminate.
    Stop,
}

impl Access {
    fn rights(self) -> PROCESS_ACCESS_RIGHTS {
        match self {
            Access::Query => PROCESS_QUERY_LIMITED_INFORMATION,
            Access::Suspend => {
                PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SUSPEND_RESUME | PROCESS_SET_QUOTA
            }
            Access::Stop => {
                PROCESS_QUERY_LIMITED_INFORMATION
                    | PROCESS_SUSPEND_RESUME
                    | PROCESS_TERMINATE
                    | PROCESS_SYNCHRONIZE
            }
        }
    }
}

/// An opened process. The handle is kept for the life of the value, which
/// rules out PID reuse while a step works on it (S10).
pub struct Target {
    info: ProcessInfo,
    handle: Handle,
    access: Access,
}

impl Target {
    /// Opens the process described by `info`. `Ok(None)` means it is gone:
    /// it exited, or its PID now belongs to another process (S6). That is
    /// not an error (A7).
    pub fn open(info: &ProcessInfo, access: Access) -> Result<Option<Target>> {
        // SAFETY: plain FFI call without pointer arguments.
        let handle = match unsafe { OpenProcess(access.rights(), false, info.pid) } {
            Ok(handle) => Handle(handle),
            Err(err) if err.code() == ERROR_INVALID_PARAMETER.to_hresult() => return Ok(None),
            Err(err) if err.code() == ERROR_ACCESS_DENIED.to_hresult() => {
                return Err(denied(info));
            }
            Err(err) => return Err(os_error(Code::ProcessOs, info, &err)),
        };
        let target = Target {
            info: info.clone(),
            handle,
            access,
        };
        if target.creation_time()? != info.created || target.exited() {
            return Ok(None);
        }
        Ok(Some(target))
    }

    pub fn info(&self) -> &ProcessInfo {
        &self.info
    }

    fn creation_time(&self) -> Result<u64> {
        let mut times = [FILETIME::default(); 4];
        let [created, exited, kernel, user] = &mut times;
        // SAFETY: the handle has PROCESS_QUERY_LIMITED_INFORMATION and the
        // four pointers are valid.
        unsafe { GetProcessTimes(self.handle.0, created, exited, kernel, user) }
            .map_err(|err| os_error(Code::ProcessOs, &self.info, &err))?;
        Ok((u64::from(times[0].dwHighDateTime) << 32) | u64::from(times[0].dwLowDateTime))
    }

    fn exited(&self) -> bool {
        let mut code = 0u32;
        // SAFETY: the handle has PROCESS_QUERY_LIMITED_INFORMATION.
        unsafe { GetExitCodeProcess(self.handle.0, &mut code) }.is_ok() && code != STILL_ACTIVE
    }

    /// A failed operation on a process that has exited meanwhile counts as
    /// done (A7).
    fn unless_exited(&self, result: Result<()>) -> Result<()> {
        match result {
            Err(_) if self.exited() => Ok(()),
            other => other,
        }
    }

    /// Full DOS path of the executable.
    pub fn image_path(&self) -> Result<PathBuf> {
        let mut buffer = vec![0u16; 32 * 1024];
        let mut len = buffer.len() as u32;
        // SAFETY: `buffer` is writable for `len` characters.
        unsafe {
            QueryFullProcessImageNameW(
                self.handle.0,
                PROCESS_NAME_WIN32,
                PWSTR(buffer.as_mut_ptr()),
                &mut len,
            )
        }
        .map_err(|err| os_error(Code::ProcessOs, &self.info, &err))?;
        Ok(PathBuf::from(String::from_utf16_lossy(
            &buffer[..len as usize],
        )))
    }

    /// Freezes every thread. Each call raises the suspend count, so the
    /// caller must pair it with exactly one [`Target::resume`] (S2).
    pub fn suspend(&self) -> Result<()> {
        // SAFETY: the handle has PROCESS_SUSPEND_RESUME.
        let result = unsafe { NtSuspendProcess(self.handle.0) }
            .ok()
            .map_err(|err| os_error(Code::SuspendFailed, &self.info, &err).step(Step::Suspend));
        self.unless_exited(result)
    }

    /// Undoes one [`Target::suspend`].
    pub fn resume(&self) -> Result<()> {
        // SAFETY: the handle has PROCESS_SUSPEND_RESUME.
        let result = unsafe { NtResumeProcess(self.handle.0) }
            .ok()
            .map_err(|err| os_error(Code::ResumeFailed, &self.info, &err).step(Step::Resume));
        self.unless_exited(result)
    }

    /// Moves the process's pages out of physical memory. On a frozen process
    /// they stay out, because nothing touches them (R5).
    pub fn trim(&self) -> Result<()> {
        if self.access != Access::Suspend {
            return Err(Error::new(Code::TrimFailed, "process not opened for trim")
                .program(&self.info.name)
                .step(Step::Trim));
        }
        // SAFETY: the handle has PROCESS_SET_QUOTA and query rights.
        let result = unsafe { EmptyWorkingSet(self.handle.0) }
            .map_err(|err| os_error(Code::TrimFailed, &self.info, &err).step(Step::Trim));
        self.unless_exited(result)
    }

    pub fn terminate(&self) -> Result<()> {
        // SAFETY: the handle has PROCESS_TERMINATE.
        let result = unsafe { TerminateProcess(self.handle.0, 1) }
            .map_err(|err| os_error(Code::TerminateFailed, &self.info, &err).step(Step::Stop));
        self.unless_exited(result)
    }

    /// Waits until the process has exited; `false` on timeout. Needs
    /// [`Access::Stop`].
    pub fn wait_exit(&self, timeout: Duration) -> bool {
        let millis = timeout.as_millis().min(u128::from(u32::MAX - 1)) as u32;
        // SAFETY: the handle has SYNCHRONIZE when opened with Access::Stop;
        // without it the call fails and we fall back to the exit code.
        let waited = unsafe { WaitForSingleObject(self.handle.0, millis) };
        waited == WAIT_OBJECT_0 || self.exited()
    }
}

/// P7: tells "needs administrator" from "protected process".
fn denied(info: &ProcessInfo) -> Error {
    // SAFETY: plain FFI call without pointer arguments.
    let protected = match unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, info.pid) }
    {
        Ok(handle) => {
            let handle = Handle(handle);
            let mut level = PROCESS_PROTECTION_LEVEL_INFORMATION::default();
            // SAFETY: `level` is a valid buffer of the size passed.
            let queried = unsafe {
                GetProcessInformation(
                    handle.0,
                    ProcessProtectionLevelInfo,
                    (&raw mut level).cast(),
                    size_of::<PROCESS_PROTECTION_LEVEL_INFORMATION>() as u32,
                )
            };
            queried.is_ok() && level.ProtectionLevel.0 != PROTECTION_LEVEL_NONE
        }
        // Not even readable: only protected system processes behave so.
        Err(_) => true,
    };
    if protected {
        Error::new(Code::Protected, "protected process; cannot be opened")
    } else {
        Error::new(
            Code::NeedsAdmin,
            "needs administrator: the process runs with higher rights",
        )
    }
    .program(&info.name)
}

fn os_error(code: Code, info: &ProcessInfo, err: &windows::core::Error) -> Error {
    Error::new(code, format!("pid {}: {}", info.pid, err.message())).program(&info.name)
}

/// Top-level windows of the given processes, visible or not.
fn windows_of(pids: &[u32]) -> Vec<HWND> {
    struct Search<'a> {
        pids: &'a [u32],
        found: Vec<HWND>,
    }
    unsafe extern "system" fn visit(hwnd: HWND, param: LPARAM) -> BOOL {
        // SAFETY: `param` is the pointer to the `Search` below, which
        // outlives the EnumWindows call; no other reference exists meanwhile.
        let search = unsafe { &mut *(param.0 as *mut Search) };
        let mut pid = 0u32;
        // SAFETY: `pid` is a valid out pointer.
        unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
        if search.pids.contains(&pid) {
            search.found.push(hwnd);
        }
        true.into()
    }
    let mut search = Search {
        pids,
        found: Vec::new(),
    };
    // SAFETY: the callback only uses `search`, which lives until the call
    // returns.
    let _ = unsafe { EnumWindows(Some(visit), LPARAM(&raw mut search as isize)) };
    search.found
}

fn is_visible(hwnd: HWND) -> bool {
    // SAFETY: a stale window handle makes the call return false.
    unsafe { IsWindowVisible(hwnd) }.as_bool()
}

fn is_minimized(hwnd: HWND) -> bool {
    // SAFETY: as above.
    unsafe { IsIconic(hwnd) }.as_bool()
}

/// Number of visible top-level windows of the given processes.
pub fn visible_window_count(pids: &[u32]) -> usize {
    windows_of(pids)
        .into_iter()
        .filter(|w| is_visible(*w))
        .count()
}

/// W7: minimises the visible top-level windows of the processes and waits
/// until they report minimised or `wait` has passed. Windows that refuse
/// (for example elevated ones) are skipped. Returns how many were asked.
pub fn minimize_windows(pids: &[u32], wait: Duration) -> usize {
    let windows: Vec<HWND> = windows_of(pids)
        .into_iter()
        .filter(|w| is_visible(*w) && !is_minimized(*w))
        .collect();
    for window in &windows {
        // SAFETY: a stale or foreign window handle makes the call fail.
        let _ = unsafe { ShowWindowAsync(*window, SW_MINIMIZE) };
    }
    let deadline = Instant::now() + wait;
    while Instant::now() < deadline && windows.iter().any(|w| is_visible(*w) && !is_minimized(*w)) {
        std::thread::sleep(Duration::from_millis(20));
    }
    windows.len()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopOutcome {
    /// None of the processes was running any more.
    NotRunning,
    /// All processes exited after the close request.
    Closed,
    /// At least one process had to be terminated.
    Killed,
}

/// A2: asks the program's windows to close, waits up to `timeout`, then
/// terminates what is left (`OnTimeout::Kill`) or leaves it running and
/// reports `CloseTimeout` (`OnTimeout::Keep`). A program without windows
/// cannot be asked, so `on_timeout` applies at once.
///
/// Processes that cannot be opened are reported with the first such error
/// after everything else was done (W2).
pub fn stop(
    processes: &[ProcessInfo],
    timeout: Duration,
    on_timeout: OnTimeout,
) -> Result<StopOutcome> {
    let mut first_error = None;
    let mut targets = Vec::new();
    for info in processes {
        match Target::open(info, Access::Stop) {
            Ok(Some(target)) => targets.push(target),
            Ok(None) => {}
            Err(err) => {
                first_error.get_or_insert(err.step(Step::Stop));
            }
        }
    }
    if targets.is_empty() {
        return first_error.map_or(Ok(StopOutcome::NotRunning), Err);
    }

    let pids: Vec<u32> = targets.iter().map(|t| t.info.pid).collect();
    let windows = windows_of(&pids);
    for window in &windows {
        // SAFETY: a stale window handle or one of a higher integrity level
        // makes the call fail, which is fine.
        let _ = unsafe { PostMessageW(Some(*window), WM_CLOSE, WPARAM(0), LPARAM(0)) };
    }
    let deadline = Instant::now()
        + if windows.is_empty() {
            Duration::ZERO
        } else {
            timeout
        };
    targets.retain(|t| !t.wait_exit(deadline.saturating_duration_since(Instant::now())));

    let outcome = if targets.is_empty() {
        StopOutcome::Closed
    } else {
        match on_timeout {
            OnTimeout::Keep => {
                let left = &targets[0].info;
                first_error.get_or_insert(
                    Error::new(
                        Code::CloseTimeout,
                        format!(
                            "did not close within {} s and was left running",
                            timeout.as_secs()
                        ),
                    )
                    .program(&left.name)
                    .step(Step::Stop),
                );
            }
            OnTimeout::Kill => {
                for target in &targets {
                    if let Err(err) = target.terminate() {
                        first_error.get_or_insert(err);
                    }
                }
                for target in &targets {
                    target.wait_exit(Duration::from_secs(2));
                }
            }
        }
        StopOutcome::Killed
    };
    first_error.map_or(Ok(outcome), Err)
}

/// A1: starts a program and returns its PID. The new process inherits no
/// handles and is not tied to kaltstellen's lifetime (P9). Without `cwd` it
/// starts in the folder of its executable. kaltstellen never triggers a UAC
/// prompt: a program that needs elevation fails with `ElevationRequired`.
pub fn start(path: &Path, args: &[String], cwd: Option<&Path>) -> Result<u32> {
    let program = path
        .file_name()
        .map_or_else(|| path.to_string_lossy(), |n| n.to_string_lossy())
        .into_owned();
    let mut command_line = String::new();
    quote_argument(&path.to_string_lossy(), &mut command_line);
    for arg in args {
        command_line.push(' ');
        quote_argument(arg, &mut command_line);
    }
    let application = wide(path.as_os_str());
    let mut command_line = wide(OsStr::new(&command_line));
    let directory = cwd.or_else(|| path.parent().filter(|p| !p.as_os_str().is_empty()));
    let directory = directory.map(|d| wide(d.as_os_str()));

    let startup = STARTUPINFOW {
        cb: size_of::<STARTUPINFOW>() as u32,
        ..Default::default()
    };
    let mut created = PROCESS_INFORMATION::default();
    // SAFETY: all strings are NUL-terminated and outlive the call; the
    // command line buffer is mutable as the API requires.
    unsafe {
        CreateProcessW(
            PCWSTR(application.as_ptr()),
            Some(PWSTR(command_line.as_mut_ptr())),
            None,
            None,
            false,
            CREATE_NEW_CONSOLE,
            None,
            directory
                .as_ref()
                .map_or(PCWSTR::null(), |d| PCWSTR(d.as_ptr())),
            &startup,
            &mut created,
        )
    }
    .map_err(|err| {
        let code = if err.code() == ERROR_ELEVATION_REQUIRED.to_hresult() {
            Code::ElevationRequired
        } else {
            Code::StartFailed
        };
        Error::new(code, format!("{}: {}", path.display(), err.message()))
            .program(&program)
            .step(Step::Start)
    })?;
    drop(Handle(created.hThread));
    drop(Handle(created.hProcess));
    Ok(created.dwProcessId)
}

fn wide(text: &OsStr) -> Vec<u16> {
    text.encode_wide().chain(std::iter::once(0)).collect()
}

/// Quotes one argument the way `CommandLineToArgvW` and the C runtime split
/// a command line.
fn quote_argument(arg: &str, out: &mut String) {
    if !arg.is_empty() && !arg.contains([' ', '\t', '"']) {
        out.push_str(arg);
        return;
    }
    out.push('"');
    let mut backslashes = 0;
    for c in arg.chars() {
        match c {
            '\\' => backslashes += 1,
            '"' => {
                out.extend(std::iter::repeat_n('\\', backslashes * 2 + 1));
                out.push('"');
                backslashes = 0;
            }
            _ => {
                out.extend(std::iter::repeat_n('\\', backslashes));
                out.push(c);
                backslashes = 0;
            }
        }
    }
    out.extend(std::iter::repeat_n('\\', backslashes * 2));
    out.push('"');
}

/// Physical memory currently available, in bytes. The measure of what a
/// trim really gained (T19), unlike the working set.
pub fn available_memory() -> Result<u64> {
    let mut status = MEMORYSTATUSEX {
        dwLength: size_of::<MEMORYSTATUSEX>() as u32,
        ..Default::default()
    };
    // SAFETY: `status` is valid and has its length set.
    unsafe { GlobalMemoryStatusEx(&mut status) }
        .map_err(|err| Error::new(Code::ProcessOs, err.message()))?;
    Ok(status.ullAvailPhys)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(pid: u32, parent: u32, name: &str, session: u32, created: u64) -> ProcessInfo {
        ProcessInfo {
            pid,
            parent_pid: parent,
            name: name.to_owned(),
            session_id: session,
            created,
        }
    }

    fn pids(processes: &[ProcessInfo]) -> Vec<u32> {
        let mut pids: Vec<u32> = processes.iter().map(|p| p.pid).collect();
        pids.sort_unstable();
        pids
    }

    /// kaltstellen is pid 900, started from a shell (800) in session 1.
    fn snapshot(mut extra: Vec<ProcessInfo>) -> Snapshot {
        let mut all = vec![
            p(4, 0, "System", 0, 1),
            p(700, 4, "explorer.exe", 1, 10),
            p(800, 700, "shell.exe", 1, 20),
            p(900, 800, "kaltstellen.exe", 1, 30),
        ];
        all.append(&mut extra);
        Snapshot::from_parts(all, 900)
    }

    #[test]
    fn program_includes_children_and_grandchildren() {
        let s = snapshot(vec![
            p(100, 700, "app.exe", 1, 40),
            p(101, 100, "helper.exe", 1, 41),
            p(102, 101, "worker.exe", 1, 42),
            p(200, 700, "other.exe", 1, 43),
        ]);
        assert_eq!(pids(&s.program("APP")), vec![100, 101, 102]);
        assert_eq!(pids(&s.program("app.exe")), vec![100, 101, 102]);
    }

    #[test]
    fn reused_parent_pid_is_not_a_child() {
        // 101 names 100 as parent but is older: the PID 100 was reused.
        let s = snapshot(vec![
            p(100, 700, "app.exe", 1, 50),
            p(101, 100, "stranger.exe", 1, 40),
        ]);
        assert_eq!(pids(&s.program("app.exe")), vec![100]);
    }

    #[test]
    fn orphans_are_not_part_of_the_tree() {
        // 102's parent 150 has exited and is not in the snapshot.
        let s = snapshot(vec![
            p(100, 700, "app.exe", 1, 40),
            p(102, 150, "orphan.exe", 1, 45),
        ]);
        assert_eq!(pids(&s.program("app.exe")), vec![100]);
    }

    #[test]
    fn other_sessions_are_ignored() {
        let s = snapshot(vec![
            p(100, 700, "app.exe", 1, 40),
            p(300, 4, "app.exe", 2, 41),
            p(301, 100, "helper.exe", 2, 42),
        ]);
        assert_eq!(pids(&s.program("app.exe")), vec![100]);
    }

    #[test]
    fn t16_self_and_ancestors_are_never_part_of_a_program() {
        // The shell that launched kaltstellen is itself marked: its other
        // children are frozen, the shell and kaltstellen are not.
        let s = snapshot(vec![
            p(801, 800, "editor.exe", 1, 31),
            p(901, 900, "child-of-kaltstellen.exe", 1, 32),
        ]);
        assert_eq!(pids(&s.program("shell.exe")), vec![801]);
        assert!(s.program("kaltstellen.exe").is_empty());
    }

    #[test]
    fn hard_protected_processes_are_never_part_of_a_program() {
        let s = snapshot(vec![
            p(100, 700, "app.exe", 1, 40),
            p(101, 100, "conhost.exe", 1, 41),
            p(102, 101, "below-conhost.exe", 1, 42),
        ]);
        assert_eq!(pids(&s.program("app.exe")), vec![100]);
        assert!(s.program("explorer.exe").is_empty());
        assert!(s.program("System").is_empty());
    }

    #[test]
    fn find_requires_matching_creation_time() {
        let s = snapshot(vec![p(100, 700, "app.exe", 1, 40)]);
        assert!(s.find(100, 40).is_some());
        assert!(s.find(100, 41).is_none());
    }

    #[test]
    fn real_snapshot_contains_this_process_and_opens_it() {
        let snapshot = Snapshot::take().unwrap();
        let own = snapshot.get(std::process::id()).expect("own process");
        assert!(own.created > 0);
        assert!(own.name.to_lowercase().ends_with(".exe"));
        // `open` compares the creation time from the snapshot with the one
        // the process handle reports, which checks the record layout.
        let target = Target::open(own, Access::Query).unwrap().expect("alive");
        let image = target.image_path().unwrap();
        let expected = std::env::current_exe().unwrap();
        assert_eq!(normalize_path(&image), normalize_path(&expected));
        // The parent exists and is older.
        let parent = snapshot.get(own.parent_pid).expect("parent process");
        assert!(parent.created <= own.created);
    }

    #[test]
    fn s6_reused_pid_counts_as_gone() {
        let snapshot = Snapshot::take().unwrap();
        let mut stale = snapshot.get(std::process::id()).unwrap().clone();
        stale.created -= 1;
        assert!(Target::open(&stale, Access::Query).unwrap().is_none());
        let missing = p(0xFFFF_FFF0, 0, "gone.exe", 0, 1);
        assert!(Target::open(&missing, Access::Query).unwrap().is_none());
    }

    #[test]
    fn quoting_follows_the_windows_rules() {
        let quote = |arg: &str| {
            let mut out = String::new();
            quote_argument(arg, &mut out);
            out
        };
        assert_eq!(quote("plain"), "plain");
        assert_eq!(quote(""), "\"\"");
        assert_eq!(quote("two words"), "\"two words\"");
        assert_eq!(quote(r"C:\dir\"), r"C:\dir\");
        assert_eq!(quote(r"C:\my dir\"), r#""C:\my dir\\""#);
        assert_eq!(quote(r#"say "hi""#), r#""say \"hi\"""#);
        assert_eq!(quote(r#"a\"b"#), r#""a\\\"b""#);
    }

    #[test]
    fn available_memory_is_reported() {
        assert!(available_memory().unwrap() > 0);
    }
}
