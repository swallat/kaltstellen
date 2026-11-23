//! Process engine (WP2) against the fixture programs.
//!
//! These tests live in the fixtures package so that Cargo builds the fixture
//! executables first. They only ever touch processes they started.
//!
//! The window tests need an interactive desktop; without one the fixture
//! windows never become visible and those tests report "skipped".

mod common;

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use common::*;
use kaltstellen::config::OnTimeout;
use kaltstellen::error::Code;
use kaltstellen::process::{self, Access, Snapshot, StopOutcome, Target};
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
    // Each one is ended by the test as soon as it is known.
    let child = read_pid(&dir.path().join("child.pid"));
    let _child = KillPid(child);
    let orphan = read_pid(&dir.path().join("orphan.pid"));
    let _orphan = KillPid(orphan);
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
