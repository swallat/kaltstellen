//! Smoke test of `kaltstellen tray` as a process (WP7): it starts, stays up,
//! refuses a second instance and ends cleanly on `WM_CLOSE`. The menu and
//! switch path against fixture programs is tested in
//! `tests/fixtures/tests/tray.rs`, which also needs this file: it makes
//! Cargo build the `kaltstellen` executable before the tests run.
//!
//! Each run uses its own configuration and data folder, so the tray's
//! single-instance mutex is its own too and a real tray is not disturbed.

use std::fs;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::thread::sleep;
use std::time::{Duration, Instant};

use kaltstellen::config::OnTimeout;
use kaltstellen::process::{self, Snapshot, StopOutcome};

const EXE: &str = env!("CARGO_BIN_EXE_kaltstellen");

fn tray(dir: &Path) -> Command {
    let mut command = Command::new(EXE);
    command
        .arg("tray")
        .env("KALTSTELLEN_CONFIG_DIR", dir.join("config"))
        .env("KALTSTELLEN_DATA_DIR", dir.join("data"))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    command
}

/// Kills the tray if the test fails half-way.
struct Guard(Child);

impl Drop for Guard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn wait_for(timeout: Duration, mut done: impl FnMut() -> bool) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if done() {
            return true;
        }
        sleep(Duration::from_millis(50));
    }
    done()
}

fn log_has(dir: &Path, text: &str) -> bool {
    fs::read_to_string(dir.join("data").join("kaltstellen.log")).is_ok_and(|log| log.contains(text))
}

#[test]
fn l1_tray_runs_once_and_ends_on_close() {
    let dir = tempfile::tempdir().unwrap();
    let mut first = Guard(tray(dir.path()).spawn().unwrap());
    let pid = first.0.id();
    assert!(
        wait_for(Duration::from_secs(10), || log_has(
            dir.path(),
            "tray: recover finished"
        )),
        "the tray never finished its start-up recovery"
    );

    let second = tray(dir.path()).output().unwrap();
    assert_eq!(second.status.code(), Some(0));
    let message = String::from_utf8_lossy(&second.stderr);
    assert!(message.contains("already running"), "{message}");

    sleep(Duration::from_millis(500));
    assert!(first.0.try_wait().unwrap().is_none(), "the tray stays up");

    // WM_CLOSE to its windows, as `taskkill` without /F does: Quit (L2).
    let info = Snapshot::take().unwrap().get(pid).cloned().unwrap();
    let outcome = process::stop(&[info], Duration::from_secs(10), OnTimeout::Kill).unwrap();
    assert_eq!(
        outcome,
        StopOutcome::Closed,
        "the tray did not end on its own"
    );
    assert_eq!(first.0.wait().unwrap().code(), Some(0));
    assert!(log_has(dir.path(), "tray: quit finished"));
    assert!(log_has(dir.path(), "tray: ended"));
}
