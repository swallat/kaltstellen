//! A real-time event session for process start and stop (M4b): the
//! `Microsoft-Windows-Kernel-Process` provider, read through Event Tracing
//! for Windows. Only the service of the optional background helper uses it;
//! starting a session needs rights the unelevated tray lacks.
//!
//! Mechanics only: the helper (`crate::helper::events`) decides what is
//! kept and who may read it. Properties are read by name with TDH, so the
//! layout of an event version does not matter.

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::thread::JoinHandle;

use windows::Win32::Foundation::{ERROR_ALREADY_EXISTS, ERROR_SUCCESS};
use windows::Win32::Storage::FileSystem::{GetLogicalDriveStringsW, QueryDosDeviceW};
use windows::Win32::System::Diagnostics::Etw::{
    CONTROLTRACE_HANDLE, CloseTrace, ControlTraceW, EVENT_CONTROL_CODE_ENABLE_PROVIDER,
    EVENT_RECORD, EVENT_TRACE_CONTROL_QUERY, EVENT_TRACE_CONTROL_STOP, EVENT_TRACE_LOGFILEW,
    EVENT_TRACE_PROPERTIES, EVENT_TRACE_REAL_TIME_MODE, EnableTraceEx2, OpenTraceW,
    PROCESS_TRACE_MODE_EVENT_RECORD, PROCESS_TRACE_MODE_REAL_TIME, PROCESSTRACE_HANDLE,
    PROPERTY_DATA_DESCRIPTOR, ProcessTrace, StartTraceW, TRACE_LEVEL_INFORMATION, TdhGetProperty,
    TdhGetPropertySize, WNODE_FLAG_TRACED_GUID,
};
use windows::core::{GUID, PCWSTR, PWSTR};

use super::wide;
use crate::error::{Code, Error, Result};

/// `Microsoft-Windows-Kernel-Process`.
const PROVIDER: GUID = GUID::from_u128(0x22FB_2CD6_0E7B_422B_A0C7_2FAD_1FD0_E716);
/// `WINEVENT_KEYWORD_PROCESS`: process start and stop, nothing else.
const KEYWORD_PROCESS: u64 = 0x10;
/// Event ids of the provider.
const EVENT_START: u16 = 1;
const EVENT_STOP: u16 = 2;
/// `ULONG_MAX`: the property is not an array element.
const NO_ARRAY: u32 = u32::MAX;
/// `INVALID_PROCESSTRACE_HANDLE`.
const INVALID_TRACE: u64 = u64::MAX;
/// Time stamps as system time (`FILETIME`), not performance counter ticks.
const CLOCK_SYSTEM_TIME: u32 = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Start,
    Stop,
}

/// One process start or stop. Times are `FILETIME` values (100 ns since
/// 1601); `0` where the event did not carry the field.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessEvent {
    pub kind: Kind,
    pub pid: u32,
    pub parent_pid: u32,
    /// The Windows session of the process; `None` when not reported.
    pub session: Option<u32>,
    /// Creation time of the process (with `pid` it names the process, S6).
    pub created: u64,
    /// When the event happened.
    pub time: u64,
    /// Image path as the kernel reports it (`\Device\HarddiskVolume3\...`).
    pub image: String,
}

type Sink = Box<dyn Fn(ProcessEvent) + Send + Sync>;

/// A running event session. Dropping it stops the session.
pub struct Session {
    control: CONTROLTRACE_HANDLE,
    trace: PROCESSTRACE_HANDLE,
    /// The `EVENT_TRACE_PROPERTIES` and the session name behind it, in a
    /// buffer aligned for 64-bit fields.
    properties: Vec<u64>,
    thread: Option<JoinHandle<()>>,
    /// Owned by this session, handed to the callback as its context.
    sink: *mut Sink,
}

fn error(what: &str, code: impl std::fmt::Display) -> Error {
    Error::new(Code::HelperObserve, format!("{what}: {code}"))
}

/// The properties block for a session called `name`.
fn properties_for(name: &str) -> (Vec<u64>, Vec<u16>) {
    let name_wide = wide(name);
    let bytes = std::mem::size_of::<EVENT_TRACE_PROPERTIES>() + name_wide.len() * 2;
    let mut buffer = vec![0u64; bytes.div_ceil(8)];
    // SAFETY: the buffer is zeroed, aligned for 8 bytes and large enough
    // for the structure and the name behind it; the name is copied inside.
    unsafe {
        let props = buffer.as_mut_ptr().cast::<EVENT_TRACE_PROPERTIES>();
        (*props).Wnode.BufferSize = bytes as u32;
        (*props).Wnode.Flags = WNODE_FLAG_TRACED_GUID;
        (*props).Wnode.ClientContext = CLOCK_SYSTEM_TIME;
        (*props).LogFileMode = EVENT_TRACE_REAL_TIME_MODE;
        (*props).BufferSize = 64;
        (*props).MinimumBuffers = 4;
        (*props).MaximumBuffers = 16;
        (*props).LoggerNameOffset = std::mem::size_of::<EVENT_TRACE_PROPERTIES>() as u32;
        let target = buffer
            .as_mut_ptr()
            .cast::<u8>()
            .add(std::mem::size_of::<EVENT_TRACE_PROPERTIES>())
            .cast::<u16>();
        std::ptr::copy_nonoverlapping(name_wide.as_ptr(), target, name_wide.len());
    }
    (buffer, name_wide)
}

fn props_ptr(buffer: &mut [u64]) -> *mut EVENT_TRACE_PROPERTIES {
    buffer.as_mut_ptr().cast()
}

/// Stops a session by name (one a crashed service left behind).
fn stop_by_name(name: &str) {
    let (mut buffer, name_wide) = properties_for(name);
    // SAFETY: valid properties block and NUL-terminated name.
    let _ = unsafe {
        ControlTraceW(
            CONTROLTRACE_HANDLE::default(),
            PCWSTR(name_wide.as_ptr()),
            props_ptr(&mut buffer),
            EVENT_TRACE_CONTROL_STOP,
        )
    };
}

/// Starts the session `name` and calls `sink` for every process start and
/// stop, on the thread of the session. Needs the rights of a service
/// account or an administrator.
pub fn start(name: &str, sink: impl Fn(ProcessEvent) + Send + Sync + 'static) -> Result<Session> {
    let (mut properties, name_wide) = properties_for(name);
    let mut control = CONTROLTRACE_HANDLE::default();
    // SAFETY: valid out handle, NUL-terminated name and properties block.
    let mut status = unsafe {
        StartTraceW(
            &mut control,
            PCWSTR(name_wide.as_ptr()),
            props_ptr(&mut properties),
        )
    };
    if status == ERROR_ALREADY_EXISTS {
        stop_by_name(name);
        let (fresh, _) = properties_for(name);
        properties = fresh;
        // SAFETY: as above.
        status = unsafe {
            StartTraceW(
                &mut control,
                PCWSTR(name_wide.as_ptr()),
                props_ptr(&mut properties),
            )
        };
    }
    if status != ERROR_SUCCESS {
        return Err(error("cannot start the event session", status.0));
    }
    let sink: *mut Sink = Box::into_raw(Box::new(Box::new(sink) as Sink));
    let mut session = Session {
        control,
        trace: PROCESSTRACE_HANDLE {
            Value: INVALID_TRACE,
        },
        properties,
        thread: None,
        sink,
    };
    // SAFETY: the session handle is valid; the provider id outlives the call.
    let enabled = unsafe {
        EnableTraceEx2(
            control,
            &PROVIDER,
            EVENT_CONTROL_CODE_ENABLE_PROVIDER.0,
            TRACE_LEVEL_INFORMATION as u8,
            KEYWORD_PROCESS,
            0,
            0,
            None,
        )
    };
    if enabled != ERROR_SUCCESS {
        return Err(error("cannot enable the process events", enabled.0));
    }
    let mut logger = name_wide.clone();
    let mut logfile = EVENT_TRACE_LOGFILEW {
        LoggerName: PWSTR(logger.as_mut_ptr()),
        Context: sink.cast(),
        ..Default::default()
    };
    logfile.Anonymous1.ProcessTraceMode =
        PROCESS_TRACE_MODE_REAL_TIME | PROCESS_TRACE_MODE_EVENT_RECORD;
    logfile.Anonymous2.EventRecordCallback = Some(on_event);
    // SAFETY: the logfile structure and the logger name are valid for the
    // call; the callback and its context live until the session is dropped.
    let trace = unsafe { OpenTraceW(&mut logfile) };
    if trace.Value == INVALID_TRACE {
        return Err(error(
            "cannot open the event session",
            windows::core::Error::from_thread(),
        ));
    }
    session.trace = trace;
    let worker = std::thread::Builder::new()
        .name("helper events".to_owned())
        .spawn(move || {
            // SAFETY: a handle returned by OpenTraceW; blocks until the
            // session is stopped.
            let status = unsafe { ProcessTrace(&[trace], None, None) };
            log::warn!("the process event trace ended ({})", status.0);
        })
        .map_err(|e| error("cannot start the event thread", e))?;
    session.thread = Some(worker);
    Ok(session)
}

impl Session {
    /// Whether the session still delivers: false once the trace thread has
    /// ended (the session was stopped from outside, or the trace failed).
    pub fn alive(&self) -> bool {
        self.thread.as_ref().is_some_and(|t| !t.is_finished())
    }

    /// Events and buffers the kernel reports lost so far; `None` if the
    /// session could not be asked.
    pub fn lost(&mut self) -> Option<u32> {
        // SAFETY: valid handle and properties block, which the call fills.
        let status = unsafe {
            ControlTraceW(
                self.control,
                PCWSTR::null(),
                props_ptr(&mut self.properties),
                EVENT_TRACE_CONTROL_QUERY,
            )
        };
        if status != ERROR_SUCCESS {
            return None;
        }
        // SAFETY: the block holds an EVENT_TRACE_PROPERTIES the call filled.
        let props = unsafe { &*props_ptr(&mut self.properties) };
        Some(props.EventsLost.saturating_add(props.RealTimeBuffersLost))
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        // SAFETY: valid handle and properties block; stopping the session
        // ends ProcessTrace on the worker thread.
        let stopped = unsafe {
            ControlTraceW(
                self.control,
                PCWSTR::null(),
                props_ptr(&mut self.properties),
                EVENT_TRACE_CONTROL_STOP,
            )
        };
        // A trace that could not be stopped would keep the join waiting, and
        // the service with it; the thread then ends with the process.
        let joined = match self.thread.take() {
            Some(thread) if stopped == ERROR_SUCCESS || thread.is_finished() => {
                let _ = thread.join();
                true
            }
            Some(_) => false,
            None => true,
        };
        if !joined {
            // The callback may still run: the sink is left alone.
            return;
        }
        if self.trace.Value != INVALID_TRACE {
            // SAFETY: the handle came from OpenTraceW and is closed once.
            let _ = unsafe { CloseTrace(self.trace) };
        }
        // SAFETY: made by Box::into_raw in `start`; the callback can no
        // longer run once the trace thread has ended.
        drop(unsafe { Box::from_raw(self.sink) });
    }
}

// SAFETY: the raw sink pointer is owned by the session and only read by the
// callback, which runs on the worker thread that is joined before it is
// freed; the sink itself is `Send + Sync`.
unsafe impl Send for Session {}

/// The callback of the trace: decodes the event and hands it on.
unsafe extern "system" fn on_event(record: *mut EVENT_RECORD) {
    let _ = catch_unwind(AssertUnwindSafe(|| {
        // SAFETY: ETW passes a valid record for the duration of the call.
        let Some(record) = (unsafe { record.as_ref() }) else {
            return;
        };
        if record.EventHeader.ProviderId != PROVIDER {
            return;
        }
        let kind = match record.EventHeader.EventDescriptor.Id {
            EVENT_START => Kind::Start,
            EVENT_STOP => Kind::Stop,
            _ => return,
        };
        // SAFETY: the context is the sink pointer from `start`.
        let Some(sink) = (unsafe { record.UserContext.cast::<Sink>().as_ref() }) else {
            return;
        };
        let record_ptr: *const EVENT_RECORD = record;
        let number = |name: &str| property_u64(record_ptr, name, 4);
        let Some(pid) = number("ProcessID") else {
            return;
        };
        sink(ProcessEvent {
            kind,
            pid: pid as u32,
            parent_pid: number("ParentProcessID").unwrap_or(0) as u32,
            session: number("SessionID").map(|s| s as u32),
            created: property_u64(record_ptr, "CreateTime", 8).unwrap_or(0),
            time: u64::try_from(record.EventHeader.TimeStamp).unwrap_or(0),
            image: property_string(record_ptr, "ImageName").unwrap_or_default(),
        });
    }));
}

fn descriptor(name: &[u16]) -> PROPERTY_DATA_DESCRIPTOR {
    PROPERTY_DATA_DESCRIPTOR {
        PropertyName: name.as_ptr() as u64,
        ArrayIndex: NO_ARRAY,
        Reserved: 0,
    }
}

/// An integer property of `size` bytes (4 or 8), `None` if the event has
/// no such property.
fn property_u64(record: *const EVENT_RECORD, name: &str, size: usize) -> Option<u64> {
    let name = wide(name);
    let wanted = [descriptor(&name)];
    let mut buffer = [0u8; 8];
    // SAFETY: the record is valid for the callback; the name outlives the
    // call; the buffer has room for `size` bytes.
    let status = unsafe { TdhGetProperty(record, None, &wanted, &mut buffer[..size]) };
    if status != 0 {
        return None;
    }
    Some(if size == 4 {
        u64::from(u32::from_le_bytes(buffer[..4].try_into().ok()?))
    } else {
        u64::from_le_bytes(buffer)
    })
}

/// A UTF-16 string property, up to its NUL.
fn property_string(record: *const EVENT_RECORD, name: &str) -> Option<String> {
    let name = wide(name);
    let wanted = [descriptor(&name)];
    let mut size = 0u32;
    // SAFETY: as in `property_u64`; valid out pointer.
    let status = unsafe { TdhGetPropertySize(record, None, &wanted, &mut size) };
    if status != 0 || size == 0 || size > 64 * 1024 {
        return None;
    }
    let mut buffer = vec![0u8; size as usize];
    // SAFETY: the buffer has the size the call asked for.
    let status = unsafe { TdhGetProperty(record, None, &wanted, &mut buffer) };
    if status != 0 {
        return None;
    }
    let units: Vec<u16> = buffer
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| u16::from_le_bytes(*pair))
        .take_while(|unit| *unit != 0)
        .collect();
    Some(String::from_utf16_lossy(&units))
}

/// The drive letters and the device names behind them, as `("C:", "\Device\HarddiskVolume3")`.
pub fn device_map() -> Vec<(String, String)> {
    let mut drives = vec![0u16; 512];
    // SAFETY: the buffer is valid for its length.
    let length = unsafe { GetLogicalDriveStringsW(Some(&mut drives)) } as usize;
    if length == 0 || length > drives.len() {
        return Vec::new();
    }
    drives[..length]
        .split(|unit| *unit == 0)
        .filter(|drive| drive.len() >= 2)
        .filter_map(|drive| {
            let mut letter = drive[..2].to_vec();
            letter.push(0);
            let mut device = vec![0u16; 1024];
            // SAFETY: NUL-terminated drive name and a valid buffer.
            let written = unsafe { QueryDosDeviceW(PCWSTR(letter.as_ptr()), Some(&mut device)) };
            if written == 0 {
                return None;
            }
            let end = device.iter().position(|unit| *unit == 0)?;
            Some((
                String::from_utf16_lossy(&letter[..2]),
                String::from_utf16_lossy(&device[..end]),
            ))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;
    use std::time::Duration;

    /// Starts a session of its own and watches a child process start and
    /// end. Skipped where the session cannot be started (no administrator
    /// rights), which is the case for a plain developer account.
    #[test]
    fn a_child_process_is_seen_starting_and_ending() {
        let (tx, rx) = mpsc::channel();
        let tx = std::sync::Mutex::new(tx);
        let name = format!("kaltstellen-test-{}", std::process::id());
        let session = match start(&name, move |event| {
            let _ = tx.lock().unwrap().send(event);
        }) {
            Ok(session) => session,
            Err(err) => {
                eprintln!("skipped: {err}");
                return;
            }
        };
        let mut child = std::process::Command::new("cmd")
            .args(["/c", "exit", "0"])
            .spawn()
            .unwrap();
        let pid = child.id();
        child.wait().unwrap();
        let mut started = None;
        let mut stopped = false;
        let deadline = std::time::Instant::now() + Duration::from_secs(15);
        while std::time::Instant::now() < deadline && !(started.is_some() && stopped) {
            let Ok(event) = rx.recv_timeout(Duration::from_secs(1)) else {
                continue;
            };
            if event.pid != pid {
                continue;
            }
            match event.kind {
                Kind::Start => started = Some(event),
                Kind::Stop => stopped = true,
            }
        }
        drop(session);
        let started = started.expect("no start event for the child");
        assert!(stopped, "no stop event for the child");
        assert!(started.parent_pid == std::process::id());
        assert!(
            started.image.to_lowercase().ends_with("cmd.exe"),
            "{}",
            started.image
        );
        assert!(started.created > 0 && started.time > 0);
        let map = device_map();
        assert!(
            crate::helper::events::win32_path(&started.image, &map).is_some(),
            "{} maps to no drive: {map:?}",
            started.image
        );
    }
}
