//! The real [`Session`] of an install: the tray of this session over its
//! pipe, `recover` in-process, and child processes for the version probe
//! and the new tray.

use std::io::Read;
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::thread::sleep;
use std::time::{Duration, Instant};

use log::{info, warn};

use super::install::{Probe, Session};
use crate::config::NEUTRAL;
use crate::engine;
use crate::error::{Code, Error, Result};
use crate::settings;
use crate::state::Store;
use crate::tray::protocol::{Body, Command as PipeCommand};
use crate::tray::{Endpoints, client};
use crate::win32::instance::Instance;

const DETACHED_PROCESS: u32 = 0x0000_0008;
const CREATE_NO_WINDOW: u32 = 0x0800_0000;
/// How long a quitting tray may take (it waits for a running switch).
const QUIT_WAIT: Duration = Duration::from_secs(30);
/// How long a tray that refused `quit` is given to end anyway.
const REFUSED_WAIT: Duration = Duration::from_secs(3);
/// How long a new tray may take until its pipe answers.
const START_WAIT: Duration = Duration::from_secs(15);
/// How long `--version` may take.
const PROBE_WAIT: Duration = Duration::from_secs(10);

/// This session's tray, state and configuration.
pub struct System {
    pub endpoints: Endpoints,
    pub data_dir: PathBuf,
    pub config_dir: Option<PathBuf>,
    /// UP1 automatic install: the tray is asked with `quit_if_idle`, so it
    /// quits only if it is idle in `neutral` at that moment; a refusal ends
    /// the install at once. Without a tray nothing is recovered.
    pub unattended: bool,
}

impl System {
    pub fn current(unattended: bool) -> Result<System> {
        let endpoints = Endpoints::current()?;
        let data_dir = crate::paths::data_dir()
            .ok_or_else(|| Error::new(Code::StateIo, "no data directory (LOCALAPPDATA unset)"))?;
        Ok(System {
            endpoints,
            data_dir,
            config_dir: crate::paths::config_dir(),
            unattended,
        })
    }

    /// One install at a time per data directory: they share the staged
    /// files next to the executable and `update.json`. `KS808` if another
    /// install runs.
    pub fn exclusive(&self) -> Result<Instance> {
        Instance::acquire(&format!("{}-update", self.endpoints.mutex))?.ok_or_else(|| {
            Error::new(
                Code::UpdateTray,
                "another update is being installed; nothing was changed",
            )
        })
    }

    fn wait_gone(&self, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        while Instance::exists(&self.endpoints.mutex) {
            if Instant::now() >= deadline {
                return false;
            }
            sleep(Duration::from_millis(100));
        }
        true
    }

    /// The active profile becomes the offer of the next tray (S5), so it can
    /// be applied again with one click; nothing applies it by itself.
    fn keep_offer(&self) {
        let Some(active) = Store::new(&self.data_dir)
            .peek()
            .map(|s| s.active_profile)
            .filter(|p| !p.eq_ignore_ascii_case(NEUTRAL))
        else {
            return;
        };
        if let Some(dir) = &self.config_dir
            && let Err(err) = settings::edit(dir, |s| s.last_profile = Some(active.clone()))
        {
            warn!("update: {err}");
        }
    }
}

/// Waits for `child` at most `timeout`; ends it after that.
fn wait_child(child: &mut Child, timeout: Duration) -> std::result::Result<(), Probe> {
    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => return Ok(()),
            Ok(None) if Instant::now() < deadline => sleep(Duration::from_millis(50)),
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(Probe::TimedOut(format!(
                    "no answer within {} s",
                    timeout.as_secs()
                )));
            }
            Err(err) => return Err(Probe::Failed(err.to_string())),
        }
    }
}

impl Session for System {
    fn tray_running(&self) -> bool {
        Instance::exists(&self.endpoints.mutex)
    }

    fn stop_tray(&mut self) -> Result<()> {
        let refused = |why: String| {
            Error::new(
                Code::UpdateTray,
                format!(
                    "the tray did not quit ({why}); quit it from its menu and run the update again"
                ),
            )
        };
        if self.unattended {
            // Asked on the tray's window thread: it quits only if `neutral`
            // is active and nothing runs or waits right now. A tray that
            // does not know the request rejects it the same way.
            return match client::request(&self.endpoints, PipeCommand::QuitIfIdle, || {}) {
                Ok(Some(Body::Rejected { lines, .. })) => Err(Error::new(
                    Code::UpdateTray,
                    format!(
                        "the tray stays ({}); nothing was changed, the update is tried again later",
                        lines.join("; ")
                    ),
                )),
                // The tray was quit from its menu meanwhile.
                Ok(None) => Err(Error::new(
                    Code::UpdateTray,
                    "the tray has ended; the automatic update needs it and changed nothing",
                )),
                other => {
                    if let Ok(Some(Body::Done(finished))) = &other {
                        for error in &finished.errors {
                            warn!("update: on quit: {error}");
                        }
                    }
                    info!("update: tray accepted quit_if_idle: {other:?}");
                    if self.wait_gone(QUIT_WAIT) {
                        Ok(())
                    } else {
                        Err(refused(format!(
                            "it still runs after {} s",
                            QUIT_WAIT.as_secs()
                        )))
                    }
                }
            };
        }
        self.keep_offer();
        match client::request(&self.endpoints, PipeCommand::Quit, || {}) {
            Ok(Some(Body::Done(finished))) => {
                for error in &finished.errors {
                    warn!("update: on quit: {error}");
                }
                info!("update: tray quit, exit code {}", finished.exit_code);
            }
            Ok(Some(Body::Rejected { lines, .. })) => {
                if !self.wait_gone(REFUSED_WAIT) {
                    return Err(refused(lines.join("; ")));
                }
            }
            Ok(Some(other)) => warn!("update: unexpected answer to quit: {other:?}"),
            Ok(None) => {}
            // The tray may end before its reply is written; what counts is
            // that it ends.
            Err(err) => info!("update: {err}"),
        }
        if self.wait_gone(QUIT_WAIT) {
            Ok(())
        } else {
            Err(refused(format!(
                "it still runs after {} s",
                QUIT_WAIT.as_secs()
            )))
        }
    }

    fn recover(&mut self) -> Result<Option<String>> {
        if self.unattended {
            // The tray that started this install has ended meanwhile (quit
            // from its menu): installing is left to the next tray.
            return Err(Error::new(
                Code::UpdateTray,
                "the tray has ended; the automatic update needs it and changed nothing",
            ));
        }
        let store = Store::new(&self.data_dir);
        let _lock = store.lock()?;
        let (mut state, _) = store.load();
        let active =
            Some(state.active_profile.clone()).filter(|p| !p.eq_ignore_ascii_case(NEUTRAL));
        if state.suspended.is_empty() && active.is_none() {
            return Ok(None);
        }
        let report = engine::recover(&mut state, &mut |s| store.save(s))?;
        if let Some(first) = report.errors.first() {
            return Err(Error::new(
                Code::UpdateTray,
                format!("resuming before the update failed ({first}); nothing was changed"),
            ));
        }
        Ok(active)
    }

    fn start_tray(&mut self, exe: &Path) -> std::result::Result<(), String> {
        // The tray outlives this process: it must not keep our stdout.
        crate::win32::console::keep_std_handles_private();
        let mut child = Command::new(exe)
            .arg("tray")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .creation_flags(DETACHED_PROCESS)
            .spawn()
            .map_err(|e| format!("cannot start {}: {e}", exe.display()))?;
        let deadline = Instant::now() + START_WAIT;
        loop {
            if let Ok(Some(status)) = child.try_wait() {
                return Err(format!("it ended with {status}"));
            }
            if let Ok(Some(Body::Status(_))) =
                client::request(&self.endpoints, PipeCommand::Status, || {})
            {
                info!("update: tray started (pid {})", child.id());
                return Ok(());
            }
            if Instant::now() >= deadline {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!(
                    "its pipe did not answer within {} s",
                    START_WAIT.as_secs()
                ));
            }
            sleep(Duration::from_millis(200));
        }
    }

    fn install_failed(&mut self, version: &str) {
        match crate::update::record::mark_failed(&self.data_dir, version) {
            Ok(()) => info!("update: version {version} is not installed automatically again"),
            Err(err) => warn!("update: {err}"),
        }
    }

    fn version_of(&self, exe: &Path) -> std::result::Result<String, Probe> {
        let mut child = Command::new(exe)
            .arg("--version")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .creation_flags(CREATE_NO_WINDOW)
            .spawn()
            .map_err(|e| Probe::Failed(format!("cannot start it: {e}")))?;
        wait_child(&mut child, PROBE_WAIT)?;
        let mut out = String::new();
        if let Some(mut stdout) = child.stdout.take() {
            let _ = stdout.read_to_string(&mut out);
        }
        Ok(out.lines().next().unwrap_or_default().to_owned())
    }
}
