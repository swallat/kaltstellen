//! The tray app (WP7) as a process, driven like a user would through its
//! menu: the test posts the `WM_COMMAND` a menu click posts. Programs are
//! copies of the heartbeat fixture under names unique to each test; every
//! tray gets its own configuration and data folder (and with it its own
//! single-instance mutex).
//!
//! The `kaltstellen` executable comes from the same target folder as the
//! fixtures; `tests/tray.rs` of the main crate makes `cargo test
//! --workspace` build it.

mod common;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::time::Duration;

use common::*;
use kaltstellen::state::Store;
use kaltstellen::tray::model::{ID_NEUTRAL, ID_PROFILE_BASE, ID_QUIT};
use kaltstellen::win32::tray::CLASS;
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetClassNameW, GetWindowThreadProcessId, PostMessageW, WM_COMMAND,
};
use windows::core::BOOL;

const LONG: Duration = Duration::from_secs(20);

fn kaltstellen_exe() -> PathBuf {
    let exe = Path::new(HEARTBEAT).with_file_name("kaltstellen.exe");
    assert!(
        exe.exists(),
        "{} missing; run `cargo test --workspace` or `cargo build` first",
        exe.display()
    );
    exe
}

/// The tray window of process `pid`.
fn tray_window(pid: u32) -> Option<HWND> {
    struct Search {
        pid: u32,
        found: Option<HWND>,
    }
    unsafe extern "system" fn visit(hwnd: HWND, param: LPARAM) -> BOOL {
        // SAFETY: `param` points to the `Search` below for the whole call.
        let search = unsafe { &mut *(param.0 as *mut Search) };
        let mut pid = 0;
        // SAFETY: valid out pointer.
        unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
        let mut class = [0u16; 64];
        // SAFETY: valid buffer.
        let len = unsafe { GetClassNameW(hwnd, &mut class) } as usize;
        if pid == search.pid && String::from_utf16_lossy(&class[..len]) == CLASS {
            search.found = Some(hwnd);
            return false.into();
        }
        true.into()
    }
    let mut search = Search { pid, found: None };
    // SAFETY: the callback only uses `search`, which outlives the call.
    let _ = unsafe { EnumWindows(Some(visit), LPARAM(&raw mut search as isize)) };
    search.found
}

/// One tray with its own folders and programs.
struct Tray {
    dir: tempfile::TempDir,
    child: Option<Child>,
    cleanup: Vec<KillProgram>,
}

impl Tray {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("config")).unwrap();
        Tray {
            dir,
            child: None,
            cleanup: Vec::new(),
        }
    }

    fn config(&self, text: &str) {
        fs::write(self.dir.path().join("config").join("config.toml"), text).unwrap();
    }

    fn data(&self) -> PathBuf {
        self.dir.path().join("data")
    }

    fn log(&self) -> String {
        fs::read_to_string(self.data().join("kaltstellen.log")).unwrap_or_default()
    }

    fn logged(&self, text: &str) -> usize {
        self.log().matches(text).count()
    }

    /// Waits until `text` was logged at least `n` times.
    fn wait_log(&self, text: &str, n: usize) {
        assert!(
            wait_for(LONG, || self.logged(text) >= n),
            "never logged {n}x {text:?}; log:\n{}",
            self.log()
        );
    }

    fn beat_file(&self, name: &str) -> PathBuf {
        self.dir.path().join(format!("{name}.beat"))
    }

    /// Starts the program `name` (a heartbeat copy).
    fn program(&mut self, name: &str) -> Fixture {
        self.cleanup.push(KillProgram(name.to_owned()));
        let exe = copy_fixture(HEARTBEAT, self.dir.path(), name);
        let beat = self.beat_file(name);
        let fixture = Fixture::spawn(str_of(&exe), &[str_of(&beat)]);
        assert!(beating(&beat), "{name} never started beating");
        fixture
    }

    fn command(&self) -> Command {
        let mut command = Command::new(kaltstellen_exe());
        command
            .arg("tray")
            .env("KALTSTELLEN_CONFIG_DIR", self.dir.path().join("config"))
            .env("KALTSTELLEN_DATA_DIR", self.data())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        command
    }

    /// `kaltstellen ARGS` against this folder's configuration and data.
    fn cli(&self, args: &[&str]) -> Command {
        let mut command = Command::new(kaltstellen_exe());
        command
            .args(args)
            .env("KALTSTELLEN_CONFIG_DIR", self.dir.path().join("config"))
            .env("KALTSTELLEN_DATA_DIR", self.data())
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        command
    }

    /// Runs `kaltstellen ARGS` to the end: (exit code, stdout, stderr).
    fn run(&self, args: &[&str]) -> (i32, String, String) {
        let output = self.cli(args).output().unwrap();
        (
            output.status.code().unwrap_or(-1),
            String::from_utf8_lossy(&output.stdout).into_owned(),
            String::from_utf8_lossy(&output.stderr).into_owned(),
        )
    }

    /// Starts the tray and waits for its start-up recovery (S3), which is
    /// the `n`-th in this folder's log.
    fn start(&mut self, n: usize) {
        self.child = Some(self.command().spawn().unwrap());
        self.wait_log("tray: recover finished", n);
    }

    fn pid(&self) -> u32 {
        self.child.as_ref().unwrap().id()
    }

    /// Posts what a click on the menu item `id` posts.
    fn click(&self, id: u16) {
        let hwnd = tray_window(self.pid()).expect("tray window");
        // SAFETY: posting to a window of another process; a stale handle
        // makes the call fail.
        unsafe { PostMessageW(Some(hwnd), WM_COMMAND, WPARAM(usize::from(id)), LPARAM(0)) }
            .expect("post WM_COMMAND");
    }

    /// Quit from the menu; returns the exit code.
    fn quit(&mut self) -> Option<i32> {
        self.click(ID_QUIT);
        let mut child = self.child.take().unwrap();
        let ended = wait_for(LONG, || child.try_wait().unwrap().is_some());
        assert!(ended, "tray did not end after Quit");
        child.wait().unwrap().code()
    }

    /// Ends the tray the hard way, like a crash.
    fn kill(&mut self) {
        let mut child = self.child.take().unwrap();
        child.kill().unwrap();
        child.wait().unwrap();
    }

    fn store(&self) -> Store {
        Store::new(self.data())
    }

    fn is_frozen(&self, name: &str) -> bool {
        frozen(&self.beat_file(name))
    }

    fn is_beating(&self, name: &str) -> bool {
        beating(&self.beat_file(name))
    }
}

impl Drop for Tray {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

fn profile(name: &str, apps: &[(&str, &str)]) -> String {
    let mut text = format!("[[profile]]\nname = \"{name}\"\n");
    for (exe, action) in apps {
        text.push_str(&format!(
            "\n  [[profile.app]]\n  exe = \"{exe}\"\n  action = \"{action}\"\n"
        ));
    }
    text.push('\n');
    text
}

#[test]
fn tray_menu_switches_and_neutral_resumes() {
    let mut t = Tray::new();
    let _a = t.program("ks-tray1-a.exe");
    t.config(&profile("play", &[("ks-tray1-a.exe", "suspend")]));
    t.start(1);
    assert_eq!(t.logged("tray: icon neutral"), 1);

    t.click(ID_PROFILE_BASE);
    t.wait_log("tray: switch to \"play\" finished", 1);
    assert!(t.is_frozen("ks-tray1-a.exe"));
    assert_eq!(t.store().peek().unwrap().active_profile, "play");
    t.wait_log("tray: icon active", 1);

    t.click(ID_NEUTRAL);
    t.wait_log("tray: switch to \"neutral\" finished", 1);
    assert!(t.is_beating("ks-tray1-a.exe"));
    t.wait_log("tray: icon neutral", 2);

    assert_eq!(t.quit(), Some(0));
}

#[test]
fn l2_quit_resumes_frozen_programs_and_keeps_stopped_records() {
    let mut t = Tray::new();
    let _a = t.program("ks-tray2-a.exe");
    let mut b = t.program("ks-tray2-b.exe");
    t.config(&profile(
        "quiet",
        &[("ks-tray2-a.exe", "suspend"), ("ks-tray2-b.exe", "stop")],
    ));
    t.start(1);
    t.click(ID_PROFILE_BASE);
    t.wait_log("tray: switch to \"quiet\" finished", 1);
    assert!(t.is_frozen("ks-tray2-a.exe"));
    assert!(wait_for(Duration::from_secs(5), || !b.alive()));

    assert_eq!(t.quit(), Some(0));
    assert!(t.is_beating("ks-tray2-a.exe"), "Quit resumes (L2)");
    let state = t.store().peek().unwrap();
    assert!(state.suspended.is_empty());
    assert_eq!(state.stopped.len(), 1, "stopped records stay (L2)");
    assert_eq!(state.stopped[0].exe, "ks-tray2-b.exe");
    assert_eq!(pids_of("ks-tray2-b.exe"), Vec::<u32>::new());
}

#[test]
fn s3_t5_tray_start_recovers_what_a_killed_tray_froze() {
    let mut t = Tray::new();
    let _a = t.program("ks-tray3-a.exe");
    t.config(&profile("play", &[("ks-tray3-a.exe", "suspend")]));
    t.start(1);
    t.click(ID_PROFILE_BASE);
    t.wait_log("tray: switch to \"play\" finished", 1);
    t.kill();
    assert!(
        t.is_frozen("ks-tray3-a.exe"),
        "still frozen after the crash"
    );

    t.start(2);
    assert!(t.is_beating("ks-tray3-a.exe"), "recovery at start (S3)");
    let state = t.store().peek().unwrap();
    assert_eq!(state.active_profile, "neutral");
    assert!(state.suspended.is_empty());
    assert_eq!(t.quit(), Some(0));
}

#[test]
fn w2_c3_error_state_and_last_valid_configuration() {
    let mut t = Tray::new();
    let _a = t.program("ks-tray4-a.exe");
    let missing = t.dir.path().join("ks-tray4-missing.exe");
    let good = profile("play", &[("ks-tray4-a.exe", "suspend")]);
    let failing = format!(
        "[[profile]]\nname = \"broken\"\n\n  [[profile.app]]\n  exe = \"ks-tray4-missing.exe\"\n  path = '{}'\n  action = \"start\"\n",
        missing.display()
    );
    t.config(&format!("{good}{failing}"));
    t.start(1);

    // T3 in the tray: a failed start sets the error state...
    t.click(ID_PROFILE_BASE + 1);
    t.wait_log(
        "tray: switch to \"broken\" finished, active profile \"broken\", 1 error(s)",
        1,
    );
    t.wait_log("tray: icon error", 1);
    // ...and the next successful switch clears it (W2, spec section 8).
    t.click(ID_PROFILE_BASE);
    t.wait_log(
        "tray: switch to \"play\" finished, active profile \"play\", 0 error(s)",
        1,
    );
    t.wait_log("tray: icon active", 1);
    assert!(t.is_frozen("ks-tray4-a.exe"));

    // C3: a broken file keeps the last valid configuration in effect.
    t.config(&format!("{good}typo = 1\n"));
    t.click(ID_NEUTRAL);
    t.wait_log("tray: switch to \"neutral\" finished", 1);
    t.click(ID_PROFILE_BASE);
    t.wait_log("tray: switch to \"play\" finished", 2);
    assert!(t.log().contains("keeping the last valid configuration"));
    assert!(
        t.is_frozen("ks-tray4-a.exe"),
        "the old profile still applies"
    );
    t.wait_log("tray: icon error", 2);
    assert_eq!(t.quit(), Some(0));
    assert!(t.is_beating("ks-tray4-a.exe"));
}

#[test]
fn t4_w3_requests_during_a_slow_switch_run_only_the_latest() {
    let mut t = Tray::new();
    t.cleanup.push(KillProgram("ks-tray5-slow.exe".to_owned()));
    let slow = copy_fixture(WINDOW, t.dir.path(), "ks-tray5-slow.exe");
    let Some(_slow) = window_fixture(str_of(&slow), &["--close-delay-ms", "2000"]) else {
        return;
    };
    let _x = t.program("ks-tray5-x.exe");
    let _y = t.program("ks-tray5-y.exe");
    t.config(&format!(
        "{}{}{}",
        profile("slow", &[("ks-tray5-slow.exe", "stop")]),
        profile("x", &[("ks-tray5-x.exe", "suspend")]),
        profile("y", &[("ks-tray5-y.exe", "suspend")]),
    ));
    t.start(1);
    t.click(ID_PROFILE_BASE);
    t.click(ID_PROFILE_BASE + 1);
    t.click(ID_PROFILE_BASE + 2);
    t.wait_log("tray: switch to \"y\" finished", 1);
    let log = t.log();
    assert!(log.contains("tray: switch to \"slow\" finished"), "{log}");
    assert!(log.contains("switch to \"x\" superseded"), "{log}");
    assert!(!log.contains("switch to \"x\" finished"), "{log}");
    assert!(t.is_frozen("ks-tray5-y.exe"));
    assert!(t.is_beating("ks-tray5-x.exe"));
    assert_eq!(t.quit(), Some(0));
}

/// The output without PIDs, which differ from run to run.
fn without_pids(text: &str) -> String {
    text.lines()
        .map(|line| match line.find(" (pid ") {
            Some(at) => format!("{} (pid _)", &line[..at]),
            None => line.to_owned(),
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn finish(child: Child) -> (i32, String, String) {
    let Output {
        status,
        stdout,
        stderr,
    } = child.wait_with_output().unwrap();
    (
        status.code().unwrap_or(-1),
        String::from_utf8_lossy(&stdout).into_owned(),
        String::from_utf8_lossy(&stderr).into_owned(),
    )
}

#[test]
fn l1_command_line_goes_through_the_tray_and_answers_like_in_process() {
    let mut t = Tray::new();
    let _a = t.program("ks-pipe1-a.exe");
    t.config(&profile("play", &[("ks-pipe1-a.exe", "suspend")]));

    // No tray: in-process (fallback), the reference output.
    let (code, alone_out, alone_err) = t.run(&["switch", "play"]);
    assert_eq!(code, 0, "{alone_err}");
    assert!(t.is_frozen("ks-pipe1-a.exe"));
    let (code, _, _) = t.run(&["switch", "neutral"]);
    assert_eq!(code, 0);
    assert!(t.is_beating("ks-pipe1-a.exe"));
    assert_eq!(t.logged("command line asks"), 0);

    t.start(1);
    let (code, out, err) = t.run(&["switch", "play"]);
    assert_eq!(code, 0, "{err}");
    assert_eq!(without_pids(&out), without_pids(&alone_out));
    assert_eq!(err, alone_err);
    assert_eq!(
        t.logged("tray: command line asks for switch to \"play\""),
        1
    );
    assert!(t.is_frozen("ks-pipe1-a.exe"));
    t.wait_log("tray: icon active", 1);

    let (code, out, _) = t.run(&["status"]);
    assert_eq!(code, 0);
    assert!(out.contains("active profile: play"), "{out}");
    assert!(out.contains("frozen: 1"), "{out}");

    // E2 code 4 with the list of profiles, as in-process.
    let (code, _, err) = t.run(&["switch", "nope"]);
    assert_eq!(code, 4);
    assert!(err.contains("KS401"), "{err}");
    assert!(err.contains("profiles: neutral, play"), "{err}");

    // E2 code 2 with file and line; neutral still works.
    t.config(&format!(
        "{}typo = 1\n",
        profile("play", &[("ks-pipe1-a.exe", "suspend")])
    ));
    let (code, _, err) = t.run(&["switch", "play"]);
    assert_eq!(code, 2);
    assert!(
        err.contains("config.toml:") && err.contains("KS202"),
        "{err}"
    );
    let (code, out, err) = t.run(&["recover"]);
    assert_eq!(code, 0, "{err}");
    assert!(out.contains("recover ks-pipe1-a.exe"), "{out}");
    assert!(t.is_beating("ks-pipe1-a.exe"));
    t.config(&profile("play", &[("ks-pipe1-a.exe", "suspend")]));

    // --no-wait returns once the tray has queued the request.
    let (code, out, _) = t.run(&["switch", "play", "--no-wait"]);
    assert_eq!(code, 0);
    assert!(out.contains("by the tray"), "{out}");
    t.wait_log("tray: switch to \"play\" finished", 2);
    assert!(t.is_frozen("ks-pipe1-a.exe"));

    // A second tray still only reports.
    let (code, _, err) = t.run(&["tray"]);
    assert_eq!(code, 0);
    assert!(err.contains("already running"), "{err}");

    assert_eq!(t.quit(), Some(0));
    assert!(t.is_beating("ks-pipe1-a.exe"));
}

#[test]
fn t4_w3_through_the_pipe_the_superseded_request_ends_with_code_3() {
    let mut t = Tray::new();
    t.cleanup.push(KillProgram("ks-pipe2-slow.exe".to_owned()));
    let slow = copy_fixture(WINDOW, t.dir.path(), "ks-pipe2-slow.exe");
    let Some(_slow) = window_fixture(str_of(&slow), &["--close-delay-ms", "3000"]) else {
        return;
    };
    let _x = t.program("ks-pipe2-x.exe");
    let _y = t.program("ks-pipe2-y.exe");
    t.config(&format!(
        "{}{}{}",
        profile("slow", &[("ks-pipe2-slow.exe", "stop")]),
        profile("x", &[("ks-pipe2-x.exe", "suspend")]),
        profile("y", &[("ks-pipe2-y.exe", "suspend")]),
    ));
    t.start(1);

    let first = t.cli(&["switch", "slow"]).spawn().unwrap();
    t.wait_log("tray: running switch to \"slow\"", 1);
    let second = t.cli(&["switch", "x"]).spawn().unwrap();
    t.wait_log("tray: switch to \"x\" waits", 1);
    let third = t.cli(&["switch", "y"]).spawn().unwrap();

    let (code, _, err) = finish(second);
    assert_eq!(code, 3, "the waiting request is superseded: {err}");
    assert!(err.contains("KS402") && err.contains("\"y\""), "{err}");
    let (code, out, err) = finish(first);
    assert_eq!(code, 0, "{err}");
    assert!(out.contains("stop ks-pipe2-slow.exe"), "{out}");
    let (code, out, err) = finish(third);
    assert_eq!(code, 0, "{err}");
    assert!(
        err.contains("waiting for the tray's running switch"),
        "{err}"
    );
    assert!(out.contains("active profile: y"), "{out}");
    assert!(t.is_frozen("ks-pipe2-y.exe"));
    assert!(t.is_beating("ks-pipe2-x.exe"), "x never ran");
    assert!(!t.log().contains("switch to \"x\" finished"));
    assert_eq!(t.quit(), Some(0));
}

#[test]
fn l1_a_request_equal_to_the_running_switch_shares_its_result() {
    let mut t = Tray::new();
    t.cleanup.push(KillProgram("ks-pipe3-slow.exe".to_owned()));
    let slow = copy_fixture(WINDOW, t.dir.path(), "ks-pipe3-slow.exe");
    let Some(_slow) = window_fixture(str_of(&slow), &["--close-delay-ms", "2000"]) else {
        return;
    };
    t.config(&profile("slow", &[("ks-pipe3-slow.exe", "stop")]));
    t.start(1);
    let first = t.cli(&["switch", "slow"]).spawn().unwrap();
    t.wait_log("tray: running switch to \"slow\"", 1);
    let (code, out, err) = t.run(&["switch", "slow"]);
    assert_eq!(code, 0, "{err}");
    assert!(out.contains("stop ks-pipe3-slow.exe"), "same result: {out}");
    assert_eq!(finish(first).0, 0);
    assert_eq!(t.logged("tray: running switch to \"slow\""), 1, "ran once");
    assert_eq!(t.quit(), Some(0));
}
