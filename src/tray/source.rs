//! The configuration as the tray sees it (C3, C4): re-read lazily when a
//! switch or the menu needs it, never by a watcher (Q1). An invalid file
//! keeps the last valid configuration in effect and is reported.

use std::fs;
use std::path::PathBuf;
use std::time::SystemTime;

use log::{info, warn};

use crate::config::{self, Config, Settings};
use crate::error::Code;

/// File time and size, to skip re-parsing an unchanged file.
type Stamp = (Option<SystemTime>, u64);

#[derive(Debug, Clone)]
pub struct Source {
    path: Option<PathBuf>,
    stamp: Option<Stamp>,
    /// The last valid configuration; empty until one was read.
    config: Config,
    /// Why the current file is not in effect, if it is not.
    problem: Option<String>,
}

fn empty() -> Config {
    Config {
        settings: Settings::default(),
        profiles: Vec::new(),
    }
}

impl Source {
    /// `path` is `None` when there is no configuration directory at all.
    pub fn new(path: Option<PathBuf>) -> Self {
        Self {
            path,
            stamp: None,
            config: empty(),
            problem: None,
        }
    }

    /// Re-reads the file if it changed since the last call.
    pub fn refresh(&mut self) {
        let Some(path) = &self.path else {
            self.problem = Some("no configuration directory (APPDATA unset)".to_owned());
            return;
        };
        let stamp = match fs::metadata(path) {
            Ok(meta) => (meta.modified().ok(), meta.len()),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                // No file: no profiles yet. Not an error; neutral works.
                if self.stamp.is_some() || self.problem.is_some() {
                    info!("{} is gone; no profiles", path.display());
                }
                self.stamp = None;
                self.config = empty();
                self.problem = None;
                return;
            }
            Err(err) => {
                self.stamp = None;
                self.problem = Some(format!("{}: {}: {err}", Code::ConfigRead, path.display()));
                return;
            }
        };
        if self.stamp == Some(stamp) {
            return;
        }
        self.stamp = Some(stamp);
        match config::load(path) {
            Ok(loaded) => {
                for warning in &loaded.warnings {
                    warn!("{}: warning: {warning}", path.display());
                }
                info!(
                    "configuration loaded: {} profile(s)",
                    loaded.config.profiles.len()
                );
                self.config = loaded.config;
                self.problem = None;
            }
            Err(rejected) => {
                // C3: rejected as a whole; the previous one stays in effect.
                warn!(
                    "{}: {rejected}; keeping the last valid configuration",
                    path.display()
                );
                let first = rejected
                    .issues
                    .first()
                    .map_or(String::new(), |issue| format!(" {issue}"));
                self.problem = Some(format!("{}{first}", rejected.code));
            }
        }
    }

    pub fn config(&self) -> &Config {
        &self.config
    }

    pub fn problem(&self) -> Option<&str> {
        self.problem.as_deref()
    }

    pub fn profile_names(&self) -> Vec<String> {
        self.config
            .profiles
            .iter()
            .map(|p| p.name.clone())
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const VALID: &str = "[[profile]]\nname = \"focus\"\n";
    const OTHER: &str = "[[profile]]\nname = \"music\"\n\n[[profile]]\nname = \"games\"\n";

    fn write(path: &std::path::Path, text: &str) {
        fs::write(path, text).unwrap();
    }

    #[test]
    fn missing_file_means_no_profiles_and_no_error() {
        let dir = tempfile::tempdir().unwrap();
        let mut source = Source::new(Some(dir.path().join("config.toml")));
        source.refresh();
        assert!(source.profile_names().is_empty());
        assert_eq!(source.problem(), None);
    }

    #[test]
    fn c3_invalid_file_keeps_the_last_valid_configuration() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("config.toml");
        write(&file, VALID);
        let mut source = Source::new(Some(file.clone()));
        source.refresh();
        assert_eq!(source.profile_names(), vec!["focus"]);

        write(&file, "[[profile]]\nname = \"focus\"\ntypo = 1\n");
        source.refresh();
        assert_eq!(source.profile_names(), vec!["focus"], "old one stays");
        let problem = source.problem().unwrap();
        assert!(problem.starts_with("KS202"), "{problem}");
        assert!(problem.contains("line 3"), "{problem}");

        // C4: the fixed file is picked up on the next use.
        write(&file, OTHER);
        source.refresh();
        assert_eq!(source.profile_names(), vec!["music", "games"]);
        assert_eq!(source.problem(), None);
    }

    #[test]
    fn invalid_first_file_gives_no_profiles_and_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("config.toml");
        write(&file, "[[profile]]\nname = \"neutral\"\n");
        let mut source = Source::new(Some(file));
        source.refresh();
        assert!(source.profile_names().is_empty());
        assert!(source.problem().unwrap().starts_with("KS203"));
    }
}
