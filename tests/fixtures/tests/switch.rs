//! Switch engine (WP4) and state recovery (WP6) against the fixtures.
//!
//! Every test copies the heartbeat fixture under names of its own, so each
//! test owns its "programs" and the tests can run in parallel.

mod common;

use std::path::{Path, PathBuf};
use std::time::Duration;

use common::*;
use kaltstellen::config::{App, AppAction, Config, OnTimeout, Profile, Settings};
use kaltstellen::engine::{self, Options, Report};
use kaltstellen::error::{Code, Step};
use kaltstellen::process::Access;
use kaltstellen::state::{State, Store};

/// One test's world: a temp folder with its own programs and state file.
struct World {
    config: Config,
    state: State,
    store: Store,
    cleanup: Vec<KillProgram>,
    /// Dropped last, after the programs running from it were ended, so
    /// the folder can be removed.
    dir: tempfile::TempDir,
}

impl World {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path().join("data"));
        World {
            dir,
            config: Config {
                settings: Settings {
                    stop_timeout_s: 2,
                    ..Settings::default()
                },
                profiles: Vec::new(),
            },
            state: State::default(),
            store,
            cleanup: Vec::new(),
        }
    }

    /// Creates the program `name` (a copy of the heartbeat fixture).
    fn program(&mut self, name: &str) -> PathBuf {
        self.cleanup.push(KillProgram(name.to_owned()));
        copy_fixture(HEARTBEAT, self.dir.path(), name)
    }

    fn beat_file(&self, name: &str) -> PathBuf {
        self.dir.path().join(format!("{name}.beat"))
    }

    /// Starts `name` beating into its beat file.
    fn run(&mut self, name: &str) -> Fixture {
        let exe = self.program(name);
        let beat = self.beat_file(name);
        let fixture = Fixture::spawn(str_of(&exe), &[str_of(&beat)]);
        assert!(beating(&beat), "{name} never started beating");
        fixture
    }

    fn profile(&mut self, name: &str, apps: Vec<App>) {
        self.config.profiles.push(Profile {
            name: name.to_owned(),
            hotkey: None,
            apps,
            triggers: Vec::new(),
            priority: 0,
            watch: false,
            services: Vec::new(),
        });
    }

    fn switch(&mut self, target: &str) -> Report {
        let store = self.store.clone();
        engine::switch(
            &self.config,
            target,
            &mut self.state,
            Options::default(),
            &mut |state| store.save(state),
        )
        .expect("switch ran")
    }

    fn is_beating(&self, name: &str) -> bool {
        beating(&self.beat_file(name))
    }

    fn is_frozen(&self, name: &str) -> bool {
        frozen(&self.beat_file(name))
    }
}

fn suspend(exe: &str) -> App {
    App {
        exe: exe.to_owned(),
        path: None,
        action: AppAction::Suspend { trim: true },
        confirmed: false,
    }
}

fn stop(exe: &str, restart_on_neutral: bool) -> App {
    App {
        exe: exe.to_owned(),
        path: None,
        action: AppAction::Stop {
            on_timeout: OnTimeout::Kill,
            restart_on_neutral,
            args: Vec::new(),
            cwd: None,
        },
        confirmed: false,
    }
}

fn start(path: &Path, args: &[&str]) -> App {
    App {
        exe: path.file_name().unwrap().to_str().unwrap().to_owned(),
        path: Some(path.to_owned()),
        action: AppAction::Start {
            args: args.iter().map(|a| (*a).to_owned()).collect(),
            cwd: None,
        },
        confirmed: false,
    }
}

fn steps(report: &Report) -> Vec<(Step, String)> {
    report
        .events
        .iter()
        .map(|e| (e.step, e.program.to_lowercase()))
        .collect()
}

#[test]
fn t1_switch_runs_resume_stop_suspend_start_in_that_order() {
    let mut w = World::new();
    let _old = w.run("ks-t1-old.exe");
    let mut closing = w.run("ks-t1-close.exe");
    let _freeze = w.run("ks-t1-freeze.exe");
    let new_exe = w.program("ks-t1-new.exe");
    let new_beat = w.beat_file("ks-t1-new.exe");

    w.profile("first", vec![suspend("ks-t1-old.exe")]);
    w.profile(
        "second",
        vec![
            // Listed in the opposite order on purpose.
            start(&new_exe, &[str_of(&new_beat)]),
            suspend("ks-t1-freeze.exe"),
            stop("ks-t1-close.exe", false),
        ],
    );

    let first = w.switch("first");
    assert!(first.errors.is_empty(), "{:?}", first.errors);
    assert!(w.is_frozen("ks-t1-old.exe"));

    let second = w.switch("SECOND");
    assert!(second.errors.is_empty(), "{:?}", second.errors);
    assert_eq!(
        steps(&second),
        vec![
            (Step::Resume, "ks-t1-old.exe".to_owned()),
            (Step::Stop, "ks-t1-close.exe".to_owned()),
            (Step::Suspend, "ks-t1-freeze.exe".to_owned()),
            (Step::Start, "ks-t1-new.exe".to_owned()),
        ]
    );
    assert!(w.is_beating("ks-t1-old.exe"));
    assert!(wait_for(Duration::from_secs(5), || !closing.alive()));
    assert!(w.is_frozen("ks-t1-freeze.exe"));
    assert!(w.is_beating("ks-t1-new.exe"));
    assert_eq!(w.state.active_profile, "second");
    assert_eq!(w.store.load().0, w.state, "state file is up to date");

    // Starting again is not needed: the program is running.
    let again = w.switch("second");
    assert!(again.events.is_empty(), "{:?}", again.events);
}

#[test]
fn t2_program_frozen_by_both_profiles_stays_frozen() {
    let mut w = World::new();
    let _shared = w.run("ks-t2-shared.exe");
    let _only_a = w.run("ks-t2-only-a.exe");
    w.profile(
        "a",
        vec![suspend("ks-t2-shared.exe"), suspend("ks-t2-only-a.exe")],
    );
    w.profile("b", vec![suspend("ks-t2-shared.exe")]);

    w.switch("a");
    let record = w.state.suspended.clone();
    let report = w.switch("b");
    assert!(report.errors.is_empty(), "{:?}", report.errors);
    assert_eq!(
        steps(&report),
        vec![(Step::Resume, "ks-t2-only-a.exe".to_owned())],
        "the shared program must see no resume and no second suspend"
    );
    assert!(w.is_frozen("ks-t2-shared.exe"));
    assert!(w.is_beating("ks-t2-only-a.exe"));
    assert_eq!(w.state.suspended.len(), 1);
    assert!(record.contains(&w.state.suspended[0]));

    // W5: frozen once, so one resume brings it back.
    let neutral = w.switch("neutral");
    assert!(neutral.errors.is_empty());
    assert!(w.is_beating("ks-t2-shared.exe"));
    assert!(w.state.suspended.is_empty());
    assert_eq!(w.state.active_profile, "neutral");
}

#[test]
fn w4_w5_applying_a_profile_twice_freezes_once() {
    let mut w = World::new();
    let _app = w.run("ks-w5-app.exe");
    w.profile("p", vec![suspend("ks-w5-app.exe")]);

    assert_eq!(w.switch("p").events.len(), 1);
    assert!(w.switch("p").events.is_empty());
    assert_eq!(w.state.suspended.len(), 1);
    w.switch("neutral");
    assert!(
        w.is_beating("ks-w5-app.exe"),
        "a second suspend was stacked"
    );
}

#[test]
fn t3_a_failed_step_does_not_stop_the_switch() {
    let mut w = World::new();
    let _app = w.run("ks-t3-app.exe");
    let missing = w.dir.path().join("ks-t3-missing.exe");
    w.profile("p", vec![start(&missing, &[]), suspend("ks-t3-app.exe")]);

    let report = w.switch("p");
    assert_eq!(report.errors.len(), 1, "{:?}", report.errors);
    assert_eq!(report.errors[0].code, Code::StartFailed);
    assert_eq!(
        report.errors[0].program.as_deref(),
        Some("ks-t3-missing.exe")
    );
    assert!(
        w.is_frozen("ks-t3-app.exe"),
        "the rest of the switch must run"
    );
    assert_eq!(w.state.active_profile, "p");
    w.switch("neutral");
}

#[test]
fn t10_restart_on_neutral_only_on_neutral() {
    let mut w = World::new();
    let mut back = w.run("ks-t10-back.exe");
    let mut gone = w.run("ks-t10-gone.exe");
    // The restart gets the entry's arguments: a beat file of its own.
    let restart_beat = w.beat_file("ks-t10-restarted");
    let mut with_args = stop("ks-t10-back.exe", true);
    if let AppAction::Stop { args, .. } = &mut with_args.action {
        args.push(str_of(&restart_beat).to_owned());
    }
    w.profile("quiet", vec![with_args, stop("ks-t10-gone.exe", false)]);
    w.profile("other", Vec::new());

    let report = w.switch("quiet");
    assert!(report.errors.is_empty(), "{:?}", report.errors);
    assert!(wait_for(Duration::from_secs(5), || !back.alive() && !gone.alive()));
    assert_eq!(w.state.stopped.len(), 2);

    // Another profile: nothing comes back, the records stay.
    let other = w.switch("other");
    assert!(other.events.is_empty(), "{:?}", other.events);
    assert!(pids_of("ks-t10-back.exe").is_empty());
    assert_eq!(w.state.stopped.len(), 2);

    let neutral = w.switch("neutral");
    assert!(neutral.errors.is_empty(), "{:?}", neutral.errors);
    assert_eq!(
        steps(&neutral),
        vec![(Step::Start, "ks-t10-back.exe".to_owned())]
    );
    assert!(
        wait_for(Duration::from_secs(5), || pids_of("ks-t10-back.exe").len()
            == 1),
        "program with restart_on_neutral did not come back"
    );
    assert!(
        beating(&restart_beat),
        "the restart did not get the arguments of its entry"
    );
    assert!(pids_of("ks-t10-gone.exe").is_empty());
    assert!(w.state.stopped.is_empty());
}

#[test]
fn t5_recover_after_a_crash_leaves_nothing_frozen() {
    let mut w = World::new();
    let _a = w.run("ks-t5-a.exe");
    let _b = w.run("ks-t5-b.exe");
    w.profile("p", vec![suspend("ks-t5-a.exe"), suspend("ks-t5-b.exe")]);
    w.switch("p");
    assert!(w.is_frozen("ks-t5-a.exe") && w.is_frozen("ks-t5-b.exe"));

    // "Crash": all that survives is the state file.
    let (mut state, problem) = w.store.load();
    assert!(problem.is_none());
    assert_eq!(state.suspended.len(), 2);
    let store = w.store.clone();
    let report = engine::recover(&mut state, &mut |s| store.save(s)).unwrap();
    assert!(report.errors.is_empty(), "{:?}", report.errors);
    assert_eq!(report.events.len(), 2);
    assert!(report.events.iter().all(|e| e.step == Step::Recover));
    assert!(w.is_beating("ks-t5-a.exe") && w.is_beating("ks-t5-b.exe"));
    assert_eq!(state.active_profile, "neutral");
    assert!(state.suspended.is_empty());
    assert_eq!(w.store.load().0, state);
}

#[test]
fn t6_stale_records_are_ignored() {
    let mut w = World::new();
    let app = w.run("ks-t6-app.exe");
    let bystander = w.run("ks-t6-bystander.exe");
    w.profile("p", vec![suspend("ks-t6-app.exe")]);
    w.switch("p");

    // The bystander is frozen by "someone else"; two forged records point at
    // it: one with a wrong creation time, one with a wrong image path.
    let target = open(&bystander.info(), Access::Suspend);
    target.suspend().unwrap();
    let real = w.state.suspended[0].clone();
    let info = bystander.info();
    let mut wrong_time = real.clone();
    wrong_time.pid = info.pid;
    wrong_time.created = info.created + 1;
    let mut wrong_image = real.clone();
    wrong_image.pid = info.pid;
    wrong_image.created = info.created;
    w.state.suspended.extend([wrong_time, wrong_image]);

    let store = w.store.clone();
    let report = engine::recover(&mut w.state, &mut |s| store.save(s)).unwrap();
    assert!(report.errors.is_empty(), "{:?}", report.errors);
    assert_eq!(report.events.len(), 1);
    assert_eq!(report.events[0].pid, Some(app.pid()));
    assert!(w.is_beating("ks-t6-app.exe"));
    assert!(
        w.is_frozen("ks-t6-bystander.exe"),
        "a process kaltstellen did not freeze was resumed"
    );
    assert!(w.state.suspended.is_empty());
    target.resume().unwrap();
}

#[test]
fn s10_records_from_before_a_reboot_are_dropped_without_resuming() {
    let mut w = World::new();
    let app = w.run("ks-s10-app.exe");
    w.profile("p", vec![suspend("ks-s10-app.exe")]);
    w.switch("p");
    assert_ne!(w.state.boot, 0, "boot marker is recorded");

    // Pretend the records are from another boot.
    w.state.boot += 1;
    let store = w.store.clone();
    let report = engine::recover(&mut w.state, &mut |s| store.save(s)).unwrap();
    assert!(report.events.is_empty());
    assert!(w.state.suspended.is_empty());
    assert!(w.is_frozen("ks-s10-app.exe"), "nothing may be touched");
    open(&app.info(), Access::Suspend).resume().unwrap();
}

#[test]
fn warn_list_needs_force() {
    // The name of a remote-access tool on the warn list; the process is a
    // heartbeat fixture in this test's folder. A real program of that name
    // must never be touched: skip before anything starts, match the entry
    // by this copy's path only, and end the copy by its PID (not by name).
    if !pids_of("rustdesk.exe").is_empty() {
        eprintln!("skipped: a real program of that name is running");
        return;
    }
    let mut w = World::new();
    let exe = copy_fixture(HEARTBEAT, w.dir.path(), "rustdesk.exe");
    let beat = w.beat_file("rustdesk.exe");
    let _app = Fixture::spawn(str_of(&exe), &[str_of(&beat)]);
    assert!(beating(&beat), "the copy never started beating");
    let mut entry = suspend("rustdesk.exe");
    entry.path = Some(exe.clone());
    w.profile("p", vec![entry]);
    let report = w.switch("p");
    assert_eq!(report.errors.len(), 1);
    assert_eq!(report.errors[0].code, Code::WarnListUnconfirmed);
    assert!(w.is_beating("rustdesk.exe"));
    assert!(w.state.suspended.is_empty());

    let store = w.store.clone();
    let forced = engine::switch(
        &w.config,
        "p",
        &mut w.state,
        Options {
            force: true,
            ..Default::default()
        },
        &mut |s| store.save(s),
    )
    .unwrap();
    assert!(forced.errors.is_empty(), "{:?}", forced.errors);
    assert!(w.is_frozen("rustdesk.exe"));
    w.switch("neutral");
    assert!(w.is_beating("rustdesk.exe"));

    // `confirmed = true` in the entry does the same without --force.
    w.config.profiles[0].apps[0].confirmed = true;
    let confirmed = w.switch("p");
    assert!(confirmed.errors.is_empty(), "{:?}", confirmed.errors);
    assert!(w.is_frozen("rustdesk.exe"));
    w.switch("neutral");
}

#[test]
fn s5_interrupted_switch_stops_after_the_current_program() {
    let mut w = World::new();
    let _a = w.run("ks-int-a.exe");
    let _b = w.run("ks-int-b.exe");
    w.profile("p", vec![suspend("ks-int-a.exe"), suspend("ks-int-b.exe")]);
    // The third check comes before the second program: interrupt there.
    let checks = std::cell::Cell::new(0);
    let interrupted = || {
        checks.set(checks.get() + 1);
        checks.get() > 2
    };
    let store = w.store.clone();
    let report = engine::switch_with(
        &w.config,
        "p",
        &mut w.state,
        Options::default(),
        &mut |s| store.save(s),
        &interrupted,
    )
    .expect("switch ran");
    assert_eq!(report.errors.len(), 1, "{:?}", report.errors);
    assert_eq!(report.errors[0].code, Code::Interrupted);
    assert!(w.is_frozen("ks-int-a.exe"), "the current program finished");
    assert!(w.is_beating("ks-int-b.exe"), "later programs are skipped");
    assert_eq!(w.state.active_profile, "neutral", "profile not reached");
    assert_eq!(w.store.load().0, w.state, "state written");

    let recovered = engine::recover(&mut w.state, &mut |s| store.save(s)).unwrap();
    assert_eq!(recovered.events.len(), 1);
    assert!(w.is_beating("ks-int-a.exe"));
}
