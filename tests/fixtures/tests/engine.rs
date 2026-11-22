//! Process engine (WP2) against the fixture programs.
//!
//! These tests live in the fixtures package so that Cargo builds the fixture
//! executables first. They only ever touch processes they started.
//!
//! The window tests need an interactive desktop; without one the fixture
//! windows never become visible and those tests report "skipped".

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::thread::sleep;
use std::time::{Duration, Instant};

use kaltstellen::config::OnTimeout;
use kaltstellen::error::Code;
use kaltstellen::process::{self, Access, ProcessInfo, Snapshot, StopOutcome, Target};

const HEARTBEAT: &str = env!("CARGO_BIN_EXE_ks-heartbeat");
const SPAWNER: &str = env!("CARGO_BIN_EXE_ks-spawner");
const WINDOW: &str = env!("CARGO_BIN_EXE_ks-window");
const DIALOG: &str = env!("CARGO_BIN_EXE_ks-dialog");

/// A fixture process that is killed when the test ends, pass or fail.
struct Fixture(Child);

impl Fixture {
    fn spawn(exe: &str, args: &[&str]) -> Self {
        let child = Command::new(exe)
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("cannot start fixture");
        Fixture(child)
    }

    fn pid(&self) -> u32 {
        self.0.id()
    }

    fn info(&self) -> ProcessInfo {
        info_of(self.pid())
    }

    fn alive(&mut self) -> bool {
        self.0.try_wait().expect("try_wait").is_none()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// Kills a process the fixture started itself (child, orphan) at test end.
struct KillPid(u32);

impl Drop for KillPid {
    fn drop(&mut self) {
        if let Some(info) = Snapshot::take().ok().and_then(|s| s.get(self.0).cloned()) {
            let _ = process::stop(&[info], Duration::ZERO, OnTimeout::Kill);
        }
    }
}

fn info_of(pid: u32) -> ProcessInfo {
    Snapshot::take()
        .expect("snapshot")
        .get(pid)
        .unwrap_or_else(|| panic!("pid {pid} not in snapshot"))
        .clone()
}

fn wait_for(timeout: Duration, mut done: impl FnMut() -> bool) -> bool {
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

fn beat(file: &Path) -> Option<u128> {
    fs::read_to_string(file).ok()?.trim().parse().ok()
}

/// The heartbeat advances within two seconds.
fn beating(file: &Path) -> bool {
    let start = beat(file);
    wait_for(Duration::from_secs(2), || {
        let now = beat(file);
        now.is_some() && now != start
    })
}

/// The heartbeat does not change for half a second.
fn frozen(file: &Path) -> bool {
    // Let a write that was under way when the process froze settle first.
    sleep(Duration::from_millis(150));
    let start = beat(file);
    sleep(Duration::from_millis(500));
    start.is_some() && beat(file) == start
}

fn read_pid(file: &Path) -> u32 {
    assert!(
        wait_for(Duration::from_secs(10), || file.exists()),
        "{} never appeared",
        file.display()
    );
    fs::read_to_string(file).unwrap().trim().parse().unwrap()
}

fn open(info: &ProcessInfo, access: Access) -> Target {
    Target::open(info, access)
        .expect("open")
        .expect("process is alive")
}

fn str_of(path: &Path) -> &str {
    path.to_str().expect("utf-8 path")
}

/// Starts a window fixture and waits for its window. `None` when there is no
/// interactive desktop.
fn window_fixture(exe: &str, args: &[&str]) -> Option<Fixture> {
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

#[test]
fn suspend_freezes_and_resume_continues() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("beat.txt");
    let fixture = Fixture::spawn(HEARTBEAT, &[str_of(&file), "--alloc-mb", "32"]);
    assert!(beating(&file), "fixture never started beating");

    let target = open(&fixture.info(), Access::Suspend);
    target.suspend().unwrap();
    assert!(frozen(&file), "heartbeat continued while suspended");
    target.trim().unwrap();
    assert!(frozen(&file), "trim woke the process");
    target.resume().unwrap();
    assert!(beating(&file), "heartbeat did not continue after resume");
}

#[test]
fn suspend_counts_stack_and_need_one_resume_each() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("beat.txt");
    let fixture = Fixture::spawn(HEARTBEAT, &[str_of(&file)]);
    assert!(beating(&file));

    let target = open(&fixture.info(), Access::Suspend);
    target.suspend().unwrap();
    target.suspend().unwrap();
    target.resume().unwrap();
    assert!(frozen(&file), "one resume undid two suspends");
    target.resume().unwrap();
    assert!(beating(&file));
}

#[test]
fn program_covers_children_but_not_orphans() {
    let dir = tempfile::tempdir().unwrap();
    let fixture = Fixture::spawn(SPAWNER, &[str_of(dir.path())]);
    let spawner = read_pid(&dir.path().join("spawner.pid"));
    let child = read_pid(&dir.path().join("child.pid"));
    let orphan = read_pid(&dir.path().join("orphan.pid"));
    let _cleanup = (KillPid(child), KillPid(orphan));
    assert_eq!(spawner, fixture.pid());
    for name in ["spawner.txt", "child.txt", "orphan.txt"] {
        assert!(beating(&dir.path().join(name)), "{name} not beating");
    }

    let snapshot = Snapshot::take().unwrap();
    let program = snapshot.program("ks-spawner");
    let pids: Vec<u32> = program.iter().map(|p| p.pid).collect();
    assert!(pids.contains(&spawner) && pids.contains(&child), "{pids:?}");
    assert!(!pids.contains(&orphan), "orphan is part of the tree");
    assert!(!pids.contains(&std::process::id()));

    let targets: Vec<Target> = program
        .iter()
        .filter(|p| p.pid == spawner || p.pid == child)
        .map(|p| open(p, Access::Suspend))
        .collect();
    for target in &targets {
        target.suspend().unwrap();
    }
    assert!(frozen(&dir.path().join("spawner.txt")));
    assert!(frozen(&dir.path().join("child.txt")));
    assert!(beating(&dir.path().join("orphan.txt")), "orphan was frozen");
    for target in &targets {
        target.resume().unwrap();
    }
    assert!(beating(&dir.path().join("child.txt")));
}

#[test]
fn program_at_matches_by_image_path() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("beat.txt");
    let fixture = Fixture::spawn(HEARTBEAT, &[str_of(&file)]);
    assert!(beating(&file));

    let snapshot = Snapshot::take().unwrap();
    // Forward slashes and different case must still match.
    let same = PathBuf::from(HEARTBEAT.replace('\\', "/").to_uppercase());
    let (found, errors) = snapshot.program_at("ks-heartbeat.exe", &same);
    assert!(errors.is_empty(), "{errors:?}");
    assert!(found.iter().any(|p| p.pid == fixture.pid()));

    let elsewhere = dir.path().join("ks-heartbeat.exe");
    let (found, _) = snapshot.program_at("ks-heartbeat.exe", &elsewhere);
    assert!(found.is_empty());
}

#[test]
fn stop_without_window_applies_on_timeout_at_once() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("beat.txt");
    let mut fixture = Fixture::spawn(HEARTBEAT, &[str_of(&file)]);
    assert!(beating(&file));
    let info = fixture.info();

    let started = Instant::now();
    let kept = process::stop(
        std::slice::from_ref(&info),
        Duration::from_secs(30),
        OnTimeout::Keep,
    );
    assert_eq!(kept.unwrap_err().code, Code::CloseTimeout);
    assert!(fixture.alive());

    let killed = process::stop(
        std::slice::from_ref(&info),
        Duration::from_secs(30),
        OnTimeout::Kill,
    );
    assert_eq!(killed.unwrap(), StopOutcome::Killed);
    assert!(
        started.elapsed() < Duration::from_secs(10),
        "waited for a window that is not there"
    );
    assert!(wait_for(Duration::from_secs(5), || !fixture.alive()));

    // Stopping what is gone is not an error.
    let again = process::stop(&[info], Duration::from_secs(1), OnTimeout::Kill);
    assert_eq!(again.unwrap(), StopOutcome::NotRunning);
}

#[test]
fn stop_closes_a_window_program_gracefully() {
    let Some(mut fixture) = window_fixture(WINDOW, &[]) else {
        return;
    };
    let outcome = process::stop(&[fixture.info()], Duration::from_secs(10), OnTimeout::Keep);
    assert_eq!(outcome.unwrap(), StopOutcome::Closed);
    assert!(!fixture.alive());
}

#[test]
fn t11_keep_leaves_a_program_with_an_open_dialog_running() {
    let Some(mut fixture) = window_fixture(DIALOG, &[]) else {
        return;
    };
    let info = fixture.info();
    let kept = process::stop(
        std::slice::from_ref(&info),
        Duration::from_secs(2),
        OnTimeout::Keep,
    );
    let err = kept.unwrap_err();
    assert_eq!(err.code, Code::CloseTimeout);
    assert_eq!(err.program.as_deref(), Some("ks-dialog.exe"));
    assert!(fixture.alive(), "keep must not end the program");

    let killed = process::stop(&[info], Duration::from_secs(1), OnTimeout::Kill);
    assert_eq!(killed.unwrap(), StopOutcome::Killed);
    assert!(wait_for(Duration::from_secs(5), || !fixture.alive()));
}

#[test]
fn w7_minimize_reports_the_windows_it_asked() {
    let Some(fixture) = window_fixture(WINDOW, &[]) else {
        return;
    };
    let pid = fixture.pid();
    assert_eq!(process::minimize_windows(&[pid], Duration::from_secs(1)), 1);
    // Already minimised: nothing left to ask.
    assert_eq!(process::minimize_windows(&[pid], Duration::from_secs(1)), 0);
    // A minimised window is still visible and the program still closes.
    let outcome = process::stop(&[fixture.info()], Duration::from_secs(10), OnTimeout::Keep);
    assert_eq!(outcome.unwrap(), StopOutcome::Closed);
}

#[test]
fn start_launches_with_quoted_arguments_and_reports_failures() {
    let dir = tempfile::tempdir().unwrap();
    let folder = dir.path().join("with space");
    fs::create_dir(&folder).unwrap();
    let file = folder.join("beat one.txt");
    let pid = process::start(
        Path::new(HEARTBEAT),
        &[str_of(&file).to_owned()],
        Some(dir.path()),
    )
    .unwrap();
    let _cleanup = KillPid(pid);
    assert!(
        beating(&file),
        "argument with a space did not arrive intact"
    );
    assert_eq!(info_of(pid).name.to_lowercase(), "ks-heartbeat.exe");

    let missing = dir.path().join("missing.exe");
    let err = process::start(&missing, &[], None).unwrap_err();
    assert_eq!(err.code, Code::StartFailed);
    assert_eq!(err.program.as_deref(), Some("missing.exe"));
}
