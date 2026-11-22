//! `settings.json` in the configuration directory (S5): what the tray keeps
//! between sessions apart from the configuration itself: the profile that
//! was active when the last session ended, which the tray offers after
//! sign-in without applying it, and the update mode (UP1).

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{Code, Error, Result};

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Settings {
    /// Active profile when the last session ended (sign-out, shutdown).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_profile: Option<String>,
    /// UP1: `on` or `off`; absent means the default (`on`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub update: Option<String>,
}

fn file(dir: &Path) -> PathBuf {
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

/// Writes the settings atomically (temporary file, then rename).
pub fn save(dir: &Path, settings: &Settings) -> Result<()> {
    let file = file(dir);
    let tmp = dir.join("settings.json.tmp");
    let text = serde_json::to_string_pretty(settings).map_err(|e| io_error(&file, e))?;
    fs::create_dir_all(dir)
        .and_then(|()| fs::write(&tmp, text))
        .and_then(|()| fs::rename(&tmp, &file))
        .map_err(|e| io_error(&file, e))
}

/// Reads, changes and writes the settings, keeping what `change` does not
/// touch. A file that cannot be read is replaced (it was reported).
pub fn edit(dir: &Path, change: impl FnOnce(&mut Settings)) -> Result<()> {
    let (mut settings, _) = load(dir);
    change(&mut settings);
    save(dir, &settings)
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
        };
        save(&dir.path().join("nested"), &settings).unwrap();
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
}
