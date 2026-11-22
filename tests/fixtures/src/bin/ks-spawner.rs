//! Process-tree fixture: `ks-spawner <dir>`.
//!
//! - writes its own heartbeat to `<dir>\spawner.txt`,
//! - starts `ks-heartbeat <dir>\child.txt` as a child (PID in `child.pid`),
//! - starts an intermediate process that launches
//!   `ks-heartbeat <dir>\orphan.txt` and exits at once, so that process is an
//!   orphan (PID in `orphan.pid`).
//!
//! Suspending `ks-spawner.exe` must freeze the child but not the orphan
//! (spec A7 tree resolution).

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::thread;
use std::time::Duration;

use ks_fixtures::{Args, common_setup, sibling_exe, start_heartbeat};

fn main() {
    let args = Args::from_env();

    if let Some(dir) = args.value("--orphan-launcher") {
        // Intermediate: launch the orphan-to-be, record its PID, exit.
        let dir = Path::new(dir);
        let orphan = heartbeat(&dir.join("orphan.txt"));
        fs::write(dir.join("orphan.pid"), orphan.to_string()).expect("orphan.pid");
        return;
    }

    common_setup(&args);
    let dir = PathBuf::from(args.positional().expect("usage: ks-spawner <dir>"));
    fs::create_dir_all(&dir).expect("cannot create fixture dir");
    start_heartbeat(dir.join("spawner.txt"));

    let child = heartbeat(&dir.join("child.txt"));
    fs::write(dir.join("child.pid"), child.to_string()).expect("child.pid");

    let me = std::env::current_exe().expect("current_exe");
    Command::new(me)
        .arg("--orphan-launcher")
        .arg(&dir)
        .status()
        .expect("cannot run orphan launcher");

    fs::write(dir.join("spawner.pid"), std::process::id().to_string()).expect("spawner.pid");
    loop {
        thread::sleep(Duration::from_secs(3600));
    }
}

fn heartbeat(file: &Path) -> u32 {
    Command::new(sibling_exe("ks-heartbeat"))
        .arg(file)
        .spawn()
        .expect("cannot start ks-heartbeat")
        .id()
}
