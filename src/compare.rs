//! Compare two profiles (spec H6): which programs only one has, and where
//! both have an entry but act differently. Pure logic over the parsed
//! configuration, shared by `kaltstellen compare` and the main window.

use std::path::PathBuf;

use crate::config::{App, AppAction, OnTimeout, Profile};
use crate::process::same_path;

/// How a program differs between profile A and profile B.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Difference {
    OnlyA,
    OnlyB,
    /// Both have an entry, with different actions.
    Action,
    /// The same action, different options (`on_timeout`, `trim`, ...).
    Options,
}

/// One program, identified as in A7 and C2: by name, and by path where both
/// entries have one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    pub exe: String,
    pub path: Option<PathBuf>,
    /// The entry in A and in B as text (`suspend · no trim`), `None` where
    /// the profile has no entry.
    pub a: Option<String>,
    pub b: Option<String>,
    /// `None` when both entries are the same.
    pub difference: Option<Difference>,
}

/// Hotkey, triggers and priority, which are shown apart from the rows. A
/// field is `None` where both profiles agree.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Meta {
    pub hotkey: Option<(String, String)>,
    pub triggers: Option<(String, String)>,
    pub priority: Option<(i32, i32)>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Comparison {
    pub rows: Vec<Row>,
    pub meta: Meta,
}

impl Comparison {
    /// How many rows have this difference; `None` counts the identical ones.
    pub fn count(&self, difference: Option<Difference>) -> usize {
        self.rows
            .iter()
            .filter(|row| row.difference == difference)
            .count()
    }

    /// Lines for the command line: one per difference, then the meta lines
    /// and a summary. Identical programs are only counted.
    pub fn lines(&self, a: &str, b: &str) -> Vec<String> {
        let mut lines = Vec::new();
        for row in &self.rows {
            let name = match &row.path {
                Some(path) => format!("{} ({})", row.exe, path.display()),
                None => row.exe.clone(),
            };
            let text = |entry: &Option<String>| entry.clone().unwrap_or_default();
            match row.difference {
                None => {}
                Some(Difference::OnlyA) => {
                    lines.push(format!("- only in {a}: {name}  {}", text(&row.a)));
                }
                Some(Difference::OnlyB) => {
                    lines.push(format!("+ only in {b}: {name}  {}", text(&row.b)));
                }
                Some(Difference::Action | Difference::Options) => {
                    lines.push(format!("~ {name}  {} -> {}", text(&row.a), text(&row.b)));
                }
            }
        }
        let none = |text: &str| {
            if text.is_empty() {
                "(none)".to_owned()
            } else {
                text.to_owned()
            }
        };
        if let Some((x, y)) = &self.meta.hotkey {
            lines.push(format!("hotkey: {} -> {}", none(x), none(y)));
        }
        if let Some((x, y)) = &self.meta.triggers {
            lines.push(format!("triggers: {} -> {}", none(x), none(y)));
        }
        if let Some((x, y)) = &self.meta.priority {
            lines.push(format!("priority: {x} -> {y}"));
        }
        lines.push(format!(
            "{} only in {a}, {} only in {b}, {} differ, {} identical",
            self.count(Some(Difference::OnlyA)),
            self.count(Some(Difference::OnlyB)),
            self.count(Some(Difference::Action)) + self.count(Some(Difference::Options)),
            self.count(None),
        ));
        lines
    }
}

/// Compares the entries, hotkey, triggers and priority of two profiles.
/// Rows come in the order of A, then the entries only B has.
pub fn compare(a: &Profile, b: &Profile) -> Comparison {
    let mut taken = vec![false; b.apps.len()];
    let mut rows = Vec::new();
    for app in &a.apps {
        let found = b
            .apps
            .iter()
            .enumerate()
            .find(|(i, other)| !taken[*i] && same_program(app, other))
            .map(|(i, _)| i);
        match found {
            Some(i) => {
                taken[i] = true;
                let other = &b.apps[i];
                let (name_a, options_a) = describe(app);
                let (name_b, options_b) = describe(other);
                let difference = if name_a != name_b {
                    Some(Difference::Action)
                } else if options_a != options_b {
                    Some(Difference::Options)
                } else {
                    None
                };
                rows.push(Row {
                    exe: app.exe.clone(),
                    path: app.path.clone().or_else(|| other.path.clone()),
                    a: Some(text(name_a, &options_a)),
                    b: Some(text(name_b, &options_b)),
                    difference,
                });
            }
            None => {
                let (name, options) = describe(app);
                rows.push(Row {
                    exe: app.exe.clone(),
                    path: app.path.clone(),
                    a: Some(text(name, &options)),
                    b: None,
                    difference: Some(Difference::OnlyA),
                });
            }
        }
    }
    for (i, app) in b.apps.iter().enumerate() {
        if !taken[i] {
            let (name, options) = describe(app);
            rows.push(Row {
                exe: app.exe.clone(),
                path: app.path.clone(),
                a: None,
                b: Some(text(name, &options)),
                difference: Some(Difference::OnlyB),
            });
        }
    }
    Comparison {
        rows,
        meta: meta(a, b),
    }
}

fn meta(a: &Profile, b: &Profile) -> Meta {
    let hotkey = |p: &Profile| p.hotkey.map(|h| h.to_string()).unwrap_or_default();
    let triggers = |p: &Profile| {
        p.triggers
            .iter()
            .map(|t| t.text())
            .collect::<Vec<_>>()
            .join(", ")
    };
    let differ = |x: String, y: String| (x != y).then_some((x, y));
    Meta {
        hotkey: differ(hotkey(a), hotkey(b)),
        triggers: differ(triggers(a), triggers(b)),
        priority: (a.priority != b.priority).then_some((a.priority, b.priority)),
    }
}

/// The same program in the sense of A7 and C2: the same name, and the same
/// path where both entries have one.
fn same_program(a: &App, b: &App) -> bool {
    a.exe.eq_ignore_ascii_case(&b.exe)
        && match (&a.path, &b.path) {
            (Some(x), Some(y)) => same_path(x, y),
            _ => true,
        }
}

/// The action word and the options that matter for a comparison.
fn describe(app: &App) -> (&'static str, Vec<String>) {
    let mut options = Vec::new();
    let name = match &app.action {
        AppAction::Ignore => "ignore",
        AppAction::Suspend { trim } => {
            options.push(if *trim { "trim" } else { "no trim" }.to_owned());
            "suspend"
        }
        AppAction::Stop {
            on_timeout,
            restart_on_neutral,
            args,
            cwd,
        } => {
            options.push(
                match on_timeout {
                    OnTimeout::Kill => "end it if it does not close",
                    OnTimeout::Keep => "keep it if it does not close",
                }
                .to_owned(),
            );
            if *restart_on_neutral {
                options.push("restart on neutral".to_owned());
            }
            launch_options(&mut options, args, cwd.as_ref());
            "stop"
        }
        AppAction::Start { args, cwd } => {
            launch_options(&mut options, args, cwd.as_ref());
            "start"
        }
    };
    if app.confirmed {
        options.push("confirmed".to_owned());
    }
    (name, options)
}

fn launch_options(options: &mut Vec<String>, args: &[String], cwd: Option<&PathBuf>) {
    if !args.is_empty() {
        options.push(format!("args {}", args.join(" ")));
    }
    if let Some(cwd) = cwd {
        options.push(format!("in {}", cwd.display()));
    }
}

fn text(name: &str, options: &[String]) -> String {
    if options.is_empty() {
        name.to_owned()
    } else {
        format!("{name} · {}", options.join(" · "))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::parse;

    fn profiles(text: &str) -> (Profile, Profile) {
        let loaded = parse(text).expect("valid test configuration");
        let mut it = loaded.config.profiles.into_iter();
        (it.next().unwrap(), it.next().unwrap())
    }

    const TWO: &str = r#"
[[profile]]
name = "focus"
hotkey = "Ctrl+Shift+Alt+F1"
  [[profile.app]]
  exe = "chat.exe"
  action = "suspend"
  [[profile.app]]
  exe = "browser.exe"
  action = "suspend"
  [[profile.app]]
  exe = "sync.exe"
  action = "suspend"
  trim = false
  [[profile.app]]
  exe = "same.exe"
  action = "ignore"
  [[profile.app]]
  exe = "updater.exe"
  action = "stop"

[[profile]]
name = "flight"
trigger = ["simulator.exe"]
priority = 10
  [[profile.app]]
  exe = "BROWSER.EXE"
  action = "stop"
  [[profile.app]]
  exe = "sync.exe"
  action = "suspend"
  [[profile.app]]
  exe = "same.exe"
  action = "ignore"
  [[profile.app]]
  exe = "helper.exe"
  path = 'C:\Tools\helper.exe'
  action = "start"
  [[profile.app]]
  exe = "updater.exe"
  action = "stop"
  on_timeout = "keep"
"#;

    #[test]
    fn rows_name_each_kind_of_difference() {
        let (a, b) = profiles(TWO);
        let result = compare(&a, &b);
        let by = |exe: &str| result.rows.iter().find(|r| r.exe == exe).unwrap();
        assert_eq!(by("chat.exe").difference, Some(Difference::OnlyA));
        assert_eq!(by("helper.exe").difference, Some(Difference::OnlyB));
        // Names compare ignoring case (A7).
        assert_eq!(by("browser.exe").difference, Some(Difference::Action));
        assert_eq!(by("sync.exe").difference, Some(Difference::Options));
        assert_eq!(by("sync.exe").a.as_deref(), Some("suspend · no trim"));
        assert_eq!(by("sync.exe").b.as_deref(), Some("suspend · trim"));
        assert_eq!(by("updater.exe").difference, Some(Difference::Options));
        assert_eq!(by("same.exe").difference, None);
        assert_eq!(result.count(None), 1);
    }

    #[test]
    fn rows_follow_a_then_the_entries_only_b_has() {
        let (a, b) = profiles(TWO);
        let names: Vec<_> = compare(&a, &b).rows.into_iter().map(|r| r.exe).collect();
        assert_eq!(
            names,
            [
                "chat.exe",
                "browser.exe",
                "sync.exe",
                "same.exe",
                "updater.exe",
                "helper.exe"
            ]
        );
    }

    #[test]
    fn hotkey_triggers_and_priority_are_listed_where_they_differ() {
        let (a, b) = profiles(TWO);
        let meta = compare(&a, &b).meta;
        assert_eq!(
            meta.hotkey,
            Some(("Ctrl+Shift+Alt+F1".to_owned(), String::new()))
        );
        assert_eq!(
            meta.triggers,
            Some((String::new(), "simulator.exe".to_owned()))
        );
        assert_eq!(meta.priority, Some((0, 10)));
    }

    #[test]
    fn equal_profiles_have_no_differences() {
        let (a, _) = profiles(TWO);
        let result = compare(&a, &a);
        assert!(result.rows.iter().all(|r| r.difference.is_none()));
        assert_eq!(result.meta, Meta::default());
    }

    #[test]
    fn two_entries_of_one_name_with_different_paths_stay_apart() {
        let text = r#"
[[profile]]
name = "a"
  [[profile.app]]
  exe = "tool.exe"
  path = 'C:\One\tool.exe'
  action = "suspend"
  [[profile.app]]
  exe = "tool.exe"
  path = 'C:\Two\tool.exe'
  action = "stop"

[[profile]]
name = "b"
  [[profile.app]]
  exe = "tool.exe"
  path = 'c:\two\TOOL.exe'
  action = "stop"
"#;
        let (a, b) = profiles(text);
        let result = compare(&a, &b);
        assert_eq!(result.rows[0].difference, Some(Difference::OnlyA));
        assert_eq!(result.rows[1].difference, None);
    }

    #[test]
    fn lines_show_each_difference_and_a_summary() {
        let (a, b) = profiles(TWO);
        let lines = compare(&a, &b).lines("focus", "flight");
        assert!(lines.contains(&"- only in focus: chat.exe  suspend · trim".to_owned()));
        assert!(lines.contains(
            &"~ browser.exe  suspend · trim -> stop · end it if it does not close".to_owned()
        ));
        assert!(lines.contains(&"hotkey: Ctrl+Shift+Alt+F1 -> (none)".to_owned()));
        assert_eq!(
            lines.last().unwrap(),
            "1 only in focus, 1 only in flight, 3 differ, 1 identical"
        );
    }
}
