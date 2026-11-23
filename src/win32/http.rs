//! One HTTP(S) `GET` through WinHTTP (spec UP1), the only network access of
//! kaltstellen. WinHTTP ships with Windows: TLS through Schannel with the
//! system's certificate store, the system's proxy settings, redirects
//! followed (never from HTTPS to HTTP, WinHTTP's default policy). Which URL
//! may be fetched is decided by the safe caller ([`crate::update`]).

use std::ffi::c_void;
use std::time::Duration;

use windows::Win32::Networking::WinHttp::{
    WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY, WINHTTP_FLAG_SECURE, WINHTTP_OPEN_REQUEST_FLAGS,
    WINHTTP_QUERY_FLAG_NUMBER, WINHTTP_QUERY_STATUS_CODE, WinHttpCloseHandle, WinHttpConnect,
    WinHttpOpen, WinHttpOpenRequest, WinHttpQueryDataAvailable, WinHttpQueryHeaders,
    WinHttpReadData, WinHttpReceiveResponse, WinHttpSendRequest, WinHttpSetTimeouts,
};
use windows::core::PCWSTR;

use super::wide;

/// Where to send the request, already checked by the caller.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target<'a> {
    pub secure: bool,
    pub host: &'a str,
    pub port: u16,
    /// Path and query, starting with `/`.
    pub path: &'a str,
}

/// The response of a `GET`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Response {
    pub status: u32,
    pub body: Vec<u8>,
}

/// A WinHTTP handle, closed when dropped.
struct Handle(*mut c_void);

impl Handle {
    fn new(raw: *mut c_void, what: &str) -> Result<Handle, String> {
        if raw.is_null() {
            Err(format!("{what}: {}", windows::core::Error::from_thread()))
        } else {
            Ok(Handle(raw))
        }
    }
}

impl Drop for Handle {
    fn drop(&mut self) {
        // SAFETY: a handle WinHTTP returned and nobody closed yet.
        let _ = unsafe { WinHttpCloseHandle(self.0) };
    }
}

/// `GET target` with `accept` as the `Accept` header. Reads at most `limit`
/// bytes of the body; a longer body is an error. `timeout` applies to each
/// step (name resolution, connect, send, each receive).
pub fn get(
    target: &Target<'_>,
    agent: &str,
    accept: &str,
    limit: usize,
    timeout: Duration,
) -> Result<Response, String> {
    let agent = wide(agent);
    let host = wide(target.host);
    let path = wide(target.path);
    let verb = wide("GET");
    let headers: Vec<u16> = format!("Accept: {accept}\r\n").encode_utf16().collect();
    let ms = i32::try_from(timeout.as_millis()).unwrap_or(i32::MAX);
    let flags = if target.secure {
        WINHTTP_FLAG_SECURE
    } else {
        WINHTTP_OPEN_REQUEST_FLAGS(0)
    };
    // SAFETY: every pointer passed points to a live, NUL-terminated buffer
    // or is null where the API allows it; handles are closed by `Handle`
    // in reverse order of creation (request, connection, session).
    unsafe {
        let session = Handle::new(
            WinHttpOpen(
                PCWSTR(agent.as_ptr()),
                WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY,
                PCWSTR::null(),
                PCWSTR::null(),
                0,
            ),
            "cannot open WinHTTP",
        )?;
        WinHttpSetTimeouts(session.0, ms, ms, ms, ms).map_err(|e| e.to_string())?;
        let connection = Handle::new(
            WinHttpConnect(session.0, PCWSTR(host.as_ptr()), target.port, 0),
            "cannot connect",
        )?;
        let request = Handle::new(
            WinHttpOpenRequest(
                connection.0,
                PCWSTR(verb.as_ptr()),
                PCWSTR(path.as_ptr()),
                PCWSTR::null(),
                PCWSTR::null(),
                std::ptr::null(),
                flags,
            ),
            "cannot open the request",
        )?;
        WinHttpSendRequest(request.0, Some(&headers), None, 0, 0, 0)
            .map_err(|e| format!("cannot send the request: {e}"))?;
        WinHttpReceiveResponse(request.0, std::ptr::null_mut())
            .map_err(|e| format!("no response: {e}"))?;
        let mut status: u32 = 0;
        let mut size = std::mem::size_of::<u32>() as u32;
        WinHttpQueryHeaders(
            request.0,
            WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER,
            PCWSTR::null(),
            Some((&raw mut status).cast()),
            &mut size,
            std::ptr::null_mut(),
        )
        .map_err(|e| format!("no status code: {e}"))?;
        let mut body = Vec::new();
        loop {
            let mut available: u32 = 0;
            WinHttpQueryDataAvailable(request.0, &mut available)
                .map_err(|e| format!("cannot read the response: {e}"))?;
            if available == 0 {
                break;
            }
            let start = body.len();
            if start + available as usize > limit {
                return Err(format!("the response is larger than {limit} bytes"));
            }
            body.resize(start + available as usize, 0);
            let mut read: u32 = 0;
            WinHttpReadData(
                request.0,
                body[start..].as_mut_ptr().cast(),
                available,
                &mut read,
            )
            .map_err(|e| format!("cannot read the response: {e}"))?;
            body.truncate(start + read as usize);
            if read == 0 {
                break;
            }
        }
        Ok(Response { status, body })
    }
}
