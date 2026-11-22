//! Locations of kaltstellen's files.
//!
//! - configuration: `%APPDATA%\kaltstellen` (`config.toml`, `settings.json`)
//! - local data: `%LOCALAPPDATA%\kaltstellen` (`state.json`, `seen.json`, logs)
//!
//! Both can be redirected with `KALTSTELLEN_CONFIG_DIR` and
//! `KALTSTELLEN_DATA_DIR`, which the tests use to stay out of the real
//! profile.

use std::env;
use std::path::PathBuf;

const APP_DIR: &str = "kaltstellen";

/// Directory of `config.toml` and `settings.json`.
pub fn config_dir() -> Option<PathBuf> {
    dir("KALTSTELLEN_CONFIG_DIR", "APPDATA")
}

/// The configuration file `config.toml`.
pub fn config_file() -> Option<PathBuf> {
    config_dir().map(|dir| dir.join("config.toml"))
}

/// Directory of the state file, the seen list and the logs.
pub fn data_dir() -> Option<PathBuf> {
    dir("KALTSTELLEN_DATA_DIR", "LOCALAPPDATA")
}

fn dir(override_var: &str, base_var: &str) -> Option<PathBuf> {
    if let Some(dir) = env::var_os(override_var).filter(|v| !v.is_empty()) {
        return Some(PathBuf::from(dir));
    }
    env::var_os(base_var)
        .filter(|v| !v.is_empty())
        .map(|base| PathBuf::from(base).join(APP_DIR))
}
