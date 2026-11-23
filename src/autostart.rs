//! Start at sign-in (plan WP7, opt-in): a value in the per-user Run key that
//! starts `kaltstellen tray`. Switched on and off with `kaltstellen
//! autostart on|off` or from the main window (`Start at sign-in`).

use std::path::Path;

use crate::error::Result;
use crate::win32::registry;

/// The per-user Run key under `HKEY_CURRENT_USER`.
pub const RUN_KEY: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Run";
pub const VALUE: &str = "kaltstellen";

/// Where the value lives. `KALTSTELLEN_RUN_KEY` replaces the key (below
/// `HKEY_CURRENT_USER`), so tests never touch the real Run key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Location {
    pub key: String,
    pub value: String,
}

impl Location {
    pub fn current() -> Self {
        let key = std::env::var("KALTSTELLEN_RUN_KEY")
            .ok()
            .filter(|k| !k.is_empty())
            .unwrap_or_else(|| RUN_KEY.to_owned());
        Self {
            key,
            value: VALUE.to_owned(),
        }
    }
}

/// The command the Run key gets: this executable, quoted, with `tray`.
pub fn command_line(exe: &Path) -> String {
    format!("\"{}\" tray", exe.display())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum State {
    Off,
    /// The value starts this executable's tray.
    On,
    /// The value exists but starts something else (another copy, edited).
    Other(String),
}

pub fn state(at: &Location, exe: &Path) -> Result<State> {
    Ok(match registry::read(&at.key, &at.value)? {
        None => State::Off,
        Some(command) if command.eq_ignore_ascii_case(&command_line(exe)) => State::On,
        Some(command) => State::Other(command),
    })
}

/// Points the value at `exe` (replacing one that points elsewhere).
pub fn enable(at: &Location, exe: &Path) -> Result<()> {
    registry::write(&at.key, &at.value, &command_line(exe))
}

/// Removes the value; true if it was there.
pub fn disable(at: &Location) -> Result<bool> {
    registry::delete(&at.key, &at.value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn run_key_value_points_at_this_executable() {
        let root = format!(
            "Software\\kaltstellen-test-{}-autostart",
            std::process::id()
        );
        let at = Location {
            key: format!("{root}\\Run"),
            value: VALUE.to_owned(),
        };
        let exe = Path::new("C:\\Program Files\\kaltstellen\\kaltstellen.exe");
        assert_eq!(
            command_line(exe),
            "\"C:\\Program Files\\kaltstellen\\kaltstellen.exe\" tray"
        );
        assert_eq!(state(&at, exe).unwrap(), State::Off);
        enable(&at, exe).unwrap();
        assert_eq!(state(&at, exe).unwrap(), State::On);
        let other = Path::new("C:\\elsewhere\\kaltstellen.exe");
        assert!(matches!(state(&at, other).unwrap(), State::Other(_)));
        assert!(disable(&at).unwrap());
        assert_eq!(state(&at, exe).unwrap(), State::Off);
        registry::delete_tree(&root).unwrap();
        assert!(!registry::exists(&root).unwrap());
    }

    #[test]
    fn the_real_run_key_is_the_default() {
        // Reads only: the environment of the test run decides the key.
        if std::env::var_os("KALTSTELLEN_RUN_KEY").is_none() {
            assert_eq!(Location::current().key, RUN_KEY);
        }
    }
}
