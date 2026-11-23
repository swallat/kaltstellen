//! Console fixture: `ks-heartbeat [<file>] [--alloc-mb <n>]
//! [--sibling <exe> --sibling-beat <file>]
//! [--run <exe> --run-args <a;b;...> --run-exit <file>]`.
//!
//! Writes a heartbeat to `<file>` (or `--heartbeat <file>`) every 100 ms and
//! runs until killed. With `--alloc-mb` it first allocates and touches that
//! much memory (trim test T19). `--sibling` starts a second heartbeat as its
//! child. `--run` starts a command as its child after 300 ms, waits for it
//! and writes its exit code to `--run-exit` (T16: a command line started
//! from inside a program).

use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::thread;
use std::time::Duration;

use ks_fixtures::{Args, common_setup, start_heartbeat};

fn main() {
    let args = Args::from_env();
    common_setup(&args);
    if args.value("--heartbeat").is_none()
        && let Some(path) = args.positional()
    {
        start_heartbeat(PathBuf::from(path));
    }
    let mut children = Vec::new();
    if let (Some(exe), Some(beat)) = (args.value("--sibling"), args.value("--sibling-beat")) {
        children.push(
            Command::new(exe)
                .arg(beat)
                .stdin(Stdio::null())
                .spawn()
                .expect("cannot start the sibling"),
        );
    }
    if let (Some(exe), Some(exit_file)) = (args.value("--run"), args.value("--run-exit")) {
        let exe = exe.to_owned();
        let exit_file = exit_file.to_owned();
        let run_args: Vec<String> = args
            .value("--run-args")
            .unwrap_or("")
            .split(';')
            .filter(|a| !a.is_empty())
            .map(str::to_owned)
            .collect();
        thread::spawn(move || {
            thread::sleep(Duration::from_millis(300));
            let code = Command::new(exe)
                .args(&run_args)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .map_or(-1, |s| s.code().unwrap_or(-1));
            let _ = std::fs::write(exit_file, code.to_string());
        });
    }
    loop {
        thread::sleep(Duration::from_secs(3600));
    }
}
