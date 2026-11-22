//! Unassigned programs (N1, WP11) for the main window: a snapshot is taken
//! whenever the list is shown.

use log::warn;

use crate::config::Config;
use crate::paths;
pub use crate::unassigned::Unassigned;

pub fn current(config: &Config, profile: &str) -> Vec<Unassigned> {
    paths::data_dir()
        .map(|dir| crate::unassigned::refresh(&dir, config, profile))
        .unwrap_or_default()
}

pub fn dismiss(profile: &str, item: &Unassigned) {
    if let Some(dir) = paths::data_dir()
        && let Err(err) = crate::unassigned::dismiss(&dir, profile, &item.exe)
    {
        warn!("gui: {err}");
    }
}
