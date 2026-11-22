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
use kaltstellen::autostart::{self, RUN_KEY, VALUE};
use kaltstellen::state::Store;
use kaltstellen::tray::model::{
    ID_AUTOSTART, ID_LAST, ID_NEUTRAL, ID_OPEN, ID_PROFILE_BASE, ID_QUIT,
};
use kaltstellen::win32::registry;
use kaltstellen::win32::tray::CLASS;
use std::os::windows::process::CommandExt;

use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::System::Console::{CTRL_BREAK_EVENT, GenerateConsoleCtrlEvent};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    HOT_KEY_MODIFIERS, MOD_ALT, MOD_CONTROL, MOD_NOREPEAT, MOD_SHIFT, RegisterHotKey,
    UnregisterHotKey,
};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetClassNameW, GetWindowThreadProcessId, IsWindowVisible, PostMessageW, WM_CLOSE,
    WM_COMMAND, WM_ENDSESSION, WM_HOTKEY, WM_POWERBROADCAST, WM_QUERYENDSESSION, WM_TIMER,
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
    window_of(pid, CLASS)
}

/// The top-level window of class `class` of process `pid`.
fn window_of(pid: u32, class: &str) -> Option<HWND> {
    struct Search<'a> {
        pid: u32,
        class: &'a str,
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
        if pid == search.pid && String::from_utf16_lossy(&class[..len]) == search.class {
            search.found = Some(hwnd);
            return false.into();
        }
        true.into()
    }
    let mut search = Search {
        pid,
        class,
        found: None,
    };
    // SAFETY: the callback only uses `search`, which outlives the call.
    let _ = unsafe { EnumWindows(Some(visit), LPARAM(&raw mut search as isize)) };
    search.found
}

/// One tray with its own folders and programs.
struct Tray {
    child: Option<Child>,
    cleanup: Vec<KillProgram>,
    /// Dropped last, after the programs running from it were ended, so
    /// the folder can be removed.
    dir: tempfile::TempDir,
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

    /// Root of this test's registry key; removed when the test ends.
    fn registry_root(&self) -> String {
        let name = self
            .dir
            .path()
            .file_name()
            .unwrap()
            .to_string_lossy()
            .into_owned();
        format!("Software\\kaltstellen-test-{name}")
    }

    fn run_key(&self) -> String {
        format!("{}\\Run", self.registry_root())
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
            .env("KALTSTELLEN_RUN_KEY", self.run_key())
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
            .env("KALTSTELLEN_RUN_KEY", self.run_key())
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

    /// Posts a window message as Windows would send it.
    fn post(&self, message: u32, wparam: usize) {
        let hwnd = tray_window(self.pid()).expect("tray window");
        // SAFETY: as in `click`.
        unsafe { PostMessageW(Some(hwnd), message, WPARAM(wparam), LPARAM(0)) }
            .expect("post message");
    }

    fn settings(&self) -> String {
        fs::read_to_string(self.dir.path().join("config").join("settings.json")).unwrap_or_default()
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
        let _ = registry::delete_tree(&self.registry_root());
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

const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;

#[test]
fn s5_ctrl_break_during_an_in_process_switch_leaves_nothing_frozen() {
    let mut t = Tray::new();
    t.cleanup.push(KillProgram("ks-ctrl-slow.exe".to_owned()));
    let slow = copy_fixture(WINDOW, t.dir.path(), "ks-ctrl-slow.exe");
    let Some(_slow) = window_fixture(str_of(&slow), &["--close-delay-ms", "3000"]) else {
        return;
    };
    let _a = t.program("ks-ctrl-a.exe");
    let _b = t.program("ks-ctrl-b.exe");
    t.config(&format!(
        "{}{}",
        profile("first", &[("ks-ctrl-a.exe", "suspend")]),
        profile(
            "second",
            &[
                ("ks-ctrl-a.exe", "suspend"),
                ("ks-ctrl-slow.exe", "stop"),
                ("ks-ctrl-b.exe", "suspend"),
            ],
        ),
    ));
    // No tray: both switches run in-process.
    assert_eq!(t.run(&["switch", "first"]).0, 0);
    assert!(t.is_frozen("ks-ctrl-a.exe"));

    let child = t
        .cli(&["switch", "second"])
        .creation_flags(CREATE_NEW_PROCESS_GROUP)
        .spawn()
        .unwrap();
    // The slow stop runs for 3 s; interrupt in the middle of it.
    std::thread::sleep(Duration::from_millis(1000));
    // SAFETY: sends Ctrl+Break to the child's own process group on the
    // console this test shares with it.
    if let Err(err) = unsafe { GenerateConsoleCtrlEvent(CTRL_BREAK_EVENT, child.id()) } {
        eprintln!("skipped: cannot send Ctrl+Break here ({err})");
        let _ = finish(child);
        return;
    }
    let (code, out, err) = finish(child);
    assert_eq!(code, 1, "{out}\n{err}");
    assert!(err.contains("KS403"), "{err}");
    assert!(
        out.contains("stop ks-ctrl-slow.exe"),
        "the running step finished: {out}"
    );
    assert!(out.contains("recover ks-ctrl-a.exe"), "{out}");
    assert!(out.contains("active profile: neutral"), "{out}");
    assert!(t.is_beating("ks-ctrl-a.exe"), "resumed after the interrupt");
    assert!(t.is_beating("ks-ctrl-b.exe"), "never frozen");
    let state = t.store().peek().unwrap();
    assert!(state.suspended.is_empty());
    assert_eq!(state.active_profile, "neutral");
}

#[test]
fn s5_t7_end_of_session_resumes_keeps_the_profile_for_next_time() {
    let mut t = Tray::new();
    let _a = t.program("ks-wp8-a.exe");
    let mut b = t.program("ks-wp8-b.exe");
    t.config(&profile(
        "play",
        &[("ks-wp8-a.exe", "suspend"), ("ks-wp8-b.exe", "stop")],
    ));
    t.start(1);
    t.click(ID_PROFILE_BASE);
    t.wait_log("tray: switch to \"play\" finished", 1);
    assert!(t.is_frozen("ks-wp8-a.exe"));
    assert!(wait_for(Duration::from_secs(5), || !b.alive()));

    // WM_QUERYENDSESSION: everything resumed before the answer.
    t.post(WM_QUERYENDSESSION, 0);
    t.wait_log("tray: end of session finished", 1);
    assert!(t.is_beating("ks-wp8-a.exe"));
    assert!(t.settings().contains("\"play\""), "{}", t.settings());
    assert_eq!(t.store().peek().unwrap().stopped.len(), 1);

    // Cancelled: the profile stays off and is offered again; records stay.
    t.post(WM_ENDSESSION, 0);
    t.wait_log("tray: end of session cancelled; offering \"play\" again", 1);
    assert_eq!(t.store().peek().unwrap().stopped.len(), 1);
    assert!(t.is_beating("ks-wp8-a.exe"));

    // Now for real: stopped records go (S5).
    t.post(WM_QUERYENDSESSION, 0);
    t.wait_log("tray: end of session finished", 2);
    t.post(WM_ENDSESSION, 1);
    t.wait_log("tray: session ends; 1 stopped record(s) cleared", 1);
    assert!(t.store().peek().unwrap().stopped.is_empty());

    // Standby is only logged.
    t.post(WM_POWERBROADCAST, 0x4);
    t.wait_log("tray: power event suspend (0x4)", 1);
    assert_eq!(t.quit(), Some(0));

    // Next sign-in: offered, not applied.
    t.start(2);
    assert_eq!(t.logged("tray: offering last profile \"play\""), 1);
    assert!(t.is_beating("ks-wp8-a.exe"), "not applied by itself");
    t.click(ID_LAST);
    t.wait_log("tray: switch to \"play\" finished", 2);
    assert!(t.is_frozen("ks-wp8-a.exe"));
    assert!(!t.settings().contains("\"play\""), "the offer is used up");
    assert_eq!(t.quit(), Some(0));
    assert!(t.is_beating("ks-wp8-a.exe"));
}

#[test]
fn autostart_from_the_command_line_and_the_menu_uses_the_injected_key() {
    // The real Run key is only read, before and after.
    let real_before = registry::read(RUN_KEY, VALUE).unwrap();
    let mut t = Tray::new();
    let expected = autostart::command_line(&kaltstellen_exe());

    let (code, out, _) = t.run(&["autostart"]);
    assert_eq!((code, out.trim()), (0, "start at sign-in: off"));
    let (code, out, _) = t.run(&["autostart", "on"]);
    assert_eq!((code, out.trim()), (0, "start at sign-in: on"));
    assert_eq!(
        registry::read(&t.run_key(), VALUE).unwrap(),
        Some(expected.clone())
    );
    let (code, out, _) = t.run(&["autostart", "off"]);
    assert_eq!((code, out.trim()), (0, "start at sign-in: off"));
    assert_eq!(registry::read(&t.run_key(), VALUE).unwrap(), None);

    // The tray's menu item toggles the same value.
    t.config("");
    t.start(1);
    t.click(ID_AUTOSTART);
    t.wait_log("tray: start at sign-in on", 1);
    assert_eq!(registry::read(&t.run_key(), VALUE).unwrap(), Some(expected));
    t.click(ID_AUTOSTART);
    t.wait_log("tray: start at sign-in off", 1);
    assert_eq!(registry::read(&t.run_key(), VALUE).unwrap(), None);
    assert_eq!(t.quit(), Some(0));

    let root = t.registry_root();
    drop(t);
    assert!(!registry::exists(&root).unwrap(), "test key removed");
    assert_eq!(
        registry::read(RUN_KEY, VALUE).unwrap(),
        real_before,
        "real Run key untouched"
    );
}

#[test]
fn l9_an_error_resolves_by_itself_when_its_cause_is_gone() {
    let mut t = Tray::new();
    t.cleanup.push(KillProgram("ks-l9-tool.exe".to_owned()));
    let missing = t.dir.path().join("tools").join("ks-l9-tool.exe");
    t.config(&format!(
        "[[profile]]\nname = \"work\"\n\n  [[profile.app]]\n  exe = \"ks-l9-tool.exe\"\n  path = '{}'\n  action = \"start\"\n",
        missing.display()
    ));
    t.start(1);
    t.click(ID_PROFILE_BASE);
    t.wait_log("tray: icon error", 1);
    // A re-check before anything changed keeps the error.
    t.post(WM_TIMER, 2);
    std::thread::sleep(Duration::from_millis(500));
    assert_eq!(t.logged("error resolved by itself"), 0);
    // The owner starts the program by hand: the error is gone without a
    // switch at the next re-check (posted here instead of waiting 30 s).
    fs::create_dir_all(missing.parent().unwrap()).unwrap();
    fs::copy(HEARTBEAT, &missing).unwrap();
    let beat = t.dir.path().join("tool.beat");
    let _tool = Fixture::spawn(str_of(&missing), &[str_of(&beat)]);
    assert!(beating(&beat));
    t.post(WM_TIMER, 2);
    t.wait_log("tray: error resolved by itself: KS110", 1);
    t.wait_log("tray: icon active", 1);
    assert_eq!(
        t.store().peek().unwrap().active_profile,
        "work",
        "no switch"
    );
    assert_eq!(t.quit(), Some(0));
}

/// A hotkey this test thread holds, like another program would; released
/// on drop. Only Ctrl+Shift+Alt with F22 to F24, which keyboards rarely have.
struct HeldHotkey(i32);

const VK_F22: u32 = 0x85;
const VK_F23: u32 = 0x86;
const VK_F24: u32 = 0x87;

impl HeldHotkey {
    fn take(id: i32, vk: u32) -> Option<HeldHotkey> {
        let mods = HOT_KEY_MODIFIERS(MOD_CONTROL.0 | MOD_SHIFT.0 | MOD_ALT.0 | MOD_NOREPEAT.0);
        // SAFETY: registers for this thread; released in Drop on the same
        // thread.
        unsafe { RegisterHotKey(None, id, mods, vk) }
            .ok()
            .map(|()| HeldHotkey(id))
    }
}

impl Drop for HeldHotkey {
    fn drop(&mut self) {
        // SAFETY: registered by this thread with this ID.
        let _ = unsafe { UnregisterHotKey(None, self.0) };
    }
}

#[test]
fn t17_wp9_hotkeys_register_report_taken_ones_once_and_switch() {
    let Some(blocker) = HeldHotkey::take(0x7101, VK_F23) else {
        eprintln!("skipped: Ctrl+Shift+Alt+F23 is held by another program");
        return;
    };
    let mut t = Tray::new();
    let _x = t.program("ks-hk-x.exe");
    let _y = t.program("ks-hk-y.exe");
    let config = |b_key: &str| {
        format!(
            "[[profile]]\nname = \"a\"\nhotkey = \"Ctrl+Shift+Alt+F23\"\n\n  [[profile.app]]\n  exe = \"ks-hk-x.exe\"\n\n\
             [[profile]]\nname = \"b\"\nhotkey = \"{b_key}\"\n\n  [[profile.app]]\n  exe = \"ks-hk-y.exe\"\n\n\
             [[profile]]\nname = \"c\"\nhotkey = \"Ctrl+Alt+7\"\n"
        )
    };
    t.config(&config("Ctrl+Shift+Alt+F24"));
    t.start(1);
    t.wait_log(
        "tray: hotkey Ctrl+Shift+Alt+F24 registered for \"b\" (id 2)",
        1,
    );
    t.wait_log(
        "tray: hotkey Ctrl+Shift+Alt+F23 for \"a\" is not available",
        1,
    );
    assert!(
        !t.log().contains("Ctrl+Alt+7 registered"),
        "C7: AltGr key left out"
    );

    // A pressed hotkey goes through the queue like a menu click. The press
    // is posted to the test tray; no real keyboard input is synthesised.
    t.post(WM_HOTKEY, 2);
    t.wait_log("tray: hotkey asks for \"b\"", 1);
    t.wait_log("tray: switch to \"b\" finished", 1);
    assert!(t.is_frozen("ks-hk-y.exe"));

    // The profile with the taken hotkey still works from the menu, and the
    // failure is not reported again.
    t.click(ID_PROFILE_BASE);
    t.wait_log("tray: switch to \"a\" finished", 1);
    assert!(t.is_frozen("ks-hk-x.exe"));
    assert_eq!(t.logged("Ctrl+Shift+Alt+F23 for \"a\" is not available"), 1);

    // Once the other program lets go, the next sync takes the hotkey.
    drop(blocker);
    t.click(ID_NEUTRAL);
    t.wait_log(
        "tray: hotkey Ctrl+Shift+Alt+F23 registered for \"a\" (id 1)",
        1,
    );

    // Changed configuration: the old hotkey is released, the new one held.
    t.config(&config("Ctrl+Shift+Alt+F22"));
    t.click(ID_NEUTRAL);
    t.wait_log("tray: hotkey Ctrl+Shift+Alt+F24 for \"b\" released", 1);
    t.wait_log(
        "tray: hotkey Ctrl+Shift+Alt+F22 registered for \"b\" (id 2)",
        1,
    );
    assert!(
        HeldHotkey::take(0x7102, VK_F24).is_some(),
        "F24 is free again"
    );

    assert_eq!(t.quit(), Some(0));
    // Nothing stays registered after the tray ends.
    assert!(HeldHotkey::take(0x7103, VK_F22).is_some());
    assert!(HeldHotkey::take(0x7104, VK_F23).is_some());
}

#[test]
fn wp10_open_kaltstellen_starts_the_main_window_as_its_own_process() {
    let mut t = Tray::new();
    t.config("[[profile]]\nname = \"focus\"\n");
    t.start(1);
    t.click(ID_OPEN);
    t.wait_log("tray: main window started (pid ", 1);
    let log = t.log();
    let pid: u32 = log
        .split("tray: main window started (pid ")
        .nth(1)
        .and_then(|rest| rest.split(')').next())
        .and_then(|n| n.parse().ok())
        .expect("pid in the log");
    let _cleanup = KillPid(pid);
    let mut window = None;
    assert!(wait_for(LONG, || {
        window = window_of(pid, kaltstellen::win32::webview::CLASS);
        // Shown once its controls exist.
        window.is_some_and(|w| unsafe { IsWindowVisible(w) }.as_bool())
    }));
    // SAFETY: posting to a window of another process.
    unsafe { PostMessageW(window, WM_CLOSE, WPARAM(0), LPARAM(0)) }.unwrap();
    assert!(wait_for(LONG, || pids_of("kaltstellen.exe")
        .iter()
        .all(|p| *p != pid)));
    assert_eq!(t.quit(), Some(0));
    // The WebView2 user data in the test folder is released a little later.
    remove_dir_retry(t.dir.path());
}

#[test]
fn t22_n1_unassigned_programs_status_and_tray_menu() {
    let mut t = Tray::new();
    t.cleanup.push(KillProgram("ks-n1-win.exe".to_owned()));
    let _a = t.program("ks-n1-a.exe");
    t.config(&profile("play", &[("ks-n1-a.exe", "ignore")]));
    // Make "play" active in-process first (no tray yet).
    assert_eq!(t.run(&["switch", "play"]).0, 0);
    let (code, out, err) = t.run(&["status", "--unassigned"]);
    assert_eq!(code, 0, "{err}");
    assert!(out.contains("unassigned in \"play\":"), "{out}");
    assert!(!out.contains("ks-n1-a.exe"), "it has an entry: {out}");

    // A program with a visible window that appears later is new.
    std::thread::sleep(Duration::from_millis(1100));
    let window = copy_fixture(WINDOW, t.dir.path(), "ks-n1-win.exe");
    let Some(_window) = window_fixture(str_of(&window), &[]) else {
        return;
    };
    let (_, out, _) = t.run(&["status", "--unassigned"]);
    assert!(out.contains("  ks-n1-win.exe (new)"), "{out}");
    let seen = fs::read_to_string(t.data().join("seen.json")).unwrap();
    assert!(seen.contains("ks-n1-win.exe"));

    // The tray counts them after applying the profile and shows the line.
    t.start(1);
    t.click(ID_PROFILE_BASE);
    t.wait_log("tray: switch to \"play\" finished", 1);
    t.wait_log("unassigned program(s) in \"play\"", 1);
    let count: usize = t
        .log()
        .split("tray: ")
        .filter_map(|l| {
            l.split(" unassigned program(s) in \"play\"")
                .next()?
                .parse()
                .ok()
        })
        .last()
        .unwrap();
    assert!(count >= 1);
    assert_eq!(t.quit(), Some(0));
}

#[test]
fn t16_a_switch_over_the_pipe_protects_its_caller_and_its_ancestors() {
    let mut t = Tray::new();
    t.cleanup.push(KillProgram("ks-t16-root.exe".to_owned()));
    t.cleanup.push(KillProgram("ks-t16-sib.exe".to_owned()));
    let root = copy_fixture(HEARTBEAT, t.dir.path(), "ks-t16-root.exe");
    let sibling = copy_fixture(HEARTBEAT, t.dir.path(), "ks-t16-sib.exe");
    let root_beat = t.dir.path().join("root.beat");
    let sibling_beat = t.dir.path().join("sib.beat");
    let exit = t.dir.path().join("client.exit");
    t.config(&profile("play", &[("ks-t16-root.exe", "suspend")]));
    t.start(1);

    // The program marked `suspend` runs `kaltstellen switch play` itself, so
    // the client and its parent are part of the program being frozen.
    let child = Command::new(&root)
        .args([
            str_of(&root_beat),
            "--sibling",
            str_of(&sibling),
            "--sibling-beat",
            str_of(&sibling_beat),
            "--run",
            str_of(&kaltstellen_exe()),
            "--run-args",
            "switch;play",
            "--run-exit",
            str_of(&exit),
        ])
        .env("KALTSTELLEN_CONFIG_DIR", t.dir.path().join("config"))
        .env("KALTSTELLEN_DATA_DIR", t.data())
        .env("KALTSTELLEN_RUN_KEY", t.run_key())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let _root = Fixture(child);
    assert!(
        wait_for(LONG, || exit.exists()),
        "the client never returned; log:\n{}",
        t.log()
    );
    assert_eq!(fs::read_to_string(&exit).unwrap(), "0");
    t.wait_log("tray: command line asks for switch to \"play\"", 1);
    assert!(t.is_beating("root"), "the caller's ancestor keeps running");
    assert!(frozen(&sibling_beat), "the rest of the program freezes");
    assert_eq!(t.run(&["switch", "neutral"]).0, 0);
    assert!(beating(&sibling_beat));
    assert_eq!(t.quit(), Some(0));
}
