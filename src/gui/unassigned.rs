//! Unassigned programs (N1, WP11) as the main window lists them.
//! Placeholder until WP11: the list is empty.

use crate::config::Config;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unassigned {
    pub exe: String,
    pub path: Option<String>,
    pub new: bool,
}

impl Unassigned {
    pub fn row(&self) -> Vec<String> {
        vec![
            self.exe.clone(),
            if self.new { "new" } else { "" }.to_owned(),
            self.path.clone().unwrap_or_default(),
        ]
    }
}

pub fn current(_config: &Config, _profile: &str) -> Vec<Unassigned> {
    Vec::new()
}

pub fn dismiss(_profile: &str, _item: &Unassigned) {}
