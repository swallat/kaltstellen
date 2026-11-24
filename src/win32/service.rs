//! The Windows service side of the optional background helper (M4a): the
//! service control manager, the elevated relaunch, the access list of the
//! helper's data folder, and the service entry point.
//!
//! Only mechanics live here. What the helper allows, answers and installs is
//! decided by safe code in [`crate::helper`].

use std::ffi::c_void;
use std::path::Path;
use std::sync::atomic::{AtomicIsize, AtomicU32, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant};

use windows::Win32::Foundation::{
    CloseHandle, ERROR_CANCELLED, ERROR_SERVICE_DOES_NOT_EXIST, ERROR_SERVICE_NOT_ACTIVE, HANDLE,
    HLOCAL, LocalFree, WAIT_OBJECT_0,
};
use windows::Win32::Security::Authorization::{
    ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1, SE_FILE_OBJECT,
    SetNamedSecurityInfoW,
};
use windows::Win32::Security::{
    ACL, DACL_SECURITY_INFORMATION, GetSecurityDescriptorDacl, GetTokenInformation,
    PROTECTED_DACL_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR, TOKEN_ELEVATION, TOKEN_QUERY,
    TokenElevation,
};
use windows::Win32::System::Services::{
    ChangeServiceConfig2W, ChangeServiceConfigW, CloseServiceHandle, ControlService,
    CreateServiceW, DeleteService, OpenSCManagerW, OpenServiceW, QueryServiceStatusEx,
    RegisterServiceCtrlHandlerExW, SC_ACTION, SC_ACTION_RESTART, SC_HANDLE, SC_MANAGER_ALL_ACCESS,
    SC_STATUS_PROCESS_INFO, SERVICE_ACCEPT_SHUTDOWN, SERVICE_ACCEPT_STOP, SERVICE_ALL_ACCESS,
    SERVICE_AUTO_START, SERVICE_CONFIG_DELAYED_AUTO_START_INFO, SERVICE_CONFIG_DESCRIPTION,
    SERVICE_CONFIG_FAILURE_ACTIONS, SERVICE_CONFIG_REQUIRED_PRIVILEGES_INFO,
    SERVICE_CONTROL_INTERROGATE, SERVICE_CONTROL_SHUTDOWN, SERVICE_CONTROL_STOP,
    SERVICE_DELAYED_AUTO_START_INFO, SERVICE_DESCRIPTIONW, SERVICE_ERROR_NORMAL,
    SERVICE_FAILURE_ACTIONSW, SERVICE_NO_CHANGE, SERVICE_REQUIRED_PRIVILEGES_INFOW,
    SERVICE_RUNNING, SERVICE_START_PENDING, SERVICE_STATUS, SERVICE_STATUS_CURRENT_STATE,
    SERVICE_STATUS_HANDLE, SERVICE_STATUS_PROCESS, SERVICE_STOP_PENDING, SERVICE_STOPPED,
    SERVICE_TABLE_ENTRYW, SERVICE_WIN32_OWN_PROCESS, StartServiceCtrlDispatcherW, StartServiceW,
};
use windows::Win32::System::Threading::{
    GetCurrentProcess, GetExitCodeProcess, INFINITE, OpenProcessToken, WaitForSingleObject,
};
use windows::Win32::UI::Shell::{SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW, ShellExecuteExW};
use windows::core::{PCWSTR, PWSTR};

use super::{wide, wide_path};
use crate::error::{Code, Error, Result};

fn service_error(what: &str, err: impl std::fmt::Display) -> Error {
    Error::new(Code::HelperService, format!("{what}: {err}"))
}

/// Does this process run elevated (an administrator's full token)?
pub fn is_elevated() -> bool {
    // SAFETY: the pseudo handle needs no closing; the token handle is
    // closed below; the out buffer is a TOKEN_ELEVATION of the right size.
    unsafe {
        let mut token = HANDLE::default();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token).is_err() {
            return false;
        }
        let mut elevation = TOKEN_ELEVATION::default();
        let mut size = 0u32;
        let result = GetTokenInformation(
            token,
            TokenElevation,
            Some((&mut elevation as *mut TOKEN_ELEVATION).cast::<c_void>()),
            std::mem::size_of::<TOKEN_ELEVATION>() as u32,
            &mut size,
        );
        let _ = CloseHandle(token);
        result.is_ok() && elevation.TokenIsElevated != 0
    }
}

/// Runs `exe` with `parameters` elevated (a UAC prompt), waits for it and
/// returns its exit code. `HelperNeedsAdmin` if the prompt was declined.
pub fn run_elevated(exe: &Path, parameters: &str) -> Result<u32> {
    let file = wide_path(exe);
    let params = wide(parameters);
    let verb = wide("runas");
    let mut info = SHELLEXECUTEINFOW {
        cbSize: std::mem::size_of::<SHELLEXECUTEINFOW>() as u32,
        fMask: SEE_MASK_NOCLOSEPROCESS,
        lpVerb: PCWSTR(verb.as_ptr()),
        lpFile: PCWSTR(file.as_ptr()),
        lpParameters: PCWSTR(params.as_ptr()),
        nShow: 1, // SW_SHOWNORMAL
        ..Default::default()
    };
    // SAFETY: the strings outlive the call; the process handle it returns
    // is waited on and closed below.
    unsafe {
        if let Err(e) = ShellExecuteExW(&mut info) {
            return Err(if e.code() == ERROR_CANCELLED.to_hresult() {
                Error::new(
                    Code::HelperNeedsAdmin,
                    "the administrator prompt was declined",
                )
            } else {
                service_error("cannot start the elevated process", e)
            });
        }
        let process = info.hProcess;
        if process.is_invalid() {
            return Err(service_error(
                "cannot wait for the elevated process",
                "no process handle",
            ));
        }
        let waited = WaitForSingleObject(process, INFINITE);
        let mut code = 1u32;
        let got = GetExitCodeProcess(process, &mut code);
        let _ = CloseHandle(process);
        if waited != WAIT_OBJECT_0 || got.is_err() {
            return Err(service_error(
                "cannot read the result of the elevated process",
                "wait failed",
            ));
        }
        Ok(code)
    }
}

/// Gives `dir` the helper's access list and stops inheritance: administrators
/// and SYSTEM full control, the owner `owner_sid` (a checked SID) read and
/// list only. So `helper.toml` and the log cannot be changed by users.
pub fn protect_data_dir(dir: &Path, owner_sid: &str) -> Result<()> {
    let sddl = wide(&format!(
        "D:P(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)(A;OICI;FRFX;;;{owner_sid})"
    ));
    let path = wide_path(dir);
    // SAFETY: NUL-terminated inputs; the descriptor is freed below; the
    // DACL pointer points into the descriptor and is used before that.
    unsafe {
        let mut descriptor = PSECURITY_DESCRIPTOR::default();
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            PCWSTR(sddl.as_ptr()),
            SDDL_REVISION_1,
            &mut descriptor,
            None,
        )
        .map_err(|e| service_error("cannot build the access list", e))?;
        let mut present = false.into();
        let mut defaulted = false.into();
        let mut dacl: *mut ACL = std::ptr::null_mut();
        let got = GetSecurityDescriptorDacl(descriptor, &mut present, &mut dacl, &mut defaulted);
        let result = if got.is_ok() && !dacl.is_null() {
            let status = SetNamedSecurityInfoW(
                PCWSTR(path.as_ptr()),
                SE_FILE_OBJECT,
                DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
                None,
                None,
                Some(dacl),
                None,
            );
            if status.is_ok() {
                Ok(())
            } else {
                Err(service_error(
                    &format!("cannot set the access list of {}", dir.display()),
                    status.0,
                ))
            }
        } else {
            Err(service_error("cannot read the access list", "no DACL"))
        };
        let _ = LocalFree(Some(HLOCAL(descriptor.0)));
        result
    }
}

/// A handle of the service control manager or of a service.
struct Scm(SC_HANDLE);

impl Drop for Scm {
    fn drop(&mut self) {
        // SAFETY: owned, closed once.
        let _ = unsafe { CloseServiceHandle(self.0) };
    }
}

fn manager() -> Result<Scm> {
    // SAFETY: plain call; the handle is closed by `Scm`.
    unsafe { OpenSCManagerW(PCWSTR::null(), PCWSTR::null(), SC_MANAGER_ALL_ACCESS) }
        .map(Scm)
        .map_err(|e| service_error("cannot open the service control manager", e))
}

/// `None` if the service does not exist.
fn open(manager: &Scm, name: &str) -> Result<Option<Scm>> {
    let name = wide(name);
    // SAFETY: NUL-terminated name; the handle is closed by `Scm`.
    match unsafe { OpenServiceW(manager.0, PCWSTR(name.as_ptr()), SERVICE_ALL_ACCESS) } {
        Ok(handle) => Ok(Some(Scm(handle))),
        Err(e) if e.code() == ERROR_SERVICE_DOES_NOT_EXIST.to_hresult() => Ok(None),
        Err(e) => Err(service_error("cannot open the service", e)),
    }
}

/// What the service control manager says about the helper service.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Info {
    pub state: State,
    /// Process id while it runs.
    pub pid: Option<u32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    Stopped,
    Starting,
    Running,
    Stopping,
    Other,
}

fn status_of(service: &Scm) -> Result<Info> {
    let mut buffer = [0u8; std::mem::size_of::<SERVICE_STATUS_PROCESS>()];
    let mut needed = 0u32;
    // SAFETY: the buffer is exactly one SERVICE_STATUS_PROCESS; it is read
    // back unaligned.
    let status = unsafe {
        QueryServiceStatusEx(
            service.0,
            SC_STATUS_PROCESS_INFO,
            Some(&mut buffer),
            &mut needed,
        )
        .map_err(|e| service_error("cannot query the service", e))?;
        std::ptr::read_unaligned(buffer.as_ptr().cast::<SERVICE_STATUS_PROCESS>())
    };
    let state = match status.dwCurrentState {
        SERVICE_STOPPED => State::Stopped,
        SERVICE_START_PENDING => State::Starting,
        SERVICE_RUNNING => State::Running,
        SERVICE_STOP_PENDING => State::Stopping,
        _ => State::Other,
    };
    Ok(Info {
        state,
        pid: (status.dwProcessId != 0).then_some(status.dwProcessId),
    })
}

/// The state of the service; `None` if it is not installed.
pub fn query(name: &str) -> Result<Option<Info>> {
    let manager = manager()?;
    match open(&manager, name)? {
        Some(service) => status_of(&service).map(Some),
        None => Ok(None),
    }
}

fn wait_for(service: &Scm, wanted: State, what: &str) -> Result<()> {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        if status_of(service)?.state == wanted {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err(service_error(what, "timed out after 30 s"));
        }
        std::thread::sleep(Duration::from_millis(200));
    }
}

fn stop_service(service: &Scm) -> Result<()> {
    let mut status = SERVICE_STATUS::default();
    // SAFETY: valid handle and out pointer.
    match unsafe { ControlService(service.0, SERVICE_CONTROL_STOP, &mut status) } {
        Ok(()) => {}
        Err(e) if e.code() == ERROR_SERVICE_NOT_ACTIVE.to_hresult() => return Ok(()),
        Err(e) => return Err(service_error("cannot stop the service", e)),
    }
    wait_for(service, State::Stopped, "the service did not stop")
}

fn start_service(service: &Scm) -> Result<()> {
    // SAFETY: valid handle, no arguments.
    unsafe { StartServiceW(service.0, None) }
        .map_err(|e| service_error("cannot start the service", e))?;
    wait_for(service, State::Running, "the service did not start")
}

/// Stops the service if it runs; nothing if it is not installed.
pub fn stop(name: &str) -> Result<()> {
    let manager = manager()?;
    match open(&manager, name)? {
        Some(service) => stop_service(&service),
        None => Ok(()),
    }
}

/// Registers (or updates) the service `name` running `command_line`, as
/// the local system account with only the privileges in `privileges`,
/// started automatically (delayed), restarted after a failure, and starts
/// it. The caller has stopped a running service and put the file in place.
pub fn install(
    name: &str,
    display: &str,
    description: &str,
    command_line: &str,
    privileges: &[&str],
) -> Result<()> {
    let manager = manager()?;
    let wname = wide(name);
    let wdisplay = wide(display);
    let wcommand = wide(command_line);
    let service = match open(&manager, name)? {
        Some(service) => {
            // SAFETY: valid handle and NUL-terminated strings.
            unsafe {
                ChangeServiceConfigW(
                    service.0,
                    SERVICE_WIN32_OWN_PROCESS,
                    SERVICE_AUTO_START,
                    SERVICE_ERROR_NORMAL,
                    PCWSTR(wcommand.as_ptr()),
                    PCWSTR::null(),
                    None,
                    PCWSTR::null(),
                    PCWSTR::null(), // account unchanged: LocalSystem
                    PCWSTR::null(),
                    PCWSTR(wdisplay.as_ptr()),
                )
            }
            .map_err(|e| service_error("cannot update the service", e))?;
            service
        }
        None => {
            // SAFETY: valid handle and NUL-terminated strings; no account
            // means LocalSystem.
            let handle = unsafe {
                CreateServiceW(
                    manager.0,
                    PCWSTR(wname.as_ptr()),
                    PCWSTR(wdisplay.as_ptr()),
                    SERVICE_ALL_ACCESS,
                    SERVICE_WIN32_OWN_PROCESS,
                    SERVICE_AUTO_START,
                    SERVICE_ERROR_NORMAL,
                    PCWSTR(wcommand.as_ptr()),
                    PCWSTR::null(),
                    None,
                    PCWSTR::null(),
                    PCWSTR::null(),
                    PCWSTR::null(),
                )
            }
            .map_err(|e| service_error("cannot create the service", e))?;
            Scm(handle)
        }
    };
    // `SERVICE_NO_CHANGE` is only needed by callers that skip fields.
    let _ = SERVICE_NO_CHANGE;
    let mut description = wide(description);
    let info = SERVICE_DESCRIPTIONW {
        lpDescription: PWSTR(description.as_mut_ptr()),
    };
    set_config(&service, SERVICE_CONFIG_DESCRIPTION.0, &info, "description")?;
    let delayed = SERVICE_DELAYED_AUTO_START_INFO {
        fDelayedAutostart: true.into(),
    };
    set_config(
        &service,
        SERVICE_CONFIG_DELAYED_AUTO_START_INFO.0,
        &delayed,
        "delayed start",
    )?;
    // A multi-string: each name NUL-terminated, the list ends with an extra NUL.
    let mut list: Vec<u16> = Vec::new();
    for privilege in privileges {
        list.extend(privilege.encode_utf16());
        list.push(0);
    }
    list.push(0);
    let required = SERVICE_REQUIRED_PRIVILEGES_INFOW {
        pmszRequiredPrivileges: PWSTR(list.as_mut_ptr()),
    };
    set_config(
        &service,
        SERVICE_CONFIG_REQUIRED_PRIVILEGES_INFO.0,
        &required,
        "required privileges",
    )?;
    let mut actions = [
        SC_ACTION {
            Type: SC_ACTION_RESTART,
            Delay: 60_000,
        },
        SC_ACTION {
            Type: SC_ACTION_RESTART,
            Delay: 60_000,
        },
    ];
    let failure = SERVICE_FAILURE_ACTIONSW {
        dwResetPeriod: 86_400,
        lpRebootMsg: PWSTR::null(),
        lpCommand: PWSTR::null(),
        cActions: actions.len() as u32,
        lpsaActions: actions.as_mut_ptr(),
    };
    set_config(
        &service,
        SERVICE_CONFIG_FAILURE_ACTIONS.0,
        &failure,
        "restart after failure",
    )?;
    if status_of(&service)?.state != State::Running {
        start_service(&service)?;
    }
    Ok(())
}

fn set_config<T>(service: &Scm, level: u32, value: &T, what: &str) -> Result<()> {
    use windows::Win32::System::Services::SERVICE_CONFIG;
    // SAFETY: `value` is the structure that `level` names, valid for the call.
    unsafe {
        ChangeServiceConfig2W(
            service.0,
            SERVICE_CONFIG(level),
            Some((value as *const T).cast::<c_void>()),
        )
    }
    .map_err(|e| service_error(&format!("cannot set the service's {what}"), e))
}

/// Stops and deletes the service; nothing if it is not installed.
pub fn remove(name: &str) -> Result<bool> {
    let manager = manager()?;
    let Some(service) = open(&manager, name)? else {
        return Ok(false);
    };
    stop_service(&service)?;
    // SAFETY: valid handle.
    unsafe { DeleteService(service.0) }
        .map_err(|e| service_error("cannot delete the service", e))?;
    Ok(true)
}

/// Tells the service body to stop.
#[derive(Clone, Default)]
pub struct Stop(Arc<(Mutex<bool>, Condvar)>);

impl Stop {
    pub fn set(&self) {
        let (flag, signal) = &*self.0;
        *flag
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = true;
        signal.notify_all();
    }

    pub fn is_set(&self) -> bool {
        *self
            .0
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Blocks until [`set`](Stop::set) was called.
    pub fn wait(&self) {
        let (flag, signal) = &*self.0;
        let mut guard = flag
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        while !*guard {
            guard = signal
                .wait(guard)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
        }
    }
}

type Body = Box<dyn FnOnce(Stop) -> Result<()> + Send>;

static BODY: Mutex<Option<Body>> = Mutex::new(None);
static STOP: OnceLock<Stop> = OnceLock::new();
static STATUS_HANDLE: AtomicIsize = AtomicIsize::new(0);
static STATE: AtomicU32 = AtomicU32::new(0);
static NAME: OnceLock<Vec<u16>> = OnceLock::new();

fn report(state: SERVICE_STATUS_CURRENT_STATE, exit_code: u32, wait_hint: u32) {
    STATE.store(state.0, Ordering::SeqCst);
    let handle = SERVICE_STATUS_HANDLE(STATUS_HANDLE.load(Ordering::SeqCst) as *mut c_void);
    if handle.0.is_null() {
        return;
    }
    let pending = state == SERVICE_START_PENDING || state == SERVICE_STOP_PENDING;
    let status = SERVICE_STATUS {
        dwServiceType: SERVICE_WIN32_OWN_PROCESS,
        dwCurrentState: state,
        dwControlsAccepted: if pending {
            0
        } else {
            SERVICE_ACCEPT_STOP | SERVICE_ACCEPT_SHUTDOWN
        },
        dwWin32ExitCode: if exit_code == 0 { 0 } else { 1066 }, // ERROR_SERVICE_SPECIFIC_ERROR
        dwServiceSpecificExitCode: exit_code,
        dwCheckPoint: u32::from(pending),
        dwWaitHint: wait_hint,
    };
    // SAFETY: a handle from RegisterServiceCtrlHandlerExW and a valid status.
    let _ = unsafe { windows::Win32::System::Services::SetServiceStatus(handle, &status) };
}

unsafe extern "system" fn control_handler(
    control: u32,
    _event_type: u32,
    _event_data: *mut c_void,
    _context: *mut c_void,
) -> u32 {
    match control {
        SERVICE_CONTROL_STOP | SERVICE_CONTROL_SHUTDOWN => {
            report(SERVICE_STOP_PENDING, 0, 10_000);
            if let Some(stop) = STOP.get() {
                stop.set();
            }
        }
        SERVICE_CONTROL_INTERROGATE => {
            report(
                SERVICE_STATUS_CURRENT_STATE(STATE.load(Ordering::SeqCst)),
                0,
                0,
            );
        }
        _ => {}
    }
    0 // NO_ERROR
}

unsafe extern "system" fn service_main(_argc: u32, _argv: *mut PWSTR) {
    let Some(name) = NAME.get() else { return };
    // SAFETY: the name is NUL-terminated and lives in a static.
    let handle = unsafe {
        RegisterServiceCtrlHandlerExW(PCWSTR(name.as_ptr()), Some(control_handler), None)
    };
    let Ok(handle) = handle else { return };
    STATUS_HANDLE.store(handle.0 as isize, Ordering::SeqCst);
    report(SERVICE_START_PENDING, 0, 10_000);
    let stop = STOP.get_or_init(Stop::default).clone();
    let body = BODY
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .take();
    let Some(body) = body else {
        report(SERVICE_STOPPED, 1, 0);
        return;
    };
    report(SERVICE_RUNNING, 0, 0);
    let code = match body(stop) {
        Ok(()) => 0,
        Err(err) => {
            log::error!("{err}");
            1
        }
    };
    report(SERVICE_STOPPED, code, 0);
}

/// Hands this process to the service control manager as the service `name`
/// and runs `body` until the manager asks it to stop (`Stop` is set) or it
/// returns. Fails at once when the process was not started as a service.
pub fn dispatch(name: &str, body: impl FnOnce(Stop) -> Result<()> + Send + 'static) -> Result<()> {
    let wname = NAME.get_or_init(|| wide(name));
    *BODY
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(Box::new(body));
    STOP.get_or_init(Stop::default);
    let table = [
        SERVICE_TABLE_ENTRYW {
            lpServiceName: PWSTR(wname.as_ptr().cast_mut()),
            lpServiceProc: Some(service_main),
        },
        SERVICE_TABLE_ENTRYW::default(),
    ];
    // SAFETY: the table ends with a null entry and, like the name, outlives
    // the call, which returns when the service has stopped.
    unsafe { StartServiceCtrlDispatcherW(table.as_ptr()) }.map_err(|e| {
        service_error(
            "not started by the service control manager (use `kaltstellen helper install`)",
            e,
        )
    })
}
