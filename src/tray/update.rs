//! The tray's part of the updates (spec UP1): one timer for the daily check,
//! the check itself in a child process, and what the menu shows.
//!
//! No network code runs in the tray: the check is `kaltstellen update
//! --check --quiet`, which writes `update.json`; the tray only reads that
//! file. While no check runs there is no thread and no polling (Q1).

use std::os::windows::process::CommandExt;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::Duration;

use log::{info, warn};

use super::model::UpdateOffer;
use crate::paths;
use crate::update::{self, Mode, Source, record};
use crate::win32::tray::Waker;

const CREATE_NO_WINDOW: u32 = 0x0800_0000;
const CREATE_NEW_CONSOLE: u32 = 0x0000_0010;

pub struct Updates {
    waker: Waker,
    /// Seconds since 1970 when the tray started.
    started: u64,
    /// Set while a check child runs; it sends once when it ended.
    ended: Option<Receiver<()>>,
}

impl Updates {
    pub fn new(waker: Waker) -> Self {
        Self {
            waker,
            started: record::now(),
            ended: None,
        }
    }

    /// How long until the next automatic check; `None` when there is none:
    /// mode `off`, a development build without a named source, or a check
    /// running now.
    pub fn next_delay(&self, config_dir: Option<&Path>) -> Option<Duration> {
        let (mode, _) = update::current_mode(config_dir);
        if self.ended.is_some() || !mode.checks() || !automatic(&Source::current()) {
            return None;
        }
        let last = paths::data_dir().and_then(|dir| record::load(&dir).0.last_check);
        Some(record::next_check(record::now(), self.started, last))
    }

    /// Starts `kaltstellen update --check --quiet` without a console.
    pub fn start_check(&mut self) {
        if self.ended.is_some() {
            return;
        }
        let child = std::env::current_exe().and_then(|exe| {
            Command::new(exe)
                .args(["update", "--check", "--quiet"])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .creation_flags(CREATE_NO_WINDOW)
                .spawn()
        });
        let mut child = match child {
            Ok(child) => child,
            Err(err) => {
                warn!("tray: cannot start the update check: {err}");
                return;
            }
        };
        info!("tray: update check started (pid {})", child.id());
        let (tx, rx): (Sender<()>, Receiver<()>) = mpsc::channel();
        let waker = self.waker.clone();
        // Only while the check runs: waits for the child, then wakes the
        // window thread.
        std::thread::spawn(move || {
            let _ = child.wait();
            let _ = tx.send(());
            waker.wake();
        });
        self.ended = Some(rx);
    }

    /// Whether a check child ended since the last call.
    pub fn check_ended(&mut self) -> bool {
        let done = self
            .ended
            .as_ref()
            .is_some_and(|rx| !matches!(rx.try_recv(), Err(mpsc::TryRecvError::Empty)));
        if done {
            self.ended = None;
        }
        done
    }
}

/// Whether this build checks by itself (UP1 "Versions").
fn automatic(source: &Source) -> bool {
    update::RELEASE_BUILD || source.named
}

/// For the menu: the mode, whether the environment sets it, and the newer
/// version from `update.json` with the reason it cannot be installed here.
pub fn menu_state(config_dir: Option<&Path>) -> (Mode, bool, Option<UpdateOffer>) {
    let (mode, locked) = update::current_mode(config_dir);
    let offer = paths::data_dir().and_then(|dir| {
        let (rec, problem) = record::load(&dir);
        if let Some(problem) = problem {
            warn!("tray: {problem}");
        }
        let version = update::known_newer(&rec)?.to_owned();
        let blocked = update::may_install(update::RELEASE_BUILD, &Source::current())
            .map_err(|e| e.reason)
            .and_then(|()| match update::running_exe() {
                Ok(exe) => update::placement(&exe, &update::program_files()),
                Err(err) => Err(err.reason),
            })
            .err();
        Some(UpdateOffer { version, blocked })
    });
    (mode, locked, offer)
}

/// Logs what the last check found.
pub fn log_result(offer: Option<&UpdateOffer>) {
    let rec = paths::data_dir().map(|dir| record::load(&dir).0);
    match (offer, rec.as_ref().and_then(|r| r.error.as_deref())) {
        (_, Some(error)) => info!("tray: update check failed: {error}"),
        (Some(offer), None) => info!("tray: update {} available", offer.version),
        (None, None) => info!(
            "tray: update check: {} is the newest version",
            update::CURRENT
        ),
    }
}

/// UP1 automatic install: `kaltstellen update` without a console window;
/// its steps go to the log. That process quits this tray and starts the new
/// one.
pub fn install_unattended() {
    let started = std::env::current_exe().and_then(|exe| {
        Command::new(exe)
            .arg("update")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .creation_flags(CREATE_NO_WINDOW)
            .spawn()
    });
    match started {
        Ok(child) => info!("tray: automatic update started (pid {})", child.id()),
        Err(err) => warn!("tray: cannot start the update: {err}"),
    }
}

/// `Install update…`: a console window with `kaltstellen update`, so the
/// steps and any error are visible. That process quits this tray.
pub fn open_install() {
    let started = std::env::current_exe().and_then(|exe| {
        Command::new(exe)
            .args(["update", "--from-tray"])
            .creation_flags(CREATE_NEW_CONSOLE)
            .spawn()
    });
    match started {
        Ok(child) => info!("tray: update started (pid {})", child.id()),
        Err(err) => warn!("tray: cannot start the update: {err}"),
    }
}
