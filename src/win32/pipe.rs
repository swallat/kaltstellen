//! L1: the tray's named pipe. Only the current user may open it: the pipe
//! gets a protected DACL with a single entry for the user's SID, refuses
//! remote clients, and the first instance is created with
//! `FILE_FLAG_FIRST_PIPE_INSTANCE`, so a pipe of that name created earlier
//! by someone else makes the tray fail instead of sharing it. The client
//! checks that the server process runs as the same user before it sends
//! anything, and connects at identification level only, so the server
//! cannot act as the client.
//!
//! Both ends hand out a [`File`]; reading and writing is safe code.

use std::ffi::c_void;
use std::fs::File;
use std::os::windows::io::{AsRawHandle, FromRawHandle};
use std::time::{Duration, Instant};

use windows::Win32::Foundation::{
    CloseHandle, DUPLICATE_HANDLE_OPTIONS, DuplicateHandle, ERROR_FILE_NOT_FOUND, ERROR_PIPE_BUSY,
    ERROR_PIPE_CONNECTED, HANDLE, HLOCAL, INVALID_HANDLE_VALUE, LocalFree,
};
use windows::Win32::Security::Authorization::{
    ConvertSecurityDescriptorToStringSecurityDescriptorW, ConvertSidToStringSidW,
    ConvertStringSecurityDescriptorToSecurityDescriptorW, GetSecurityInfo, SDDL_REVISION_1,
    SE_KERNEL_OBJECT,
};
use windows::Win32::Security::{
    DACL_SECURITY_INFORMATION, GetTokenInformation, PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES,
    TOKEN_QUERY, TOKEN_USER, TokenUser,
};
use windows::Win32::Storage::FileSystem::{
    CreateFileW, FILE_FLAG_FIRST_PIPE_INSTANCE, FILE_FLAGS_AND_ATTRIBUTES, FILE_GENERIC_READ,
    FILE_GENERIC_WRITE, FILE_SHARE_NONE, OPEN_EXISTING, PIPE_ACCESS_DUPLEX,
    SECURITY_IDENTIFICATION, SECURITY_SQOS_PRESENT,
};
use windows::Win32::System::IO::CancelSynchronousIo;
use windows::Win32::System::Pipes::{
    ConnectNamedPipe, CreateNamedPipeW, GetNamedPipeClientProcessId, GetNamedPipeServerProcessId,
    PIPE_READMODE_BYTE, PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_BYTE, PIPE_UNLIMITED_INSTANCES,
    PIPE_WAIT, WaitNamedPipeW,
};
use windows::Win32::System::RemoteDesktop::ProcessIdToSessionId;
use windows::Win32::System::Threading::{
    GetCurrentProcess, GetCurrentProcessId, GetCurrentThread, OpenProcess, OpenProcessToken,
    PROCESS_QUERY_LIMITED_INFORMATION, THREAD_TERMINATE,
};
use windows::core::{PCWSTR, PWSTR};

use super::wide;
use crate::error::{Code, Error, Result};

const BUFFER: u32 = 16 * 1024;

fn pipe_error(what: &str, err: impl std::fmt::Display) -> Error {
    Error::new(Code::TrayPipe, format!("{what}: {err}"))
}

/// Takes a string the system allocated with `LocalAlloc`.
///
/// # Safety
/// `text` must be a NUL-terminated string from `LocalAlloc`, not used
/// afterwards.
unsafe fn take_local_string(text: PWSTR) -> String {
    // SAFETY: guaranteed by the caller.
    unsafe {
        let value = text.to_string().unwrap_or_default();
        let _ = LocalFree(Some(HLOCAL(text.0.cast())));
        value
    }
}

/// The user SID of the process behind `process`, as `S-1-5-…`.
fn user_sid_of(process: HANDLE) -> Result<String> {
    // SAFETY: the token handle is closed below; the buffer is sized by the
    // first call and outlives every use of the SID pointer inside it.
    unsafe {
        let mut token = HANDLE::default();
        OpenProcessToken(process, TOKEN_QUERY, &mut token)
            .map_err(|e| pipe_error("cannot open process token", e))?;
        let mut size = 0u32;
        let _ = GetTokenInformation(token, TokenUser, None, 0, &mut size);
        let mut buffer = vec![0u64; (size as usize).div_ceil(8).max(1)];
        let result = GetTokenInformation(
            token,
            TokenUser,
            Some(buffer.as_mut_ptr().cast::<c_void>()),
            size,
            &mut size,
        );
        let _ = CloseHandle(token);
        result.map_err(|e| pipe_error("cannot read the token user", e))?;
        let user = &*(buffer.as_ptr().cast::<TOKEN_USER>());
        let mut text = PWSTR::null();
        ConvertSidToStringSidW(user.User.Sid, &mut text)
            .map_err(|e| pipe_error("cannot format the user SID", e))?;
        Ok(take_local_string(text))
    }
}

/// The SID of the user this process runs as.
pub fn current_user_sid() -> Result<String> {
    // SAFETY: the pseudo handle of the own process needs no closing.
    user_sid_of(unsafe { GetCurrentProcess() })
}

/// The Windows session this process runs in.
pub fn current_session() -> u32 {
    let mut session = 0;
    // SAFETY: valid out pointer.
    let _ = unsafe { ProcessIdToSessionId(GetCurrentProcessId(), &mut session) };
    session
}

/// The Windows session a process runs in, `None` if it cannot be told.
pub fn session_of(pid: u32) -> Option<u32> {
    let mut session = 0;
    // SAFETY: valid out pointer.
    unsafe { ProcessIdToSessionId(pid, &mut session) }.ok()?;
    Some(session)
}

/// A security descriptor owned by this value.
struct Descriptor(PSECURITY_DESCRIPTOR);

impl Drop for Descriptor {
    fn drop(&mut self) {
        // SAFETY: allocated by ConvertStringSecurityDescriptorToSecurityDescriptorW.
        let _ = unsafe { LocalFree(Some(HLOCAL(self.0.0))) };
    }
}

/// The server end: hands out one connected [`File`] per client.
pub struct Listener {
    name: Vec<u16>,
    descriptor: Descriptor,
    /// The instance waiting for the next client.
    next: HANDLE,
}

// The handles and the descriptor are only used by the thread that owns the
// listener.
unsafe impl Send for Listener {}

impl Listener {
    /// Creates the pipe `name` (`\\.\pipe\…`) for the current user only.
    /// Fails if a pipe of that name exists already.
    pub fn bind(name: &str) -> Result<Listener> {
        // Protected DACL, one entry: all access for this user. Nobody else,
        // not even administrators or SYSTEM, is let in.
        Listener::bind_sddl(name, &format!("D:P(A;;GA;;;{})", current_user_sid()?))
    }

    /// Creates the helper's pipe `name`: the service runs as SYSTEM and
    /// serves the owner recorded at install. The access list names the
    /// owner and SYSTEM (the service itself, to wake its own accept loop at
    /// stop) and nobody else. `sid` must be a checked SID string.
    pub fn bind_for(name: &str, sid: &str) -> Result<Listener> {
        Listener::bind_sddl(
            name,
            &format!("D:P(A;;0x{HELPER_ACCESS:x};;;{sid})(A;;GA;;;SY)"),
        )
    }

    fn bind_sddl(name: &str, sddl: &str) -> Result<Listener> {
        let sddl = wide(sddl);
        let mut descriptor = PSECURITY_DESCRIPTOR::default();
        // SAFETY: NUL-terminated input; the output is freed by `Descriptor`.
        unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                PCWSTR(sddl.as_ptr()),
                SDDL_REVISION_1,
                &mut descriptor,
                None,
            )
        }
        .map_err(|e| pipe_error("cannot build the pipe's access list", e))?;
        let mut listener = Listener {
            name: wide(name),
            descriptor: Descriptor(descriptor),
            next: INVALID_HANDLE_VALUE,
        };
        listener.next = listener.instance(true)?;
        Ok(listener)
    }

    fn instance(&self, first: bool) -> Result<HANDLE> {
        let attributes = SECURITY_ATTRIBUTES {
            nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: self.descriptor.0.0,
            bInheritHandle: false.into(),
        };
        let mut mode = PIPE_ACCESS_DUPLEX;
        if first {
            mode |= FILE_FLAG_FIRST_PIPE_INSTANCE;
        }
        // SAFETY: name and attributes are valid for the call.
        let handle = unsafe {
            CreateNamedPipeW(
                PCWSTR(self.name.as_ptr()),
                mode,
                PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
                PIPE_UNLIMITED_INSTANCES,
                BUFFER,
                BUFFER,
                0,
                Some(&attributes),
            )
        };
        if handle == INVALID_HANDLE_VALUE {
            return Err(pipe_error(
                "cannot create the pipe",
                windows::core::Error::from_thread(),
            ));
        }
        Ok(handle)
    }

    /// Waits for the next client and returns its connection.
    pub fn accept(&mut self) -> Result<File> {
        if self.next == INVALID_HANDLE_VALUE {
            self.next = self.instance(false)?;
        }
        let handle = self.next;
        // SAFETY: a valid pipe instance; blocking call.
        let connected = match unsafe { ConnectNamedPipe(handle, None) } {
            Ok(()) => Ok(()),
            Err(e) if e.code() == ERROR_PIPE_CONNECTED.to_hresult() => Ok(()),
            Err(e) => Err(e),
        };
        self.next = INVALID_HANDLE_VALUE;
        if let Err(e) = connected {
            // SAFETY: the instance is ours and closed once.
            let _ = unsafe { CloseHandle(handle) };
            return Err(pipe_error("cannot accept a client", e));
        }
        // SAFETY: the handle is ours; the File closes it.
        Ok(unsafe { File::from_raw_handle(handle.0) })
    }
}

impl Drop for Listener {
    fn drop(&mut self) {
        if self.next != INVALID_HANDLE_VALUE {
            // SAFETY: owned and closed once.
            let _ = unsafe { CloseHandle(self.next) };
        }
    }
}

/// Whose pipe a client expects.
#[derive(Clone, Copy)]
pub enum Peer {
    /// The tray: a process of the same user.
    Tray,
    /// The helper: a process of the local system account.
    Helper,
}

impl Peer {
    fn code(self) -> Code {
        match self {
            Peer::Tray => Code::TrayUnreachable,
            Peer::Helper => Code::HelperUnreachable,
        }
    }

    fn what(self) -> &'static str {
        match self {
            Peer::Tray => "the tray's pipe",
            Peer::Helper => "the helper's pipe",
        }
    }
}

/// The access the helper's pipe grants its owner, and what a client asks
/// for: read and write, but not `FILE_APPEND_DATA`, which on a pipe is the
/// right to create further instances of it.
const HELPER_ACCESS: u32 = (FILE_GENERIC_READ.0 | FILE_GENERIC_WRITE.0) & !0x4;

/// Connects to the tray's pipe `name`. `Ok(None)` if no such pipe exists.
/// Waits up to `busy` while all instances are busy. Fails with
/// `TrayUnreachable` if the server runs as another user.
pub fn connect(name: &str, busy: Duration) -> Result<Option<File>> {
    connect_peer(name, busy, Peer::Tray)
}

/// Like [`connect`], for either end; a helper's server must run as SYSTEM.
pub fn connect_peer(name: &str, busy: Duration, peer: Peer) -> Result<Option<File>> {
    let wide_name = wide(name);
    let deadline = Instant::now() + busy;
    let handle = loop {
        // SAFETY: NUL-terminated name; the handle is wrapped below.
        let result = unsafe {
            CreateFileW(
                PCWSTR(wide_name.as_ptr()),
                match peer {
                    Peer::Tray => (FILE_GENERIC_READ | FILE_GENERIC_WRITE).0,
                    Peer::Helper => HELPER_ACCESS,
                },
                FILE_SHARE_NONE,
                None,
                OPEN_EXISTING,
                FILE_FLAGS_AND_ATTRIBUTES(SECURITY_SQOS_PRESENT.0 | SECURITY_IDENTIFICATION.0),
                None,
            )
        };
        match result {
            Ok(handle) => break handle,
            Err(e) if e.code() == ERROR_FILE_NOT_FOUND.to_hresult() => return Ok(None),
            Err(e) if e.code() == ERROR_PIPE_BUSY.to_hresult() => {
                let left = deadline.saturating_duration_since(Instant::now());
                if left.is_zero() {
                    return Err(Error::new(
                        peer.code(),
                        format!("{} stays busy", peer.what()),
                    ));
                }
                // SAFETY: NUL-terminated name.
                let _ =
                    unsafe { WaitNamedPipeW(PCWSTR(wide_name.as_ptr()), left.as_millis() as u32) };
            }
            Err(e) => {
                return Err(Error::new(
                    peer.code(),
                    format!("cannot open {}: {e}", peer.what()),
                ));
            }
        }
    };
    // SAFETY: the handle is ours; the File closes it on every path below.
    let file = unsafe { File::from_raw_handle(handle.0) };
    match peer {
        Peer::Tray => {
            if server_user(&file)? != current_user_sid()? {
                return Err(Error::new(
                    peer.code(),
                    "the pipe is served by a process of another user; not used",
                ));
            }
        }
        // A standard user cannot open the token of a SYSTEM process, so the
        // helper's server is identified by its process id instead: the
        // caller compares `server_pid` with the service's (see
        // `helper::client`).
        Peer::Helper => {}
    }
    Ok(Some(file))
}

fn server_user(file: &File) -> Result<String> {
    let pipe = HANDLE(file.as_raw_handle());
    let mut pid = 0u32;
    // SAFETY: valid pipe handle and out pointer; the process handle is
    // closed below.
    unsafe {
        GetNamedPipeServerProcessId(pipe, &mut pid)
            .map_err(|e| pipe_error("cannot identify the pipe server", e))?;
        let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid)
            .map_err(|e| pipe_error("cannot open the pipe server", e))?;
        let sid = user_sid_of(process);
        let _ = CloseHandle(process);
        sid
    }
}

/// The PID of the server at the other end of a client connection.
pub fn server_pid(file: &File) -> Option<u32> {
    let mut pid = 0u32;
    // SAFETY: valid pipe handle and out pointer.
    unsafe { GetNamedPipeServerProcessId(HANDLE(file.as_raw_handle()), &mut pid) }.ok()?;
    Some(pid)
}

/// The PID of the client at the other end of a server connection (T16).
pub fn client_pid(file: &File) -> Option<u32> {
    let mut pid = 0u32;
    // SAFETY: valid pipe handle and out pointer.
    unsafe { GetNamedPipeClientProcessId(HANDLE(file.as_raw_handle()), &mut pid) }.ok()?;
    Some(pid)
}

/// Runs `io` (blocking pipe I/O on this thread) and ends it with
/// `CancelSynchronousIo` once `timeout` has passed; the I/O then fails.
/// For a client that connects and never sends, or never reads, so it
/// cannot hold a thread of the tray for ever.
///
/// `io` should do nothing but the I/O: a cancel that is already under way
/// when it returns may also hit a synchronous call that follows inside
/// `io`, but never one after this function returned. Without a thread
/// handle `io` simply runs without a deadline.
pub fn with_deadline<T>(timeout: Duration, io: impl FnOnce() -> T) -> T {
    let deadline = Deadline::new(timeout);
    let result = io();
    drop(deadline);
    result
}

/// See [`with_deadline`]. Dropping it waits until the watcher is done, so
/// no cancel can come later.
struct Deadline {
    shared: std::sync::Arc<(std::sync::Mutex<bool>, std::sync::Condvar)>,
    watcher: Option<std::thread::JoinHandle<()>>,
}

/// A thread handle that may be sent to the watcher.
struct ThreadHandle(HANDLE);
// SAFETY: a kernel handle is valid in every thread of the process.
unsafe impl Send for ThreadHandle {}

impl Drop for ThreadHandle {
    fn drop(&mut self) {
        // SAFETY: owned and closed once.
        let _ = unsafe { CloseHandle(self.0) };
    }
}

impl Deadline {
    /// `None` if the thread handle cannot be had; the I/O then simply has
    /// no deadline.
    fn new(timeout: Duration) -> Option<Deadline> {
        let mut thread = HANDLE::default();
        // SAFETY: duplicates this thread's pseudo handle into a real one
        // with only the right CancelSynchronousIo needs.
        unsafe {
            DuplicateHandle(
                GetCurrentProcess(),
                GetCurrentThread(),
                GetCurrentProcess(),
                &mut thread,
                THREAD_TERMINATE.0,
                false,
                DUPLICATE_HANDLE_OPTIONS(0),
            )
        }
        .ok()?;
        let thread = ThreadHandle(thread);
        let shared = std::sync::Arc::new((std::sync::Mutex::new(false), std::sync::Condvar::new()));
        let watch = std::sync::Arc::clone(&shared);
        let watcher = std::thread::Builder::new()
            .name("pipe deadline".to_owned())
            .spawn(move || {
                let thread = thread;
                let (done, signal) = &*watch;
                let guard = done
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                let (guard, _) = signal
                    .wait_timeout_while(guard, timeout, |done| !*done)
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                if !*guard {
                    // Under the lock: the watched thread cannot leave
                    // `with_deadline` before this cancel is done.
                    // SAFETY: a valid thread handle with THREAD_TERMINATE.
                    let _ = unsafe { CancelSynchronousIo(thread.0) };
                }
            })
            .ok()?;
        Some(Deadline {
            shared,
            watcher: Some(watcher),
        })
    }
}

impl Drop for Deadline {
    fn drop(&mut self) {
        let (done, signal) = &*self.shared;
        *done
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = true;
        signal.notify_all();
        if let Some(watcher) = self.watcher.take() {
            let _ = watcher.join();
        }
    }
}

/// The DACL of the pipe behind `file` in SDDL form, for tests and checks.
pub fn dacl(file: &File) -> Result<String> {
    let handle = HANDLE(file.as_raw_handle());
    let mut descriptor = PSECURITY_DESCRIPTOR::default();
    // SAFETY: the returned descriptor is freed by `Descriptor`.
    unsafe {
        let status = GetSecurityInfo(
            handle,
            SE_KERNEL_OBJECT,
            DACL_SECURITY_INFORMATION,
            None,
            None,
            None,
            None,
            Some(&mut descriptor),
        );
        if status.is_err() {
            return Err(pipe_error("cannot read the pipe's access list", status.0));
        }
        let descriptor = Descriptor(descriptor);
        let mut text = PWSTR::null();
        ConvertSecurityDescriptorToStringSecurityDescriptorW(
            descriptor.0,
            SDDL_REVISION_1,
            DACL_SECURITY_INFORMATION,
            &mut text,
            None,
        )
        .map_err(|e| pipe_error("cannot format the access list", e))?;
        Ok(take_local_string(text))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader, Write};

    fn name() -> String {
        use std::sync::atomic::{AtomicU32, Ordering};
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        format!("\\\\.\\pipe\\kaltstellen-test-{}-{n}", std::process::id())
    }

    #[test]
    fn l1_pipe_admits_only_the_current_user() {
        let name = name();
        let mut listener = Listener::bind(&name).unwrap();
        let server = std::thread::spawn(move || {
            let mut conn = listener.accept().unwrap();
            let mut line = String::new();
            BufReader::new(&conn).read_line(&mut line).unwrap();
            conn.write_all(line.to_uppercase().as_bytes()).unwrap();
            conn.sync_all().unwrap();
        });
        let mut client = connect(&name, Duration::from_secs(2)).unwrap().unwrap();
        let sid = current_user_sid().unwrap();
        assert!(sid.starts_with("S-1-5-"), "{sid}");
        // Exactly one entry, for this user, and inheritance blocked.
        let dacl = dacl(&client).unwrap();
        let trustee = dacl
            .strip_prefix("D:P(A;;FA;;;")
            .and_then(|rest| rest.strip_suffix(')'))
            .unwrap_or_else(|| panic!("unexpected DACL {dacl}"));
        // SDDL abbreviates the built-in accounts (CI runs as the local
        // administrator): RID 500 is written `LA`, RID 501 `LG`.
        let alias = match sid.rsplit('-').next() {
            Some("500") => Some("LA"),
            Some("501") => Some("LG"),
            _ => None,
        };
        assert!(
            trustee == sid || Some(trustee) == alias,
            "DACL {dacl} does not name only {sid}"
        );
        client.write_all(b"ping\n").unwrap();
        let mut answer = String::new();
        BufReader::new(&client).read_line(&mut answer).unwrap();
        assert_eq!(answer, "PING\n");
        server.join().unwrap();
    }

    #[test]
    fn l1_a_silent_client_cannot_hold_a_server_thread() {
        /// Runs `serve` on the server end while the client connects and
        /// then neither sends nor reads; returns what `serve` returned and
        /// how long it took.
        fn against_a_silent_client<T: Send + 'static>(
            serve: impl FnOnce(&File) -> T + Send + 'static,
        ) -> (T, Duration) {
            let name = name();
            let mut listener = Listener::bind(&name).unwrap();
            let server = std::thread::spawn(move || {
                let conn = listener.accept().unwrap();
                let started = Instant::now();
                let result = serve(&conn);
                (result, started.elapsed())
            });
            let _client = connect(&name, Duration::from_secs(2)).unwrap().unwrap();
            server.join().unwrap()
        }
        let short = Duration::from_millis(300);
        // Never sends: the read fails.
        let (read, took) = against_a_silent_client(move |conn| {
            with_deadline(short, || {
                BufReader::new(conn).read_line(&mut String::new()).is_err()
            })
        });
        assert!(read, "the read must fail at the deadline");
        assert!(took < Duration::from_secs(5), "took {took:?}");
        // Never reads: a reply larger than the pipe buffer fails, and so
        // does waiting until a small one was read.
        let (write, took) = against_a_silent_client(move |mut conn| {
            with_deadline(short, || conn.write_all(&[b'x'; 1 << 20]).is_err())
        });
        assert!(write, "the write must fail at the deadline");
        assert!(took < Duration::from_secs(5), "took {took:?}");
        let (flush, took) = against_a_silent_client(move |mut conn| {
            conn.write_all(b"small\n").unwrap();
            with_deadline(short, || conn.sync_all().is_err())
        });
        assert!(flush, "the flush must fail at the deadline");
        assert!(took < Duration::from_secs(5), "took {took:?}");

        // A client that sends in time is not disturbed, and a deadline that
        // has ended does not touch later reads of the thread.
        let name = name();
        let mut listener = Listener::bind(&name).unwrap();
        let server = std::thread::spawn(move || {
            let conn = listener.accept().unwrap();
            let mut reader = BufReader::new(&conn);
            let mut first = String::new();
            with_deadline(Duration::from_secs(5), || reader.read_line(&mut first)).unwrap();
            let mut second = String::new();
            reader.read_line(&mut second).unwrap();
            (first, second)
        });
        let mut client = connect(&name, Duration::from_secs(2)).unwrap().unwrap();
        client.write_all(b"one\n").unwrap();
        std::thread::sleep(Duration::from_millis(500));
        client.write_all(b"two\n").unwrap();
        let (first, second) = server.join().unwrap();
        assert_eq!((first.as_str(), second.as_str()), ("one\n", "two\n"));
    }

    #[test]
    fn no_pipe_means_none_and_a_taken_name_fails() {
        assert!(connect(&name(), Duration::ZERO).unwrap().is_none());
        let name = name();
        let _first = Listener::bind(&name).unwrap();
        let err = Listener::bind(&name).err().expect("second bind must fail");
        assert_eq!(err.code, Code::TrayPipe);
    }
}
