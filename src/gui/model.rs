//! Logic of the main window that needs no screen: the running programs that
//! can be added, the checks before adding one, and the status line after a
//! switch.

use std::collections::BTreeMap;
use std::path::Path;

use serde::Serialize;

use crate::exit;
use crate::process::ProcessInfo;
use crate::protection::{self, Protection};
use crate::tray::protocol::LastSwitch;

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

/// M2: checks a file dropped onto the window. The path comes from WebView2
/// (the file the user dropped), but is untrusted like every other input of
/// the page: it must be a full path to an existing `.exe` file. Returns the
/// executable name, or why the file is refused.
pub fn check_dropped(path: &Path, max_text: usize) -> Result<String, String> {
    let text = path.to_str().ok_or("a dropped name is not valid text")?;
    if text.len() > max_text || text.chars().any(char::is_control) {
        return Err("a dropped name is too long or has control characters".to_owned());
    }
    let name = path
        .file_name()
        .map_or_else(|| text.to_owned(), |n| n.to_string_lossy().into_owned());
    if !path.is_absolute() {
        return Err(format!(
            "\"{name}\" has no full path; drop it from Explorer"
        ));
    }
    if path.is_dir() {
        return Err(format!(
            "{name} is a folder; drop the program's .exe file instead"
        ));
    }
    if !path
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("exe"))
    {
        return Err(format!("{name} is not a program (.exe)"));
    }
    if !path.is_file() {
        return Err(format!("{name} cannot be found"));
    }
    Ok(name)
}

/// M5: a marker on one entry, for an error of the last switch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Failure {
    /// `KS110`.
    pub code: String,
    /// A few words for the marker: `start failed`.
    pub label: &'static str,
    /// The whole error as the tray menu shows it, for the tooltip.
    pub text: String,
}

/// The few words a marker shows for an error code (`docs/errors.md`).
pub fn failure_label(code: &str) -> &'static str {
    match code {
        "KS102" => "needs administrator",
        "KS103" => "protected process",
        "KS104" => "protected program",
        "KS105" => "freeze failed",
        "KS106" => "resume failed",
        "KS107" => "trim failed",
        "KS108" => "did not close",
        "KS109" => "could not be ended",
        "KS110" => "start failed",
        "KS111" => "start needs elevation",
        "KS112" => "minimise failed",
        "KS113" => "needs confirmation",
        _ => "failed",
    }
}

/// M5 `[assumed]`: the errors of the last switch that belong to the entry
/// `exe` (with its optional `path`) of `profile`. Only the profile that
/// switch made active gets markers. An error belongs to an entry of the
/// same executable name; where both the error and the entry name a path,
/// the paths must be the same too, since C2 lets two entries share a name
/// with different paths. Errors that name a child process or no program
/// stay in the status line only.
pub fn failures(
    last: Option<&LastSwitch>,
    profile: &str,
    exe: &str,
    path: Option<&Path>,
) -> Vec<Failure> {
    let Some(last) = last.filter(|l| l.profile.trim().eq_ignore_ascii_case(profile.trim())) else {
        return Vec::new();
    };
    last.errors
        .iter()
        .filter(|e| e.program.eq_ignore_ascii_case(exe))
        .filter(|e| match (path, e.path.as_deref()) {
            (Some(entry), Some(error)) => crate::process::same_path(entry, Path::new(error)),
            _ => true,
        })
        .map(|e| Failure {
            label: failure_label(&e.code),
            code: e.code.clone(),
            text: e.text.clone(),
        })
        .collect()
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
    fn m2_only_existing_exe_files_with_a_full_path_can_be_dropped() {
        let dir = tempfile::tempdir().unwrap();
        let exe = dir.path().join("Tool.EXE");
        std::fs::write(&exe, b"MZ").unwrap();
        let text = dir.path().join("notes.txt");
        std::fs::write(&text, b"x").unwrap();
        let folder = dir.path().join("folder.exe");
        std::fs::create_dir(&folder).unwrap();
        let link = dir.path().join("tool.lnk");
        std::fs::write(&link, b"L").unwrap();
        assert_eq!(check_dropped(&exe, 4096), Ok("Tool.EXE".to_owned()));
        let refused = |path: &Path| check_dropped(path, 4096).unwrap_err();
        assert!(
            refused(&text).contains("not a program"),
            "{}",
            refused(&text)
        );
        assert!(refused(&link).contains("not a program"));
        assert!(refused(&folder).contains("is a folder"));
        assert!(refused(&dir.path().join("gone.exe")).contains("cannot be found"));
        assert!(refused(Path::new("tool.exe")).contains("no full path"));
        assert!(refused(Path::new("")).contains("no full path"));
        let control = dir.path().join("a\u{7}b.exe");
        assert!(refused(&control).contains("control"));
        assert!(check_dropped(&exe, 10).is_err(), "too long");
    }

    #[test]
    fn m5_errors_are_matched_to_entries_by_name_and_path() {
        use crate::tray::protocol::ErrorLine;
        let line = |code: &str, program: &str, path: Option<&str>| ErrorLine {
            code: code.into(),
            program: program.into(),
            path: path.map(Into::into),
            text: format!("{code} {program}: boom"),
        };
        let last = LastSwitch {
            profile: "Focus".into(),
            errors: vec![
                line("KS110", "tool.exe", Some("C:\\A\\tool.exe")),
                line("KS105", "Browser.exe", None),
                line("KS999", "odd.exe", None),
            ],
        };
        let p = |s: &str| Some(Path::new(s).to_owned());
        let of = |profile: &str, exe: &str, path: Option<std::path::PathBuf>| {
            failures(Some(&last), profile, exe, path.as_deref())
        };
        // By name, any case; profile names compare like C8 says.
        let browser = of(" focus ", "browser.exe", None);
        assert_eq!(
            browser,
            vec![Failure {
                code: "KS105".into(),
                label: "freeze failed",
                text: "KS105 Browser.exe: boom".into(),
            }]
        );
        // An entry with a path that the error does not name: by name.
        assert_eq!(of("focus", "BROWSER.EXE", p("C:\\B\\browser.exe")).len(), 1);
        // Two entries of one name (C2): only the one with the failed path.
        let tool = of("focus", "tool.exe", p("c:/a/TOOL.exe"));
        assert_eq!(tool.len(), 1);
        assert_eq!(tool[0].label, "start failed");
        assert!(of("focus", "tool.exe", p("C:\\B\\tool.exe")).is_empty());
        // An entry without a path takes the error of its name.
        assert_eq!(of("focus", "tool.exe", None).len(), 1);
        // Unknown codes still get a marker; other names and profiles none.
        assert_eq!(of("focus", "odd.exe", None)[0].label, "failed");
        assert!(of("focus", "other.exe", None).is_empty());
        assert!(of("play", "browser.exe", None).is_empty());
        assert!(failures(None, "focus", "browser.exe", None).is_empty());
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
