//! The main window (`kaltstellen gui`, WP10) as a process, without eyes:
//! the window and its controls exist, messages posted to the controls have
//! the expected effect on the configuration file, and the process ends on
//! `WM_CLOSE`. What it looks like is in `docs/manual-checks.md`.

mod common;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use common::*;
use kaltstellen::gui::model::ids;
use kaltstellen::win32::gui::CLASS;
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetClassNameW, GetDlgItem, GetWindowThreadProcessId, IsWindowVisible,
    PostMessageW, SMTO_ABORTIFHUNG, SendMessageTimeoutW, WM_CLOSE, WM_COMMAND, WM_GETTEXT,
    WM_SETTEXT,
};
use windows::core::BOOL;

const LONG: Duration = Duration::from_secs(20);
const LB_SETCURSEL: u32 = 0x0186;
const LBN_SELCHANGE: usize = 1;
const LVM_GETITEMCOUNT: u32 = 0x1004;

fn kaltstellen_exe() -> PathBuf {
    Path::new(HEARTBEAT).with_file_name("kaltstellen.exe")
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
        // SAFETY: valid out pointer and buffer.
        unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
        let mut name = [0u16; 64];
        let len = unsafe { GetClassNameW(hwnd, &mut name) } as usize;
        // Visible: shown once all controls exist.
        let visible = unsafe { IsWindowVisible(hwnd) }.as_bool();
        if visible && pid == search.pid && String::from_utf16_lossy(&name[..len]) == search.class {
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

struct Gui {
    child: Option<Child>,
    main: HWND,
    /// Dropped last, after the programs running from it were ended, so
    /// the folder can be removed.
    dir: tempfile::TempDir,
}

impl Gui {
    fn config_file(&self) -> PathBuf {
        self.dir.path().join("config").join("config.toml")
    }

    fn command(dir: &Path, args: &[&str]) -> Command {
        let mut command = Command::new(kaltstellen_exe());
        command
            .args(args)
            .env("KALTSTELLEN_CONFIG_DIR", dir.join("config"))
            .env("KALTSTELLEN_DATA_DIR", dir.join("data"))
            .env(
                "KALTSTELLEN_RUN_KEY",
                "Software\\kaltstellen-test-gui-unused\\Run",
            )
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        command
    }

    fn open(config: &str) -> Gui {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("config")).unwrap();
        fs::write(dir.path().join("config").join("config.toml"), config).unwrap();
        let child = Self::command(dir.path(), &["gui"]).spawn().unwrap();
        let pid = child.id();
        let mut main = None;
        assert!(
            wait_for(LONG, || {
                main = window_of(pid, CLASS);
                main.is_some()
            }),
            "the main window never appeared"
        );
        Gui {
            dir,
            child: Some(child),
            main: main.unwrap(),
        }
    }

    fn control(&self, id: u16) -> HWND {
        // SAFETY: lookup of a child window of another process.
        unsafe { GetDlgItem(Some(self.main), i32::from(id)) }
            .unwrap_or_else(|_| panic!("control {id} missing"))
    }

    fn send(&self, hwnd: HWND, msg: u32, wparam: usize, lparam: isize) -> usize {
        let mut result = 0usize;
        // SAFETY: the system marshals WM_SETTEXT across processes; the other
        // messages carry plain numbers.
        unsafe {
            SendMessageTimeoutW(
                hwnd,
                msg,
                WPARAM(wparam),
                LPARAM(lparam),
                SMTO_ABORTIFHUNG,
                5000,
                Some(&mut result),
            )
        };
        result
    }

    fn set_text(&self, id: u16, text: &str) {
        let text: Vec<u16> = text.encode_utf16().chain(Some(0)).collect();
        self.send(self.control(id), WM_SETTEXT, 0, text.as_ptr() as isize);
    }

    /// The text of a control, read with WM_GETTEXT, which the system
    /// marshals across processes (GetWindowText would not ask an edit
    /// control of another process).
    fn text(&self, id: u16) -> String {
        let mut buffer = [0u16; 512];
        let n = self.send(
            self.control(id),
            WM_GETTEXT,
            buffer.len(),
            buffer.as_mut_ptr() as isize,
        );
        String::from_utf16_lossy(&buffer[..n.min(buffer.len())])
    }

    /// What a click on the button posts to the window.
    fn click(&self, id: u16) {
        let button = self.control(id);
        // SAFETY: posting to a window of another process.
        unsafe {
            PostMessageW(
                Some(self.main),
                WM_COMMAND,
                WPARAM(usize::from(id)),
                LPARAM(button.0 as isize),
            )
        }
        .unwrap();
    }

    fn select_profile(&self, index: usize) {
        let list = self.control(ids::PROFILES);
        self.send(list, LB_SETCURSEL, index, 0);
        // SAFETY: as in `click`.
        unsafe {
            PostMessageW(
                Some(self.main),
                WM_COMMAND,
                WPARAM(usize::from(ids::PROFILES) | (LBN_SELCHANGE << 16)),
                LPARAM(list.0 as isize),
            )
        }
        .unwrap();
    }

    fn rows(&self, id: u16) -> usize {
        self.send(self.control(id), LVM_GETITEMCOUNT, 0, 0)
    }

    fn file(&self) -> String {
        fs::read_to_string(self.config_file()).unwrap_or_default()
    }

    fn close(&mut self) -> Option<i32> {
        // SAFETY: as in `click`.
        unsafe { PostMessageW(Some(self.main), WM_CLOSE, WPARAM(0), LPARAM(0)) }.unwrap();
        let mut child = self.child.take().unwrap();
        assert!(
            wait_for(LONG, || child.try_wait().unwrap().is_some()),
            "the window did not close"
        );
        child.wait().unwrap().code()
    }
}

impl Drop for Gui {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

const CONFIG: &str = "# Owner's notes stay.\n[[profile]]\nname = \"focus\"   # main\n\n  [[profile.app]]\n  exe = \"ks-gui-a.exe\"\n";

#[test]
fn wp10_window_has_its_controls_and_edits_keep_the_file_valid_and_commented() {
    let mut g = Gui::open(CONFIG);
    for id in [
        ids::PROFILES,
        ids::NAME,
        ids::NEW,
        ids::COPY,
        ids::RENAME,
        ids::DELETE,
        ids::APPLY,
        ids::ENTRIES,
        ids::ACTION,
        ids::SET_ACTION,
        ids::REMOVE,
        ids::ADD_FILE,
        ids::RUNNING,
        ids::ADD_RUNNING,
        ids::REFRESH,
        ids::UNASSIGNED,
        ids::ADD_UNASSIGNED,
        ids::DISMISS,
        ids::STATUS,
    ] {
        g.control(id);
    }
    assert_eq!(g.rows(ids::ENTRIES), 1, "focus has one entry");
    assert!(g.rows(ids::RUNNING) > 0, "running programs are listed");

    g.set_text(ids::NAME, "music");
    g.click(ids::NEW);
    assert!(wait_for(LONG, || g.file().contains("name = \"music\"")));
    assert!(
        g.text(ids::STATUS).contains("created; saved"),
        "{}",
        g.text(ids::STATUS)
    );

    g.select_profile(0);
    // The selection fills the name field; wait for it before typing.
    assert!(wait_for(LONG, || g.text(ids::NAME) == "focus"));
    g.set_text(ids::NAME, "deep focus");
    g.click(ids::RENAME);
    assert!(wait_for(LONG, || g
        .file()
        .contains("name = \"deep focus\"")));
    let file = g.file();
    assert!(
        file.contains("# Owner's notes stay.") && file.contains("# main"),
        "{file}"
    );
    kaltstellen::config::parse(&file).expect("still valid");

    // Refused edit: a duplicate name. The file stays as it is.
    g.set_text(ids::NAME, "MUSIC");
    let before = g.file();
    g.click(ids::RENAME);
    assert!(wait_for(LONG, || g.text(ids::STATUS).contains("KS205")));
    assert_eq!(g.file(), before);

    g.select_profile(1);
    assert!(wait_for(LONG, || g.rows(ids::ENTRIES) == 0));
    g.select_profile(0);
    assert!(wait_for(LONG, || g.rows(ids::ENTRIES) == 1));

    assert_eq!(g.close(), Some(0));
}

#[test]
fn wp10_apply_runs_the_switch_and_reports_it() {
    let mut g = Gui::open(CONFIG);
    let exe = copy_fixture(HEARTBEAT, g.dir.path(), "ks-gui-a.exe");
    let _cleanup = KillProgram("ks-gui-a.exe".to_owned());
    let beat = g.dir.path().join("a.beat");
    let _a = Fixture::spawn(str_of(&exe), &[str_of(&beat)]);
    assert!(beating(&beat));
    g.select_profile(0);
    g.click(ids::APPLY);
    assert!(
        wait_for(LONG, || g.text(ids::STATUS) == "Applied \"focus\"."),
        "{}",
        g.text(ids::STATUS)
    );
    assert!(frozen(&beat));
    // Back to neutral with the command line, then close.
    let out = Gui::command(g.dir.path(), &["switch", "neutral"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0));
    assert!(beating(&beat));
    assert_eq!(g.close(), Some(0));
}

#[test]
fn wp10_an_invalid_file_is_not_opened_and_a_second_window_only_reports() {
    let dir = tempfile::tempdir().unwrap();
    fs::create_dir_all(dir.path().join("config")).unwrap();
    fs::write(dir.path().join("config").join("config.toml"), "typo = 1\n").unwrap();
    let out = Gui::command(dir.path(), &["gui"]).output().unwrap();
    assert_eq!(out.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&out.stderr).contains("KS202"));

    let mut g = Gui::open(CONFIG);
    let out = Gui::command(g.dir.path(), &["gui"]).output().unwrap();
    assert_eq!(out.status.code(), Some(0));
    assert!(String::from_utf8_lossy(&out.stderr).contains("already open"));
    assert_eq!(g.close(), Some(0));
}
