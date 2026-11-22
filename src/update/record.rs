//! What the updater remembers between runs (`update.json` in the data
//! directory) and when the tray checks next (spec UP1 "What is remembered",
//! "Automatic trigger").

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::error::{Code, Error, Result};

/// Time between two automatic checks.
pub const INTERVAL: Duration = Duration::from_secs(24 * 60 * 60);
/// The first automatic check waits this long after the tray started, so it
/// does not compete with the sign-in.
pub const FIRST_DELAY: Duration = Duration::from_secs(5 * 60);

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Record {
    /// Seconds since 1970 of the last check, successful or not.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_check: Option<u64>,
    /// The newest version found.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latest: Option<String>,
    /// Its release page.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub page: Option<String>,
    /// Why the last check failed; absent after a successful one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

fn file(dir: &Path) -> PathBuf {
    dir.join("update.json")
}

fn io_error(file: &Path, err: impl std::fmt::Display) -> Error {
    Error::new(Code::UpdateRecord, format!("{}: {err}", file.display()))
}

/// Reads the record; a missing file is "never checked". A damaged file is
/// reported and treated the same.
pub fn load(dir: &Path) -> (Record, Option<Error>) {
    let file = file(dir);
    match fs::read_to_string(&file) {
        Ok(text) => match serde_json::from_str(&text) {
            Ok(record) => (record, None),
            Err(err) => (Record::default(), Some(io_error(&file, err))),
        },
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => (Record::default(), None),
        Err(err) => (Record::default(), Some(io_error(&file, err))),
    }
}

/// Writes the record atomically (temporary file, then rename).
pub fn save(dir: &Path, record: &Record) -> Result<()> {
    let file = file(dir);
    let tmp = dir.join("update.json.tmp");
    let text = serde_json::to_string_pretty(record).map_err(|e| io_error(&file, e))?;
    fs::create_dir_all(dir)
        .and_then(|()| fs::write(&tmp, text))
        .and_then(|()| fs::rename(&tmp, &file))
        .map_err(|e| io_error(&file, e))
}

/// Seconds since 1970.
pub fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// How long the tray waits before its next check: [`FIRST_DELAY`] after its
/// start at the earliest, [`INTERVAL`] after the last check. All times in
/// seconds since 1970.
pub fn next_check(now: u64, started: u64, last_check: Option<u64>) -> Duration {
    let first = started.saturating_add(FIRST_DELAY.as_secs());
    let after_last = last_check.map_or(0, |last| last.saturating_add(INTERVAL.as_secs()));
    // A clock set back puts the last check in the future: then it is due
    // one interval from now at the latest.
    let after_last = after_last.min(now.saturating_add(INTERVAL.as_secs()));
    Duration::from_secs(first.max(after_last).saturating_sub(now))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn up1_record_round_trips_and_tolerates_damage() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(load(dir.path()), (Record::default(), None));
        let record = Record {
            last_check: Some(42),
            latest: Some("1.2.0".into()),
            page: Some("https://x".into()),
            error: None,
        };
        save(dir.path(), &record).unwrap();
        assert_eq!(load(dir.path()), (record, None));
        fs::write(dir.path().join("update.json"), "{ broken").unwrap();
        let (record, problem) = load(dir.path());
        assert_eq!(record, Record::default());
        assert_eq!(problem.unwrap().code, Code::UpdateRecord);
    }

    #[test]
    fn up1_schedule_once_a_day_never_right_at_start() {
        let day = INTERVAL.as_secs();
        let five = FIRST_DELAY.as_secs();
        let start = 1_000_000;
        // Never checked: five minutes after the start.
        assert_eq!(next_check(start, start, None).as_secs(), five);
        // Checked an hour ago: 23 hours from now.
        assert_eq!(
            next_check(start, start, Some(start - 3600)).as_secs(),
            day - 3600
        );
        // Checked two days ago: still not before five minutes.
        assert_eq!(
            next_check(start, start, Some(start - 2 * day)).as_secs(),
            five
        );
        // Long after the start and overdue: now.
        assert_eq!(
            next_check(start + day, start, Some(start - day)).as_secs(),
            0
        );
        // The clock went back a year: one interval at most.
        assert_eq!(
            next_check(start, start - day, Some(start + 365 * day)).as_secs(),
            day
        );
    }
}
