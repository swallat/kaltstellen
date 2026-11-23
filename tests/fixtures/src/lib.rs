//! Shared code for kaltstellen's test fixtures.
//!
//! These programs exist only to be suspended, resumed, closed and killed by
//! the acceptance tests. They are never shipped. Unlike the product code they
//! may use `unsafe` (decision 2026-11-22: the Q4 rule covers the tool, not
//! its test helpers).
//!
//! Each fixture can write a heartbeat: the current time in milliseconds since
//! the Unix epoch, rewritten every 100 ms. A heartbeat that stops advancing
//! proves the process is frozen.

use std::fs;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

pub mod window;

pub const HEARTBEAT_INTERVAL: Duration = Duration::from_millis(100);

/// Minimal argument parsing: `--flag value` pairs and positional arguments.
pub struct Args {
    items: Vec<String>,
}

impl Args {
    pub fn from_env() -> Self {
        Self {
            items: std::env::args().skip(1).collect(),
        }
    }

    pub fn flag(&self, name: &str) -> bool {
        self.items.iter().any(|a| a == name)
    }

    pub fn value(&self, name: &str) -> Option<&str> {
        self.items
            .iter()
            .position(|a| a == name)
            .and_then(|i| self.items.get(i + 1))
            .map(String::as_str)
    }

    pub fn number(&self, name: &str) -> Option<u64> {
        self.value(name).map(|v| {
            v.parse()
                .unwrap_or_else(|_| panic!("{name} expects a number, got {v:?}"))
        })
    }

    /// First argument that is neither a flag nor the value of one.
    pub fn positional(&self) -> Option<&str> {
        let mut skip_next = false;
        for item in &self.items {
            if skip_next {
                skip_next = false;
            } else if item.starts_with("--") {
                skip_next = VALUE_FLAGS.contains(&item.as_str());
            } else {
                return Some(item);
            }
        }
        None
    }
}

/// Flags that take a value; used by [`Args::positional`].
const VALUE_FLAGS: &[&str] = &[
    "--heartbeat",
    "--alloc-mb",
    "--close-delay-ms",
    "--close-when",
    "--title",
    "--orphan-launcher",
];

pub fn now_millis() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

/// Starts a background thread that rewrites `path` every 100 ms.
pub fn start_heartbeat(path: PathBuf) {
    thread::spawn(move || {
        loop {
            // Write to a temp file and rename, so readers never see a torn value.
            let tmp = path.with_extension("tmp");
            if fs::write(&tmp, now_millis().to_string()).is_ok() {
                let _ = fs::rename(&tmp, &path);
            }
            thread::sleep(HEARTBEAT_INTERVAL);
        }
    });
}

/// Writes this process's PID to `path`, for tests that need it.
pub fn write_pid(path: &Path) {
    fs::write(path, std::process::id().to_string()).expect("cannot write pid file");
}

/// Allocates `mb` megabytes and touches every page so they count in the
/// working set (trim test T19). The memory is leaked on purpose.
pub fn allocate_mb(mb: u64) {
    let mut block = vec![0u8; (mb * 1024 * 1024) as usize];
    for page in block.chunks_mut(4096) {
        page[0] = 1;
    }
    std::mem::forget(block);
}

/// Path of a sibling fixture executable (same directory as this one).
pub fn sibling_exe(name: &str) -> PathBuf {
    let me = std::env::current_exe().expect("cannot find own executable");
    me.with_file_name(format!("{name}.exe"))
}

/// Applies the options every fixture understands: `--heartbeat <file>`,
/// `--alloc-mb <n>`.
pub fn common_setup(args: &Args) {
    if let Some(mb) = args.number("--alloc-mb") {
        allocate_mb(mb);
    }
    if let Some(path) = args.value("--heartbeat") {
        start_heartbeat(PathBuf::from(path));
    }
}
