//! H1: the programs of the current state, in the three groups the dialog
//! `From the current state…` offers: running now, held by kaltstellen and
//! seen earlier. Reads only: no process is touched, `seen.json` is not
//! written.
//!
//! The process list has no paths (A8), so each program that is listed is
//! opened with `PROCESS_QUERY_LIMITED_INFORMATION` to read its image path
//! (the fallback of A7) and the handle is closed at once. A program that
//! cannot be opened is of higher integrity (P7) and is listed without a
//! path.

use std::collections::{BTreeMap, HashSet};

use crate::config::{Action, App};
use crate::error::Code;
use crate::kinds::{self, Kind, Source};
use crate::process::{self, Access, Snapshot, Target, WindowInfo};
use crate::protection::{self, Protection};
use crate::state::State;
use crate::unassigned::{NOTIFY_ICONS_KEY, Seen};

/// At most this many programs of the third group.
pub const MAX_SEEN: usize = 100;

/// A program that runs now, as read from the system.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Running {
    pub exe: String,
    pub path: Option<String>,
    /// Could not be opened: higher integrity (P7).
    pub elevated: bool,
    pub private_bytes: u64,
    /// No visible window and no notification-area icon (the scope of N2).
    pub background: bool,
    /// What its visible windows say (H3).
    pub windows: WindowInfo,
}

/// A program the state file records as frozen or stopped (S6).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Held {
    pub exe: String,
    pub path: Option<String>,
    pub action: Action,
}

/// One row of the dialog.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    pub exe: String,
    pub path: Option<String>,
    pub private_bytes: u64,
    pub protection: Protection,
    pub elevated: bool,
    pub background: bool,
    /// Group 2: the action that holds the program.
    pub held: Option<Action>,
    /// Group 3: seconds since 1970 when it was first seen.
    pub first_seen: Option<u64>,
    /// H3: the likely kind, a guess, and where it came from.
    pub kind: Kind,
    pub source: Source,
    /// H3: what its windows say (running programs).
    pub windows: WindowInfo,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Groups {
    pub running: Vec<Row>,
    pub held: Vec<Row>,
    pub seen: Vec<Row>,
    /// Update mode: programs left out because the profile has an entry.
    pub existing: usize,
}

fn guess_of(exe: &str, path: Option<&str>) -> (Kind, Source) {
    let guess = kinds::classify(exe, path, None);
    (guess.kind, guess.source)
}

/// The state's records as held programs, one per name: frozen wins over
/// stopped.
pub fn held_of(state: &State) -> Vec<Held> {
    let mut by_name: BTreeMap<String, Held> = BTreeMap::new();
    for record in &state.stopped {
        by_name.insert(
            record.exe.to_lowercase(),
            Held {
                exe: record.exe.clone(),
                path: record.image.clone().filter(|p| !p.is_empty()),
                action: Action::Stop,
            },
        );
    }
    for record in &state.suspended {
        let path = Some(record.image.clone())
            .filter(|p| !p.is_empty())
            .or_else(|| record.path.clone());
        by_name.insert(
            record.exe.to_lowercase(),
            Held {
                exe: record.exe.clone(),
                path,
                action: Action::Suspend,
            },
        );
    }
    by_name.into_values().collect()
}

/// Builds the three groups. `entries` are the entries of the profile that is
/// updated (none when a profile is made): programs with an entry are left
/// out and counted.
pub fn assemble(running: &[Running], held: &[Held], seen: &Seen, entries: &[App]) -> Groups {
    let merged = merge_unreadable(running);
    let running = merged.as_slice();
    let decided: HashSet<String> = entries.iter().map(|a| a.exe.to_lowercase()).collect();
    let mut groups = Groups::default();
    let mut counted: HashSet<String> = HashSet::new();
    let mut skip = |exe: &str, groups: &mut Groups| -> bool {
        let key = exe.to_lowercase();
        if protection::is_hard_protected(exe) {
            return true;
        }
        if decided.contains(&key) {
            if counted.insert(key) {
                groups.existing += 1;
            }
            return true;
        }
        false
    };

    let held_names: HashSet<String> = held.iter().map(|h| h.exe.to_lowercase()).collect();
    let mut running_names: HashSet<String> = HashSet::new();
    for program in running {
        running_names.insert(program.exe.to_lowercase());
        if held_names.contains(&program.exe.to_lowercase()) || skip(&program.exe, &mut groups) {
            continue;
        }
        groups.running.push(Row {
            exe: program.exe.clone(),
            path: program.path.clone(),
            private_bytes: program.private_bytes,
            protection: protection::of(&program.exe),
            elevated: program.elevated,
            background: program.background,
            held: None,
            first_seen: None,
            kind: guess_of(&program.exe, program.path.as_deref()).0,
            source: guess_of(&program.exe, program.path.as_deref()).1,
            windows: program.windows,
        });
    }
    groups.running.sort_by(|a, b| {
        a.background
            .cmp(&b.background)
            .then(b.private_bytes.cmp(&a.private_bytes))
            .then(a.exe.to_lowercase().cmp(&b.exe.to_lowercase()))
    });

    for program in held {
        if skip(&program.exe, &mut groups) {
            continue;
        }
        groups.held.push(Row {
            exe: program.exe.clone(),
            path: program.path.clone(),
            private_bytes: 0,
            protection: protection::of(&program.exe),
            elevated: false,
            background: false,
            held: Some(program.action),
            first_seen: None,
            kind: guess_of(&program.exe, program.path.as_deref()).0,
            source: guess_of(&program.exe, program.path.as_deref()).1,
            windows: WindowInfo::default(),
        });
    }
    groups.held.sort_by_key(|r| r.exe.to_lowercase());

    let mut listed: HashSet<String> = HashSet::new();
    let mut earlier: Vec<&crate::unassigned::SeenProgram> = seen.programs.iter().collect();
    earlier.sort_by(|a, b| {
        b.first_seen
            .cmp(&a.first_seen)
            .then(a.exe.to_lowercase().cmp(&b.exe.to_lowercase()))
    });
    for program in earlier {
        let key = program.exe.to_lowercase();
        if running_names.contains(&key) || held_names.contains(&key) || !listed.insert(key) {
            continue;
        }
        if skip(&program.exe, &mut groups) {
            continue;
        }
        if groups.seen.len() >= MAX_SEEN {
            break;
        }
        groups.seen.push(Row {
            exe: program.exe.clone(),
            path: program.path.clone(),
            private_bytes: 0,
            protection: protection::of(&program.exe),
            elevated: false,
            background: false,
            held: None,
            first_seen: Some(program.first_seen),
            kind: guess_of(&program.exe, program.path.as_deref()).0,
            source: guess_of(&program.exe, program.path.as_deref()).1,
            windows: WindowInfo::default(),
        });
    }
    groups
}

/// A process whose image path cannot be read (an elevated service) and a
/// process of the same name with a path are one program for an entry: C2
/// refuses two entries of a name when one has no path, so the unreadable
/// one is folded into the one with a path (memory and windows added). Rows
/// of one name that all have paths stay apart (several unreadable ones are
/// one row already).
fn merge_unreadable(running: &[Running]) -> Vec<Running> {
    let mut out: Vec<Running> = running
        .iter()
        .filter(|r| r.path.is_some())
        .cloned()
        .collect();
    for row in running.iter().filter(|r| r.path.is_none()) {
        let target = out
            .iter_mut()
            .filter(|o| o.exe.eq_ignore_ascii_case(&row.exe))
            .max_by_key(|o| o.private_bytes);
        match target {
            Some(target) => {
                target.private_bytes += row.private_bytes;
                target.background &= row.background;
                target.windows.windows += row.windows.windows;
                target.windows.dialog |= row.windows.dialog;
                target.windows.unsaved |= row.windows.unsaved;
                target.windows.hung |= row.windows.hung;
            }
            None => out.push(row.clone()),
        }
    }
    out
}

/// Reads the programs of this session that run now: one per name and image
/// folder (C2), their private memory summed, with the path read as the
/// module header says. kaltstellen itself and hard-protected programs are
/// left out (they are never listed, P6).
pub fn running_now() -> Vec<Running> {
    let Ok(snapshot) = Snapshot::take() else {
        return Vec::new();
    };
    // Without its own session nothing is known to belong to the user.
    let Some(session) = snapshot.own_session() else {
        return Vec::new();
    };
    let own = std::env::current_exe()
        .ok()
        .and_then(|p| p.file_name().map(|n| n.to_string_lossy().to_lowercase()));
    let names: HashSet<String> = snapshot
        .processes()
        .iter()
        .filter(|p| p.session_id == session)
        .map(|p| p.name.to_lowercase())
        .collect();
    let icons: HashSet<String> = crate::unassigned::icon_programs(NOTIFY_ICONS_KEY, &names)
        .into_iter()
        .map(|o| o.exe.to_lowercase())
        .collect();
    let visible = process::pids_with_visible_windows();
    let windows = process::window_infos();
    // (name, folder-path) -> row; a program that cannot be opened has no
    // path and is one row per name.
    let mut rows: BTreeMap<(String, Option<String>), Running> = BTreeMap::new();
    for p in snapshot.processes() {
        let key = p.name.to_lowercase();
        if p.session_id != session
            || p.pid == 0
            || p.name.is_empty()
            || own.as_deref() == Some(key.as_str())
            || protection::is_hard_protected(&p.name)
        {
            continue;
        }
        let (path, elevated) = match Target::open(p, Access::Query) {
            Ok(Some(target)) => (
                target
                    .image_path()
                    .ok()
                    .map(|p| p.to_string_lossy().into_owned()),
                false,
            ),
            Ok(None) => continue,
            Err(err) if matches!(err.code, Code::NeedsAdmin | Code::Protected) => (None, true),
            // Any other failure says nothing about the program: not listed.
            Err(_) => continue,
        };
        let shown = visible.contains(&p.pid) || icons.contains(&key);
        let row = rows
            .entry((key, path.clone().map(|p| p.to_lowercase())))
            .or_insert_with(|| Running {
                exe: p.name.clone(),
                path,
                elevated,
                private_bytes: 0,
                background: true,
                windows: WindowInfo::default(),
            });
        if let Some(info) = windows.get(&p.pid) {
            row.windows.windows += info.windows;
            row.windows.dialog |= info.dialog;
            row.windows.unsaved |= info.unsaved;
            row.windows.hung |= info.hung;
        }
        row.private_bytes += p.private_bytes;
        row.background &= !shown;
    }
    rows.into_values().collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::{Stopped, Suspended};
    use crate::unassigned::SeenProgram;
    use std::path::PathBuf;

    fn running(exe: &str, bytes: u64, background: bool) -> Running {
        Running {
            exe: exe.to_owned(),
            path: Some(format!("C:\\apps\\{exe}")),
            elevated: false,
            private_bytes: bytes,
            background,
            windows: WindowInfo::default(),
        }
    }

    fn seen_with(programs: &[(&str, u64)]) -> Seen {
        Seen {
            programs: programs
                .iter()
                .map(|(exe, first_seen)| SeenProgram {
                    exe: (*exe).to_owned(),
                    path: None,
                    first_seen: *first_seen,
                })
                .collect(),
            ..Seen::default()
        }
    }

    #[test]
    fn h1_an_unreadable_process_joins_the_one_of_its_name_that_has_a_path() {
        let mut service = running("parsecd.exe", 7, true);
        service.path = None;
        service.elevated = true;
        let groups = assemble(
            &[service.clone(), running("parsecd.exe", 30, false), service],
            &[],
            &Seen::default(),
            &[],
        );
        // One row, so the profile gets one entry (C2), memory added.
        assert_eq!(groups.running.len(), 1);
        assert!(groups.running[0].path.is_some());
        assert_eq!(groups.running[0].private_bytes, 44);
        assert!(!groups.running[0].background);
        // Without a program that has a path the unreadable one stays.
        let mut alone = running("nvcontainer.exe", 5, true);
        alone.path = None;
        let groups = assemble(&[alone], &[], &Seen::default(), &[]);
        assert_eq!(groups.running.len(), 1);
        assert!(groups.running[0].path.is_none());
    }

    #[test]
    fn h1_groups_running_held_and_seen() {
        let held = vec![Held {
            exe: "chat.exe".into(),
            path: None,
            action: Action::Suspend,
        }];
        let groups = assemble(
            &[
                running("sync.exe", 10, true),
                running("browser.exe", 500, false),
                running("chat.exe", 50, false),
                running("mail.exe", 20, false),
            ],
            &held,
            &seen_with(&[("old.exe", 5), ("mail.exe", 9), ("newer.exe", 8)]),
            &[],
        );
        let names = |rows: &[Row]| rows.iter().map(|r| r.exe.clone()).collect::<Vec<_>>();
        // Windows first, largest first; background programs last.
        assert_eq!(
            names(&groups.running),
            ["browser.exe", "mail.exe", "sync.exe"]
        );
        // chat.exe is group 2 only.
        assert_eq!(names(&groups.held), ["chat.exe"]);
        assert_eq!(groups.held[0].held, Some(Action::Suspend));
        // Running programs are not "seen earlier"; newest first.
        assert_eq!(names(&groups.seen), ["newer.exe", "old.exe"]);
        assert_eq!(groups.seen[0].first_seen, Some(8));
    }

    #[test]
    fn h1_hard_protected_programs_are_never_listed() {
        let groups = assemble(
            &[running("csrss.exe", 1, false), running("a.exe", 1, false)],
            &[],
            &seen_with(&[("winlogon.exe", 1)]),
            &[],
        );
        assert_eq!(groups.running.len(), 1);
        assert!(groups.seen.is_empty());
    }

    #[test]
    fn h1_update_mode_leaves_out_programs_that_have_an_entry() {
        let entries = vec![App::new(
            "BROWSER.exe",
            None::<PathBuf>,
            crate::config::AppAction::Ignore,
        )];
        let groups = assemble(
            &[
                running("browser.exe", 1, false),
                running("mail.exe", 1, false),
            ],
            &[],
            &seen_with(&[("browser.exe", 1)]),
            &entries,
        );
        assert_eq!(groups.running.len(), 1);
        assert_eq!(groups.running[0].exe, "mail.exe");
        assert_eq!(groups.existing, 1);
    }

    #[test]
    fn h1_seen_group_is_capped() {
        let many: Vec<(String, u64)> = (0..150).map(|i| (format!("p{i}.exe"), i)).collect();
        let refs: Vec<(&str, u64)> = many.iter().map(|(n, t)| (n.as_str(), *t)).collect();
        let groups = assemble(&[], &[], &seen_with(&refs), &[]);
        assert_eq!(groups.seen.len(), MAX_SEEN);
        assert_eq!(groups.seen[0].exe, "p149.exe");
    }

    #[test]
    fn h1_frozen_records_win_over_stopped_ones() {
        let state = State {
            suspended: vec![Suspended {
                pid: 1,
                created: 1,
                image: "C:\\x\\tool.exe".into(),
                exe: "tool.exe".into(),
                path: None,
            }],
            stopped: vec![
                Stopped {
                    exe: "Tool.exe".into(),
                    image: None,
                    restart_on_neutral: false,
                    args: Vec::new(),
                    cwd: None,
                },
                Stopped {
                    exe: "gone.exe".into(),
                    image: Some("C:\\x\\gone.exe".into()),
                    restart_on_neutral: false,
                    args: Vec::new(),
                    cwd: None,
                },
            ],
            ..State::default()
        };
        let held = held_of(&state);
        assert_eq!(held.len(), 2);
        let tool = held.iter().find(|h| h.exe == "tool.exe").unwrap();
        assert_eq!(tool.action, Action::Suspend);
        assert_eq!(tool.path.as_deref(), Some("C:\\x\\tool.exe"));
        let gone = held.iter().find(|h| h.exe == "gone.exe").unwrap();
        assert_eq!(gone.action, Action::Stop);
    }
}
