//! The command line's side of the pipe (L1): talk to the running tray, or
//! report that there is none so the command runs in-process.

use std::fs::File;
use std::io::{BufRead, BufReader, Read, Write};
use std::thread::sleep;
use std::time::{Duration, Instant};

use super::Endpoints;
use super::protocol::{self, Body, Command};
use crate::error::{Code, Error, Result};
use crate::win32::{instance::Instance, pipe};

/// How long a starting tray (mutex taken, pipe not yet there) is waited for.
const STARTING: Duration = Duration::from_secs(5);
/// How long all pipe instances may be busy.
const BUSY: Duration = Duration::from_secs(5);

fn unreachable(reason: impl Into<String>) -> Error {
    Error::new(Code::TrayUnreachable, reason)
}

/// Sends `command` to the tray. `Ok(None)`: no tray runs in this session.
/// Otherwise the final reply, or `Accepted` for a request that does not
/// wait. `on_waiting` is called when the request waits behind a running
/// switch. Errors are `KS501` or `KS505` (exit code 5).
pub fn request(
    endpoints: &Endpoints,
    command: Command,
    mut on_waiting: impl FnMut(),
) -> Result<Option<Body>> {
    let deadline = Instant::now() + STARTING;
    let conn = loop {
        match pipe::connect(&endpoints.pipe, BUSY)? {
            Some(conn) => break conn,
            None if !Instance::exists(&endpoints.mutex) => return Ok(None),
            // The tray is starting and has no pipe yet.
            None if Instant::now() < deadline => sleep(Duration::from_millis(100)),
            None => {
                return Err(unreachable(
                    "a tray runs in this session but does not answer on its pipe",
                ));
            }
        }
    };
    converse(conn, command, &mut on_waiting).map(Some)
}

/// How long [`status_now`] waits while all pipe instances are busy.
const QUICK: Duration = Duration::from_millis(500);

/// M5: the tray's status for the main window, which must not hang on its
/// own thread: no waiting for a tray that is still starting and only a
/// short one for a busy pipe, and at most [`ANSWER`] for the answer (a tray
/// that accepted but is slow, e.g. in its end-of-session wait). `Ok(None)`
/// when no tray has a pipe (none runs, or it is still starting and knows
/// no switch yet); `Err` when a tray did not answer that way.
pub fn status_now(endpoints: &Endpoints) -> Result<Option<protocol::TrayStatus>> {
    let Some(conn) = pipe::connect(&endpoints.pipe, QUICK)? else {
        return Ok(None);
    };
    match pipe::with_deadline(ANSWER, || converse(conn, Command::Status, &mut || {}))? {
        Body::Status(status) => Ok(Some(status)),
        _ => Err(unreachable("the tray did not answer the status request")),
    }
}

/// How long [`status_now`] waits for the tray's answer once connected.
const ANSWER: Duration = Duration::from_secs(1);

fn converse(mut conn: File, command: Command, on_waiting: &mut dyn FnMut()) -> Result<Body> {
    let waits = command.waits();
    conn.write_all(protocol::encode(&protocol::request(command)).as_bytes())
        .map_err(|e| unreachable(format!("cannot send to the tray: {e}")))?;
    let mut reader = BufReader::new(conn);
    loop {
        let mut line = String::new();
        let read = (&mut reader)
            .take(protocol::MAX_LINE as u64)
            .read_line(&mut line)
            .map_err(|e| unreachable(format!("lost the tray: {e}")))?;
        if read == 0 {
            return Err(unreachable(
                "the tray closed the connection without an answer",
            ));
        }
        let body = protocol::decode_reply(line.trim_end())?.body;
        match body {
            Body::Accepted { waiting } if waits => {
                if waiting {
                    on_waiting();
                }
            }
            body => return Ok(body),
        }
    }
}
