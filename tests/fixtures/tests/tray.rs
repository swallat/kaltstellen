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
use std::process::{Child, Command, Stdio};
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

    /// Starts the tray and waits for its start-up recovery (S3), which is
    /// the `n`-th in this folder's log.
    fn start(&mut self, n: usize) {
        self.child = Some(self.command().spawn().unwrap());
        self.wait_log("tray: Recover finished", n);
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
    t.wait_log("tray: Switch(\"play\") finished", 1);
    assert!(t.is_frozen("ks-tray1-a.exe"));
    assert_eq!(t.store().peek().unwrap().active_profile, "play");
    t.wait_log("tray: icon active", 1);

    t.click(ID_NEUTRAL);
    t.wait_log("tray: Switch(\"neutral\") finished", 1);
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
    t.wait_log("tray: Switch(\"quiet\") finished", 1);
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
    t.wait_log("tray: Switch(\"play\") finished", 1);
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
        "tray: Switch(\"broken\") finished, active profile \"broken\", 1 error(s)",
        1,
    );
    t.wait_log("tray: icon error", 1);
    // ...and the next successful switch clears it (W2, spec section 8).
    t.click(ID_PROFILE_BASE);
    t.wait_log(
        "tray: Switch(\"play\") finished, active profile \"play\", 0 error(s)",
        1,
    );
    t.wait_log("tray: icon active", 1);
    assert!(t.is_frozen("ks-tray4-a.exe"));

    // C3: a broken file keeps the last valid configuration in effect.
    t.config(&format!("{good}typo = 1\n"));
    t.click(ID_NEUTRAL);
    t.wait_log("tray: Switch(\"neutral\") finished", 1);
    t.click(ID_PROFILE_BASE);
    t.wait_log("tray: Switch(\"play\") finished", 2);
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
    t.wait_log("tray: Switch(\"y\") finished", 1);
    let log = t.log();
    assert!(log.contains("tray: Switch(\"slow\") finished"), "{log}");
    assert!(log.contains("request for \"x\" superseded"), "{log}");
    assert!(!log.contains("Switch(\"x\") finished"), "{log}");
    assert!(t.is_frozen("ks-tray5-y.exe"));
    assert!(t.is_beating("ks-tray5-x.exe"));
    assert_eq!(t.quit(), Some(0));
}
