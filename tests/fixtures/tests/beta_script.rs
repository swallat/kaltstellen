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
use std::path::{Path, PathBuf};
use std::process::Command;

use common::*;
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

/// Runs the script with `args`; panics with its output unless it exits 0.
fn script(ps: &Path, args: &[&str]) -> String {
    let out = command(ps)
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "Bypass",
            "-File",
        ])
        .arg(SCRIPT)
        .args(args)
        .output()
        .expect("cannot run powershell");
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        out.status.success(),
        "beta-test.ps1 {args:?} failed:\n{text}"
    );
    text
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
    let out = command(ps)
        .args(["-NoProfile", "-NonInteractive", "-Command", &filter])
        .output()
        .expect("cannot run powershell");
    String::from_utf8_lossy(&out.stdout).trim().to_owned()
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
            let _ = command(self.ps)
                .args([
                    "-NoProfile",
                    "-NonInteractive",
                    "-ExecutionPolicy",
                    "Bypass",
                    "-File",
                ])
                .arg(SCRIPT)
                .args(["teardown", "-Unattended", "-Root"])
                .arg(&self.root)
                .output();
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
        "trigger": null,
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
    script(
        &ps,
        &[&["collect", "-ReportDir", str_of(&report)][..], &common[..]].concat(),
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
    // Teardown is idempotent.
    let again = script(&ps, &[&["teardown"][..], &common[..]].concat());
    assert!(again.contains("Nothing to tear down"), "{again}");
}
