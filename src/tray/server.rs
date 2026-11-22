//! The tray's side of the pipe (L1): one thread accepts clients, one short
//! thread per client reads its request, hands it to the window thread and
//! writes the replies back. Decisions are made on the window thread.

use std::fs::File;
use std::io::{BufRead, BufReader, Read, Write};
use std::sync::mpsc::{Sender, channel};
use std::thread;
use std::time::Duration;

use log::{info, warn};

use super::protocol::{self, Body, Command};
use crate::error::Code;
use crate::exit;
use crate::process::{Guest, Snapshot};
use crate::win32::pipe::{self, Listener};

/// A request for the window thread and where its replies go.
pub struct Incoming {
    pub command: Command,
    pub reply: Sender<Body>,
    /// The client process, from the pipe itself (not from the message),
    /// with its creation time from the snapshot (T16).
    pub caller: Option<Guest>,
}

/// The process at the other end of `conn`, identified by PID and creation
/// time so that a reused PID cannot pose as it.
fn caller_of(conn: &File) -> Option<Guest> {
    let pid = pipe::client_pid(conn)?;
    let snapshot = Snapshot::take().ok()?;
    let created = snapshot.get(pid)?.created;
    Some(Guest { pid, created })
}

/// How long a client may take to send its request, and to read the last
/// reply. The request is one short line written right after connecting.
pub const IDLE_CLIENT: Duration = Duration::from_secs(10);

fn rejected(exit_code: u8, line: String) -> Body {
    Body::Rejected {
        exit_code,
        lines: vec![line],
    }
}

/// Serves `listener` until the process ends. `wake` makes the window thread
/// look at its inbox.
pub fn spawn(
    mut listener: Listener,
    inbox: Sender<Incoming>,
    wake: impl Fn() + Clone + Send + 'static,
) {
    let accept = move || {
        let mut failures = 0;
        loop {
            match listener.accept() {
                Ok(conn) => {
                    failures = 0;
                    let inbox = inbox.clone();
                    let wake = wake.clone();
                    let spawned = thread::Builder::new()
                        .name("pipe client".to_owned())
                        .spawn(move || serve(conn, &inbox, wake));
                    if let Err(err) = spawned {
                        warn!("tray: cannot serve a pipe client: {err}");
                    }
                }
                Err(err) => {
                    warn!("tray: {err}");
                    failures += 1;
                    if failures > 10 {
                        warn!("tray: giving up on the pipe");
                        return;
                    }
                    thread::sleep(Duration::from_millis(200));
                }
            }
        }
    };
    if let Err(err) = thread::Builder::new().name("pipe".to_owned()).spawn(accept) {
        warn!("tray: cannot start the pipe thread: {err}");
    }
}

/// Writes one reply; a client that stops reading gets [`IDLE_CLIENT`].
fn send(conn: &mut File, body: Body) -> bool {
    let line = protocol::encode(&protocol::reply(body));
    pipe::with_deadline(IDLE_CLIENT, || conn.write_all(line.as_bytes())).is_ok()
}

/// Waits until the client has read everything, then the connection closes;
/// a client that stops reading gets [`IDLE_CLIENT`].
fn finish(conn: &File) {
    let _ = pipe::with_deadline(IDLE_CLIENT, || conn.sync_all());
}

fn serve(mut conn: File, inbox: &Sender<Incoming>, wake: impl Fn()) {
    let mut line = String::new();
    // A client that connects and never sends (or never ends its line) must
    // not hold this thread for ever.
    let read = pipe::with_deadline(IDLE_CLIENT, || {
        BufReader::new((&conn).take(protocol::MAX_LINE as u64)).read_line(&mut line)
    });
    if let Err(err) = &read {
        info!("tray: pipe client sent no request: {err}");
    }
    let request = match read {
        Ok(0) | Err(_) => return,
        Ok(_) => protocol::decode_request(line.trim_end()),
    };
    let command = match request {
        Ok(request) => request.command,
        Err(err) => {
            warn!("tray: pipe: {err}");
            send(
                &mut conn,
                rejected(exit::UNREACHABLE, format!("error: {err}")),
            );
            finish(&conn);
            return;
        }
    };
    info!("tray: pipe request {command:?}");
    let waits = command.waits();
    let (reply, replies) = channel();
    let caller = caller_of(&conn);
    if inbox
        .send(Incoming {
            command,
            reply,
            caller,
        })
        .is_err()
    {
        send(
            &mut conn,
            rejected(
                exit::UNREACHABLE,
                format!("error: {}: the tray is ending", Code::TrayUnreachable),
            ),
        );
        finish(&conn);
        return;
    }
    wake();
    loop {
        match replies.recv() {
            Ok(body) => {
                let last = body.is_final() || !waits;
                if !send(&mut conn, body) || last {
                    break;
                }
            }
            Err(_) => {
                send(
                    &mut conn,
                    rejected(
                        exit::UNREACHABLE,
                        format!(
                            "error: {}: the tray ended before the request finished",
                            Code::TrayUnreachable
                        ),
                    ),
                );
                break;
            }
        }
    }
    // Waits until the client has read everything, then the connection closes.
    finish(&conn);
}
