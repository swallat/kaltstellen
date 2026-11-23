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
    match request_now(endpoints, Command::Status, ANSWER)? {
        None => Ok(None),
        Some(Body::Status(status)) => Ok(Some(status)),
        Some(_) => Err(unreachable("the tray did not answer the status request")),
    }
}

/// [`request`] with the short waits of [`status_now`], for the main window:
/// no wait for a starting tray, [`QUICK`] for a busy pipe, at most `answer`
/// for the reply.
fn request_now(endpoints: &Endpoints, command: Command, answer: Duration) -> Result<Option<Body>> {
    let Some(conn) = pipe::connect(&endpoints.pipe, QUICK)? else {
        return Ok(None);
    };
    pipe::with_deadline(answer, || converse(conn, command, &mut || {})).map(Some)
}

/// How long [`status_now`] waits for the tray's answer once connected.
const ANSWER: Duration = Duration::from_secs(1);
/// How long the main window waits for the answer to `set_automation` and
/// `set_update_mode`: longer than for `status`, so a tray that is busy for a
/// moment is not reported as one that takes the setting only at its next
/// start (the window waits for it on its own thread only on a click).
const SET_ANSWER: Duration = Duration::from_secs(3);

/// How a setting that moved from the tray menu to the main window (owner,
/// 2026-11-23) took effect.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Delivery {
    /// The running tray took it over at once and wrote `settings.json`;
    /// its status afterwards.
    Tray(Box<protocol::TrayStatus>),
    /// No tray runs: written to `settings.json` here; a tray reads it when
    /// it starts.
    Saved,
    /// An older tray that does not know the command (`KS505`): written to
    /// `settings.json` here; that tray keeps its own setting until it
    /// restarts. The reason, for the log.
    OlderTray(String),
    /// A tray runs but did not answer in time, or refused it because it is
    /// quitting: written to `settings.json` here; whether that tray took
    /// it is not known. The reason, for the log and the caller.
    Unanswered(String),
}

impl Delivery {
    /// For the log.
    pub fn how(&self) -> String {
        match self {
            Delivery::Tray(_) => "taken over by the tray".to_owned(),
            Delivery::Saved => "saved, no tray runs".to_owned(),
            Delivery::OlderTray(reason) => {
                format!("saved, the older tray keeps its own until it restarts: {reason}")
            }
            Delivery::Unanswered(reason) => {
                format!("saved, the tray did not answer in time: {reason}")
            }
        }
    }
}

/// Sends `command` (`set_automation`, `set_update_mode`) to the running
/// tray, which writes `settings.json` itself; without a tray, or when the
/// tray does not take it, `save` writes the file here. `quick`: the short
/// waits of the main window instead of those of the command line.
pub fn deliver(
    command: Command,
    quick: bool,
    save: impl FnOnce() -> Result<()>,
) -> Result<Delivery> {
    let sent = super::Endpoints::current().and_then(|endpoints| {
        if quick {
            request_now(&endpoints, command, SET_ANSWER)
        } else {
            request(&endpoints, command, || {})
        }
    });
    let reason = match sent {
        Ok(Some(Body::Status(status))) => return Ok(Delivery::Tray(Box::new(status))),
        Ok(None) => {
            save()?;
            return Ok(Delivery::Saved);
        }
        // The tray refused it for a reason of its own (an unknown mode, a
        // mode that could not be saved): reported, nothing written here.
        Ok(Some(Body::Rejected { exit_code, lines })) if exit_code == crate::exit::ERRORS => {
            return Err(Error::new(Code::SettingsIo, lines.join(" ")));
        }
        // An older tray: it does not know the command (`KS505`).
        Ok(Some(Body::Rejected { lines, .. }))
            if lines
                .iter()
                .any(|l| l.contains(&Code::TrayProtocol.to_string())) =>
        {
            let reason = lines.join(" ");
            log::warn!("an older tray did not take the setting: {reason}");
            save()?;
            return Ok(Delivery::OlderTray(reason));
        }
        // One that quits (`KS501`).
        Ok(Some(Body::Rejected { lines, .. })) => lines.join(" "),
        Ok(Some(other)) => format!("unexpected answer from the tray: {other:?}"),
        Err(err) => err.to_string(),
    };
    log::warn!("the tray did not take the setting in time: {reason}");
    save()?;
    Ok(Delivery::Unanswered(reason))
}

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

fn edit_settings(change: impl FnOnce(&mut crate::settings::Settings)) -> Result<()> {
    let dir = crate::paths::config_dir().ok_or_else(|| {
        Error::new(
            Code::SettingsIo,
            "no configuration directory (APPDATA unset)",
        )
    })?;
    crate::settings::edit(&dir, change)
}

/// U4: pauses (`true`) or resumes automatic switching (main window,
/// `kaltstellen automation on|off`).
pub fn set_automation(paused: bool, quick: bool) -> Result<Delivery> {
    deliver(Command::SetAutomation { paused }, quick, || {
        edit_settings(|s| s.triggers_paused = paused)
    })
}

/// UP1: sets the update mode (main window, `kaltstellen update --mode`).
/// The tray checks at once if the mode in effect checks; without a tray
/// nothing checks here.
pub fn set_update_mode(mode: crate::update::Mode, quick: bool) -> Result<Delivery> {
    let command = Command::SetUpdateMode {
        mode: mode.name().to_owned(),
    };
    deliver(command, quick, || {
        edit_settings(|s| s.update = Some(mode.name().to_owned()))
    })
}
