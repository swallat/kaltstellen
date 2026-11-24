//! Smoke test of `scripts/beta-test.ps1`, the guided beta test: setup in a
//! sandbox with this build and fixture copies as the program list, a few
//! steps unattended, a simulated sign-out, collect and teardown. Afterwards
//! nothing of it may be left: no process started from the sandbox, no
//! temporary folder, no test registry key, no state file. The real
//! configuration is never touched (`-Sandbox`, and `-Root` keeps the
//! script's own files in the test's folder). Runs Windows PowerShell 5.1,
//! the oldest shell the script supports; skipped where it is missing.

mod common;

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use common::*;
use kaltstellen::autostart::{RUN_KEY, VALUE};
use kaltstellen::win32::registry;

const SCRIPT: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../scripts/beta-test.ps1");

fn powershell() -> Option<PathBuf> {
    let root = std::env::var_os("SystemRoot")?;
    let exe = Path::new(&root).join(r"System32\WindowsPowerShell\v1.0\powershell.exe");
    exe.exists().then_some(exe)
}

/// Windows PowerShell with the environment of the test, without the module
/// path of a PowerShell 7 that may have started cargo.
fn command(ps: &Path) -> Command {
    let mut command = Command::new(ps);
    command.env_remove("PSModulePath");
    command
}

/// The longest one run of the script may take (the slow ARM64 runner
/// included); a hung run fails the test instead of the whole CI job.
const RUN_TIMEOUT: Duration = Duration::from_secs(300);

/// Runs `command` to the end or until [`RUN_TIMEOUT`] (then it is killed):
/// its exit success, `None` on the timeout, and its output.
fn run(mut command: Command) -> (Option<bool>, String) {
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("cannot run powershell");
    let drain = |mut pipe: Box<dyn Read + Send>| {
        thread::spawn(move || {
            let mut bytes = Vec::new();
            let _ = pipe.read_to_end(&mut bytes);
            String::from_utf8_lossy(&bytes).into_owned()
        })
    };
    let out = drain(Box::new(child.stdout.take().unwrap()));
    let err = drain(Box::new(child.stderr.take().unwrap()));
    let deadline = Instant::now() + RUN_TIMEOUT;
    let status = loop {
        if let Some(status) = child.try_wait().expect("try_wait") {
            break Some(status);
        }
        if Instant::now() > deadline {
            let _ = child.kill();
            let _ = child.wait();
            break None;
        }
        thread::sleep(Duration::from_millis(200));
    };
    let text = format!("{}{}", out.join().unwrap(), err.join().unwrap());
    (status.map(|s| s.success()), text)
}

/// Runs the script with `args`; panics with its output unless it exits 0.
fn script(ps: &Path, args: &[&str]) -> String {
    let mut c = command(ps);
    c.args([
        "-NoProfile",
        "-NonInteractive",
        "-ExecutionPolicy",
        "Bypass",
        "-File",
    ])
    .arg(SCRIPT)
    .args(args);
    let (ok, text) = run(c);
    let ok = ok.unwrap_or_else(|| {
        panic!("beta-test.ps1 {args:?} did not end within {RUN_TIMEOUT:?}:\n{text}")
    });
    assert!(ok, "beta-test.ps1 {args:?} failed:\n{text}");
    text
}

/// The real Run value with its kind ("String|..." or "ExpandString|..."),
/// empty when there is none; read through PowerShell, which names the kind.
/// A missing Run key is the same as a key without the value: on a fresh
/// machine something outside the sandbox (WebView2 or Edge, first sign-in
/// setup) may create the key while this test runs. `OpenSubKey` returns
/// nothing only for a missing key; any other error still shows as text.
fn run_value(ps: &Path) -> String {
    let read = format!(
        "$k = [Microsoft.Win32.Registry]::CurrentUser.OpenSubKey('{RUN_KEY}'); if ($k -and $null -ne $k.GetValue('{VALUE}')) {{ \"$($k.GetValueKind('{VALUE}'))|$($k.GetValue('{VALUE}', '', 'DoNotExpandEnvironmentNames'))\" }}"
    );
    let mut c = command(ps);
    c.args(["-NoProfile", "-NonInteractive", "-Command", &read]);
    run(c).1.trim().to_owned()
}

/// What the real configuration looks like: the Run value (with its kind)
/// and the files of %APPDATA%\kaltstellen and %LOCALAPPDATA%\kaltstellen
/// (top level: name and size; for kaltstellen.log* only the name, since
/// another kaltstellen on the machine may write to it). The sandbox must not
/// change any of them.
fn real_state(ps: &Path) -> (String, Vec<(String, u64)>) {
    let run = run_value(ps);
    let mut files = Vec::new();
    for base in ["APPDATA", "LOCALAPPDATA"] {
        let Some(root) = std::env::var_os(base) else {
            continue;
        };
        let dir = Path::new(&root).join("kaltstellen");
        if let Ok(entries) = fs::read_dir(&dir) {
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().into_owned();
                let len = if name.starts_with("kaltstellen.log") {
                    0
                } else {
                    entry.metadata().map(|m| m.len()).unwrap_or(0)
                };
                files.push((format!("{base}\\{name}"), len));
            }
        }
    }
    files.sort();
    (run, files)
}
fn str_of(path: &Path) -> &str {
    path.to_str().expect("path is not UTF-8")
}

/// Processes whose executable lies below `dir` (through PowerShell, which
/// reads the image path).
fn processes_below(ps: &Path, dir: &Path) -> String {
    let filter = format!(
        "Get-Process | Where-Object {{ $_.Path -and $_.Path.StartsWith('{}', 'OrdinalIgnoreCase') }} | ForEach-Object {{ \"$($_.Id) $($_.Path)\" }}",
        str_of(dir).replace('\'', "''")
    );
    let mut c = command(ps);
    c.args(["-NoProfile", "-NonInteractive", "-Command", &filter]);
    run(c).1.trim().to_owned()
}

/// Runs `teardown` when the test fails half-way, so a failed run leaves
/// nothing behind either.
struct Teardown<'a> {
    ps: &'a Path,
    root: PathBuf,
}

impl Drop for Teardown<'_> {
    fn drop(&mut self) {
        if self.root.join("beta-state.json").exists() {
            let mut c = command(self.ps);
            c.args([
                "-NoProfile",
                "-NonInteractive",
                "-ExecutionPolicy",
                "Bypass",
                "-File",
            ])
            .arg(SCRIPT)
            .args(["teardown", "-Unattended", "-Root"])
            .arg(&self.root);
            let _ = run(c);
        }
    }
}

#[test]
fn beta_script_runs_in_a_sandbox_and_leaves_nothing_behind() {
    let Some(ps) = powershell() else {
        eprintln!("beta_script: skipped, Windows PowerShell not found");
        return;
    };
    let dir = tempfile::tempdir().expect("tempdir");
    let bin = dir.path().join("bin");
    fs::create_dir(&bin).unwrap();
    let hb = copy_fixture(HEARTBEAT, &bin, "ks-beta-smoke-hb.exe");
    let big = copy_fixture(HEARTBEAT, &bin, "ks-beta-smoke-big.exe");
    let win = copy_fixture(WINDOW, &bin, "ks-beta-smoke-win.exe");
    let _kill = [
        KillProgram("ks-beta-smoke-hb.exe".into()),
        KillProgram("ks-beta-smoke-big.exe".into()),
        KillProgram("ks-beta-smoke-win.exe".into()),
    ];
    let program = |exe: &str, path: &Path, args: serde_json::Value, hidden: bool| serde_json::json!({ "exe": exe, "path": str_of(path), "args": args, "hidden": hidden, "disposable": true });
    let list = serde_json::json!({
        "suspend": [
            program("ks-beta-smoke-hb.exe", &hb, serde_json::json!(["{heartbeat}"]), true),
            program("ks-beta-smoke-win.exe", &win, serde_json::json!(["--title", "ks-beta-smoke-win"]), false),
        ],
        "stop": [],
        "trigger": "builtin",
        "large": program("ks-beta-smoke-big.exe", &big, serde_json::json!(["{heartbeat}", "--alloc-mb", "64"]), true),
        "elevated": null,
        "tray_program": null,
        "hotkey": "Ctrl+Shift+Alt+F12",
    });
    let programs = dir.path().join("programs.json");
    fs::write(&programs, serde_json::to_string_pretty(&list).unwrap()).unwrap();
    let root = dir.path().join("root");
    let _teardown = Teardown {
        ps: &ps,
        root: root.clone(),
    };
    let report = dir.path().join("report");
    let exe = kaltstellen_exe();
    let common = ["-Root", str_of(&root), "-Unattended"];
    let real_before = real_state(&ps);

    let mut args = vec![
        "setup",
        "-Sandbox",
        "-Exe",
        str_of(&exe),
        "-ProgramList",
        str_of(&programs),
        "-NoEdit",
    ];
    args.extend(common);
    script(&ps, &args);

    let state: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(root.join("beta-state.json")).unwrap()).unwrap();
    let temp_root = PathBuf::from(state["temp_root"].as_str().unwrap());
    let registry_root = state["registry_root"].as_str().unwrap().to_owned();
    let session = PathBuf::from(state["session_dir"].as_str().unwrap());
    assert!(temp_root.join("config").join("config.toml").exists());
    assert!(registry_root.starts_with("Software\\kaltstellen-beta-"));

    let listed = script(&ps, &[&["run", "list"][..], &common[..]].concat());
    assert!(listed.contains("t19"), "{listed}");
    script(
        &ps,
        &[&["run", "tray,t19", "-SettleSeconds", "2"][..], &common[..]].concat(),
    );
    script(
        &ps,
        &[&["run", "t7", "-SimulateSessionEnd"][..], &common[..]].concat(),
    );
    let collected = script(
        &ps,
        &[&["collect", "-ReportDir", str_of(&report)][..], &common[..]].concat(),
    );
    let excerpt = collected
        .lines()
        .find_map(|l| {
            l.trim()
                .strip_suffix(" kaltstellen.log line(s) in the excerpt")
        })
        .and_then(|n| n.parse::<usize>().ok())
        .unwrap_or(0);
    assert!(excerpt > 0, "empty log excerpt:\n{collected}");

    let results = fs::read_to_string(session.join("results.jsonl")).unwrap();
    let records: Vec<serde_json::Value> = results
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    let outcome = |test: &str, step: &str| {
        records
            .iter()
            .rev()
            .find(|r| r["test"] == test && r["step"] == step)
            .and_then(|r| r["outcome"].as_str())
            .unwrap_or("missing")
            .to_owned()
    };
    for (test, step) in [
        ("tray", "profile"),
        ("tray", "quit"),
        ("t19", "trim-on"),
        ("t19", "trim-off"),
        ("t7", "resume"),
        ("t7", "offer"),
    ] {
        assert_eq!(outcome(test, step), "pass", "{test}/{step}:\n{results}");
    }
    let zips: Vec<_> = fs::read_dir(&report).unwrap().flatten().collect();
    assert_eq!(zips.len(), 1, "one report zip");

    script(&ps, &[&["teardown"][..], &common[..]].concat());
    assert!(
        !root.join("beta-state.json").exists(),
        "state file left behind"
    );
    assert!(session.join("state.done.json").exists());
    assert!(!temp_root.exists(), "{} left behind", temp_root.display());
    assert!(
        !registry::exists(&registry_root).unwrap(),
        "HKCU\\{registry_root} left behind"
    );
    assert_eq!(
        processes_below(&ps, &temp_root),
        "",
        "processes from the sandbox left running"
    );
    for name in [
        "ks-beta-smoke-hb.exe",
        "ks-beta-smoke-big.exe",
        "ks-beta-smoke-win.exe",
    ] {
        assert!(pids_of(name).is_empty(), "{name} left running");
    }
    assert_eq!(
        real_state(&ps),
        real_before,
        "the real Run value, %APPDATA%\\kaltstellen or %LOCALAPPDATA%\\kaltstellen changed"
    );
    // Teardown is idempotent.
    let again = script(&ps, &[&["teardown"][..], &common[..]].concat());
    assert!(again.contains("Nothing to tear down"), "{again}");
}

/// Without a program list the script brings its own test programs (compiled
/// from `scripts/beta-test-programs.cs` with the compiler of the .NET
/// Framework): the self-check, the stop entries (one closes, one refuses and
/// stays, one is killed), the unassigned tray program and the memory
/// measurement all run on them, and teardown removes every trace.
#[test]
fn beta_script_brings_its_own_test_programs() {
    let Some(ps) = powershell() else {
        eprintln!("beta_script: skipped, Windows PowerShell not found");
        return;
    };
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path().join("root");
    let _teardown = Teardown {
        ps: &ps,
        root: root.clone(),
    };
    let exe = kaltstellen_exe();
    let common = ["-Root", str_of(&root), "-Unattended"];
    let real_before = real_state(&ps);
    let names = [
        "ks-beta-freeze1.exe",
        "ks-beta-freeze2.exe",
        "ks-beta-stop.exe",
        "ks-beta-stubborn.exe",
        "ks-beta-stubborn-kill.exe",
        "ks-beta-large.exe",
        "ks-beta-tray.exe",
    ];
    let _kill = names.map(|n| KillProgram(n.into()));

    let mut args = vec!["setup", "-Sandbox", "-Exe", str_of(&exe)];
    args.extend(common);
    script(&ps, &args);

    let state: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(root.join("beta-state.json")).unwrap()).unwrap();
    let temp_root = PathBuf::from(state["temp_root"].as_str().unwrap());
    let session = PathBuf::from(state["session_dir"].as_str().unwrap());
    for name in names {
        assert!(
            temp_root.join("test").join(name).exists(),
            "{name} not built"
        );
    }
    // No beta-test.json was needed or created.
    assert!(!root.join("beta-test.json").exists());

    script(
        &ps,
        &[
            &["run", "programs,stop,unassigned,t19", "-SettleSeconds", "2"][..],
            &common[..],
        ]
        .concat(),
    );
    let results = fs::read_to_string(session.join("results.jsonl")).unwrap();
    let records: Vec<serde_json::Value> = results
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    let outcome = |test: &str, step: &str| {
        records
            .iter()
            .rev()
            .find(|r| r["test"] == test && r["step"] == step)
            .and_then(|r| r["outcome"].as_str())
            .unwrap_or("missing")
            .to_owned()
    };
    for (test, step) in [
        ("programs", "start"),
        ("programs", "close"),
        ("stop", "closes"),
        ("stop", "restart"),
        ("unassigned", "listed"),
        ("t19", "trim-on"),
        ("t19", "trim-off"),
    ] {
        assert_eq!(outcome(test, step), "pass", "{test}/{step}:\n{results}");
    }

    script(&ps, &[&["teardown"][..], &common[..]].concat());
    assert!(!temp_root.exists(), "{} left behind", temp_root.display());
    assert_eq!(
        processes_below(&ps, &temp_root),
        "",
        "processes from the sandbox left running"
    );
    for name in names {
        assert!(pids_of(name).is_empty(), "{name} left running");
    }
    assert_eq!(
        real_state(&ps),
        real_before,
        "the real configuration changed"
    );
}
