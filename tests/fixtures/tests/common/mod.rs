//! Helpers shared by the tests that run against the fixture programs.
#![allow(dead_code)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::thread::sleep;
use std::time::{Duration, Instant};

use kaltstellen::config::OnTimeout;
use kaltstellen::process::{self, Access, ProcessInfo, Snapshot, Target};

pub const HEARTBEAT: &str = env!("CARGO_BIN_EXE_ks-heartbeat");
pub const SPAWNER: &str = env!("CARGO_BIN_EXE_ks-spawner");
pub const WINDOW: &str = env!("CARGO_BIN_EXE_ks-window");
pub const DIALOG: &str = env!("CARGO_BIN_EXE_ks-dialog");

/// Copies a fixture executable under its own name, so that a test can treat
/// it as a program of its own while other tests run in parallel.
pub fn copy_fixture(exe: &str, dir: &Path, name: &str) -> PathBuf {
    let copy = dir.join(name);
    fs::copy(exe, &copy).expect("cannot copy fixture");
    copy
}

/// A fixture process that is killed when the test ends, pass or fail.
pub struct Fixture(pub Child);

impl Fixture {
    pub fn spawn(exe: &str, args: &[&str]) -> Self {
        let child = Command::new(exe)
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("cannot start fixture");
        Fixture(child)
    }

    pub fn pid(&self) -> u32 {
        self.0.id()
    }

    pub fn info(&self) -> ProcessInfo {
        info_of(self.pid())
    }

    pub fn alive(&mut self) -> bool {
        self.0.try_wait().expect("try_wait").is_none()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// Kills a process the test did not spawn itself (child, orphan, started by
/// kaltstellen) at test end.
pub struct KillPid(pub u32);

impl Drop for KillPid {
    fn drop(&mut self) {
        kill_pid(self.0);
    }
}

pub fn kill_pid(pid: u32) {
    if let Some(info) = Snapshot::take().ok().and_then(|s| s.get(pid).cloned()) {
        let _ = process::stop(&[info], Duration::ZERO, OnTimeout::Kill);
    }
}

/// Kills every process with this executable name at test end. Only for
/// names unique to one test (see [`copy_fixture`]).
pub struct KillProgram(pub String);

impl Drop for KillProgram {
    fn drop(&mut self) {
        if let Ok(snapshot) = Snapshot::take() {
            for info in snapshot.program(&self.0) {
                // Undo a freeze first; a frozen process dies anyway, but its
                // suspend count should not outlive the test's intent.
                let _ = process::stop(&[info], Duration::ZERO, OnTimeout::Kill);
            }
        }
    }
}

pub fn pids_of(exe: &str) -> Vec<u32> {
    Snapshot::take()
        .expect("snapshot")
        .program(exe)
        .iter()
        .map(|p| p.pid)
        .collect()
}

pub fn info_of(pid: u32) -> ProcessInfo {
    Snapshot::take()
        .expect("snapshot")
        .get(pid)
        .unwrap_or_else(|| panic!("pid {pid} not in snapshot"))
        .clone()
}

pub fn wait_for(timeout: Duration, mut done: impl FnMut() -> bool) -> bool {
    let deadline = Instant::now() + timeout;
    loop {
        if done() {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        sleep(Duration::from_millis(25));
    }
}

pub fn beat(file: &Path) -> Option<u128> {
    fs::read_to_string(file).ok()?.trim().parse().ok()
}

/// The heartbeat advances within two seconds.
pub fn beating(file: &Path) -> bool {
    let start = beat(file);
    wait_for(Duration::from_secs(2), || {
        let now = beat(file);
        now.is_some() && now != start
    })
}

/// The heartbeat does not change for half a second.
pub fn frozen(file: &Path) -> bool {
    // Let a write that was under way when the process froze settle first.
    sleep(Duration::from_millis(150));
    let start = beat(file);
    sleep(Duration::from_millis(500));
    start.is_some() && beat(file) == start
}

pub fn read_pid(file: &Path) -> u32 {
    assert!(
        wait_for(Duration::from_secs(10), || file.exists()),
        "{} never appeared",
        file.display()
    );
    fs::read_to_string(file).unwrap().trim().parse().unwrap()
}

pub fn open(info: &ProcessInfo, access: Access) -> Target {
    Target::open(info, access)
        .expect("open")
        .expect("process is alive")
}

pub fn str_of(path: &Path) -> &str {
    path.to_str().expect("utf-8 path")
}

/// Starts a window fixture and waits for its window. `None` when there is no
/// interactive desktop.
pub fn window_fixture(exe: &str, args: &[&str]) -> Option<Fixture> {
    let fixture = Fixture::spawn(exe, args);
    let pid = fixture.pid();
    if wait_for(Duration::from_secs(5), || {
        process::visible_window_count(&[pid]) > 0
    }) {
        Some(fixture)
    } else {
        eprintln!("skipped: no interactive desktop, the fixture window never appeared");
        None
    }
}
