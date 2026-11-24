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

use super::events;
use super::protocol::{self, Context};
use super::{PIPE_NAME, SERVICE_NAME, data_dir, policy, policy_file, version};
use crate::error::{Code, Error, Result};
use crate::win32::etw;
use crate::win32::pipe::{self, Listener, Peer};
use crate::win32::service::{self, Stop};

/// Clients served at the same time; more wait in the pipe's queue.
const MAX_CLIENTS: usize = 8;
/// How long one conversation may take.
const DEADLINE: Duration = Duration::from_secs(5);
/// Name of the kernel event session.
const EVENT_SESSION: &str = "kaltstellen-helper-process";

/// One of the client slots; given back on drop.
struct Slot(Arc<AtomicUsize>);

impl Drop for Slot {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

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
    // The pipe's name can be taken by any local user before the delayed
    // start; try again for a while before giving up.
    let mut attempt = 0;
    let mut listener = loop {
        match Listener::bind_for(PIPE_NAME, &policy.owner_sid) {
            Ok(listener) => break listener,
            Err(err) if attempt < 12 && !stop.is_set() => {
                attempt += 1;
                warn!("{err}; trying again in 5 s");
                std::thread::sleep(Duration::from_secs(5));
            }
            Err(err) => return Err(Error::new(Code::HelperService, err.reason)),
        }
    };
    info!(
        "helper {} serves {} (roles: observe {}, act {}, services {}; only observe is implemented)",
        version(),
        policy.owner_sid,
        policy.roles.observe,
        policy.roles.act,
        policy.roles.services
    );
    // The role observe: a kernel event session for process start and stop.
    // Without it (not switched on, or it cannot start) the helper still
    // answers; the status then says the role does not run.
    let ring = Arc::new(events::Ring::new());
    let mut session = None;
    if policy.roles.observe {
        let sink = Arc::clone(&ring);
        match etw::start(EVENT_SESSION, move |event| match event {
            etw::Event::Process(event) => sink.push(&event, etw::device_map),
            etw::Event::Network(event) => sink.traffic.add(&event),
        }) {
            Ok(mut started) => {
                match started.enable_network() {
                    Ok(()) => ring.traffic.set_running(true),
                    Err(err) => warn!("observe: no network counters: {err}"),
                }
                info!("observe: the process event session runs");
                ring.set_running(true);
                session = Some(started);
            }
            Err(err) => error!("observe does not run: {err}"),
        }
    }
    let context = Arc::new(Context {
        version: version().to_owned(),
        pid: std::process::id(),
        roles: policy.roles.clone(),
        events: session.as_ref().map(|_| Arc::clone(&ring)),
        session_of: pipe::session_of,
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
                // The slot is given back when the thread ends, however it ends.
                let slot = Slot(active);
                let spawned = std::thread::Builder::new()
                    .name("helper client".to_owned())
                    .spawn(move || {
                        let _slot = slot;
                        handle(conn, &context);
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
    stop.running();
    let mut reported = 0;
    while !stop.wait_for(Duration::from_secs(30)) {
        if let Some(session) = session.as_mut() {
            if ring.running() && !session.alive() {
                ring.set_running(false);
                ring.traffic.set_running(false);
                error!("observe: the process event session has ended; the role does not run");
            }
            if let Some(lost) = session.lost() {
                ring.set_lost(lost);
                if lost > reported {
                    warn!("observe: {lost} process events or buffers lost so far");
                    reported = lost;
                }
            }
        }
    }
    info!("helper stopping");
    drop(session);
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
    // The session of the caller decides which events it may read.
    let session = pipe::client_pid(&conn).and_then(pipe::session_of);
    let mut duplex = Duplex {
        reader: BufReader::new(conn),
        writer,
    };
    let result = pipe::with_deadline(DEADLINE, || protocol::serve(&mut duplex, context, session));
    if let Err(err) = result {
        warn!("{err}");
    }
}
