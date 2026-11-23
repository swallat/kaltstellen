//! The main window (`kaltstellen gui`, WebView2) as a process, without
//! eyes: the real page is driven through its bridge (`--drive`), the
//! configuration file and a fixture show the effect, the window captures
//! itself (`--screenshot`), and no WebView2 process stays behind. What it
//! looks like is in `docs/manual-checks.md`.

mod common;

use std::collections::HashSet;
use std::fs;
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};
use std::time::Duration;

use common::*;
use kaltstellen::config::OnTimeout;
use kaltstellen::process::{Access, Snapshot, Target};
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

/// A test folder with configuration and data; removed last.
struct World {
    dir: tempfile::TempDir,
}

impl Drop for World {
    fn drop(&mut self) {
        let _ = kaltstellen::win32::registry::delete_tree(&self.run_root());
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

    /// A Run key of this test only, removed with the folder.
    fn run_root(&self) -> String {
        let name = self.dir.path().file_name().unwrap().to_string_lossy();
        format!("Software\\kaltstellen-test-{name}")
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
            .env("KALTSTELLEN_RUN_KEY", format!("{}\\Run", self.run_root()))
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        command
    }

    fn config(&self) -> String {
        fs::read_to_string(self.path("config").join("config.toml")).unwrap()
    }
}

/// A process for good: PID and creation time (S6). A PID alone may be
/// reused by an unrelated process, for example one of a test running in
/// parallel, which would then look like a WebView2 process left behind.
type Proc = (u32, u64);

/// The process a test started, while its `Child` still holds a handle (so
/// its PID cannot be reused yet).
fn root_of(pid: u32) -> Proc {
    let info = info_of(pid);
    (info.pid, info.created)
}

/// Processes below `root` (WebView2 starts its browser process as a child),
/// also after `root` has ended. A child counts only if it was created after
/// its parent: Windows keeps a dead parent's PID in `parent_pid`, and that
/// PID may be in use again.
fn descendants(root: Proc) -> HashSet<Proc> {
    let snapshot = Snapshot::take().unwrap();
    let mut found: Vec<Proc> = vec![root];
    loop {
        let before = found.len();
        for p in snapshot.processes() {
            let child = (p.pid, p.created);
            if !found.contains(&child)
                && found
                    .iter()
                    .any(|&(parent, created)| parent == p.parent_pid && created <= p.created)
            {
                found.push(child);
            }
        }
        if found.len() == before {
            break;
        }
    }
    found.into_iter().skip(1).collect()
}

/// The WebView2 processes of `world`, by the user data folder on their
/// command line. The tree alone can miss them: on the CI runner a short
/// window's WebView2 processes were not seen below it (not started there,
/// or gone between two polls). Only processes in a snapshot before and one
/// after the query count, so a PID that ended and was taken by another
/// WebView2 meanwhile is left out. A failed query fails the test, so it
/// never reads as "none left". It finds running processes; one that is
/// being torn down may have no readable command line any more and is not
/// listed (seen on the ARM64 runner), so it does not replace the tree for
/// processes that are ending.
fn webview_of(world: &World) -> HashSet<Proc> {
    let snapshot = |s: Snapshot| -> HashSet<Proc> {
        s.processes()
            .iter()
            .filter(|p| p.name.eq_ignore_ascii_case("msedgewebview2.exe"))
            .map(|p| (p.pid, p.created))
            .collect()
    };
    let before = snapshot(Snapshot::take().unwrap());
    let folder = world.path("data").join("webview2");
    let script = format!(
        "$ErrorActionPreference = 'Stop'; $ProgressPreference = 'SilentlyContinue'; \
         Get-CimInstance Win32_Process -Filter \"Name='msedgewebview2.exe'\" | \
         Where-Object {{ $_.CommandLine -and $_.CommandLine.Contains('{}') }} | \
         ForEach-Object {{ $_.ProcessId }}",
        folder.to_string_lossy().replace('\'', "''")
    );
    let out = Command::new("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-Command", &script])
        .stdin(Stdio::null())
        .output()
        .expect("powershell.exe");
    assert!(
        out.status.success() && out.stderr.is_empty(),
        "the WebView2 query failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let pids: HashSet<u32> = String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|l| l.trim().parse().ok())
        .collect();
    snapshot(Snapshot::take().unwrap())
        .intersection(&before)
        .filter(|(pid, _)| pids.contains(pid))
        .copied()
        .collect()
}

fn page_ready(world: &World) -> bool {
    fs::read_to_string(world.path("data").join("kaltstellen.log"))
        .is_ok_and(|log| log.contains("gui: page ready"))
}

/// The processes of `procs` that still run (same PID and creation time),
/// with their names. A process that has exited but is still listed because
/// some other process holds a handle to it (WMI, for example) does not
/// count: opening it checks creation time and exit code. One that cannot be
/// opened is taken as running, with the reason in its name.
fn alive(procs: &HashSet<Proc>) -> Vec<(Proc, String)> {
    let snapshot = Snapshot::take().unwrap();
    procs
        .iter()
        .filter_map(|&(pid, created)| snapshot.find(pid, created))
        .filter_map(|info| {
            let name = match Target::open(info, Access::Query) {
                Ok(Some(_)) => info.name.clone(),
                Ok(None) => return None,
                Err(err) => format!("{} (open: {err})", info.name),
            };
            Some(((info.pid, info.created), name))
        })
        .collect()
}

/// Runs `kaltstellen gui ARGS` to the end, watching the WebView2 processes
/// it starts below it; returns its output and those processes. Only the
/// tree, polled quickly: asking by data folder ([`webview_of`]) takes
/// seconds on the CI runner, long enough for a short window and its
/// WebView2 processes to come and go unseen.
fn run_window(world: &World, args: &[&str]) -> (Output, HashSet<Proc>) {
    let mut child = world.command(args).spawn().unwrap();
    let root = root_of(child.id());
    let mut webview = HashSet::new();
    let ended = wait_for(LONG, || {
        webview.extend(descendants(root));
        child.try_wait().unwrap().is_some()
    });
    // A last look below the ended window process.
    webview.extend(descendants(root));
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

    // M2: icons through the real host: one from a system program, none for
    // a name that neither runs nor has a path.
    let notepad = std::path::Path::new(&std::env::var("SystemRoot").unwrap())
        .join("System32")
        .join("notepad.exe");
    let notepad = serde_json::to_string(notepad.to_str().unwrap()).unwrap();
    let steps = format!(
        r#"[
        {{"type": "create_profile", "name": "play"}},
        {{"type": "add_entry", "profile": "play", "exe": "ks-wv-a.exe"}},
        {{"type": "set_hotkey", "profile": "play", "hotkey": "Ctrl+Alt+Banana"}},
        {{"type": "set_update_mode", "mode": "off"}},
        {{"type": "install_update"}},
        {{"type": "apply", "profile": "play"}},
        {{"type": "add_entry", "profile": "play", "exe": "explorer.exe"}},
        {{"type": "add_entry", "profile": "play", "exe": "notepad.exe", "path": {notepad}}},
        {{"type": "icons", "size": 32, "programs": [
            {{"exe": "notepad.exe", "path": {notepad}}},
            {{"exe": "ks-no-such-program.exe"}}
        ]}},
        {{"type": "set_triggers", "profile": "play", "triggers": ["ks-wv-sim.exe"], "priority": 2}},
        {{"type": "set_triggers", "profile": "play", "triggers": ["ks-wv-a.exe"], "priority": 0}}
    ]"#
    );
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
        vec![
            true, true, false, true, false, true, false, true, true, true, false
        ],
        "{results}"
    );
    assert!(
        results[6]["error"].as_str().unwrap().contains("protected"),
        "{results}"
    );
    // UP1 in the window: the mode is saved like the tray's (off: no check,
    // so no network); nothing to install without a known newer version.
    let settings = fs::read_to_string(world.path("config").join("settings.json")).unwrap();
    assert!(settings.contains("\"update\": \"off\""), "{settings}");
    assert!(
        results[4]["error"]
            .as_str()
            .unwrap()
            .contains("check first"),
        "{results}"
    );
    assert_eq!(results[5]["status"], "Applied \"play\".", "{results}");
    assert!(
        results[8]["data"][0]
            .as_str()
            .is_some_and(|u| u.starts_with("data:image/png;base64,")),
        "{results}"
    );
    assert!(results[8]["data"][1].is_null(), "{results}");

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
    // U4: triggers set through the page; one its own profile freezes is
    // refused.
    assert!(
        config.contains("trigger = [\"ks-wv-sim.exe\"]") && config.contains("priority = 2"),
        "{config}"
    );
    assert!(
        results[10]["error"].as_str().unwrap().contains("freezes"),
        "{results}"
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

    // Below the window and by data folder (the tree may not see WebView2).
    assert_no_webview_left(&world, &webview, "drive");
    let back = world.command(&["switch", "neutral"]).output().unwrap();
    assert_eq!(back.status.code(), Some(0));
    if !beating(&beat) {
        // Seen once locally under load; say whether the resume is missing
        // or the heartbeat is only late.
        let status = world.command(&["status"]).output().unwrap();
        panic!(
            "no heartbeat after neutral (beat {:?}, again beating: {}):\n{}{}\nswitch said: {}{}",
            beat_value(&beat),
            beating(&beat),
            String::from_utf8_lossy(&status.stdout),
            String::from_utf8_lossy(&status.stderr),
            String::from_utf8_lossy(&back.stdout),
            String::from_utf8_lossy(&back.stderr)
        );
    }
}

/// `common::beat`, for tests whose `beat` variable hides it.
fn beat_value(file: &std::path::Path) -> Option<u128> {
    beat(file)
}

/// Runs the JSON array `steps` through the page (`--drive`) and returns
/// the results; checks that no WebView2 process stays behind.
fn drive(world: &World, name: &str, steps: &str) -> serde_json::Value {
    let input = world.path(&format!("{name}.json"));
    let output_file = world.path(&format!("{name}-results.json"));
    fs::write(&input, steps).unwrap();
    let (output, webview) = run_window(
        world,
        &[
            "gui",
            "--drive",
            input.to_str().unwrap(),
            "--drive-out",
            output_file.to_str().unwrap(),
        ],
    );
    assert_eq!(
        output.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_no_webview_left(world, &webview, name);
    serde_json::from_str(&fs::read_to_string(output_file).unwrap()).unwrap()
}

/// A tray for a test world; ended (killed) when dropped.
struct TrayOf(std::process::Child);

impl TrayOf {
    fn start(world: &World) -> TrayOf {
        let child = world
            .command(&["tray"])
            // Never ask about start at sign-in in tests (L8).
            .env("KALTSTELLEN_AUTOSTART_PROMPT", "skip")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let tray = TrayOf(child);
        let log = world.path("data").join("kaltstellen.log");
        assert!(
            wait_for(LONG, || fs::read_to_string(&log)
                .is_ok_and(|l| l.contains("tray: recover finished"))),
            "the tray did not start"
        );
        tray
    }
}

impl Drop for TrayOf {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[test]
fn m5_failure_markers_come_from_the_trays_error_state_and_clear_when_resolved() {
    // A start entry whose file is missing (KS110) beside one that works.
    let world = World::new("");
    let missing = world.path("ks-wv-m5.exe");
    let _cleanup = KillProgram("ks-wv-m5.exe".to_owned());
    let config = format!(
        "[[profile]]\nname = \"broken\"\n\n  [[profile.app]]\n  exe = \"ks-wv-m5.exe\"\n  \
         path = '{}'\n  action = \"start\"\n\n  [[profile.app]]\n  exe = \"ks-wv-m5-other.exe\"\n  \
         action = \"ignore\"\n",
        missing.display()
    );
    fs::write(world.path("config").join("config.toml"), config).unwrap();
    let _tray = TrayOf::start(&world);

    let results = drive(
        &world,
        "m5-fail",
        r#"[{"type": "get_state"}, {"type": "apply", "profile": "broken"}]"#,
    );
    let entries = |result: &serde_json::Value| result["state"]["profiles"][0]["entries"].clone();
    assert_eq!(entries(&results[0])[0]["errors"], serde_json::json!([]));
    assert!(
        results[1]["status"]
            .as_str()
            .unwrap()
            .contains("with errors"),
        "{results}"
    );
    // The switch went through the tray; its error is on the entry only.
    let errors = &entries(&results[1])[0]["errors"];
    assert_eq!(errors[0]["code"], "KS110", "{results}");
    assert_eq!(errors[0]["label"], "start failed", "{results}");
    assert!(
        errors[0]["text"].as_str().unwrap().contains("ks-wv-m5.exe"),
        "{results}"
    );
    assert_eq!(entries(&results[1])[1]["errors"], serde_json::json!([]));

    // L9: started by hand at the entry's path, the error resolves; the next
    // refresh shows no marker, without a switch.
    copy_fixture(HEARTBEAT, world.dir.path(), "ks-wv-m5.exe");
    let beat = world.path("m5.beat");
    let _program = Fixture::spawn(str_of(&missing), &[str_of(&beat)]);
    assert!(beating(&beat));
    let results = drive(&world, "m5-resolved", r#"[{"type": "get_state"}]"#);
    assert_eq!(
        entries(&results[0])[0]["errors"],
        serde_json::json!([]),
        "{results}"
    );
    let log = fs::read_to_string(world.path("data").join("kaltstellen.log")).unwrap();
    assert!(log.contains("error resolved by itself: KS110"), "{log}");
}

#[test]
fn m2_files_dropped_from_disk_are_added_through_the_host_and_nothing_navigates() {
    // A drop from Explorer itself cannot be automated (docs/manual-checks.md
    // 13d). The host drops files on disk through DevTools
    // (`Input.dispatchDragEvent`, step `drop_real`), so the page's drop
    // handler, `postMessageWithAdditionalObjects`, the host's reading of
    // `ICoreWebView2File::Path` and the bridge's checks all run; then the
    // page still answers (nothing navigated away). Files made up in the
    // page (`drop`) are refused by WebView2 itself, and a `drop_files`
    // without files by the host: the page cannot name paths.
    let world = World::new(CONFIG);
    let exe = copy_fixture(HEARTBEAT, world.dir.path(), "ks-wv-drop.exe");
    let notes = world.path("notes.txt");
    fs::write(&notes, "x").unwrap();
    let paths = serde_json::to_string(&[str_of(&exe), str_of(&notes)]).unwrap();
    let steps = format!(
        r#"[
            {{"type": "drop_real", "paths": {paths}}},
            {{"type": "drop", "names": ["ks-made-up.exe"]}},
            {{"type": "drop_files"}},
            {{"type": "get_state"}}
        ]"#
    );
    let results = drive(&world, "m2-drop", &steps);
    let real = &results[0];
    assert_eq!(real["ok"], true, "{results}");
    assert_eq!(
        real["data"]["files"][0]["exe"], "ks-wv-drop.exe",
        "{results}"
    );
    assert_eq!(real["data"]["files"][0]["path"], str_of(&exe), "{results}");
    assert!(
        real["data"]["refused"][0]
            .as_str()
            .unwrap()
            .contains("not a program"),
        "{results}"
    );
    assert!(
        real["status"].as_str().unwrap().contains("1 of 2 added"),
        "{results}"
    );
    assert_eq!(results[1]["ok"], false, "{results}");
    assert_eq!(results[2]["ok"], false, "{results}");
    assert!(
        results[2]["error"].as_str().unwrap().contains("no file"),
        "{results}"
    );
    // Still the page, and the status from the bridge survives a refresh.
    assert_eq!(results[3]["ok"], true, "{results}");
    let config = world.config();
    assert!(
        config.contains("exe = \"ks-wv-drop.exe\"") && config.contains(str_of(&exe)),
        "{config}"
    );
    kaltstellen::config::parse(&config).expect("still valid");
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
        // Three windows of the parallel tests may share the machine. The
        // tree can miss a short window's WebView2 (see `webview_of`), so the
        // processes of this test's data folder must be gone too.
        assert_no_webview_left(&world, &webview, &format!("{view}-{theme}"));
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
    let mut first = world.command(&["gui"]).spawn().unwrap();
    let pid = first.id();
    // Ends the window if an assertion below fails.
    let _guard = KillProcess(info_of(pid));
    let started = wait_for(LONG, || {
        page_ready(&world) || first.try_wait().ok().flatten().is_some()
    });
    if first.try_wait().ok().flatten().is_some() || !started {
        let _ = first.kill();
        let out = first.wait_with_output().unwrap();
        panic!(
            "the window started no WebView2 (exit {:?}): {}\nlog:\n{}",
            out.status.code(),
            String::from_utf8_lossy(&out.stderr),
            fs::read_to_string(world.path("data").join("kaltstellen.log")).unwrap_or_default()
        );
    }
    // The query by data folder finds this window's WebView2 processes; this
    // proves it works here, which the leak checks of all three tests rely on.
    let mut by_folder = HashSet::new();
    assert!(
        wait_for(LONG, || {
            by_folder = webview_of(&world);
            !by_folder.is_empty()
        }),
        "the data-folder query finds no WebView2 process of an open window"
    );
    let mut webview = descendants(root_of(pid));
    webview.extend(by_folder);
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
    assert_no_webview_left(&world, &webview, "second window");
}

/// How long WebView2 may take to end after its window closed. On the ARM64
/// runner, with the other window tests running beside it, it took more
/// than 30 s and less than 2 minutes; a process left behind runs on.
const WEBVIEW_END: Duration = Duration::from_secs(120);

/// Waits until neither a process of `webview` nor a WebView2 process of
/// `world`'s data folder runs (at most [`WEBVIEW_END`]); on failure names
/// what was still running at the last look. The process tree is cheap to
/// ask and polled first; the data-folder query starts PowerShell and asks
/// WMI, which loads a slow runner, so it follows once the tree is clear.
fn assert_no_webview_left(world: &World, webview: &HashSet<Proc>, what: &str) {
    let start = std::time::Instant::now();
    let mut below = Vec::new();
    let tree_clear = wait_for(WEBVIEW_END, || {
        below = alive(webview);
        below.is_empty()
    });
    assert!(
        tree_clear,
        "{what}: WebView2 processes below the window still run after {} s: {below:?}",
        WEBVIEW_END.as_secs()
    );
    // At least 15 s for this phase; a second between queries, so they do
    // not load the runner that WebView2 is ending on.
    let mut by_folder = HashSet::new();
    let budget = WEBVIEW_END
        .saturating_sub(start.elapsed())
        .max(Duration::from_secs(15));
    let folder_clear = wait_for(budget, || {
        by_folder = webview_of(world);
        if !by_folder.is_empty() {
            std::thread::sleep(Duration::from_secs(1));
        }
        by_folder.is_empty()
    });
    assert!(
        folder_clear,
        "{what}: WebView2 processes of {} still run after {} s: {by_folder:?}",
        world.path("data").join("webview2").display(),
        WEBVIEW_END.as_secs()
    );
}
