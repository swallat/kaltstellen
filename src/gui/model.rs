//! Logic of the main window that needs no screen: the running programs that
//! can be added, the checks before adding one, and the status line after a
//! switch.

use std::collections::BTreeMap;

use crate::exit;
use crate::process::ProcessInfo;
use crate::protection::{self, Protection};

/// A running program that can be added (M2).
#[derive(Debug, Clone, PartialEq)]
pub struct Candidate {
    pub exe: String,
    pub private_bytes: u64,
    pub protection: Protection,
}

/// Programs of `session`, one line per executable name with the memory of
/// all its processes, largest first.
pub fn candidates(processes: &[ProcessInfo], session: u32) -> Vec<Candidate> {
    let mut by_name: BTreeMap<String, (String, u64)> = BTreeMap::new();
    for p in processes {
        if p.session_id != session || p.name.is_empty() || p.pid == 0 {
            continue;
        }
        let entry = by_name
            .entry(p.name.to_lowercase())
            .or_insert_with(|| (p.name.clone(), 0));
        entry.1 += p.private_bytes;
    }
    let mut list: Vec<Candidate> = by_name
        .into_values()
        .map(|(exe, private_bytes)| Candidate {
            protection: protection::of(&exe),
            exe,
            private_bytes,
        })
        .collect();
    list.sort_by(|a, b| {
        b.private_bytes
            .cmp(&a.private_bytes)
            .then(a.exe.cmp(&b.exe))
    });
    list
}

/// What adding `exe` needs (M4, P6).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AddCheck {
    Allowed,
    /// Warn list: ask first; adding then sets `confirmed = true`.
    Confirm(String),
    Refused(String),
}

pub fn check_add(exe: &str) -> AddCheck {
    match protection::of(exe) {
        Protection::Hard => AddCheck::Refused(format!(
            "{exe} is a protected system program and is never touched"
        )),
        Protection::Warn(category) => AddCheck::Confirm(format!(
            "{exe} is a {}. Freezing or closing it can cut you off or break things. Add it anyway?",
            category.description()
        )),
        Protection::None => AddCheck::Allowed,
    }
}

/// The status line after `kaltstellen switch TARGET` ended with `code`.
pub fn applied(target: &str, code: u8, stderr: &str) -> String {
    let first_error = stderr
        .lines()
        .find(|l| !l.trim().is_empty() && !l.starts_with("waiting for"))
        .unwrap_or("")
        .trim_start_matches("error: ");
    match code {
        exit::OK => format!("Applied \"{target}\"."),
        exit::ERRORS => format!("Applied \"{target}\" with errors: {first_error}"),
        exit::CONFIG_INVALID => format!("Not applied, the configuration is invalid: {first_error}"),
        exit::SUPERSEDED => format!("Not applied: a later request replaced it ({first_error})"),
        _ => format!("Not applied: {first_error}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn process(pid: u32, name: &str, session: u32, bytes: u64) -> ProcessInfo {
        ProcessInfo {
            pid,
            parent_pid: 0,
            name: name.into(),
            session_id: session,
            created: 1,
            private_bytes: bytes,
        }
    }

    #[test]
    fn m2_candidates_group_by_program_in_this_session() {
        let list = candidates(
            &[
                process(10, "Browser.exe", 1, 100 << 20),
                process(11, "browser.exe", 1, 50 << 20),
                process(12, "explorer.exe", 1, 80 << 20),
                process(13, "other.exe", 2, 999 << 20),
                process(0, "", 1, 0),
            ],
            1,
        );
        let names: Vec<&str> = list.iter().map(|c| c.exe.as_str()).collect();
        assert_eq!(names, vec!["Browser.exe", "explorer.exe"]);
        assert_eq!(list[0].private_bytes, 150 << 20);
        assert_eq!(list[1].protection, Protection::Hard);
    }

    #[test]
    fn m4_hard_protected_refused_warn_list_asks() {
        assert!(matches!(check_add("csrss.exe"), AddCheck::Refused(_)));
        assert!(matches!(check_add("anydesk.exe"), AddCheck::Confirm(_)));
        assert_eq!(check_add("notepad.exe"), AddCheck::Allowed);
    }

    #[test]
    fn status_line_summarises_the_switch() {
        assert_eq!(applied("focus", 0, ""), "Applied \"focus\".");
        assert_eq!(
            applied("focus", 1, "error: KS110 x.exe: missing\n"),
            "Applied \"focus\" with errors: KS110 x.exe: missing"
        );
        assert!(applied("focus", 3, "error: KS402: superseded").contains("later request"));
        assert!(
            applied("x", 2, "c.toml:3: error KS202: bad")
                .starts_with("Not applied, the configuration")
        );
    }
}
