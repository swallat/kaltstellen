//! Updates (spec UP1) as processes, offline: a local HTTP server stands in
//! for GitHub, serving release metadata, `checksums.txt` and the
//! executable. `KALTSTELLEN_UPDATE_URL` points the updater at it.
//!
//! Every test copies `kaltstellen.exe` into its own temporary folder under
//! a name of its own and updates that copy, never the build output. Trays,
//! programs and the registry key of a test are removed when it ends.

mod common;

use std::collections::HashMap;
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{Receiver, channel};
use std::time::Duration;

use common::*;
use kaltstellen::tray::model::{ID_UPDATE_AUTO, ID_UPDATE_MANUAL};
use kaltstellen::win32::hash;
use kaltstellen::win32::registry;
use kaltstellen::win32::tray::CLASS;
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetClassNameW, GetWindowThreadProcessId, PostMessageW, WM_COMMAND,
};
use windows::core::BOOL;

const LONG: Duration = Duration::from_secs(30);
const BAD_UPDATE: &str = env!("CARGO_BIN_EXE_ks-badupdate");
/// Appended to the served copy so the swap can be seen in the bytes; data
/// after the image does not change what runs.
const MARKER: &[u8] = b"\0ks-update-test: the served version\0";

/// The version the built executable reports, without the build metadata
/// (`+<commit>`) a development build appends.
fn built_version() -> String {
    let out = Command::new(kaltstellen_exe())
        .arg("--version")
        .output()
        .unwrap();
    String::from_utf8_lossy(&out.stdout)
        .trim()
        .strip_prefix("kaltstellen ")
        .expect("kaltstellen <version>")
        .split('+')
        .next()
        .unwrap_or_default()
        .to_owned()
}

/// Serves `routes` (path to body) on loopback until the test process
/// ends; anything else is 404.
fn serve(routes: HashMap<String, Vec<u8>>) -> String {
    serve_gated(routes, None)
}

/// [`serve`], but a path ending in `gate.0` is answered only when the test
/// sends on the other end of `gate.1` (a download that takes as long as
/// the test wants) or has dropped it.
fn serve_gated(
    routes: HashMap<String, Vec<u8>>,
    gate: Option<(&'static str, Receiver<()>)>,
) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let mut reader = BufReader::new(stream);
            let mut first = String::new();
            if reader.read_line(&mut first).is_err() {
                continue;
            }
            loop {
                let mut line = String::new();
                if reader.read_line(&mut line).unwrap_or(0) <= 2 {
                    break;
                }
            }
            let path = first.split_whitespace().nth(1).unwrap_or("/");
            if let Some((suffix, gate)) = &gate
                && path.ends_with(suffix)
            {
                let _ = gate.recv_timeout(LONG);
            }
            let (status, body) = match routes.get(path) {
                Some(body) => ("200 OK", body.clone()),
                None => ("404 Not Found", Vec::new()),
            };
            let mut stream = reader.into_inner();
            let head = format!(
                "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            let _ = stream.write_all(head.as_bytes());
            let _ = stream.write_all(&body);
        }
    });
    base
}

/// A release `version` whose executable is `served`; `checksums.txt` holds
/// the sum of `summed`. Returns the API base for `KALTSTELLEN_UPDATE_URL`.
fn github(version: &str, served: &[u8], summed: &[u8]) -> String {
    github_gated(version, served, summed, None)
}

/// [`github`] whose executable downloads only when `gate` lets it.
fn github_gated(version: &str, served: &[u8], summed: &[u8], gate: Option<Receiver<()>>) -> String {
    let arch = std::env::consts::ARCH;
    let name = format!("kaltstellen-{version}-windows-{arch}.exe");
    let sum = hash::hex(&hash::sha256(summed).unwrap());
    let mut routes = HashMap::new();
    routes.insert(format!("/dl/{name}"), served.to_vec());
    routes.insert(
        "/dl/checksums.txt".to_owned(),
        format!("{sum}  {name}\n").into_bytes(),
    );
    // Two steps: start the server with the files, then add the metadata
    // that points at it.
    let files = serve_gated(routes, gate.map(|g| (".exe", g)));
    let latest = format!(
        r#"{{"tag_name":"v{version}","html_url":"{files}/page","draft":false,"prerelease":false,
            "assets":[{{"name":"{name}","browser_download_url":"{files}/dl/{name}"}},
                      {{"name":"checksums.txt","browser_download_url":"{files}/dl/checksums.txt"}}]}}"#
    );
    let api = serve(HashMap::from([(
        "/repos/o/r/releases/latest".to_owned(),
        latest.into_bytes(),
    )]));
    format!("{api}/repos/o/r")
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

/// A copy of kaltstellen with its own folders, tray and programs.
struct Setup {
    exe: PathBuf,
    original: Vec<u8>,
    api: String,
    tray: Option<Child>,
    cleanup: Vec<KillProgram>,
    /// Dropped last, after everything running from it was ended.
    dir: tempfile::TempDir,
}

impl Setup {
    fn new(api: String) -> Setup {
        let dir = tempfile::tempdir().unwrap();
        let tag: String = dir
            .path()
            .file_name()
            .unwrap()
            .to_string_lossy()
            .chars()
            .filter(char::is_ascii_alphanumeric)
            .collect::<String>()
            .to_lowercase();
        fs::create_dir_all(dir.path().join("config")).unwrap();
        fs::create_dir_all(dir.path().join("bin")).unwrap();
        let name = format!("ks-upd-{tag}.exe");
        let exe = dir.path().join("bin").join(&name);
        fs::copy(kaltstellen_exe(), &exe).unwrap();
        let original = fs::read(&exe).unwrap();
        Setup {
            exe,
            original,
            api,
            tray: None,
            cleanup: vec![KillProgram(name)],
            dir,
        }
    }

    fn data(&self) -> PathBuf {
        self.dir.path().join("data")
    }

    fn config_dir(&self) -> PathBuf {
        self.dir.path().join("config")
    }

    fn registry_root(&self) -> String {
        let name = self.dir.path().file_name().unwrap().to_string_lossy();
        format!("Software\\kaltstellen-test-{name}")
    }

    fn env(&self, command: &mut Command) {
        command
            .env("KALTSTELLEN_CONFIG_DIR", self.config_dir())
            .env("KALTSTELLEN_DATA_DIR", self.data())
            .env(
                "KALTSTELLEN_RUN_KEY",
                format!("{}\\Run", self.registry_root()),
            )
            .env("KALTSTELLEN_UPDATE_URL", &self.api)
            .env_remove("KALTSTELLEN_UPDATE")
            // A tray that an update starts again must not ask about start
            // at sign-in either (L8).
            .env("KALTSTELLEN_AUTOSTART_PROMPT", "skip")
            .stdin(Stdio::null());
    }

    /// `<copy> ARGS` to the end: (exit code, stdout, stderr).
    fn run(&self, args: &[&str], extra: &[(&str, &str)]) -> (i32, String, String) {
        let mut command = Command::new(&self.exe);
        command.args(args);
        self.env(&mut command);
        command.envs(extra.iter().copied());
        let out = command.output().unwrap();
        (
            out.status.code().unwrap_or(-1),
            String::from_utf8_lossy(&out.stdout).into_owned(),
            String::from_utf8_lossy(&out.stderr).into_owned(),
        )
    }

    fn log(&self) -> String {
        fs::read_to_string(self.data().join("kaltstellen.log")).unwrap_or_default()
    }

    fn wait_log(&self, text: &str, n: usize) {
        assert!(
            wait_for(LONG, || self.log().matches(text).count() >= n),
            "never logged {n}x {text:?}; log:\n{}",
            self.log()
        );
    }

    fn config(&self, text: &str) {
        fs::write(self.config_dir().join("config.toml"), text).unwrap();
    }

    /// A heartbeat copy that the profile `play` suspends.
    fn program(&mut self) -> (Fixture, PathBuf) {
        let name = format!(
            "ks-updhb-{}.exe",
            self.exe
                .file_stem()
                .unwrap()
                .to_string_lossy()
                .trim_start_matches("ks-upd-")
        );
        self.cleanup.push(KillProgram(name.clone()));
        let exe = copy_fixture(HEARTBEAT, self.dir.path(), &name);
        let beat = self.dir.path().join("hb.beat");
        let fixture = Fixture::spawn(str_of(&exe), &[str_of(&beat)]);
        assert!(beating(&beat), "heartbeat never started");
        self.config(&format!(
            "[[profile]]\nname = \"play\"\n\n[[profile.app]]\nexe = \"{name}\"\naction = \"suspend\"\n"
        ));
        (fixture, beat)
    }

    /// Starts the tray from the copy and waits for its start-up recovery.
    fn start_tray(&mut self) {
        self.start_tray_with(&[]);
    }

    /// [`Setup::start_tray`] with more environment, which the processes it
    /// starts (the update, the next tray) inherit.
    fn start_tray_with(&mut self, extra: &[(&str, &str)]) {
        let before = self.log().matches("tray: recover finished").count();
        let mut command = Command::new(&self.exe);
        command
            .arg("tray")
            // Never ask about start at sign-in in tests (L8).
            .env("KALTSTELLEN_AUTOSTART_PROMPT", "skip")
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        self.env(&mut command);
        command.envs(extra.iter().copied());
        self.tray = Some(command.spawn().unwrap());
        self.wait_log("tray: recover finished", before + 1);
    }

    fn file(&self, suffix: &str) -> Option<Vec<u8>> {
        let stem = self.exe.file_stem().unwrap().to_string_lossy();
        fs::read(self.exe.with_file_name(format!("{stem}.{suffix}"))).ok()
    }

    fn bin_names(&self) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(self.exe.parent().unwrap())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }
}

impl Drop for Setup {
    fn drop(&mut self) {
        if let Some(mut child) = self.tray.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
        // Trays started by the updater run from the copy: KillProgram.
        self.cleanup.clear();
        let _ = registry::delete_tree(&self.registry_root());
    }
}

#[test]
fn up1_check_reports_a_newer_release_and_records_it() {
    let s = Setup::new(github("9.9.9", b"x", b"x"));
    let (code, out, err) = s.run(&["update", "--check"], &[]);
    assert_eq!(code, 0, "{out}{err}");
    assert!(out.contains("version 9.9.9 is available"), "{out}");
    assert!(out.contains("install it with: kaltstellen update"), "{out}");
    let record = fs::read_to_string(s.data().join("update.json")).unwrap();
    assert!(record.contains("\"latest\": \"9.9.9\""), "{record}");
    // The tray's form prints nothing.
    let (code, out, err) = s.run(&["update", "--check", "--quiet"], &[]);
    assert_eq!((code, out.as_str(), err.as_str()), (0, "", ""));
    // No server: KS801, exit code 1, recorded.
    // A port that was just free: nothing answers there.
    let port = TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let offline = Setup::new(format!("http://127.0.0.1:{port}/repos/o/r"));
    let (code, _, err) = offline.run(&["update", "--check"], &[]);
    assert_eq!(code, 1);
    assert!(err.contains("KS801"), "{err}");
    assert!(
        fs::read_to_string(offline.data().join("update.json"))
            .unwrap()
            .contains("KS801")
    );
}

#[test]
fn up1_mode_is_saved_and_nothing_to_do_without_a_newer_release() {
    let version = built_version();
    let s = Setup::new(github(&version, b"x", b"x"));
    let (code, out, _) = s.run(&["update", "--mode", "off"], &[]);
    assert_eq!(code, 0);
    assert_eq!(out.trim(), "update mode: off");
    let settings = fs::read_to_string(s.config_dir().join("settings.json")).unwrap();
    assert!(settings.contains("\"update\": \"off\""), "{settings}");
    // The earlier form still works: on means auto.
    let (code, out, _) = s.run(&["update", "--auto", "on"], &[]);
    assert_eq!((code, out.trim()), (0, "update mode: auto"));
    let (code, out, _) = s.run(
        &["update", "--mode", "manual"],
        &[("KALTSTELLEN_UPDATE", "off")],
    );
    assert_eq!(code, 0);
    assert!(out.contains("KALTSTELLEN_UPDATE is set"), "{out}");
    // The same version: nothing is downloaded or replaced.
    let (code, out, err) = s.run(&["update"], &[]);
    assert_eq!(code, 0, "{err}");
    assert!(out.contains("is the newest version"), "{out}");
    assert_eq!(s.bin_names().len(), 1);
}

#[test]
fn up1_update_with_a_tray_resumes_swaps_and_restarts_it() {
    let version = built_version();
    let mut served = fs::read(kaltstellen_exe()).unwrap();
    served.extend_from_slice(MARKER);
    let mut s = Setup::new(github(&version, &served, &served));
    let (_program, beat) = s.program();
    s.start_tray();
    let (code, _, err) = s.run(&["switch", "play"], &[]);
    assert_eq!(code, 0, "{err}");
    assert!(frozen(&beat), "play did not freeze the program");

    // --force: the served release has the same version as the copy.
    let (code, out, err) = s.run(&["update", "--force"], &[]);
    assert_eq!(code, 0, "{out}{err}");
    assert!(out.contains("quitting the tray"), "{out}");
    assert!(
        out.contains(&format!("updated to version {version}")),
        "{out}"
    );

    // Nothing stayed frozen; the new file is in place, the old one kept.
    assert!(
        beating(&beat),
        "the program is still frozen after the update"
    );
    assert_eq!(fs::read(&s.exe).unwrap(), served);
    assert_eq!(s.file("old.exe").unwrap(), s.original);
    // The old tray quit as from the menu, a new one answers.
    let old_tray = s.tray.as_mut().unwrap();
    assert!(wait_for(LONG, || old_tray.try_wait().unwrap().is_some()));
    assert_eq!(old_tray.wait().unwrap().code(), Some(0));
    s.wait_log("tray: pipe ready", 2);
    let (code, out, _) = s.run(&["status"], &[]);
    assert_eq!(code, 0);
    assert!(out.contains("active profile: neutral"), "{out}");
    // The profile is offered again, not applied.
    let settings = fs::read_to_string(s.config_dir().join("settings.json")).unwrap();
    assert!(
        settings.contains("\"last_profile\": \"play\""),
        "{settings}"
    );
    assert!(s.log().contains("tray: offering last profile \"play\""));
}

#[test]
fn up1_a_new_tray_that_does_not_start_is_rolled_back() {
    let version = built_version();
    let bad = fs::read(BAD_UPDATE).unwrap();
    let mut s = Setup::new(github(&version, &bad, &bad));
    let (_program, beat) = s.program();
    s.start_tray();
    let (code, out, err) = s.run(&["update", "--force"], &[("KS_FAKE_VERSION", &version)]);
    assert_eq!(code, 1, "{out}{err}");
    assert!(err.contains("KS806"), "{err}");
    assert!(out.contains("the previous tray runs again"), "{out}");
    assert_eq!(fs::read(&s.exe).unwrap(), s.original);
    assert_eq!(s.file("failed.exe").unwrap(), bad);
    assert!(s.file("old.exe").is_none());
    assert!(beating(&beat));
    // The previous version's tray is back.
    s.wait_log("tray: pipe ready", 2);
}

#[test]
fn up1_wrong_checksum_or_broken_file_changes_nothing() {
    let version = built_version();
    let served = fs::read(kaltstellen_exe()).unwrap();
    let mut s = Setup::new(github(&version, &served, b"something else"));
    let (_program, beat) = s.program();
    s.start_tray();
    let (code, _, err) = s.run(&["switch", "play"], &[]);
    assert_eq!(code, 0, "{err}");
    let (code, _, err) = s.run(&["update", "--force"], &[]);
    assert_eq!(code, 1);
    assert!(err.contains("KS803"), "{err}");
    // The tray was not asked to quit; the program stays as switched.
    assert!(s.tray.as_mut().unwrap().try_wait().unwrap().is_none());
    assert!(frozen(&beat));
    assert_eq!(s.bin_names().len(), 1);

    // A file with the right sum that is no program: refused by the probe.
    let junk = b"MZ not really a program".to_vec();
    let t = Setup::new(github(&version, &junk, &junk));
    let (code, _, err) = t.run(&["update", "--force"], &[]);
    assert_eq!(code, 1);
    assert!(err.contains("KS806"), "{err}");
    assert_eq!(t.bin_names().len(), 1);
    assert_eq!(fs::read(&t.exe).unwrap(), t.original);
}

#[test]
fn up1_tray_checks_when_checking_is_switched_on() {
    let mut s = Setup::new(github("9.9.9", b"x", b"x"));
    fs::write(
        s.config_dir().join("settings.json"),
        "{ \"update\": \"off\" }",
    )
    .unwrap();
    s.start_tray();
    assert!(s.log().contains("tray: started"));
    assert!(
        !s.log().contains("next update check"),
        "off must not schedule"
    );
    let pid = s.tray.as_ref().unwrap().id();
    let hwnd = tray_window(pid).expect("tray window");
    // SAFETY: posting to a window of another process.
    unsafe {
        PostMessageW(
            Some(hwnd),
            WM_COMMAND,
            WPARAM(usize::from(ID_UPDATE_MANUAL)),
            LPARAM(0),
        )
    }
    .unwrap();
    s.wait_log("tray: update 9.9.9 available", 1);
    s.wait_log("tray: next update check in", 1);
    let settings = fs::read_to_string(s.config_dir().join("settings.json")).unwrap();
    assert!(settings.contains("\"update\": \"manual\""), "{settings}");
    assert!(
        !s.log().contains("automatically"),
        "manual never installs by itself"
    );
    let record = fs::read_to_string(s.data().join("update.json")).unwrap();
    assert!(record.contains("9.9.9"));
}

/// Posts a menu command to the tray of `pid`.
fn click(pid: u32, id: u16) {
    let hwnd = tray_window(pid).expect("tray window");
    // SAFETY: posting to a window of another process.
    unsafe { PostMessageW(Some(hwnd), WM_COMMAND, WPARAM(usize::from(id)), LPARAM(0)) }.unwrap();
}

#[test]
fn up1_automatic_install_waits_for_neutral_and_an_idle_tray() {
    // A newer release whose file is this build: it downloads and verifies,
    // then fails its version check (it is not 9.9.9) and changes nothing.
    let served = fs::read(kaltstellen_exe()).unwrap();
    let mut s = Setup::new(github("9.9.9", &served, &served));
    let (_program, beat) = s.program();
    s.start_tray();
    let (code, _, err) = s.run(&["switch", "play"], &[]);
    assert_eq!(code, 0, "{err}");
    assert!(frozen(&beat));
    // Mode auto (the default) from the menu checks at once.
    let pid = s.tray.as_ref().unwrap().id();
    click(pid, ID_UPDATE_AUTO);
    s.wait_log("tray: update 9.9.9 available", 1);
    s.wait_log("tray: update 9.9.9 waits until neutral is active", 1);
    assert!(
        !s.log().contains("installing update 9.9.9"),
        "never while a profile is active"
    );
    assert!(frozen(&beat), "nothing was thawed");
    // Back to neutral: now it installs by itself.
    let (code, _, _) = s.run(&["switch", "neutral"], &[]);
    assert_eq!(code, 0);
    s.wait_log("tray: installing update 9.9.9 automatically", 1);
    s.wait_log("tray: automatic update started", 1);
    // The staged file did not report 9.9.9: rolled back before the tray was
    // touched (KS806), the executable is unchanged and the tray still runs.
    s.wait_log("KS806", 1);
    assert_eq!(fs::read(&s.exe).unwrap(), s.original);
    assert!(s.tray.as_mut().unwrap().try_wait().unwrap().is_none());
    assert!(beating(&beat));
}

#[test]
fn up1_automatic_install_gives_up_when_a_profile_starts_during_the_download() {
    // A "new version" that passes the probe; its download waits until the
    // test lets it go.
    let bad = fs::read(BAD_UPDATE).unwrap();
    let (release, gate) = channel();
    let mut s = Setup::new(github_gated("9.9.9", &bad, &bad, Some(gate)));
    let (_program, beat) = s.program();
    s.start_tray_with(&[("KS_FAKE_VERSION", "9.9.9")]);
    let pid = s.tray.as_ref().unwrap().id();
    // Neutral and idle: the automatic install starts and downloads.
    click(pid, ID_UPDATE_AUTO);
    s.wait_log("tray: installing update 9.9.9 automatically", 1);
    s.wait_log("tray: automatic update started", 1);
    // Meanwhile a profile is switched on.
    let (code, _, err) = s.run(&["switch", "play"], &[]);
    assert_eq!(code, 0, "{err}");
    assert!(frozen(&beat));
    // The download ends now; later ones (the second attempt) run at once.
    release.send(()).unwrap();
    drop(release);
    // When the download is done, the tray refuses to quit; the install
    // gives up without changing anything.
    s.wait_log("the tray is busy; it waits", 1);
    s.wait_log("tried again later", 1);
    assert!(s.tray.as_mut().unwrap().try_wait().unwrap().is_none());
    assert!(frozen(&beat), "nothing was thawed");
    assert_eq!(fs::read(&s.exe).unwrap(), s.original);
    assert!(
        wait_for(LONG, || s.bin_names().len() == 1),
        "the staged file is left behind: {:?}",
        s.bin_names()
    );
    let record = fs::read_to_string(s.data().join("update.json")).unwrap();
    assert!(!record.contains("\"failed\""), "{record}");
    // Back to neutral: the install runs again.
    let (code, _, _) = s.run(&["switch", "neutral"], &[]);
    assert_eq!(code, 0);
    assert!(beating(&beat));
    s.wait_log("tray: installing update 9.9.9 automatically", 2);
    // This time the tray quits, but the new tray does not start: rolled
    // back, and the version is remembered as failed.
    s.wait_log("update: the previous tray runs again", 1);
    s.wait_log("the automatic install tries it again in 1 day(s)", 1);
    assert_eq!(fs::read(&s.exe).unwrap(), s.original);
    let record = fs::read_to_string(s.data().join("update.json")).unwrap();
    assert!(record.contains("\"failed\": \"9.9.9\""), "{record}");
}

#[test]
fn up1_a_version_that_did_not_start_waits_and_is_tried_again_later() {
    let bad = fs::read(BAD_UPDATE).unwrap();
    let mut s = Setup::new(github("9.9.9", &bad, &bad));
    let fake = [("KS_FAKE_VERSION", "9.9.9")];
    s.start_tray_with(&fake);
    let pid = s.tray.as_ref().unwrap().id();
    click(pid, ID_UPDATE_AUTO);
    // Installed, the new tray did not start, rolled back.
    s.wait_log("update: the previous tray runs again", 1);
    s.wait_log("the automatic install tries it again in 1 day(s)", 1);
    s.wait_log("tray: pipe ready", 2);
    let old_tray = s.tray.as_mut().unwrap();
    assert!(wait_for(LONG, || old_tray.try_wait().unwrap().is_some()));
    assert_eq!(fs::read(&s.exe).unwrap(), s.original);
    // The tray that runs again (started by the update) and any later one
    // leave it alone, also after the next check.
    drop(KillProgram(
        s.exe.file_name().unwrap().to_string_lossy().into_owned(),
    ));
    s.start_tray_with(&fake);
    let pid = s.tray.as_ref().unwrap().id();
    // Logged once by each tray after its start-up recovery: the one the
    // update started again, and this one. This one's start-up schedule
    // line is in the log by then.
    s.wait_log("tray: update 9.9.9 could not be installed here recently", 2);
    // The check that the click starts has run and been decided on: the
    // tray schedules the next one right after deciding.
    let scheduled = s.log().matches("tray: next update check in").count();
    click(pid, ID_UPDATE_AUTO);
    s.wait_log("tray: next update check in", scheduled + 1);
    assert_eq!(
        s.log()
            .matches("tray: installing update 9.9.9 automatically")
            .count(),
        1
    );
    // The automatic form refuses it too; on request it is still tried.
    let (code, _, err) = s.run(&["update", "--unattended"], &fake);
    assert_eq!(code, 1);
    assert!(
        err.contains("could not be installed here recently"),
        "{err}"
    );
    let (code, out, err) = s.run(&["update", "--force"], &fake);
    assert_eq!(code, 1, "{out}{err}");
    assert!(out.contains("downloading version 9.9.9"), "{out}");
    assert!(err.contains("KS806"), "{err}");
    // That was the second failure in a row: two days now.
    s.wait_log(
        "(2 time(s) in a row); the automatic install tries it again in 2 day(s)",
        1,
    );
    // When the two days are over, a tray that just sits in neutral tries it
    // again by itself, on its own timer (no check, no switch). The tray
    // the update started again is ended first, so nothing reads the record
    // while it is edited: the wait ends 10 s from now.
    drop(KillProgram(
        s.exe.file_name().unwrap().to_string_lossy().into_owned(),
    ));
    let file = s.data().join("update.json");
    let mut record: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&file).unwrap()).unwrap();
    assert_eq!(record["failures"], 2, "{record}");
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    record["failed_at"] = (now - 2 * 24 * 60 * 60 + 10).into();
    fs::write(&file, record.to_string()).unwrap();
    let count = |s: &Setup, text: &str| s.log().matches(text).count();
    let held = count(
        &s,
        "tray: update 9.9.9 could not be installed here recently",
    );
    let armed = count(&s, "tray: failed update tried again in");
    let installs = count(&s, "tray: installing update 9.9.9 automatically");
    s.start_tray_with(&fake);
    // The new tray holds it back first and arms its timer for it ...
    s.wait_log(
        "tray: update 9.9.9 could not be installed here recently",
        held + 1,
    );
    s.wait_log("tray: failed update tried again in", armed + 1);
    assert_eq!(
        count(&s, "tray: installing update 9.9.9 automatically"),
        installs,
        "installed before the wait was over"
    );
    // ... and the timer tries it again.
    s.wait_log("tray: installing update 9.9.9 automatically", installs + 1);
    s.wait_log(
        "(3 time(s) in a row); the automatic install tries it again in 4 day(s)",
        1,
    );
    assert_eq!(fs::read(&s.exe).unwrap(), s.original);
}
