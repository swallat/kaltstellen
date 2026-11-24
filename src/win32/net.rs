//! H2: how many TCP connections (established) and UDP endpoints each
//! process owns. The tables name the owner, so no process is opened and no
//! administrator right is needed. A hint that a program talks to the
//! network, not a volume (spec H2, R10).

use std::collections::HashMap;
use std::ffi::c_void;

use windows::Win32::NetworkManagement::IpHelper::{
    GetExtendedTcpTable, GetExtendedUdpTable, MIB_TCP6ROW_OWNER_PID, MIB_TCPROW_OWNER_PID,
    MIB_UDP6ROW_OWNER_PID, MIB_UDPROW_OWNER_PID, TCP_TABLE_OWNER_PID_ALL, UDP_TABLE_OWNER_PID,
};
use windows::Win32::Networking::WinSock::{AF_INET, AF_INET6};

/// `MIB_TCP_STATE_ESTAB`.
const ESTABLISHED: u32 = 5;
const NO_ERROR: u32 = 0;
const INSUFFICIENT_BUFFER: u32 = 122;

/// Connections of one process.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Connections {
    pub tcp: u32,
    pub udp: u32,
}

/// Per PID, over IPv4 and IPv6. A table that cannot be read counts as empty
/// (the figures are a hint).
pub fn by_process() -> HashMap<u32, Connections> {
    let mut found: HashMap<u32, Connections> = HashMap::new();
    for pid in tcp_owners(u32::from(AF_INET.0)) {
        found.entry(pid).or_default().tcp += 1;
    }
    for pid in tcp_owners(u32::from(AF_INET6.0)) {
        found.entry(pid).or_default().tcp += 1;
    }
    for pid in udp_owners(u32::from(AF_INET.0)) {
        found.entry(pid).or_default().udp += 1;
    }
    for pid in udp_owners(u32::from(AF_INET6.0)) {
        found.entry(pid).or_default().udp += 1;
    }
    found
}

/// Reads a table into an 8-byte aligned buffer, growing it as asked.
fn table(read: impl Fn(*mut c_void, &mut u32) -> u32) -> Option<Vec<u64>> {
    let mut size = 0u32;
    let mut buffer: Vec<u64> = Vec::new();
    for _ in 0..4 {
        let status = read(buffer.as_mut_ptr().cast(), &mut size);
        match status {
            NO_ERROR => return Some(buffer),
            INSUFFICIENT_BUFFER => buffer = vec![0; (size as usize).div_ceil(8) + 64],
            _ => return None,
        }
    }
    None
}

/// The rows of a `MIB_*TABLE_OWNER_PID` buffer: a `u32` count, then rows.
fn rows<T: Copy>(buffer: &[u64]) -> Vec<T> {
    let bytes = buffer.len() * 8;
    if bytes < 4 {
        return Vec::new();
    }
    let base = buffer.as_ptr().cast::<u8>();
    // SAFETY: the buffer holds at least the 4-byte count.
    let count = unsafe { base.cast::<u32>().read_unaligned() } as usize;
    let row = size_of::<T>();
    // Rows start after the count, aligned like the C struct (4 bytes).
    let mut rows = Vec::new();
    for i in 0..count {
        let start = 4 + i * row;
        if start + row > bytes {
            break;
        }
        // SAFETY: the row lies inside the buffer (checked above); the read is
        // unaligned-safe and `T` is a plain-old-data table row.
        rows.push(unsafe { base.add(start).cast::<T>().read_unaligned() });
    }
    rows
}

fn tcp_owners(family: u32) -> Vec<u32> {
    let read = |data: *mut c_void, size: &mut u32| -> u32 {
        // SAFETY: `data` is null or a buffer of `*size` bytes, `size` is valid.
        unsafe {
            GetExtendedTcpTable(
                if *size == 0 { None } else { Some(data) },
                size,
                false,
                family,
                TCP_TABLE_OWNER_PID_ALL,
                0,
            )
        }
    };
    let Some(buffer) = table(read) else {
        return Vec::new();
    };
    if family == u32::from(AF_INET.0) {
        rows::<MIB_TCPROW_OWNER_PID>(&buffer)
            .into_iter()
            .filter(|r| r.dwState == ESTABLISHED)
            .map(|r| r.dwOwningPid)
            .collect()
    } else {
        rows::<MIB_TCP6ROW_OWNER_PID>(&buffer)
            .into_iter()
            .filter(|r| r.dwState == ESTABLISHED)
            .map(|r| r.dwOwningPid)
            .collect()
    }
}

fn udp_owners(family: u32) -> Vec<u32> {
    let read = |data: *mut c_void, size: &mut u32| -> u32 {
        // SAFETY: `data` is null or a buffer of `*size` bytes, `size` is valid.
        unsafe {
            GetExtendedUdpTable(
                if *size == 0 { None } else { Some(data) },
                size,
                false,
                family,
                UDP_TABLE_OWNER_PID,
                0,
            )
        }
    };
    let Some(buffer) = table(read) else {
        return Vec::new();
    };
    if family == u32::from(AF_INET.0) {
        rows::<MIB_UDPROW_OWNER_PID>(&buffer)
            .into_iter()
            .map(|r| r.dwOwningPid)
            .collect()
    } else {
        rows::<MIB_UDP6ROW_OWNER_PID>(&buffer)
            .into_iter()
            .map(|r| r.dwOwningPid)
            .collect()
    }
}
