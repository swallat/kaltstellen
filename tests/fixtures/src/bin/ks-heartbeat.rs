//! Console fixture: `ks-heartbeat [<file>] [--alloc-mb <n>]`.
//!
//! Writes a heartbeat to `<file>` (or `--heartbeat <file>`) every 100 ms and
//! runs until killed. With `--alloc-mb` it first allocates and touches that
//! much memory (trim test T19).

use std::path::PathBuf;
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
    loop {
        thread::sleep(Duration::from_secs(3600));
    }
}
