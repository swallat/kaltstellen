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
use std::path::PathBuf;
use std::process::{Child, Command, Output, Stdio};
use std::time::Duration;

use common::*;
use kaltstellen::autostart::{self, RUN_KEY, VALUE};
use kaltstellen::state::Store;
use kaltstellen::tray::model::{ID_BACK, ID_LAST, ID_NEUTRAL, ID_OPEN, ID_PROFILE_BASE, ID_QUIT};
use kaltstellen::win32::registry;
use kaltstellen::win32::tray::{CLASS, WM_TRAY};
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
            // Never ask about start at sign-in in tests (L8).
            .env("KALTSTELLEN_AUTOSTART_PROMPT", "skip")
            .env("KALTSTELLEN_CONFIG_DIR", self.dir.path().join("config"))
            .env("KALTSTELLEN_DATA_DIR", self.data())
            .env("KALTSTELLEN_RUN_KEY", self.run_key())
            // U4: poll and debounce fast enough for tests.
            .env("KALTSTELLEN_TRIGGER_POLL_MS", "200")
            .env("KALTSTELLEN_TRIGGER_DEBOUNCE_MS", "1000")
            // U4: notifications go to the log only, none stays on screen.
            .env("KALTSTELLEN_NOTIFY", "log")
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

    /// Posts a notification event as the shell posts it with
    /// `NOTIFYICON_VERSION_4` (U4). The tests run with
    /// `KALTSTELLEN_NOTIFY=log`, so the shell shows nothing and the test
    /// plays its part.
    fn notification_event(&self, event: isize) {
        let hwnd = tray_window(self.pid()).expect("tray window");
        // SAFETY: as in `click`.
        unsafe { PostMessageW(Some(hwnd), WM_TRAY, WPARAM(0), LPARAM((1 << 16) | event)) }
            .expect("post the notification event");
    }

    /// `NIN_BALLOONSHOW`: the notification is on screen.
    fn notification_shown(&self) {
        self.notification_event(0x402);
    }

    /// `NIN_BALLOONTIMEOUT`: it went away by itself.
    fn notification_timed_out(&self) {
        self.notification_event(0x404);
    }

    /// `NIN_BALLOONUSERCLICK`: the user clicked it.
    fn click_notification(&self) {
        self.notification_event(0x405);
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
        // The tray first, so it cannot write the Run value again.
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
        let _ = registry::delete_tree(&self.registry_root());
    }
}

/// A slow-stop window (`ks-window --close-when`): its stop ends only once
/// [`Gate::open`] is called (at the latest after 60 s), so a test decides
/// exactly how long a switch runs instead of racing a timer.
struct Gate(PathBuf);

impl Gate {
    fn new(t: &Tray, name: &str) -> Gate {
        Gate(t.dir.path().join(format!("{name}.gate")))
    }

    fn args(&self) -> [&str; 2] {
        ["--close-when", str_of(&self.0)]
    }

    fn open(&self) {
        fs::write(&self.0, b"").unwrap();
    }

    /// Waits until the window got `WM_CLOSE`: the stop step runs.
    fn wait_closing(&self) {
        let marker = ks_fixtures::window::closing_marker(&self.0);
        assert!(wait_for(LONG, || marker.exists()), "the stop never began");
    }
}

/// `[settings]` for tests with a gated stop: a stop timeout well above the
/// time the test holds the gate.
const SLOW_SETTINGS: &str = "[settings]\nstop_timeout_s = 30\n\n";

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

/// `kaltstellen quit` ends the tray of its data directory like the menu's
/// Quit; without a tray it says so and succeeds.
#[test]
fn l2_quit_from_the_command_line() {
    let mut t = Tray::new();
    let _a = t.program("ks-tray2q-a.exe");
    t.config(&profile("quiet", &[("ks-tray2q-a.exe", "suspend")]));
    t.start(1);
    t.click(ID_PROFILE_BASE);
    t.wait_log("tray: switch to \"quiet\" finished", 1);
    assert!(t.is_frozen("ks-tray2q-a.exe"));

    let (code, out, err) = t.run(&["quit"]);
    assert_eq!(code, 0, "{out}{err}");
    let mut child = t.child.take().unwrap();
    assert!(
        wait_for(LONG, || child.try_wait().unwrap().is_some()),
        "the tray ends"
    );
    assert!(t.is_beating("ks-tray2q-a.exe"), "quit resumes (L2)");

    let (code, out, _) = t.run(&["quit"]);
    assert_eq!(code, 0);
    assert!(out.contains("no tray runs"), "{out}");
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
    let gate = Gate::new(&t, "tray5");
    let Some(_slow) = window_fixture(str_of(&slow), &gate.args()) else {
        return;
    };
    let _x = t.program("ks-tray5-x.exe");
    let _y = t.program("ks-tray5-y.exe");
    t.config(&format!(
        "{SLOW_SETTINGS}{}{}{}",
        profile("slow", &[("ks-tray5-slow.exe", "stop")]),
        profile("x", &[("ks-tray5-x.exe", "suspend")]),
        profile("y", &[("ks-tray5-y.exe", "suspend")]),
    ));
    t.start(1);
    t.click(ID_PROFILE_BASE);
    t.click(ID_PROFILE_BASE + 1);
    t.click(ID_PROFILE_BASE + 2);
    // Both later clicks wait behind the slow stop, which ends only now.
    t.wait_log("tray: switch to \"y\" waits", 1);
    gate.open();
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
    let gate = Gate::new(&t, "pipe2");
    let Some(_slow) = window_fixture(str_of(&slow), &gate.args()) else {
        return;
    };
    let _x = t.program("ks-pipe2-x.exe");
    let _y = t.program("ks-pipe2-y.exe");
    t.config(&format!(
        "{SLOW_SETTINGS}{}{}{}",
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
    // Both wait behind the slow stop, which ends only now.
    t.wait_log("tray: switch to \"y\" waits", 1);
    gate.open();

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
    let gate = Gate::new(&t, "pipe3");
    let Some(_slow) = window_fixture(str_of(&slow), &gate.args()) else {
        return;
    };
    t.config(&format!(
        "{SLOW_SETTINGS}{}",
        profile("slow", &[("ks-pipe3-slow.exe", "stop")])
    ));
    t.start(1);
    let first = t.cli(&["switch", "slow"]).spawn().unwrap();
    t.wait_log("tray: running switch to \"slow\"", 1);
    let second = t.cli(&["switch", "slow"]).spawn().unwrap();
    // It joins the running switch, which ends only now.
    t.wait_log(
        "tray: switch to \"slow\" dropped, the running job leads there",
        1,
    );
    gate.open();
    let (code, out, err) = finish(second);
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
    let gate = Gate::new(&t, "ctrl");
    let Some(_slow) = window_fixture(str_of(&slow), &gate.args()) else {
        return;
    };
    let _a = t.program("ks-ctrl-a.exe");
    let _b = t.program("ks-ctrl-b.exe");
    t.config(&format!(
        "{SLOW_SETTINGS}{}{}",
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
    // Interrupt in the middle of the slow stop, which lasts until the gate
    // opens. The Ctrl handler is in place before the switch logs its start.
    t.wait_log("switch from \"first\" to \"second\"", 1);
    gate.wait_closing();
    // SAFETY: sends Ctrl+Break to the child's own process group on the
    // console this test shares with it.
    let sent = unsafe { GenerateConsoleCtrlEvent(CTRL_BREAK_EVENT, child.id()) };
    if let Err(err) = sent {
        eprintln!("skipped: cannot send Ctrl+Break here ({err})");
        gate.open();
        let _ = finish(child);
        return;
    }
    // The stop ends only once the handler has taken the interrupt.
    t.wait_log("console: interrupt requested", 1);
    gate.open();
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
fn autostart_from_the_command_line_uses_the_injected_key_and_not_the_menu() {
    // The real Run key is only read, before and after.
    let real_before = registry::read(RUN_KEY, VALUE).unwrap();
    let mut t = Tray::new();
    let expected = autostart::command_line(&kaltstellen_exe());

    let (code, out, _) = t.run(&["autostart"]);
    assert_eq!((code, out.trim()), (0, "start at sign-in: off"));
    // Only showing it is no answer to the tray's question (L8).
    assert!(!t.settings().contains("\"autostart_asked\": true"));
    let (code, out, _) = t.run(&["autostart", "on"]);
    assert_eq!((code, out.trim()), (0, "start at sign-in: on"));
    assert_eq!(
        registry::read(&t.run_key(), VALUE).unwrap(),
        Some(expected.clone())
    );
    // Choosing it is: a tray started afterwards (by the setup) does not ask.
    assert!(t.settings().contains("\"autostart_asked\": true"));
    let (code, out, _) = t.run(&["autostart", "off"]);
    assert_eq!((code, out.trim()), (0, "start at sign-in: off"));
    assert_eq!(registry::read(&t.run_key(), VALUE).unwrap(), None);

    // The setting is in the main window and on the command line, not in
    // the tray menu (owner, 2026-11-23): the old menu ID does nothing, and
    // the command line works the same while a tray runs.
    t.config("");
    t.start(1);
    t.click(4);
    t.wait_log("tray: command 4 has no meaning; ignored", 1);
    assert_eq!(registry::read(&t.run_key(), VALUE).unwrap(), None);
    let (code, out, _) = t.run(&["autostart", "on"]);
    assert_eq!((code, out.trim()), (0, "start at sign-in: on"));
    assert_eq!(registry::read(&t.run_key(), VALUE).unwrap(), Some(expected));
    let (code, _, _) = t.run(&["autostart", "off"]);
    assert_eq!(code, 0);
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
    // A pipe request is answered on the window thread after the timer
    // message posted before it, so the re-check has run by then.
    assert_eq!(t.run(&["status"]).0, 0);
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

#[test]
fn l9_a_failed_start_resolves_only_when_the_program_runs_from_its_path() {
    // Two folders with a program of one name (C2): the entry's copy in `a`
    // is missing, the one in `b` runs. B's process must not resolve A's
    // error; A's own does, without a switch.
    let mut t = Tray::new();
    t.cleanup.push(KillProgram("ks-l9b-tool.exe".to_owned()));
    let in_a = t.dir.path().join("a").join("ks-l9b-tool.exe");
    let in_b = t.dir.path().join("b").join("ks-l9b-tool.exe");
    t.config(&format!(
        "[[profile]]\nname = \"work\"\n\n  [[profile.app]]\n  exe = \"ks-l9b-tool.exe\"\n  path = '{}'\n  action = \"start\"\n",
        in_a.display()
    ));
    t.start(1);
    t.click(ID_PROFILE_BASE);
    t.wait_log("tray: icon error", 1);
    fs::create_dir_all(in_b.parent().unwrap()).unwrap();
    fs::copy(HEARTBEAT, &in_b).unwrap();
    let beat_b = t.dir.path().join("b.beat");
    let _b = Fixture::spawn(str_of(&in_b), &[str_of(&beat_b)]);
    assert!(beating(&beat_b));
    t.post(WM_TIMER, 2);
    // Answered after the re-check, which a status request runs as well.
    assert_eq!(t.run(&["status"]).0, 0);
    assert_eq!(t.logged("error resolved by itself"), 0);
    fs::create_dir_all(in_a.parent().unwrap()).unwrap();
    fs::copy(HEARTBEAT, &in_a).unwrap();
    let beat_a = t.dir.path().join("a.beat");
    let _a = Fixture::spawn(str_of(&in_a), &[str_of(&beat_a)]);
    assert!(beating(&beat_a));
    t.post(WM_TIMER, 2);
    t.wait_log("tray: error resolved by itself: KS110", 1);
    t.wait_log("tray: icon active", 1);
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

/// A double-click in Explorer: no arguments and a console of its own. The
/// main window opens and this process runs the tray, instead of printing
/// the help into a window that closes at once. From a terminal (a shared
/// console, or none, as in the other tests) it stays the help.
#[test]
fn a_double_click_opens_the_main_window_and_the_tray() {
    use std::os::windows::process::CommandExt;
    const CREATE_NEW_CONSOLE: u32 = 0x0000_0010;
    const DETACHED_PROCESS: u32 = 0x0000_0008;
    let mut t = Tray::new();
    t.config("[[profile]]\nname = \"focus\"\n");
    // The environment of a test tray, but no `tray` argument.
    let without_arguments = |flags: u32| {
        let mut command = Command::new(kaltstellen_exe());
        for (key, value) in t.command().get_envs() {
            if let Some(value) = value {
                command.env(key, value);
            }
        }
        command
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .creation_flags(flags);
        command
    };
    // No console of its own (here: none at all): the help, nothing started.
    let status = without_arguments(DETACHED_PROCESS).status().unwrap();
    assert_ne!(status.code(), Some(0));
    assert!(t.log().is_empty(), "{}", t.log());

    t.child = Some(without_arguments(CREATE_NEW_CONSOLE).spawn().unwrap());
    t.wait_log("tray: recover finished", 1);
    t.wait_log("tray: started without a terminal; console released", 1);
    t.wait_log("tray: main window started (pid ", 1);
    let pid: u32 = t
        .log()
        .split("tray: main window started (pid ")
        .nth(1)
        .and_then(|rest| rest.split(')').next())
        .and_then(|n| n.parse().ok())
        .expect("pid in the log");
    let _cleanup = KillPid(pid);
    let mut window = None;
    assert!(wait_for(LONG, || {
        window = window_of(pid, kaltstellen::win32::webview::CLASS);
        window.is_some_and(|w| unsafe { IsWindowVisible(w) }.as_bool())
    }));
    // SAFETY: posting to a window of another process.
    unsafe { PostMessageW(window, WM_CLOSE, WPARAM(0), LPARAM(0)) }.unwrap();
    assert!(wait_for(LONG, || pids_of("kaltstellen.exe")
        .iter()
        .all(|p| *p != pid)));
    assert_eq!(t.quit(), Some(0));
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
    // M2, N1: what it is, from its file: the fixture has no version
    // resource and no signature. The same after its name in `list --info`;
    // plain `list` stays as it was.
    assert!(
        out.lines()
            .any(|l| l.starts_with("  ks-n1-win.exe (new)") && l.ends_with("  unsigned")),
        "{out}"
    );
    let (code, out, _) = t.run(&["list", "ks-n1-win"]);
    assert_eq!(code, 0);
    assert!(out.starts_with("     PID   PARENT SESSION  NAME"), "{out}");
    assert!(
        out.lines().count() > 1 && out.lines().skip(1).all(|l| l.ends_with("  ks-n1-win.exe")),
        "{out}"
    );
    let (code, out, _) = t.run(&["list", "--info", "ks-n1-win"]);
    assert_eq!(code, 0);
    assert!(out.starts_with("     PID   PARENT SESSION  NAME"), "{out}");
    assert!(
        out.lines()
            .any(|l| l.contains("  ks-n1-win.exe  ") && l.ends_with("unsigned")),
        "{out}"
    );
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

#[test]
fn l8_the_tray_asks_once_about_start_at_sign_in() {
    let mut t = Tray::new();
    t.config("");
    for (n, answer) in [(1, "yes"), (2, "no")] {
        let mut command = t.command();
        command.env("KALTSTELLEN_AUTOSTART_PROMPT", answer);
        t.child = Some(command.spawn().unwrap());
        t.wait_log("tray: recover finished", n);
        if n == 1 {
            t.wait_log("tray: start at sign-in question answered yes", 1);
            t.wait_log("tray: start at sign-in on", 1);
            let expected = autostart::command_line(&kaltstellen_exe());
            assert_eq!(registry::read(&t.run_key(), VALUE).unwrap(), Some(expected));
            assert!(
                t.settings().contains("\"autostart_asked\": true"),
                "{}",
                t.settings()
            );
        }
        assert_eq!(t.quit(), Some(0));
    }
    // The second start did not ask again, whatever the answer would be.
    assert_eq!(t.logged("question answered"), 1);
}

/// A profile with trigger programs (U4).
fn triggered(name: &str, priority: i32, triggers: &[&str], apps: &[(&str, &str)]) -> String {
    let list: Vec<String> = triggers.iter().map(|t| format!("\"{t}\"")).collect();
    let head = format!(
        "[[profile]]\nname = \"{name}\"\ntrigger = [{}]\npriority = {priority}\n",
        list.join(", ")
    );
    // The entries of `profile`, without its first two lines.
    let entries = profile(name, apps);
    let entries = entries
        .split_once('\n')
        .and_then(|(_, rest)| rest.split_once('\n'))
        .map_or("", |(_, rest)| rest);
    format!("{head}{entries}")
}

#[test]
fn u4_a_trigger_applies_its_profile_and_the_end_returns() {
    let mut t = Tray::new();
    let _a = t.program("ks-trig1-a.exe");
    t.config(&format!(
        "{}{}",
        profile("work", &[]),
        triggered(
            "flight",
            0,
            &["ks-trig1-sim.exe"],
            &[("ks-trig1-a.exe", "suspend")]
        ),
    ));
    t.start(1);
    t.wait_log("tray: trigger poll every 200 ms", 1);
    let (_, out, _) = t.run(&["status"]);
    assert!(out.contains("tray: automatic switching on"), "{out}");

    let sim = t.program("ks-trig1-sim.exe");
    t.wait_log("tray: trigger ks-trig1-sim.exe of \"flight\" started", 1);
    t.wait_log(
        "tray: notification: kaltstellen: flight: ks-trig1-sim.exe started: switched to flight. Click here: Back to neutral",
        1,
    );
    assert!(
        t.log()
            .contains("tray: automatic switch to \"flight\" finished")
    );
    assert!(t.is_frozen("ks-trig1-a.exe"));
    assert!(
        t.is_beating("ks-trig1-sim.exe"),
        "the trigger itself runs on"
    );
    let (_, out, _) = t.run(&["status"]);
    assert!(out.contains("active profile: flight"), "{out}");
    assert!(
        out.contains(
            "tray: \"flight\" applied by trigger ks-trig1-sim.exe; back to \"neutral\" when it ends"
        ),
        "{out}"
    );

    // The trigger ends: back to the profile from before, after the
    // debounce.
    drop(sim);
    t.wait_log(
        "tray: notification: kaltstellen: neutral: ks-trig1-sim.exe ended: switched to neutral. Click here: Back to flight",
        1,
    );
    assert!(t.is_beating("ks-trig1-a.exe"));
    // A click before the notification is on screen does nothing and does
    // not use the way back up; one while it is on screen goes back.
    t.click_notification();
    t.wait_log("tray: notification clicked; nothing to go back to", 1);
    t.notification_shown();
    t.click_notification();
    t.wait_log("tray: back to \"flight\"", 1);
    t.wait_log("tray: switch to \"flight\" finished", 1);
    assert!(t.is_frozen("ks-trig1-a.exe"));
    assert_eq!(t.quit(), Some(0));
}

#[test]
fn u4_manual_switch_and_the_notification_click_hold_automation() {
    let mut t = Tray::new();
    let _a = t.program("ks-trig2-a.exe");
    let _b = t.program("ks-trig2-b.exe");
    t.config(&format!(
        "{}{}",
        triggered(
            "flight",
            0,
            &["ks-trig2-sim.exe", "ks-trig2-alt.exe"],
            &[("ks-trig2-a.exe", "suspend")]
        ),
        profile("work", &[("ks-trig2-b.exe", "suspend")]),
    ));
    t.start(1);
    t.wait_log("tray: trigger poll every", 1);
    let sim = t.program("ks-trig2-sim.exe");
    t.wait_log("tray: notification: kaltstellen: flight:", 1);
    assert!(t.is_frozen("ks-trig2-a.exe"));

    // `Back to neutral` by a click on the notification on screen: a manual
    // switch, which holds automation while the trigger keeps running.
    t.notification_shown();
    t.click_notification();
    t.wait_log("tray: back to \"neutral\"", 1);
    t.wait_log("tray: switch to \"neutral\" finished", 1);
    assert!(t.is_beating("ks-trig2-a.exe"));
    let (_, out, _) = t.run(&["status"]);
    assert!(
        out.contains("automatic switching held by a manual switch"),
        "{out}"
    );
    // The way back is used up: another click does nothing.
    t.click_notification();
    t.wait_log("tray: notification clicked; nothing to go back to", 1);
    // Several polls later (the status line is answered between them):
    // still held, nothing applied again.
    std::thread::sleep(Duration::from_millis(1500));
    let (_, out, _) = t.run(&["status"]);
    assert!(out.contains("active profile: neutral"), "{out}");
    assert_eq!(
        t.logged("tray: automatic switch to \"flight\" finished"),
        1,
        "held"
    );

    // A manual switch from the menu while held; then the trigger ends: the
    // hold ends, but nothing returns, the user chose "work" (rule 5). The
    // decision falls in the poll that logs the change.
    t.click(ID_PROFILE_BASE + 1);
    t.wait_log("tray: switch to \"work\" finished", 1);
    drop(sim);
    t.wait_log(
        "tray: the trigger situation changed; automatic switching goes on",
        1,
    );
    assert_eq!(t.logged("tray: automatic switch to \"neutral\""), 0);
    assert_eq!(t.store().peek().unwrap().active_profile, "work");
    assert!(t.is_frozen("ks-trig2-b.exe"));

    // A trigger starts again: automation applies its profile from "work".
    let _alt = t.program("ks-trig2-alt.exe");
    t.wait_log("Click here: Back to work", 1);
    assert!(t.is_frozen("ks-trig2-a.exe"));
    assert!(t.is_beating("ks-trig2-b.exe"));
    // Two notifications on screen: the click cannot tell which one.
    t.notification_shown();
    t.notification_shown();
    t.click_notification();
    t.wait_log("tray: notification clicked; nothing to go back to", 2);
    // Both gone: a later click (from the notification centre) does
    // nothing either; the menu item stays.
    t.notification_timed_out();
    t.notification_timed_out();
    t.wait_log("tray: notification gone; Back to stays in the menu", 1);
    t.click_notification();
    t.wait_log("tray: notification clicked; nothing to go back to", 3);
    assert!(t.is_frozen("ks-trig2-a.exe"));
    t.click(ID_BACK);
    t.wait_log("tray: back to \"work\"", 1);
    t.wait_log("tray: switch to \"work\" finished", 2);
    assert!(t.is_frozen("ks-trig2-b.exe"));
    assert_eq!(t.quit(), Some(0));
}

#[test]
fn u4_back_to_in_the_menu_works_without_the_notification() {
    let mut t = Tray::new();
    let _a = t.program("ks-trig9-a.exe");
    t.config(&triggered(
        "flight",
        0,
        &["ks-trig9-sim.exe"],
        &[("ks-trig9-a.exe", "suspend")],
    ));
    t.start(1);
    t.wait_log("tray: trigger poll every", 1);
    let _sim = t.program("ks-trig9-sim.exe");
    t.wait_log("tray: notification: kaltstellen: flight:", 1);
    assert!(t.is_frozen("ks-trig9-a.exe"));
    // No notification was ever shown (Do Not Disturb): the menu item
    // `Back to neutral` still goes back, as a manual switch.
    t.click(ID_BACK);
    t.wait_log("tray: menu asks to go back", 1);
    t.wait_log("tray: back to \"neutral\"", 1);
    t.wait_log("tray: switch to \"neutral\" finished", 1);
    assert!(t.is_beating("ks-trig9-a.exe"));
    let (_, out, _) = t.run(&["status"]);
    assert!(
        out.contains("automatic switching held by a manual switch"),
        "{out}"
    );
    // Used up by that request: the item is gone.
    t.click(ID_BACK);
    t.wait_log("tray: command 13 has no meaning; ignored", 1);
    assert_eq!(t.logged("tray: back to"), 1);
    assert_eq!(t.quit(), Some(0));
}

#[test]
fn u4_return_to_a_deleted_profile_goes_to_neutral() {
    let mut t = Tray::new();
    let _a = t.program("ks-trig7-a.exe");
    let work = profile("work", &[]);
    let flight = triggered(
        "flight",
        0,
        &["ks-trig7-sim.exe"],
        &[("ks-trig7-a.exe", "suspend")],
    );
    t.config(&format!("{work}{flight}"));
    t.start(1);
    t.wait_log("tray: trigger poll every", 1);
    t.click(ID_PROFILE_BASE);
    t.wait_log("tray: switch to \"work\" finished", 1);
    let sim = t.program("ks-trig7-sim.exe");
    t.wait_log("tray: automatic switch to \"flight\" finished", 1);
    // "work" leaves the file while the trigger runs.
    t.config(&flight);
    drop(sim);
    t.wait_log(
        "tray: \"work\" is no longer in the configuration; going to neutral",
        1,
    );
    t.wait_log("tray: automatic switch to \"neutral\" finished", 1);
    assert!(t.is_beating("ks-trig7-a.exe"));
    assert_eq!(t.quit(), Some(0));
}

#[test]
fn u4_an_automatic_request_does_not_replace_a_waiting_manual_one() {
    let mut t = Tray::new();
    t.cleanup.push(KillProgram("ks-trig8-slow.exe".to_owned()));
    let slow = copy_fixture(WINDOW, t.dir.path(), "ks-trig8-slow.exe");
    // The switch to "slow" lasts until the gate opens.
    let gate = Gate::new(&t, "trig8");
    let Some(_slow) = window_fixture(str_of(&slow), &gate.args()) else {
        return;
    };
    let _b = t.program("ks-trig8-b.exe");
    // Copied now, started while the slow switch runs.
    t.cleanup.push(KillProgram("ks-trig8-sim.exe".to_owned()));
    let sim_exe = copy_fixture(HEARTBEAT, t.dir.path(), "ks-trig8-sim.exe");
    let sim_beat = t.beat_file("ks-trig8-sim.exe");
    t.config(&format!(
        "{SLOW_SETTINGS}{}{}{}",
        profile("slow", &[("ks-trig8-slow.exe", "stop")]),
        profile("work", &[("ks-trig8-b.exe", "suspend")]),
        triggered("flight", 0, &["ks-trig8-sim.exe"], &[]),
    ));
    t.start(1);
    t.wait_log("tray: trigger poll every", 1);
    t.click(ID_PROFILE_BASE);
    t.wait_log("tray: running switch to \"slow\"", 1);
    t.click(ID_PROFILE_BASE + 1);
    t.wait_log("tray: switch to \"work\" waits", 1);
    let sim = Fixture::spawn(str_of(&sim_exe), &[str_of(&sim_beat)]);
    t.wait_log(
        "tray: automatic switch to \"flight\" ignored, a manual request waits",
        1,
    );
    gate.open();
    t.wait_log("tray: switch to \"work\" finished", 1);
    assert!(t.is_frozen("ks-trig8-b.exe"));
    // Automation owns nothing: the end of the trigger changes nothing.
    drop(sim);
    t.wait_log("tray: trigger ks-trig8-sim.exe of \"flight\" ended", 1);
    assert_eq!(t.logged("automatic switch to \"flight\" finished"), 0);
    assert_eq!(t.logged("automatic switch to \"work\""), 0);
    assert_eq!(t.store().peek().unwrap().active_profile, "work");
    assert_eq!(t.quit(), Some(0));
}

#[test]
fn u4_a_higher_priority_takes_over_and_hands_back() {
    let mut t = Tray::new();
    let _a = t.program("ks-trig3-a.exe");
    let _b = t.program("ks-trig3-b.exe");
    t.config(&format!(
        "{}{}",
        triggered(
            "low",
            1,
            &["ks-trig3-low.exe"],
            &[("ks-trig3-a.exe", "suspend")]
        ),
        triggered(
            "high",
            5,
            &["ks-trig3-high.exe"],
            &[("ks-trig3-b.exe", "suspend")]
        ),
    ));
    t.start(1);
    t.wait_log("tray: trigger poll every", 1);
    let low = t.program("ks-trig3-low.exe");
    t.wait_log("tray: automatic switch to \"low\" finished", 1);
    assert!(t.is_frozen("ks-trig3-a.exe"));

    let high = t.program("ks-trig3-high.exe");
    t.wait_log("tray: automatic switch to \"high\" finished", 1);
    assert!(t.is_frozen("ks-trig3-b.exe"));
    assert!(t.is_beating("ks-trig3-a.exe"));
    assert!(
        t.log()
            .contains("tray: trigger for \"low\" waits while \"high\" is active")
    );
    let (_, out, _) = t.run(&["status"]);
    assert!(
        out.contains(
            "tray: \"high\" applied by trigger ks-trig3-high.exe; back to \"neutral\" when it ends"
        ),
        "{out}"
    );
    assert!(
        out.contains("tray: trigger for \"low\" waits while \"high\" is active"),
        "{out}"
    );

    // The higher one ends while the lower runs: back to the lower one.
    drop(high);
    t.wait_log("tray: automatic switch to \"low\" finished", 2);
    assert!(t.is_frozen("ks-trig3-a.exe"));
    assert!(t.is_beating("ks-trig3-b.exe"));
    // Then the lower one ends: back to the profile before the first.
    drop(low);
    t.wait_log("tray: automatic switch to \"neutral\" finished", 1);
    assert!(t.is_beating("ks-trig3-a.exe"));
    assert_eq!(t.quit(), Some(0));
}

#[test]
fn u4_automatic_switching_can_be_paused_and_stays_paused() {
    let mut t = Tray::new();
    let _a = t.program("ks-trig4-a.exe");
    t.config(&triggered(
        "flight",
        0,
        &["ks-trig4-sim.exe"],
        &[("ks-trig4-a.exe", "suspend")],
    ));
    t.start(1);
    t.wait_log("tray: trigger poll every", 1);
    // The switch is in the main window (owner, 2026-11-23); the window and
    // `kaltstellen automation` hand it to the running tray over the pipe
    // (`set_automation`), which takes it over at once and writes it.
    let (code, out, err) = t.run(&["automation", "off"]);
    assert_eq!(
        (code, out.trim()),
        (0, "automatic switching: paused"),
        "{err}"
    );
    t.wait_log("tray: automatic switching paused", 1);
    t.wait_log("tray: trigger poll stopped", 1);
    assert!(
        t.settings().contains("\"triggers_paused\": true"),
        "{}",
        t.settings()
    );
    // The same again changes nothing.
    let (code, _, _) = t.run(&["automation", "off"]);
    assert_eq!(code, 0);
    t.wait_log("tray: automatic switching already paused; unchanged", 1);
    let (_, out, _) = t.run(&["automation"]);
    assert_eq!(out.trim(), "automatic switching: paused");
    // The old menu ID does nothing.
    t.click(12);
    t.wait_log("tray: command 12 has no meaning; ignored", 1);
    let _sim = t.program("ks-trig4-sim.exe");
    // The poll is stopped while paused; Quit is the later marker.
    assert!(t.is_beating("ks-trig4-a.exe"));
    assert_eq!(t.quit(), Some(0));
    assert_eq!(t.logged("automatic switch to"), 0);

    // Remembered across a restart (settings.json), and shown from it
    // without a tray.
    let (_, out, _) = t.run(&["automation"]);
    assert_eq!(out.trim(), "automatic switching: paused");
    t.start(2);
    assert_eq!(t.logged("tray: automatic switching is paused"), 1);
    let (_, out, _) = t.run(&["status"]);
    assert!(out.contains("tray: automatic switching paused"), "{out}");
    assert_eq!(t.logged("automatic switch to"), 0);

    // On again: a trigger that runs counts as starting.
    let (code, out, _) = t.run(&["automation", "on"]);
    assert_eq!((code, out.trim()), (0, "automatic switching: on"));
    t.wait_log("tray: automatic switching on", 1);
    t.wait_log("tray: automatic switch to \"flight\" finished", 1);
    assert!(t.is_frozen("ks-trig4-a.exe"));
    assert!(
        !t.settings().contains("triggers_paused"),
        "{}",
        t.settings()
    );
    // On again while on: nothing is applied again.
    let (code, _, _) = t.run(&["automation", "on"]);
    assert_eq!(code, 0);
    t.wait_log("tray: automatic switching already on; unchanged", 1);
    assert_eq!(t.logged("automatic switch to \"flight\" finished"), 1);
    assert_eq!(t.quit(), Some(0));
    assert!(t.is_beating("ks-trig4-a.exe"));

    // Without a tray the command line writes the file; the next tray
    // starts paused.
    let (code, out, _) = t.run(&["automation", "off"]);
    assert_eq!((code, out.trim()), (0, "automatic switching: paused"));
    assert!(t.settings().contains("\"triggers_paused\": true"));
    t.start(3);
    assert_eq!(t.logged("tray: automatic switching is paused"), 2);
    assert_eq!(t.quit(), Some(0));
}

#[test]
fn u4_s5_after_a_trigger_the_profile_from_before_is_offered() {
    let mut t = Tray::new();
    let _a = t.program("ks-trig5-a.exe");
    let _b = t.program("ks-trig5-b.exe");
    t.config(&format!(
        "{}{}",
        profile("work", &[("ks-trig5-b.exe", "suspend")]),
        triggered(
            "flight",
            0,
            &["ks-trig5-sim.exe"],
            &[("ks-trig5-a.exe", "suspend")]
        ),
    ));
    t.start(1);
    t.wait_log("tray: trigger poll every", 1);
    t.click(ID_PROFILE_BASE);
    t.wait_log("tray: switch to \"work\" finished", 1);
    let _sim = t.program("ks-trig5-sim.exe");
    t.wait_log("tray: automatic switch to \"flight\" finished", 1);

    t.post(WM_QUERYENDSESSION, 0);
    t.wait_log("tray: end of session finished", 1);
    t.wait_log("tray: trigger poll stopped", 1);
    assert!(t.is_beating("ks-trig5-a.exe"));
    let settings = t.settings();
    assert!(
        settings.contains("\"last_profile\": \"work\""),
        "{settings}"
    );

    // Cancelled: triggers start afresh; the one that still runs applies.
    t.post(WM_ENDSESSION, 0);
    t.wait_log("tray: automatic switch to \"flight\" finished", 2);
    assert!(t.is_frozen("ks-trig5-a.exe"));
    assert_eq!(t.quit(), Some(0));
}

#[test]
fn u4_check_config_refuses_a_trigger_its_own_profile_freezes() {
    let t = Tray::new();
    t.config(&triggered(
        "flight",
        0,
        &["ks-trig6-sim.exe"],
        &[("ks-trig6-sim.exe", "suspend")],
    ));
    let (code, _, err) = t.run(&["check-config"]);
    assert_eq!(code, 2, "{err}");
    assert!(err.contains("KS203"), "{err}");
    assert!(
        err.contains("trigger \"ks-trig6-sim.exe\" is a program that this profile freezes"),
        "{err}"
    );
    t.config(&triggered(
        "flight",
        7,
        &["ks-trig6-sim.exe"],
        &[("ks-trig6-a.exe", "suspend")],
    ));
    let (code, out, err) = t.run(&["check-config"]);
    assert_eq!(code, 0, "{err}");
    assert!(
        out.contains("profile \"flight\": 1 program(s), trigger ks-trig6-sim.exe (priority 7)"),
        "{out}"
    );
}

#[test]
fn u4_automation_from_the_command_line_without_triggers_says_so() {
    // No tray: the command line writes settings.json itself.
    let t = Tray::new();
    t.config(&profile("plain", &[("ks-auto0-a.exe", "suspend")]));
    let (code, out, err) = t.run(&["automation", "off"]);
    assert_eq!(code, 0, "{err}");
    assert_eq!(
        out.lines().collect::<Vec<_>>(),
        [
            "automatic switching: paused",
            "note: no profile has triggers yet; automatic switching does nothing until one has"
        ]
    );
    assert!(t.settings().contains("\"triggers_paused\": true"));
    let (code, out, _) = t.run(&["automation", "on"]);
    assert_eq!(code, 0);
    assert!(out.starts_with("automatic switching: on\n"), "{out}");
    assert!(!t.settings().contains("triggers_paused"));
}
