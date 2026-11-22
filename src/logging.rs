//! File logger (spec E1): `kaltstellen.log` in the data directory, rotated at
//! 1 MB, keeping three files (`kaltstellen.log`, `.log.1`, `.log.2`).
//!
//! Lines look like `2026-11-22T10:15:03.120Z INFO  switch: message`
//! (UTC, so no time-zone lookup is needed). The log never leaves the machine.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use log::{LevelFilter, Log, Metadata, Record};

pub const FILE_NAME: &str = "kaltstellen.log";
pub const MAX_BYTES: u64 = 1024 * 1024;
/// Total number of files including the current one.
pub const FILES: usize = 3;

/// An append-only file that rotates itself once it would exceed `max_bytes`.
pub struct RotatingFile {
    path: PathBuf,
    max_bytes: u64,
    files: usize,
    file: File,
    size: u64,
}

impl RotatingFile {
    pub fn open(path: PathBuf, max_bytes: u64, files: usize) -> io::Result<Self> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let file = OpenOptions::new().create(true).append(true).open(&path)?;
        let size = file.metadata()?.len();
        Ok(Self {
            path,
            max_bytes,
            files: files.max(1),
            file,
            size,
        })
    }

    /// Writes one record; rotates first if the record would not fit.
    pub fn write_record(&mut self, bytes: &[u8]) -> io::Result<()> {
        if self.size > 0 && self.size + bytes.len() as u64 > self.max_bytes {
            self.rotate()?;
        }
        self.file.write_all(bytes)?;
        self.size += bytes.len() as u64;
        Ok(())
    }

    fn rotated(&self, n: usize) -> PathBuf {
        let mut name = self.path.as_os_str().to_owned();
        name.push(format!(".{n}"));
        PathBuf::from(name)
    }

    fn rotate(&mut self) -> io::Result<()> {
        // Shift .log.(n-1) -> .log.n, dropping the oldest, then .log -> .log.1.
        let oldest = self.files - 1;
        if oldest == 0 {
            self.file = File::create(&self.path)?;
            self.size = 0;
            return Ok(());
        }
        let _ = fs::remove_file(self.rotated(oldest));
        for n in (1..oldest).rev() {
            let from = self.rotated(n);
            if from.exists() {
                fs::rename(&from, self.rotated(n + 1))?;
            }
        }
        // Rust opens files with FILE_SHARE_DELETE, so the open file can be
        // renamed; the old handle then points at `.log.1` and is replaced.
        fs::rename(&self.path, self.rotated(1))?;
        self.file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)?;
        self.size = 0;
        Ok(())
    }
}

struct FileLogger {
    level: LevelFilter,
    file: Mutex<RotatingFile>,
}

impl Log for FileLogger {
    fn enabled(&self, metadata: &Metadata) -> bool {
        metadata.level() <= self.level
    }

    fn log(&self, record: &Record) {
        if !self.enabled(record.metadata()) {
            return;
        }
        let line = format!(
            "{} {:<5} {}: {}\n",
            timestamp(SystemTime::now()),
            record.level(),
            record.target(),
            record.args()
        );
        if let Ok(mut file) = self.file.lock() {
            // A logger must not fail its caller; a lost line is acceptable.
            let _ = file.write_record(line.as_bytes());
        }
    }

    fn flush(&self) {
        if let Ok(mut file) = self.file.lock() {
            let _ = file.file.flush();
        }
    }
}

/// Installs the global file logger writing to `dir\kaltstellen.log`.
pub fn init(dir: &Path, level: LevelFilter) -> io::Result<()> {
    let file = RotatingFile::open(dir.join(FILE_NAME), MAX_BYTES, FILES)?;
    let logger = FileLogger {
        level,
        file: Mutex::new(file),
    };
    // The logger lives for the rest of the process.
    log::set_logger(Box::leak(Box::new(logger))).map_err(|e| io::Error::other(e.to_string()))?;
    log::set_max_level(level);
    Ok(())
}

/// RFC 3339 UTC timestamp with milliseconds.
pub fn timestamp(time: SystemTime) -> String {
    let since = time.duration_since(UNIX_EPOCH).unwrap_or_default();
    let secs = since.as_secs();
    let (y, m, d) = civil_from_days((secs / 86_400) as i64);
    let rem = secs % 86_400;
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}.{:03}Z",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60,
        since.subsec_millis()
    )
}

/// Days since 1970-01-01 to (year, month, day); Howard Hinnant's algorithm.
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = yoe + era * 400 + i64::from(m <= 2);
    (y, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn timestamp_formats_utc() {
        assert_eq!(timestamp(UNIX_EPOCH), "1970-01-01T00:00:00.000Z");
        // 2026-11-22T10:15:03.120Z
        let t = UNIX_EPOCH + Duration::from_millis(1_795_342_503_120);
        assert_eq!(timestamp(t), "2026-11-22T10:15:03.120Z");
        // Leap day.
        let t = UNIX_EPOCH + Duration::from_secs(951_782_400);
        assert_eq!(timestamp(t), "2000-02-29T00:00:00.000Z");
    }

    #[test]
    fn rotates_and_keeps_three_files() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(FILE_NAME);
        let mut file = RotatingFile::open(path.clone(), 100, 3).unwrap();
        for i in 0..10 {
            file.write_record(format!("{i:0>59}\n").as_bytes()).unwrap();
        }
        drop(file);

        let mut names: Vec<String> = fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect();
        names.sort();
        assert_eq!(
            names,
            ["kaltstellen.log", "kaltstellen.log.1", "kaltstellen.log.2"]
        );
        // Each file holds one 60-byte record; the newest is in the current file.
        assert!(fs::read_to_string(&path).unwrap().starts_with("0000"));
        assert!(fs::read_to_string(&path).unwrap().trim_end().ends_with('9'));
        assert!(fs::metadata(&path).unwrap().len() <= 100);
    }

    #[test]
    fn appends_to_existing_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(FILE_NAME);
        fs::write(&path, "old\n").unwrap();
        let mut file = RotatingFile::open(path.clone(), MAX_BYTES, FILES).unwrap();
        file.write_record(b"new\n").unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "old\nnew\n");
    }
}
