//! The main window (`kaltstellen gui`, WebView2) as a process, without
//! eyes: the real page is driven through its bridge (`--drive`), the
//! configuration file and a fixture show the effect, the window captures
//! itself (`--screenshot`), and no WebView2 process stays behind. What it
//! looks like is in `docs/manual-checks.md`.

mod common;

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::Duration;

use common::*;
use kaltstellen::config::OnTimeout;
use kaltstellen::process::Snapshot;
use windows::Win32::Foundation::{HWND, LPARAM};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetClassNameW, GetWindowThreadProcessId, IsIconic, SW_MINIMIZE, ShowWindow,
};
use windows::core::BOOL;

/// The visible main window of process `pid`.
fn main_window(pid: u32) -> Option<HWND> {
    struct Search {
        pid: u32,
        found: Option<HWND>,
    }
    unsafe extern "system" fn visit(hwnd: HWND, param: LPARAM) -> BOOL {
        // SAFETY: `param` points to the `Search` below for the whole call.
        let search = unsafe { &mut *(param.0 as *mut Search) };
        let mut pid = 0;
        let mut class = [0u16; 64];
        // SAFETY: valid out pointer and buffer.
        let len = unsafe {
            GetWindowThreadProcessId(hwnd, Some(&mut pid));
            GetClassNameW(hwnd, &mut class)
        } as usize;
        if pid == search.pid
            && String::from_utf16_lossy(&class[..len]) == kaltstellen::win32::webview::CLASS
        {
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

const LONG: Duration = Duration::from_secs(120);

fn kaltstellen_exe() -> PathBuf {
    Path::new(HEARTBEAT).with_file_name("kaltstellen.exe")
}

/// A test folder with configuration and data; removed last.
struct World {
    dir: tempfile::TempDir,
}

impl Drop for World {
    fn drop(&mut self) {
        remove_dir_retry(self.dir.path());
    }
}

impl World {
    fn new(config: &str) -> World {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("config")).unwrap();
        fs::write(dir.path().join("config").join("config.toml"), config).unwrap();
        World { dir }
    }

    fn path(&self, name: &str) -> PathBuf {
        self.dir.path().join(name)
    }

    fn command(&self, args: &[&str]) -> Command {
        let mut command = Command::new(kaltstellen_exe());
        command
            .args(args)
            .env("KALTSTELLEN_CONFIG_DIR", self.path("config"))
            .env("KALTSTELLEN_DATA_DIR", self.path("data"))
            .env_remove("KALTSTELLEN_UPDATE")
            .env(
                "KALTSTELLEN_RUN_KEY",
                "Software\\kaltstellen-test-gui-unused\\Run",
            )
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        command
    }

    fn config(&self) -> String {
        fs::read_to_string(self.path("config").join("config.toml")).unwrap()
    }
}

/// Processes below `pid` (WebView2 starts its browser process as a child).
fn descendants(pid: u32) -> HashSet<u32> {
    let snapshot = Snapshot::take().unwrap();
    let mut found: HashSet<u32> = HashSet::from([pid]);
    loop {
        let before = found.len();
        for p in snapshot.processes() {
            if found.contains(&p.parent_pid) {
                found.insert(p.pid);
            }
        }
        if found.len() == before {
            break;
        }
    }
    found.remove(&pid);
    found
}

fn alive(pids: &HashSet<u32>) -> Vec<u32> {
    let snapshot = Snapshot::take().unwrap();
    pids.iter()
        .copied()
        .filter(|p| snapshot.get(*p).is_some())
        .collect()
}

/// Runs `kaltstellen gui ARGS` to the end, watching the WebView2 processes
/// it starts; returns its output and those processes.
fn run_window(world: &World, args: &[&str]) -> (Output, HashSet<u32>) {
    let mut child = world.command(args).spawn().unwrap();
    let pid = child.id();
    let mut webview = HashSet::new();
    let ended = wait_for(LONG, || {
        webview.extend(descendants(pid));
        child.try_wait().unwrap().is_some()
    });
    if !ended {
        let _ = child.kill();
    }
    let output = child.wait_with_output().unwrap();
    assert!(ended, "the window did not close by itself");
    (output, webview)
}

const CONFIG: &str = "# Owner's notes stay.\n[[profile]]\nname = \"focus\"   # main\n";

#[test]
fn wp10_the_page_drives_the_bridge_edits_the_file_and_applies_against_a_fixture() {
    let world = World::new(CONFIG);
    let exe = copy_fixture(HEARTBEAT, world.dir.path(), "ks-wv-a.exe");
    let _cleanup = KillProgram("ks-wv-a.exe".to_owned());
    let beat = world.path("a.beat");
    let _a = Fixture::spawn(str_of(&exe), &[str_of(&beat)]);
    assert!(beating(&beat));

    let steps = r#"[
        {"type": "create_profile", "name": "play"},
        {"type": "add_entry", "profile": "play", "exe": "ks-wv-a.exe"},
        {"type": "add_entry", "profile": "play", "exe": "explorer.exe"},
        {"type": "set_hotkey", "profile": "play", "hotkey": "Ctrl+Alt+Banana"},
        {"type": "set_update_mode", "mode": "off"},
        {"type": "install_update"},
        {"type": "apply", "profile": "play"}
    ]"#;
    fs::write(world.path("steps.json"), steps).unwrap();
    let png = world.path("after.png");
    let (output, webview) = run_window(
        &world,
        &[
            "gui",
            "--drive",
            world.path("steps.json").to_str().unwrap(),
            "--drive-out",
            world.path("results.json").to_str().unwrap(),
            "--screenshot",
            png.to_str().unwrap(),
        ],
    );
    assert_eq!(
        output.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let results: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(world.path("results.json")).unwrap()).unwrap();
    let ok: Vec<bool> = results
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["ok"].as_bool().unwrap())
        .collect();
    assert_eq!(
        ok,
        vec![true, true, false, false, true, false, true],
        "{results}"
    );
    assert!(
        results[2]["error"].as_str().unwrap().contains("protected"),
        "{results}"
    );
    // UP1 in the window: the mode is saved like the tray's (off: no check,
    // so no network); nothing to install without a known newer version.
    let settings = fs::read_to_string(world.path("config").join("settings.json")).unwrap();
    assert!(settings.contains("\"update\": \"off\""), "{settings}");
    assert!(
        results[5]["error"]
            .as_str()
            .unwrap()
            .contains("check first"),
        "{results}"
    );
    assert_eq!(results[6]["status"], "Applied \"play\".", "{results}");

    let config = world.config();
    assert!(config.starts_with("# Owner's notes stay."), "{config}");
    assert!(
        config.contains("# main") && config.contains("name = \"play\""),
        "{config}"
    );
    assert!(
        config.contains("exe = \"ks-wv-a.exe\"") && !config.contains("explorer"),
        "{config}"
    );
    kaltstellen::config::parse(&config).expect("still valid");
    assert!(frozen(&beat), "applied through the window");

    let image = fs::read(&png).unwrap();
    assert!(image.starts_with(b"\x89PNG"), "a PNG");
    assert!(
        image.len() > 10_000,
        "not a blank image: {} bytes",
        image.len()
    );

    assert!(!webview.is_empty(), "WebView2 processes were started");
    assert!(
        wait_for(Duration::from_secs(15), || alive(&webview).is_empty()),
        "WebView2 processes left behind: {:?}",
        alive(&webview)
    );
    let back = world.command(&["switch", "neutral"]).output().unwrap();
    assert_eq!(back.status.code(), Some(0));
    assert!(beating(&beat));
}

#[test]
fn wp10_screenshot_views_render_in_both_themes() {
    let world = World::new(CONFIG);
    for (view, theme) in [("entry", "dark"), ("add", "light")] {
        let png = world.path(&format!("{view}-{theme}.png"));
        let (output, webview) = run_window(
            &world,
            &[
                "gui",
                "--screenshot",
                png.to_str().unwrap(),
                "--view",
                view,
                "--theme",
                theme,
            ],
        );
        assert_eq!(output.status.code(), Some(0));
        let image = fs::read(&png).unwrap();
        assert!(
            image.starts_with(b"\x89PNG") && image.len() > 10_000,
            "{view}-{theme}"
        );
        assert!(wait_for(Duration::from_secs(15), || alive(&webview).is_empty()));
    }
}

#[test]
fn wp10_missing_runtime_invalid_file_and_second_window() {
    let world = World::new(CONFIG);
    // WebView2's own override pointing nowhere: as if no runtime existed.
    let out = world
        .command(&["gui"])
        .env(
            "WEBVIEW2_BROWSER_EXECUTABLE_FOLDER",
            world.path("no-runtime"),
        )
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(
        err.contains("KS702") && err.contains("config.toml"),
        "{err}"
    );

    let bad = World::new("typo = 1\n");
    let out = bad.command(&["gui"]).output().unwrap();
    assert_eq!(out.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&out.stderr).contains("KS202"));

    // A second window while one is open only reports.
    let first = world.command(&["gui"]).spawn().unwrap();
    let pid = first.id();
    assert!(wait_for(LONG, || !descendants(pid).is_empty()));
    let webview = descendants(pid);
    // M6: a second start brings the open window back, also when minimised.
    assert!(wait_for(LONG, || main_window(pid).is_some()));
    let window = main_window(pid).unwrap();
    // SAFETY: a window of a process this test started.
    unsafe {
        let _ = ShowWindow(window, SW_MINIMIZE);
    }
    assert!(wait_for(LONG, || unsafe { IsIconic(window) }.as_bool()));
    let second = world.command(&["gui"]).output().unwrap();
    assert_eq!(second.status.code(), Some(0));
    let message = String::from_utf8_lossy(&second.stderr);
    assert!(message.contains("brought it to the front"), "{message}");
    assert!(
        wait_for(LONG, || !unsafe { IsIconic(window) }.as_bool()),
        "restored"
    );
    // Closed like the user would (WM_CLOSE): the process and its WebView2
    // processes end.
    let info = Snapshot::take().unwrap().get(pid).cloned().unwrap();
    let outcome =
        kaltstellen::process::stop(&[info], Duration::from_secs(20), OnTimeout::Kill).unwrap();
    assert_eq!(outcome, kaltstellen::process::StopOutcome::Closed);
    let first = first.wait_with_output().unwrap();
    assert_eq!(
        first.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&first.stderr)
    );
    assert!(wait_for(Duration::from_secs(15), || alive(&webview).is_empty()));
}
