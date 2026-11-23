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
    /// A version that could not be installed here (KS805, KS806). The
    /// automatic install tries it again only after [`retry_wait`]; on
    /// request it can be installed at any time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failed: Option<String>,
    /// Seconds since 1970 of the last failed install of `failed`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failed_at: Option<u64>,
    /// How often `failed` could not be installed in a row.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub failures: u32,
}

fn is_zero(n: &u32) -> bool {
    *n == 0
}

const DAY: u64 = 24 * 60 * 60;

/// How long the automatic install leaves a version alone after it failed
/// `failures` times in a row (UP1): 1, 2, 4 and 7 days, then 7 days each
/// time.
pub fn retry_wait(failures: u32) -> Duration {
    let days = match failures {
        0 => 0,
        1 => 1,
        2 => 2,
        3 => 4,
        _ => 7,
    };
    Duration::from_secs(days * DAY)
}

impl Record {
    /// Whether `version` could not be installed here last time.
    pub fn failed_before(&self, version: &str) -> bool {
        self.failed.as_deref() == Some(version)
    }

    /// Whether the automatic install still leaves `version` alone at `now`
    /// (seconds since 1970) because it failed recently. A failure time in
    /// the future (clock set back) or a record without one does not hold it
    /// back.
    pub fn held_back(&self, version: &str, now: u64) -> bool {
        self.retry_at(version, now).is_some()
    }

    /// While [`Record::held_back`]: when the automatic install may try
    /// `version` again (seconds since 1970).
    pub fn retry_at(&self, version: &str, now: u64) -> Option<u64> {
        if !self.failed_before(version) {
            return None;
        }
        let at = self.failed_at.filter(|&at| at <= now)?;
        let until = at.saturating_add(retry_wait(self.failures).as_secs());
        (now < until).then_some(until)
    }
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
    // One temporary file per process: a check and an install may save at
    // the same time.
    let tmp = dir.join(format!("update.json.{}.tmp", std::process::id()));
    let text = serde_json::to_string_pretty(record).map_err(|e| io_error(&file, e))?;
    fs::create_dir_all(dir)
        .and_then(|()| fs::write(&tmp, text))
        .and_then(|()| fs::rename(&tmp, &file))
        .map_err(|e| {
            // Named after the process: nothing else would remove it.
            let _ = fs::remove_file(&tmp);
            io_error(&file, e)
        })
}

/// Remembers that `version` could not be installed here at `now` (UP1); the
/// automatic install waits [`retry_wait`] before it tries again. Returns
/// how often it failed in a row.
pub fn mark_failed(dir: &Path, version: &str, now: u64) -> Result<u32> {
    let (mut record, _) = load(dir);
    record.failures = if record.failed_before(version) {
        record.failures.saturating_add(1)
    } else {
        1
    };
    record.failed = Some(version.to_owned());
    record.failed_at = Some(now);
    save(dir, &record)?;
    Ok(record.failures)
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
            failed: None,
            failed_at: None,
            failures: 0,
        };
        save(dir.path(), &record).unwrap();
        assert_eq!(load(dir.path()), (record.clone(), None));
        // A failed version is added to what is there and counted.
        assert_eq!(mark_failed(dir.path(), "1.2.0", 100).unwrap(), 1);
        assert_eq!(mark_failed(dir.path(), "1.2.0", 200).unwrap(), 2);
        let (marked, _) = load(dir.path());
        assert!(marked.failed_before("1.2.0"));
        assert!(!marked.failed_before("1.2.1"));
        assert_eq!((marked.failed_at, marked.failures), (Some(200), 2));
        assert_eq!(marked.latest, record.latest);
        // Another version starts counting again.
        assert_eq!(mark_failed(dir.path(), "1.2.1", 300).unwrap(), 1);
        // A file written before the field existed still reads.
        fs::write(dir.path().join("update.json"), r#"{"latest":"1.0.0"}"#).unwrap();
        assert_eq!(load(dir.path()).0.failed, None);
        fs::write(dir.path().join("update.json"), "{ broken").unwrap();
        let (record, problem) = load(dir.path());
        assert_eq!(record, Record::default());
        assert_eq!(problem.unwrap().code, Code::UpdateRecord);
    }

    #[test]
    fn up1_a_failed_version_is_tried_again_after_a_growing_wait() {
        let day = 24 * 60 * 60;
        let waits: Vec<u64> = (1..=6).map(|n| retry_wait(n).as_secs() / day).collect();
        assert_eq!(waits, [1, 2, 4, 7, 7, 7]);
        let at = 1_000_000;
        let record = |failures| Record {
            failed: Some("1.2.0".into()),
            failed_at: Some(at),
            failures,
            ..Record::default()
        };
        // Once: held back for a day, not a second longer.
        assert!(record(1).held_back("1.2.0", at));
        assert!(record(1).held_back("1.2.0", at + day - 1));
        assert!(!record(1).held_back("1.2.0", at + day));
        // Three times: four days.
        assert!(record(3).held_back("1.2.0", at + 4 * day - 1));
        assert!(!record(3).held_back("1.2.0", at + 4 * day));
        // Another version is never held back.
        assert!(!record(1).held_back("1.2.1", at));
        // The clock went back: not held back.
        assert!(!record(1).held_back("1.2.0", at - 1));
        // A record from before the time was kept: not held back.
        let old = Record {
            failed: Some("1.2.0".into()),
            ..Record::default()
        };
        assert!(old.failed_before("1.2.0") && !old.held_back("1.2.0", at));
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
