//! State file `state.json`: what kaltstellen suspended and stopped, written
//! atomically (S1, S6, S9, S10), plus the lock that keeps two in-process
//! switches apart (L1).

use std::fs::{self, File, OpenOptions};
use std::os::windows::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::config::NEUTRAL;
use crate::error::{Code, Error, Result};

const ERROR_SHARING_VIOLATION: i32 = 32;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct State {
    #[serde(default = "neutral")]
    pub active_profile: String,
    /// Boot marker of the Windows session the records belong to (S10).
    #[serde(default)]
    pub boot: u64,
    #[serde(default)]
    pub suspended: Vec<Suspended>,
    #[serde(default)]
    pub stopped: Vec<Stopped>,
}

fn neutral() -> String {
    NEUTRAL.to_owned()
}

impl Default for State {
    fn default() -> Self {
        Self {
            active_profile: neutral(),
            boot: 0,
            suspended: Vec::new(),
            stopped: Vec::new(),
        }
    }
}

/// One process kaltstellen froze.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Suspended {
    pub pid: u32,
    /// Creation time; with the PID it identifies the process (S6).
    pub created: u64,
    /// Full image path, checked again before resuming (S10).
    pub image: String,
    /// The profile entry the process was frozen for.
    pub exe: String,
    pub path: Option<String>,
}

/// One program kaltstellen closed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Stopped {
    pub exe: String,
    /// Image path of the program, used to start it again.
    pub image: Option<String>,
    pub restart_on_neutral: bool,
}

/// The state file in kaltstellen's data directory.
#[derive(Debug, Clone)]
pub struct Store {
    dir: PathBuf,
}

impl Store {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    pub fn file(&self) -> PathBuf {
        self.dir.join("state.json")
    }

    /// Reads the state. A missing file is an empty state. A corrupt file is
    /// renamed to `state.json.bad` and treated as empty (S9); that, and a
    /// file that cannot be read, is reported in the second value.
    pub fn load(&self) -> (State, Option<Error>) {
        let file = self.file();
        let text = match fs::read_to_string(&file) {
            Ok(text) => text,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                return (State::default(), None);
            }
            Err(err) => {
                return (State::default(), Some(io_error(&file, &err)));
            }
        };
        match serde_json::from_str(&text) {
            Ok(state) => (state, None),
            Err(err) => {
                let bad = self.dir.join("state.json.bad");
                let renamed = fs::rename(&file, &bad);
                let reason = match renamed {
                    Ok(()) => format!("state file was corrupt ({err}); kept as {}", bad.display()),
                    Err(rename) => {
                        format!("state file is corrupt ({err}) and cannot be renamed: {rename}")
                    }
                };
                (
                    State::default(),
                    Some(Error::new(Code::StateCorrupt, reason)),
                )
            }
        }
    }

    /// Reads the state for display only: no renaming of a corrupt file, no
    /// error. `None` if there is no readable, valid file.
    pub fn peek(&self) -> Option<State> {
        serde_json::from_str(&fs::read_to_string(self.file()).ok()?).ok()
    }

    /// Writes the state atomically: temporary file, then rename.
    pub fn save(&self, state: &State) -> Result<()> {
        let file = self.file();
        let tmp = self.dir.join("state.json.tmp");
        let text = serde_json::to_string_pretty(state)
            .map_err(|err| Error::new(Code::StateIo, err.to_string()))?;
        fs::create_dir_all(&self.dir)
            .and_then(|()| fs::write(&tmp, text))
            .and_then(|()| fs::rename(&tmp, &file))
            .map_err(|err| io_error(&file, &err))
    }

    /// Takes the exclusive lock for an in-process switch. Fails with
    /// `StateLocked` while another kaltstellen process holds it. The lock is
    /// released when the value is dropped, also if the process dies.
    pub fn lock(&self) -> Result<Lock> {
        let file = self.dir.join("state.lock");
        fs::create_dir_all(&self.dir).map_err(|err| io_error(&file, &err))?;
        OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(false)
            .share_mode(0)
            .open(&file)
            .map(Lock)
            .map_err(|err| {
                if err.raw_os_error() == Some(ERROR_SHARING_VIOLATION) {
                    Error::new(Code::StateLocked, "another kaltstellen switch is running")
                } else {
                    io_error(&file, &err)
                }
            })
    }
}

/// Held for the duration of an in-process switch.
#[derive(Debug)]
pub struct Lock(#[allow(dead_code)] File);

fn io_error(file: &Path, err: &std::io::Error) -> Error {
    Error::new(Code::StateIo, format!("{}: {err}", file.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> State {
        State {
            active_profile: "focus".into(),
            boot: 133_000_000_000_000_001,
            suspended: vec![Suspended {
                pid: 4242,
                created: 133_000_000_123_456_789,
                image: "C:\\apps\\browser.exe".into(),
                exe: "browser.exe".into(),
                path: None,
            }],
            stopped: vec![Stopped {
                exe: "chat.exe".into(),
                image: Some("C:\\apps\\chat.exe".into()),
                restart_on_neutral: true,
            }],
        }
    }

    #[test]
    fn missing_file_is_an_empty_neutral_state() {
        let dir = tempfile::tempdir().unwrap();
        let (state, problem) = Store::new(dir.path()).load();
        assert_eq!(state, State::default());
        assert_eq!(state.active_profile, "neutral");
        assert!(problem.is_none());
    }

    #[test]
    fn save_and_load_round_trip_exactly() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path().join("nested"));
        store.save(&sample()).unwrap();
        store.save(&sample()).unwrap();
        assert_eq!(store.load(), (sample(), None));
        assert!(!dir.path().join("nested").join("state.json.tmp").exists());
    }

    #[test]
    fn s9_corrupt_file_is_renamed_and_treated_as_empty() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path());
        fs::write(store.file(), "{ not json").unwrap();
        let (state, problem) = store.load();
        assert_eq!(state, State::default());
        assert_eq!(problem.unwrap().code, Code::StateCorrupt);
        assert!(!store.file().exists());
        assert_eq!(
            fs::read_to_string(dir.path().join("state.json.bad")).unwrap(),
            "{ not json"
        );
        // The next start is clean.
        assert_eq!(store.load(), (State::default(), None));
    }

    #[test]
    fn lock_is_exclusive_until_dropped() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path());
        let first = store.lock().unwrap();
        assert_eq!(store.lock().unwrap_err().code, Code::StateLocked);
        drop(first);
        store.lock().unwrap();
    }
}
