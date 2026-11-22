//! What the main window shows, computed without a screen: the controls and
//! their places, the rows of the entry table, the running programs that can
//! be added, and the text of the status line.

use std::collections::BTreeMap;

use crate::config::{Action, App, AppAction, OnTimeout, Profile};
use crate::exit;
use crate::process::ProcessInfo;
use crate::protection::{self, Protection};
use crate::win32::gui::{Control, Kind};

/// Control IDs; tests find the controls by them.
pub mod ids {
    pub const PROFILES: u16 = 101;
    pub const NAME: u16 = 102;
    pub const NEW: u16 = 103;
    pub const COPY: u16 = 104;
    pub const RENAME: u16 = 105;
    pub const DELETE: u16 = 106;
    pub const APPLY: u16 = 107;
    pub const ENTRIES: u16 = 201;
    pub const ACTION: u16 = 202;
    pub const SET_ACTION: u16 = 203;
    pub const REMOVE: u16 = 204;
    pub const ADD_FILE: u16 = 205;
    pub const RUNNING: u16 = 301;
    pub const ADD_RUNNING: u16 = 302;
    pub const REFRESH: u16 = 303;
    pub const UNASSIGNED: u16 = 401;
    pub const DISMISS: u16 = 402;
    pub const ADD_UNASSIGNED: u16 = 403;
    pub const STATUS: u16 = 501;
}

/// Client size of the window at 96 DPI.
pub const SIZE: (i32, i32) = (900, 640);

/// The actions in the order of the drop-down list.
pub const ACTIONS: [Action; 4] = [Action::Start, Action::Stop, Action::Suspend, Action::Ignore];
pub const ACTION_NAMES: [&str; 4] = ["start", "stop", "suspend", "ignore"];

fn control(id: u16, kind: Kind, text: &str, rect: (i32, i32, i32, i32)) -> Control {
    Control {
        id,
        kind,
        text: text.to_owned(),
        rect,
    }
}

fn columns(list: &[(&str, i32)]) -> Kind {
    Kind::Table(list.iter().map(|(t, w)| ((*t).to_owned(), *w)).collect())
}

/// The controls of the main window (spec section 8: profiles left, entries
/// right, adding programs and the status line below).
pub fn controls() -> Vec<Control> {
    use ids::*;
    vec![
        control(0, Kind::Label, "Profiles", (10, 10, 200, 18)),
        control(PROFILES, Kind::List, "", (10, 30, 200, 280)),
        control(0, Kind::Label, "Name", (10, 318, 200, 18)),
        control(NAME, Kind::Edit, "", (10, 338, 200, 24)),
        control(NEW, Kind::Button, "&New", (10, 370, 97, 28)),
        control(COPY, Kind::Button, "&Copy", (113, 370, 97, 28)),
        control(RENAME, Kind::Button, "&Rename", (10, 404, 97, 28)),
        control(DELETE, Kind::Button, "&Delete", (113, 404, 97, 28)),
        control(APPLY, Kind::Button, "&Apply profile", (10, 446, 200, 32)),
        control(
            0,
            Kind::Label,
            "Programs in the profile",
            (220, 10, 400, 18),
        ),
        control(
            ENTRIES,
            columns(&[("Program", 200), ("Action", 80), ("Options", 380)]),
            "",
            (220, 30, 670, 200),
        ),
        control(ACTION, Kind::Choice, "", (220, 238, 140, 24)),
        control(SET_ACTION, Kind::Button, "Set ac&tion", (366, 236, 120, 28)),
        control(REMOVE, Kind::Button, "Re&move", (492, 236, 120, 28)),
        control(
            ADD_FILE,
            Kind::Button,
            "Add &executable…",
            (618, 236, 160, 28),
        ),
        control(0, Kind::Label, "Running programs", (220, 276, 300, 18)),
        control(
            RUNNING,
            columns(&[("Program", 220), ("Memory", 90), ("Protection", 340)]),
            "",
            (220, 296, 670, 150),
        ),
        control(
            ADD_RUNNING,
            Kind::Button,
            "Add &to profile",
            (220, 452, 150, 28),
        ),
        control(REFRESH, Kind::Button, "Re&fresh", (376, 452, 110, 28)),
        control(
            0,
            Kind::Label,
            "Unassigned programs (N1)",
            (220, 490, 400, 18),
        ),
        control(
            UNASSIGNED,
            columns(&[("Program", 220), ("Seen", 90), ("Path", 340)]),
            "",
            (220, 510, 670, 80),
        ),
        control(
            ADD_UNASSIGNED,
            Kind::Button,
            "Add to profi&le",
            (10, 510, 200, 28),
        ),
        control(DISMISS, Kind::Button, "D&ismiss", (10, 544, 200, 28)),
        control(STATUS, Kind::Label, "", (10, 604, 880, 24)),
    ]
}

/// The entry table's row for one entry: program, action, options.
pub fn row(app: &App) -> Vec<String> {
    let mut options = Vec::new();
    if let Some(path) = &app.path {
        options.push(format!("path {}", path.display()));
    }
    let action = match &app.action {
        AppAction::Start { args, cwd } => {
            if !args.is_empty() {
                options.push(format!("args {}", args.join(" ")));
            }
            if let Some(cwd) = cwd {
                options.push(format!("in {}", cwd.display()));
            }
            "start"
        }
        AppAction::Stop {
            on_timeout,
            restart_on_neutral,
            args,
            cwd,
        } => {
            if *on_timeout == OnTimeout::Keep {
                options.push("keep on timeout".to_owned());
            }
            if *restart_on_neutral {
                options.push("restart on neutral".to_owned());
            }
            if !args.is_empty() {
                options.push(format!("args {}", args.join(" ")));
            }
            if let Some(cwd) = cwd {
                options.push(format!("in {}", cwd.display()));
            }
            "stop"
        }
        AppAction::Suspend { trim } => {
            options.push(if *trim { "trim" } else { "no trim" }.to_owned());
            "suspend"
        }
        AppAction::Ignore => "ignore",
    };
    if app.confirmed {
        options.push("confirmed".to_owned());
    }
    vec![app.exe.clone(), action.to_owned(), options.join(", ")]
}

pub fn rows(profile: &Profile) -> Vec<Vec<String>> {
    profile.apps.iter().map(row).collect()
}

pub fn action_index(app: &App) -> usize {
    match app.action {
        AppAction::Start { .. } => 0,
        AppAction::Stop { .. } => 1,
        AppAction::Suspend { .. } => 2,
        AppAction::Ignore => 3,
    }
}

/// A running program that can be added (M2).
#[derive(Debug, Clone, PartialEq)]
pub struct Candidate {
    pub exe: String,
    pub private_bytes: u64,
    pub protection: Protection,
}

impl Candidate {
    pub fn row(&self) -> Vec<String> {
        let protection = match self.protection {
            Protection::Hard => "protected system program; cannot be added".to_owned(),
            Protection::Warn(category) => format!("warn list: {}", category.description()),
            Protection::None => String::new(),
        };
        vec![
            self.exe.clone(),
            format!("{:.1} MB", self.private_bytes as f64 / 1_048_576.0),
            protection,
        ]
    }
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
        exit::UNKNOWN_PROFILE => format!("Not applied: {first_error}"),
        _ => format!("Not applied: {first_error}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn controls_have_unique_ids_and_fit_the_window() {
        let list = controls();
        let mut seen = std::collections::HashSet::new();
        for c in &list {
            if c.id != 0 {
                assert!(seen.insert(c.id), "duplicate id {}", c.id);
            }
            let (x, y, w, h) = c.rect;
            assert!(x + w <= SIZE.0 && y + h <= SIZE.1, "{c:?} outside");
        }
    }

    #[test]
    fn rows_describe_action_and_options() {
        let mut app = App::new(
            "chat.exe",
            Some(PathBuf::from("C:\\apps\\chat.exe")),
            AppAction::Stop {
                on_timeout: OnTimeout::Keep,
                restart_on_neutral: true,
                args: vec!["--min".into()],
                cwd: None,
            },
        );
        app.confirmed = true;
        assert_eq!(
            row(&app),
            vec![
                "chat.exe",
                "stop",
                "path C:\\apps\\chat.exe, keep on timeout, restart on neutral, args --min, confirmed"
            ]
        );
        assert_eq!(action_index(&app), 1);
    }

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
        assert_eq!(list[0].row()[1], "150.0 MB");
        assert_eq!(list[1].protection, Protection::Hard);
        assert!(list[1].row()[2].contains("cannot be added"));
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
