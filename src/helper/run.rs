//! The service body: `kaltstellen helper run`, started by the service
//! control manager from the protected copy (M4a).
//!
//! It reads the policy, serves the pipe for the recorded owner and answers
//! `hello` and `status`. The roles *observe*, *act* and *services* are
//! added by M4b to M4e.

use std::fs::File;
use std::io::{BufRead, BufReader, Read, Write};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use log::{error, info, warn};

use super::protocol::{self, Context};
use super::{PIPE_NAME, SERVICE_NAME, data_dir, policy, policy_file, version};
use crate::error::{Code, Error, Result};
use crate::win32::pipe::{self, Listener, Peer};
use crate::win32::service::{self, Stop};

/// Clients served at the same time; more wait in the pipe's queue.
const MAX_CLIENTS: usize = 8;
/// How long one conversation may take.
const DEADLINE: Duration = Duration::from_secs(5);

/// A pipe connection read through a buffer and written directly.
struct Duplex {
    reader: BufReader<File>,
    writer: File,
}

impl Read for Duplex {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        self.reader.read(buf)
    }
}

impl BufRead for Duplex {
    fn fill_buf(&mut self) -> std::io::Result<&[u8]> {
        self.reader.fill_buf()
    }
    fn consume(&mut self, amount: usize) {
        self.reader.consume(amount);
    }
}

impl Write for Duplex {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.writer.write(buf)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.writer.flush()
    }
}

/// Runs as the service; returns the process exit code.
pub fn main() -> u8 {
    if let Some(dir) = data_dir()
        && let Err(err) = crate::logging::init(&dir, log::LevelFilter::Info)
    {
        eprintln!(
            "warning: {}: cannot open the log in {}: {err}",
            Code::LogSetup,
            dir.display()
        );
    }
    match service::dispatch(SERVICE_NAME, serve) {
        Ok(()) => 0,
        Err(err) => {
            error!("{err}");
            eprintln!("{err}");
            1
        }
    }
}

fn serve(stop: Stop) -> Result<()> {
    let path = policy_file().ok_or_else(|| Error::new(Code::HelperPolicy, "no data folder"))?;
    let policy = policy::Policy::read(&path)?;
    let mut listener = Listener::bind_for(PIPE_NAME, &policy.owner_sid)
        .map_err(|e| Error::new(Code::HelperService, e.reason))?;
    info!(
        "helper {} serves {} (observe {}, act {}, services {}; none implemented yet)",
        version(),
        policy.owner_sid,
        policy.roles.observe,
        policy.roles.act,
        policy.roles.services
    );
    let context = Arc::new(Context {
        version: version().to_owned(),
        pid: std::process::id(),
        roles: policy.roles.clone(),
    });
    let active = Arc::new(AtomicUsize::new(0));
    let accepting = stop.clone();
    std::thread::Builder::new()
        .name("helper pipe".to_owned())
        .spawn(move || {
            loop {
                let conn = listener.accept();
                if accepting.is_set() {
                    return;
                }
                let conn = match conn {
                    Ok(conn) => conn,
                    Err(err) => {
                        warn!("{err}");
                        std::thread::sleep(Duration::from_millis(200));
                        continue;
                    }
                };
                if active.fetch_add(1, Ordering::SeqCst) >= MAX_CLIENTS {
                    active.fetch_sub(1, Ordering::SeqCst);
                    warn!("too many clients at once; one was dropped");
                    continue;
                }
                let context = Arc::clone(&context);
                let active = Arc::clone(&active);
                let spawned = std::thread::Builder::new()
                    .name("helper client".to_owned())
                    .spawn(move || {
                        handle(conn, &context);
                        active.fetch_sub(1, Ordering::SeqCst);
                    });
                if let Err(err) = spawned {
                    error!("cannot start a client thread: {err}");
                }
            }
        })
        .map_err(|e| {
            Error::new(
                Code::HelperService,
                format!("cannot start the pipe thread: {e}"),
            )
        })?;
    stop.wait();
    info!("helper stopping");
    // Wake the accept loop, which sees the stop flag and ends.
    let _ = pipe::connect_peer(PIPE_NAME, Duration::from_millis(500), Peer::Helper);
    Ok(())
}

fn handle(conn: File, context: &Context) {
    let writer = match conn.try_clone() {
        Ok(writer) => writer,
        Err(err) => {
            warn!("cannot duplicate a connection: {err}");
            return;
        }
    };
    let mut duplex = Duplex {
        reader: BufReader::new(conn),
        writer,
    };
    let result = pipe::with_deadline(DEADLINE, || protocol::serve(&mut duplex, context));
    if let Err(err) = result {
        warn!("{err}");
    }
}
