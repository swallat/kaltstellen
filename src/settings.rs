//! `settings.json` in the configuration directory (S5): what the tray keeps
//! between sessions apart from the configuration itself: the profile that
//! was active when the last session ended, which the tray offers after
//! sign-in without applying it, the update mode (UP1) and whether automatic
//! triggers are paused (U4).

use std::fs;
use std::os::windows::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::error::{Code, Error, Result};

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Settings {
    /// Active profile when the last session ended (sign-out, shutdown).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_profile: Option<String>,
    /// UP1: `auto`, `manual` or `off` (an earlier `on` reads as `auto`);
    /// absent means the default (`auto`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub update: Option<String>,
    /// L8: the tray has asked once whether it should start with Windows.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub autostart_asked: bool,
    /// U4: `Automatic switching` (main window, `kaltstellen automation`) is
    /// off: all triggers are paused, also after a restart.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub triggers_paused: bool,
}

/// `settings.json` in the configuration directory `dir`.
pub fn file(dir: &Path) -> PathBuf {
    dir.join("settings.json")
}

fn io_error(file: &Path, err: impl std::fmt::Display) -> Error {
    Error::new(Code::SettingsIo, format!("{}: {err}", file.display()))
}

/// Reads the settings; a missing file is the default. A file that cannot be
/// read or parsed is reported and treated as default.
pub fn load(dir: &Path) -> (Settings, Option<Error>) {
    let file = file(dir);
    match fs::read_to_string(&file) {
        Ok(text) => match serde_json::from_str(&text) {
            Ok(settings) => (settings, None),
            Err(err) => (Settings::default(), Some(io_error(&file, err))),
        },
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => (Settings::default(), None),
        Err(err) => (Settings::default(), Some(io_error(&file, err))),
    }
}

/// Writes the settings atomically: a temporary file, then a rename over the
/// old file. Only [`edit`] calls it, under the edit lock, so one fixed
/// temporary name is enough and a crash leaves at most that one file, which
/// the next edit overwrites. It is removed when the write fails. A reader,
/// a virus scanner or an indexer that has a file open for a moment can make
/// the rename fail (access denied or a sharing violation); it is tried
/// again until `busy` has passed.
fn save(dir: &Path, settings: &Settings, busy: Duration) -> Result<()> {
    let file = file(dir);
    let tmp = dir.join("settings.json.tmp");
    let text = serde_json::to_string_pretty(settings).map_err(|e| io_error(&file, e))?;
    let result = fs::create_dir_all(dir)
        .and_then(|()| fs::write(&tmp, text))
        .and_then(|()| {
            let deadline = Instant::now() + busy;
            loop {
                match fs::rename(&tmp, &file) {
                    Err(err) if transient(&err) && Instant::now() < deadline => {
                        std::thread::sleep(Duration::from_millis(10));
                    }
                    other => break other,
                }
            }
        });
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result.map_err(|e| io_error(&file, e))
}

/// An error another process causes for a moment by holding a file open.
fn transient(err: &std::io::Error) -> bool {
    err.kind() == std::io::ErrorKind::PermissionDenied
        || err.raw_os_error() == Some(ERROR_SHARING_VIOLATION)
}

/// How long [`edit`] waits for another process's edit, and a rename for a
/// reader.
const BUSY: Duration = Duration::from_secs(2);
const ERROR_SHARING_VIOLATION: i32 = 32;

/// Held while one process reads, changes and writes the settings: a lock
/// file opened without sharing, so the tray, the main window and the
/// command line do not lose each other's changes. Closed (released) when
/// dropped; the file itself stays.
struct EditLock(#[allow(dead_code)] fs::File);

fn lock(dir: &Path) -> Result<EditLock> {
    let path = dir.join("settings.lock");
    fs::create_dir_all(dir).map_err(|e| io_error(&path, e))?;
    let deadline = Instant::now() + BUSY;
    loop {
        match fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(false)
            .share_mode(0)
            .open(&path)
        {
            Ok(file) => return Ok(EditLock(file)),
            Err(err)
                if err.raw_os_error() == Some(ERROR_SHARING_VIOLATION)
                    && Instant::now() < deadline =>
            {
                std::thread::sleep(Duration::from_millis(10));
            }
            Err(err) => return Err(io_error(&path, err)),
        }
    }
}

/// Reads, changes and writes the settings, keeping what `change` does not
/// touch, under the edit lock. A file that cannot be read is replaced (it
/// was reported).
pub fn edit(dir: &Path, change: impl FnOnce(&mut Settings)) -> Result<()> {
    let _lock = lock(dir)?;
    let (mut settings, _) = load(dir);
    change(&mut settings);
    save(dir, &settings, BUSY)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn s5_last_profile_round_trips_and_defaults() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(load(dir.path()), (Settings::default(), None));
        let settings = Settings {
            last_profile: Some("focus".into()),
            update: None,
            autostart_asked: true,
            triggers_paused: true,
        };
        save(&dir.path().join("nested"), &settings, BUSY).unwrap();
        assert_eq!(load(&dir.path().join("nested")), (settings, None));
        fs::write(dir.path().join("settings.json"), "{ broken").unwrap();
        let (settings, problem) = load(dir.path());
        assert_eq!(settings, Settings::default());
        assert_eq!(problem.unwrap().code, Code::SettingsIo);
    }

    #[test]
    fn up1_edit_keeps_the_other_keys() {
        let dir = tempfile::tempdir().unwrap();
        edit(dir.path(), |s| s.last_profile = Some("play".into())).unwrap();
        edit(dir.path(), |s| s.update = Some("off".into())).unwrap();
        edit(dir.path(), |s| s.last_profile = None).unwrap();
        let (settings, _) = load(dir.path());
        assert_eq!(settings.update.as_deref(), Some("off"));
        assert_eq!(settings.last_profile, None);
    }

    #[test]
    fn edits_at_the_same_time_lose_nothing_and_leave_no_temporary_file() {
        let dir = tempfile::tempdir().unwrap();
        let threads: Vec<_> = (0..8)
            .map(|_| {
                let dir = dir.path().to_owned();
                std::thread::spawn(move || {
                    for _ in 0..10 {
                        edit(&dir, |s| {
                            let mut text = s.last_profile.take().unwrap_or_default();
                            text.push('x');
                            s.last_profile = Some(text);
                        })
                        .unwrap();
                    }
                })
            })
            .collect();
        for thread in threads {
            thread.join().unwrap();
        }
        let (settings, problem) = load(dir.path());
        assert_eq!(problem, None);
        assert_eq!(settings.last_profile.map(|p| p.len()), Some(80));
        let left: Vec<_> = fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|name| name.ends_with(".tmp"))
            .collect();
        assert!(left.is_empty(), "{left:?}");
    }

    #[test]
    fn a_failed_save_removes_its_temporary_file() {
        let dir = tempfile::tempdir().unwrap();
        // A folder where the file should be: the rename fails.
        fs::create_dir(dir.path().join("settings.json")).unwrap();
        assert!(save(dir.path(), &Settings::default(), Duration::ZERO).is_err());
        let names: Vec<_> = fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, ["settings.json"]);
    }
}
