//! TOML configuration: schema, defaults and validation rules C1 to C7.
//!
//! [`parse`] either returns a fully validated [`Config`] (plus warnings) or
//! rejects the file as a whole with every problem found and its line (C3).

use std::collections::HashMap;
use std::fmt;
use std::fs;
use std::ops::Range;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use toml::Spanned;

use crate::error::Code;
use crate::hotkey::Hotkey;
use crate::protection;

/// Name of the built-in profile without entries (C1).
pub const NEUTRAL: &str = "neutral";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    pub settings: Settings,
    pub profiles: Vec<Profile>,
}

impl Config {
    /// Looks a profile up by name, ignoring case.
    pub fn profile(&self, name: &str) -> Option<&Profile> {
        let key = name.trim().to_lowercase();
        self.profiles.iter().find(|p| p.name.to_lowercase() == key)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Settings {
    /// Action of entries that do not name one. Never `start`.
    pub default_action: Action,
    pub trim: bool,
    pub stop_timeout_s: u32,
    pub minimize_before_suspend: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            default_action: Action::Suspend,
            trim: true,
            stop_timeout_s: 8,
            minimize_before_suspend: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Profile {
    pub name: String,
    pub hotkey: Option<Hotkey>,
    pub apps: Vec<App>,
    /// U4: programs whose start applies this profile automatically.
    pub triggers: Vec<Trigger>,
    /// U4: decides between two profiles whose triggers run at once; higher
    /// wins. Default `0`.
    pub priority: i32,
}

/// U4: a trigger program, by executable name and optionally its full path
/// (A7, as for entries).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Trigger {
    /// Executable file name with extension (`simulator.exe`).
    pub exe: String,
    pub path: Option<PathBuf>,
}

impl Trigger {
    /// As written in the file: the path if there is one, else the name.
    pub fn text(&self) -> String {
        match &self.path {
            Some(path) => path.to_string_lossy().into_owned(),
            None => self.exe.clone(),
        }
    }

    /// Whether this trigger and a program given by `exe` and `path` can
    /// match the same process: the same name, unless both have paths that
    /// differ (A7, C2).
    pub fn overlaps(&self, exe: &str, path: Option<&Path>) -> bool {
        self.exe.eq_ignore_ascii_case(exe)
            && match (&self.path, path) {
                (Some(a), Some(b)) => crate::process::same_path(a, b),
                _ => true,
            }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct App {
    /// Executable file name, always with an extension (`tool.exe`).
    pub exe: String,
    /// Full path, to tell programs with equal names apart (A7).
    pub path: Option<PathBuf>,
    pub action: AppAction,
    /// P6: the owner confirmed that this warn-list program may be touched.
    pub confirmed: bool,
}

impl App {
    pub fn new(exe: impl Into<String>, path: Option<PathBuf>, action: AppAction) -> Self {
        Self {
            exe: exe.into(),
            path,
            action,
            confirmed: false,
        }
    }
}

/// What a profile does with a program, with the options valid for it (C6).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AppAction {
    Start {
        args: Vec<String>,
        cwd: Option<PathBuf>,
    },
    Stop {
        on_timeout: OnTimeout,
        restart_on_neutral: bool,
        /// Arguments and folder for the restart on `neutral`.
        args: Vec<String>,
        cwd: Option<PathBuf>,
    },
    Suspend {
        /// Entry override, else `settings.trim`.
        trim: bool,
    },
    Ignore,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Action {
    Start,
    Stop,
    Suspend,
    Ignore,
}

impl Action {
    fn name(self) -> &'static str {
        match self {
            Action::Start => "start",
            Action::Stop => "stop",
            Action::Suspend => "suspend",
            Action::Ignore => "ignore",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OnTimeout {
    Kill,
    Keep,
}

/// One problem in the configuration file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Issue {
    /// 1-based line, when the problem can be pinned to one.
    pub line: Option<usize>,
    pub message: String,
}

impl fmt::Display for Issue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.line {
            Some(line) => write!(f, "line {line}: {}", self.message),
            None => f.write_str(&self.message),
        }
    }
}

/// A valid configuration and the non-fatal findings (for example C7).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Loaded {
    pub config: Config,
    pub warnings: Vec<Issue>,
}

/// The file was rejected as a whole (C3).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rejected {
    /// `ConfigRead`, `ConfigSyntax` or `ConfigInvalid`.
    pub code: Code,
    pub issues: Vec<Issue>,
}

impl fmt::Display for Rejected {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.code)?;
        for issue in &self.issues {
            write!(f, "\n  {issue}")?;
        }
        Ok(())
    }
}

impl std::error::Error for Rejected {}

/// Reads and validates the configuration file.
pub fn load(path: &Path) -> Result<Loaded, Rejected> {
    let text = fs::read_to_string(path).map_err(|err| Rejected {
        code: Code::ConfigRead,
        issues: vec![Issue {
            line: None,
            message: format!("cannot read file: {err}"),
        }],
    })?;
    parse(&text)
}

/// The lines of a rejected file as the command line prints them, in the
/// form editors can jump to: `file:line: error KSnnn: message`.
pub fn rejected_lines(file: &Path, rejected: &Rejected) -> Vec<String> {
    rejected
        .issues
        .iter()
        .map(|issue| {
            let line = issue.line.map_or(String::new(), |l| format!(":{l}"));
            format!(
                "{}{line}: error {}: {}",
                file.display(),
                rejected.code,
                issue.message
            )
        })
        .collect()
}

/// The configuration a switch to `target` uses, read from `file`, and the
/// lines to show on stderr. `neutral` needs none: bringing everything back
/// must work even when the file is missing or invalid. `Err` carries the
/// lines explaining why a switch to another profile cannot run (E2 exit
/// code 2). Shared by the in-process command line and the tray's pipe, so
/// both answer alike.
pub fn for_switch(file: Option<&Path>, target: &str) -> Result<(Config, Vec<String>), Vec<String>> {
    let neutral = target.trim().eq_ignore_ascii_case(NEUTRAL);
    let empty = Config {
        settings: Settings::default(),
        profiles: Vec::new(),
    };
    let Some(file) = file else {
        let lines = vec!["error: no configuration directory (APPDATA unset)".to_owned()];
        return if neutral {
            Ok((empty, lines))
        } else {
            Err(lines)
        };
    };
    match load(file) {
        Ok(loaded) => {
            let notes = loaded
                .warnings
                .iter()
                .map(|warning| format!("{}: warning: {warning}", file.display()))
                .collect();
            Ok((loaded.config, notes))
        }
        Err(rejected) if neutral && rejected.code == Code::ConfigRead => Ok((empty, Vec::new())),
        Err(rejected) => {
            log::warn!("{}: {rejected}", file.display());
            let mut lines = rejected_lines(file, &rejected);
            if neutral {
                lines.push("warning: switching to neutral without the configuration".to_owned());
                Ok((empty, lines))
            } else {
                Err(lines)
            }
        }
    }
}

/// Validates configuration text.
pub fn parse(text: &str) -> Result<Loaded, Rejected> {
    let raw: RawConfig = toml::from_str(text).map_err(|err| Rejected {
        code: Code::ConfigSyntax,
        issues: vec![Issue {
            line: err.span().map(|span| line_of(text, span.start)),
            message: err.message().to_owned(),
        }],
    })?;
    let mut check = Check {
        text,
        errors: Vec::new(),
        warnings: Vec::new(),
    };
    let config = check.config(raw);
    if check.errors.is_empty() {
        Ok(Loaded {
            config,
            warnings: check.warnings,
        })
    } else {
        check.errors.sort_by_key(|issue| issue.line);
        Err(Rejected {
            code: Code::ConfigInvalid,
            issues: check.errors,
        })
    }
}

fn line_of(text: &str, offset: usize) -> usize {
    let end = offset.min(text.len());
    text.as_bytes()[..end]
        .iter()
        .filter(|&&b| b == b'\n')
        .count()
        + 1
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawConfig {
    settings: Option<RawSettings>,
    #[serde(default)]
    profile: Vec<RawProfile>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawSettings {
    default_action: Option<Spanned<Action>>,
    trim: Option<bool>,
    stop_timeout_s: Option<u32>,
    minimize_before_suspend: Option<bool>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawProfile {
    name: Spanned<String>,
    hotkey: Option<Spanned<String>>,
    #[serde(default)]
    app: Vec<RawApp>,
    trigger: Option<Vec<Spanned<String>>>,
    priority: Option<Spanned<i64>>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawApp {
    exe: Spanned<String>,
    path: Option<Spanned<String>>,
    action: Option<Spanned<Action>>,
    args: Option<Spanned<Vec<String>>>,
    cwd: Option<Spanned<String>>,
    on_timeout: Option<Spanned<OnTimeout>>,
    restart_on_neutral: Option<Spanned<bool>>,
    trim: Option<Spanned<bool>>,
    confirmed: Option<Spanned<bool>>,
}

struct Check<'a> {
    text: &'a str,
    errors: Vec<Issue>,
    warnings: Vec<Issue>,
}

impl Check<'_> {
    fn line(&self, span: &Range<usize>) -> usize {
        line_of(self.text, span.start)
    }

    fn error(&mut self, span: &Range<usize>, message: String) {
        let line = Some(self.line(span));
        self.errors.push(Issue { line, message });
    }

    fn config(&mut self, raw: RawConfig) -> Config {
        let mut settings = Settings::default();
        if let Some(raw) = raw.settings {
            if let Some(action) = raw.default_action {
                if *action.get_ref() == Action::Start {
                    self.error(
                        &action.span(),
                        "default_action cannot be \"start\": a start entry needs its own path"
                            .to_owned(),
                    );
                } else {
                    settings.default_action = *action.get_ref();
                }
            }
            settings.trim = raw.trim.unwrap_or(settings.trim);
            settings.stop_timeout_s = raw.stop_timeout_s.unwrap_or(settings.stop_timeout_s);
            settings.minimize_before_suspend = raw
                .minimize_before_suspend
                .unwrap_or(settings.minimize_before_suspend);
        }

        let mut names: HashMap<String, usize> = HashMap::new();
        let mut hotkeys: HashMap<Hotkey, String> = HashMap::new();
        // U4: the triggers of the profiles so far, with their profile.
        let mut triggers_seen: Vec<(Trigger, String)> = Vec::new();
        let mut profiles = Vec::new();
        for raw in raw.profile {
            let span = raw.name.span();
            let name = raw.name.get_ref().trim().to_owned();
            // C1
            if name.is_empty() {
                self.error(&span, "profile name must not be empty".to_owned());
            } else if name.eq_ignore_ascii_case(NEUTRAL) {
                self.error(
                    &span,
                    format!("\"{NEUTRAL}\" is the built-in profile and cannot be defined"),
                );
            } else if let Some(first) = names.get(&name.to_lowercase()) {
                self.error(
                    &span,
                    format!("profile \"{name}\" is already defined in line {first}"),
                );
            } else {
                names.insert(name.to_lowercase(), self.line(&span));
            }

            let hotkey = raw
                .hotkey
                .and_then(|raw| self.hotkey(&raw, &name, &mut hotkeys));
            let apps = self.apps(raw.app, &settings);
            let triggers = raw
                .trigger
                .map(|list| self.triggers(list, &name, &apps, &mut triggers_seen))
                .unwrap_or_default();
            let priority = match raw.priority {
                Some(raw) => i32::try_from(*raw.get_ref()).unwrap_or_else(|_| {
                    self.error(
                        &raw.span(),
                        format!(
                            "priority {} is out of range ({} to {})",
                            raw.get_ref(),
                            i32::MIN,
                            i32::MAX
                        ),
                    );
                    0
                }),
                None => 0,
            };
            profiles.push(Profile {
                name,
                hotkey,
                apps,
                triggers,
                priority,
            });
        }
        Config { settings, profiles }
    }

    /// C5 and C7.
    fn hotkey(
        &mut self,
        raw: &Spanned<String>,
        profile: &str,
        seen: &mut HashMap<Hotkey, String>,
    ) -> Option<Hotkey> {
        let span = raw.span();
        let hotkey = match Hotkey::parse(raw.get_ref()) {
            Ok(hotkey) => hotkey,
            Err(message) => {
                self.error(&span, message);
                return None;
            }
        };
        if hotkey.steals_altgr() {
            let line = Some(self.line(&span));
            self.warnings.push(Issue {
                line,
                message: format!(
                    "hotkey {hotkey} is ignored: Ctrl+Alt with a letter or digit types a \
                     character on keyboards with AltGr; use a function key instead"
                ),
            });
            return None;
        }
        if let Some(other) = seen.get(&hotkey) {
            self.error(
                &span,
                format!("hotkey {hotkey} is already used by profile \"{other}\""),
            );
            return None;
        }
        seen.insert(hotkey, profile.to_owned());
        Some(hotkey)
    }

    fn apps(&mut self, raw: Vec<RawApp>, settings: &Settings) -> Vec<App> {
        // C2: exe -> (path, line) of the entries seen so far.
        let mut seen: HashMap<String, Vec<(Option<String>, usize)>> = HashMap::new();
        let mut apps = Vec::new();
        for raw in raw {
            let span = raw.exe.span();
            let Some(exe) = self.exe(&raw.exe) else {
                continue;
            };
            let path = self.absolute_path(raw.path.as_ref(), "path");
            if let (Some(path), Some(raw_path)) = (&path, &raw.path) {
                let file = path.file_name().and_then(|f| f.to_str()).unwrap_or("");
                if !file.eq_ignore_ascii_case(&exe) {
                    self.error(
                        &raw_path.span(),
                        format!("path points to \"{file}\", but exe is \"{exe}\""),
                    );
                }
            }

            let path_key = path.as_ref().map(|p| p.to_string_lossy().to_lowercase());
            let earlier = seen.entry(exe.to_lowercase()).or_default();
            let clash = earlier
                .iter()
                .find(|(other, _)| other.is_none() || path_key.is_none() || *other == path_key)
                .map(|(_, line)| *line);
            earlier.push((path_key, self.line(&span)));
            if let Some(line) = clash {
                self.error(
                    &span,
                    format!(
                        "\"{exe}\" already has an entry in this profile (line {line}); \
                         two entries for one name need different paths"
                    ),
                );
            }

            let action = raw
                .action
                .as_ref()
                .map_or(settings.default_action, |a| *a.get_ref());
            // C6: every option belongs to certain actions.
            const STOP: &[Action] = &[Action::Stop];
            const LAUNCH: &[Action] = &[Action::Start, Action::Stop];
            const TOUCH: &[Action] = &[Action::Stop, Action::Suspend];
            let options = [
                ("args", raw.args.as_ref().map(Spanned::span), LAUNCH),
                ("cwd", raw.cwd.as_ref().map(Spanned::span), LAUNCH),
                (
                    "on_timeout",
                    raw.on_timeout.as_ref().map(Spanned::span),
                    STOP,
                ),
                (
                    "restart_on_neutral",
                    raw.restart_on_neutral.as_ref().map(Spanned::span),
                    STOP,
                ),
                (
                    "trim",
                    raw.trim.as_ref().map(Spanned::span),
                    &[Action::Suspend][..],
                ),
                (
                    "confirmed",
                    raw.confirmed.as_ref().map(Spanned::span),
                    TOUCH,
                ),
            ];
            for (key, option_span, valid_for) in options {
                if let Some(option_span) = option_span
                    && !valid_for.contains(&action)
                {
                    let names: Vec<String> = valid_for
                        .iter()
                        .map(|a| format!("\"{}\"", a.name()))
                        .collect();
                    self.error(
                        &option_span,
                        format!(
                            "{key} is only valid with action = {}, but \"{exe}\" has \
                             action = \"{}\"",
                            names.join(" or "),
                            action.name()
                        ),
                    );
                }
            }
            let restart = raw
                .restart_on_neutral
                .as_ref()
                .is_some_and(|r| *r.get_ref());
            if action == Action::Stop && !restart {
                // Arguments of a stop entry are for the restart only.
                for (key, option_span) in [
                    ("args", raw.args.as_ref().map(Spanned::span)),
                    ("cwd", raw.cwd.as_ref().map(Spanned::span)),
                ] {
                    if let Some(option_span) = option_span {
                        self.error(
                            &option_span,
                            format!(
                                "{key} of a stop entry is used for the restart and needs \
                                 restart_on_neutral = true"
                            ),
                        );
                    }
                }
            }
            let confirmed = raw.confirmed.is_some_and(Spanned::into_inner);

            let action = match action {
                Action::Start => {
                    if path.is_none() {
                        self.error(
                            &span,
                            format!(
                                "\"{exe}\" has action = \"start\" and needs a path; \
                                 a name alone cannot be launched"
                            ),
                        );
                    }
                    AppAction::Start {
                        args: raw.args.map(Spanned::into_inner).unwrap_or_default(),
                        cwd: self.absolute_path(raw.cwd.as_ref(), "cwd"),
                    }
                }
                Action::Stop => AppAction::Stop {
                    on_timeout: raw.on_timeout.map_or(OnTimeout::Kill, Spanned::into_inner),
                    restart_on_neutral: restart,
                    args: raw.args.map(Spanned::into_inner).unwrap_or_default(),
                    cwd: self.absolute_path(raw.cwd.as_ref(), "cwd"),
                },
                Action::Suspend => AppAction::Suspend {
                    trim: raw.trim.map_or(settings.trim, Spanned::into_inner),
                },
                Action::Ignore => AppAction::Ignore,
            };
            apps.push(App {
                exe,
                path,
                action,
                confirmed,
            });
        }
        apps
    }

    /// U4: the trigger programs of `profile`. Each is an executable name or
    /// a full path (A7). Refused: a program twice, a program that already
    /// triggers another profile, and a program that its own profile stops
    /// or freezes (the profile would end or freeze its own trigger).
    fn triggers(
        &mut self,
        raw: Vec<Spanned<String>>,
        profile: &str,
        apps: &[App],
        seen: &mut Vec<(Trigger, String)>,
    ) -> Vec<Trigger> {
        let mut triggers: Vec<Trigger> = Vec::new();
        for raw in raw {
            let span = raw.span();
            let text = raw.get_ref().trim();
            if text.is_empty() {
                self.error(&span, "a trigger must not be empty".to_owned());
                continue;
            }
            let trigger = if text.contains(['\\', '/', ':']) {
                let path = PathBuf::from(text);
                let file = path
                    .file_name()
                    .and_then(|f| f.to_str())
                    .unwrap_or("")
                    .to_owned();
                if !path.is_absolute() {
                    self.error(
                        &span,
                        format!("trigger \"{text}\" must be a program name or an absolute path"),
                    );
                    continue;
                }
                if !file.contains('.') {
                    self.error(
                        &span,
                        format!("trigger \"{text}\" must end in the program's file name"),
                    );
                    continue;
                }
                Trigger {
                    exe: file,
                    path: Some(path),
                }
            } else {
                Trigger {
                    exe: crate::process::with_extension(text),
                    path: None,
                }
            };
            let overlaps = |t: &Trigger| t.overlaps(&trigger.exe, trigger.path.as_deref());
            if triggers.iter().any(overlaps) {
                self.error(
                    &span,
                    format!(
                        "\"{}\" is already a trigger of this profile; two triggers with one \
                         name need different paths",
                        trigger.text()
                    ),
                );
                continue;
            }
            if let Some((_, other)) = seen.iter().find(|(t, _)| overlaps(t)) {
                self.error(
                    &span,
                    format!(
                        "\"{}\" is already a trigger of profile \"{other}\"; a program can \
                         trigger one profile only",
                        trigger.text()
                    ),
                );
                continue;
            }
            if let Some(app) = apps.iter().find(|app| {
                matches!(
                    app.action,
                    AppAction::Stop { .. } | AppAction::Suspend { .. }
                ) && trigger.overlaps(&app.exe, app.path.as_deref())
            }) {
                let verb = if matches!(app.action, AppAction::Stop { .. }) {
                    "stops"
                } else {
                    "freezes"
                };
                self.error(
                    &span,
                    format!(
                        "trigger \"{}\" is a program that this profile {verb} (entry \"{}\"); \
                         the profile would end or freeze its own trigger",
                        trigger.text(),
                        app.exe
                    ),
                );
                continue;
            }
            seen.push((trigger.clone(), profile.to_owned()));
            triggers.push(trigger);
        }
        triggers
    }

    /// Checks an `exe` value and returns it with an extension.
    fn exe(&mut self, raw: &Spanned<String>) -> Option<String> {
        let span = raw.span();
        let name = raw.get_ref().trim();
        if name.is_empty() {
            self.error(&span, "exe must not be empty".to_owned());
            return None;
        }
        if name.contains(['\\', '/', ':']) {
            self.error(
                &span,
                format!("exe \"{name}\" must be a file name; put the folder into path"),
            );
            return None;
        }
        let exe = if name.contains('.') {
            name.to_owned()
        } else {
            format!("{name}.exe")
        };
        // P6: hard-protected programs are refused whatever the action is.
        if protection::is_hard_protected(&exe) {
            self.error(
                &span,
                format!(
                    "{}: \"{exe}\" is a protected system program and cannot be used in a profile",
                    Code::HardProtected
                ),
            );
            return None;
        }
        Some(exe)
    }

    /// An empty string counts as "not set".
    fn absolute_path(&mut self, raw: Option<&Spanned<String>>, key: &str) -> Option<PathBuf> {
        let raw = raw?;
        let text = raw.get_ref().trim();
        if text.is_empty() {
            return None;
        }
        let path = PathBuf::from(text);
        if !path.is_absolute() {
            self.error(
                &raw.span(),
                format!("{key} \"{text}\" must be an absolute path"),
            );
            return None;
        }
        Some(path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ok(text: &str) -> Loaded {
        parse(text).unwrap_or_else(|rejected| panic!("unexpected rejection: {rejected}"))
    }

    /// The single issue of a rejected file as `(line, message)`.
    fn rejected(text: &str) -> (usize, String) {
        let rejected = parse(text).expect_err("file should be rejected");
        assert_eq!(rejected.issues.len(), 1, "{rejected}");
        let issue = &rejected.issues[0];
        (issue.line.expect("issue has a line"), issue.message.clone())
    }

    #[test]
    fn empty_file_is_valid_and_uses_defaults() {
        let loaded = ok("");
        assert_eq!(loaded.config.settings, Settings::default());
        assert!(loaded.config.profiles.is_empty());
        assert!(loaded.warnings.is_empty());
    }

    #[test]
    fn example_config_is_valid() {
        let loaded = ok(include_str!("../config.example.toml"));
        assert!(loaded.warnings.is_empty());
        assert!(!loaded.config.profiles.is_empty());
    }

    #[test]
    fn spec_example_maps_to_typed_entries() {
        let loaded = ok(r#"
[settings]
default_action = "suspend"
trim = true
stop_timeout_s = 12
minimize_before_suspend = false

[[profile]]
name = "focus"
hotkey = "Ctrl+Shift+Alt+F1"

  [[profile.app]]
  exe = "browser.exe"
  path = ""
  action = "suspend"

  [[profile.app]]
  exe = "tool.exe"
  path = "C:\\tools\\tool.exe"
  action = "start"
  args = ["--minimized"]
  cwd = "C:\\tools"

  [[profile.app]]
  exe = "chat.exe"
  action = "stop"
  on_timeout = "keep"
  restart_on_neutral = true
"#);
        let config = loaded.config;
        assert_eq!(config.settings.stop_timeout_s, 12);
        assert!(!config.settings.minimize_before_suspend);
        let focus = config.profile("FOCUS").expect("lookup ignores case");
        assert_eq!(focus.hotkey.unwrap().to_string(), "Ctrl+Shift+Alt+F1");
        assert_eq!(
            focus.apps,
            vec![
                App {
                    exe: "browser.exe".into(),
                    path: None,
                    action: AppAction::Suspend { trim: true },
                    confirmed: false,
                },
                App {
                    exe: "tool.exe".into(),
                    path: Some(PathBuf::from("C:\\tools\\tool.exe")),
                    action: AppAction::Start {
                        args: vec!["--minimized".into()],
                        cwd: Some(PathBuf::from("C:\\tools")),
                    },
                    confirmed: false,
                },
                App {
                    exe: "chat.exe".into(),
                    path: None,
                    action: AppAction::Stop {
                        on_timeout: OnTimeout::Keep,
                        restart_on_neutral: true,
                        args: Vec::new(),
                        cwd: None,
                    },
                    confirmed: false,
                },
            ]
        );
    }

    #[test]
    fn missing_action_uses_default_action_and_exe_gets_extension() {
        let loaded = ok(r#"
[settings]
default_action = "stop"

[[profile]]
name = "a"
  [[profile.app]]
  exe = "Tool"
"#);
        assert_eq!(
            loaded.config.profiles[0].apps[0],
            App {
                exe: "Tool.exe".into(),
                path: None,
                action: AppAction::Stop {
                    on_timeout: OnTimeout::Kill,
                    restart_on_neutral: false,
                    args: Vec::new(),
                    cwd: None,
                },
                confirmed: false,
            }
        );
    }

    #[test]
    fn entry_trim_overrides_settings() {
        let loaded = ok(r#"
[[profile]]
name = "a"
  [[profile.app]]
  exe = "big.exe"
  trim = false
  [[profile.app]]
  exe = "small.exe"
"#);
        let apps = &loaded.config.profiles[0].apps;
        assert_eq!(apps[0].action, AppAction::Suspend { trim: false });
        assert_eq!(apps[1].action, AppAction::Suspend { trim: true });
    }

    #[test]
    fn c3_syntax_error_names_the_line() {
        let rejected = parse("[settings]\ntrim = maybe\n").unwrap_err();
        assert_eq!(rejected.code, Code::ConfigSyntax);
        assert_eq!(rejected.issues[0].line, Some(2));
    }

    #[test]
    fn c3_unknown_keys_are_errors() {
        for text in [
            "[settings]\ntrimm = true\n",
            "colour = 1\n",
            "[[profile]]\nname = \"a\"\nhotkeys = \"F13\"\n",
            "[[profile]]\nname = \"a\"\n[[profile.app]]\nexe = \"a.exe\"\nactoin = \"stop\"\n",
        ] {
            let rejected = parse(text).unwrap_err();
            assert_eq!(rejected.code, Code::ConfigSyntax, "{text}");
            assert!(rejected.issues[0].line.is_some(), "{text}");
            assert!(
                rejected.issues[0].message.contains("unknown field"),
                "{text}"
            );
        }
    }

    #[test]
    fn c3_unknown_action_is_an_error() {
        let rejected = parse(
            "[[profile]]\nname = \"a\"\n[[profile.app]]\nexe = \"a.exe\"\naction = \"freeze\"\n",
        )
        .unwrap_err();
        assert_eq!(rejected.code, Code::ConfigSyntax);
        assert_eq!(rejected.issues[0].line, Some(5));
    }

    #[test]
    fn c1_profile_names() {
        let (line, message) = rejected("[[profile]]\nname = \"  \"\n");
        assert_eq!(line, 2);
        assert!(message.contains("empty"));

        let (line, message) = rejected("[[profile]]\nname = \"Neutral\"\n");
        assert_eq!(line, 2);
        assert!(message.contains("built-in"));

        let (line, message) =
            rejected("[[profile]]\nname = \"sim\"\n\n[[profile]]\nname = \"SIM\"\n");
        assert_eq!(line, 5);
        assert!(message.contains("line 2"));
    }

    #[test]
    fn c2_exe_once_per_profile_but_in_several_profiles() {
        let (line, message) = rejected(
            "[[profile]]\nname = \"a\"\n[[profile.app]]\nexe = \"x.exe\"\n[[profile.app]]\nexe = \"X\"\n",
        );
        assert_eq!(line, 6);
        assert!(message.contains("line 4"));

        ok(
            "[[profile]]\nname = \"a\"\n[[profile.app]]\nexe = \"x.exe\"\n\
            [[profile]]\nname = \"b\"\n[[profile.app]]\nexe = \"x.exe\"\n",
        );
    }

    #[test]
    fn c2_same_name_with_different_paths_is_allowed() {
        ok(r#"
[[profile]]
name = "a"
  [[profile.app]]
  exe = "x.exe"
  path = "C:\\one\\x.exe"
  [[profile.app]]
  exe = "x.exe"
  path = "C:\\two\\x.exe"
"#);
        let (_, message) = rejected(
            r#"
[[profile]]
name = "a"
  [[profile.app]]
  exe = "x.exe"
  path = "C:\\one\\x.exe"
  [[profile.app]]
  exe = "x.exe"
  path = "c:\\ONE\\x.exe"
"#,
        );
        assert!(message.contains("already has an entry"));
    }

    #[test]
    fn c5_duplicate_hotkey() {
        let (line, message) = rejected(
            "[[profile]]\nname = \"a\"\nhotkey = \"Ctrl+Shift+F1\"\n\
             [[profile]]\nname = \"b\"\nhotkey = \"shift+ctrl+f1\"\n",
        );
        assert_eq!(line, 6);
        assert!(message.contains("profile \"a\""));
    }

    #[test]
    fn c6_options_belong_to_one_action() {
        for (option, action) in [
            ("args = [\"-x\"]", "suspend"),
            ("cwd = \"C:\\\\x\"", "ignore"),
            ("on_timeout = \"keep\"", "suspend"),
            ("restart_on_neutral = true", "ignore"),
            ("trim = false", "stop"),
        ] {
            let text = format!(
                "[[profile]]\nname = \"a\"\n[[profile.app]]\nexe = \"a.exe\"\naction = \"{action}\"\n{option}\n"
            );
            let (line, message) = rejected(&text);
            assert_eq!(line, 6, "{option}");
            assert!(message.contains("only valid with"), "{message}");
        }
    }

    #[test]
    fn c6_stop_entry_may_carry_restart_arguments() {
        let loaded = ok(r#"
[[profile]]
name = "a"
  [[profile.app]]
  exe = "chat.exe"
  action = "stop"
  restart_on_neutral = true
  args = ["--minimized"]
  cwd = "C:\\chat"
"#);
        assert_eq!(
            loaded.config.profiles[0].apps[0].action,
            AppAction::Stop {
                on_timeout: OnTimeout::Kill,
                restart_on_neutral: true,
                args: vec!["--minimized".into()],
                cwd: Some(PathBuf::from("C:\\chat")),
            }
        );

        // Without a restart the arguments would never be used.
        let (line, message) = rejected(
            "[[profile]]\nname = \"a\"\n[[profile.app]]\nexe = \"chat.exe\"\naction = \"stop\"\nargs = [\"-x\"]\n",
        );
        assert_eq!(line, 6);
        assert!(message.contains("restart_on_neutral = true"), "{message}");
    }

    #[test]
    fn p6_confirmed_is_for_entries_that_touch_a_program() {
        let loaded = ok(
            "[[profile]]\nname = \"a\"\n[[profile.app]]\nexe = \"a.exe\"\nconfirmed = true\n\
             [[profile.app]]\nexe = \"b.exe\"\naction = \"stop\"\n",
        );
        let apps = &loaded.config.profiles[0].apps;
        assert!(apps[0].confirmed);
        assert!(!apps[1].confirmed);

        for action in ["start", "ignore"] {
            let text = format!(
                "[[profile]]\nname = \"a\"\n[[profile.app]]\nexe = \"a.exe\"\npath = \"C:\\\\x\\\\a.exe\"\naction = \"{action}\"\nconfirmed = true\n"
            );
            let (line, message) = rejected(&text);
            assert_eq!(line, 7, "{action}");
            assert!(message.contains("confirmed is only valid"), "{message}");
        }
    }

    #[test]
    fn c6_start_requires_path() {
        let (line, message) = rejected(
            "[[profile]]\nname = \"a\"\n[[profile.app]]\nexe = \"a.exe\"\naction = \"start\"\n",
        );
        assert_eq!(line, 4);
        assert!(message.contains("needs a path"));

        let (line, _) = rejected("[settings]\ndefault_action = \"start\"\n");
        assert_eq!(line, 2);
    }

    #[test]
    fn c7_bad_hotkey_syntax_is_an_error() {
        let (line, message) = rejected("[[profile]]\nname = \"a\"\nhotkey = \"Ctrl+Nope\"\n");
        assert_eq!(line, 3);
        assert!(message.contains("unknown key"));
    }

    #[test]
    fn c7_ctrl_alt_printable_is_warned_and_ignored() {
        let loaded = ok("[[profile]]\nname = \"a\"\nhotkey = \"Ctrl+Alt+7\"\n");
        assert_eq!(loaded.config.profiles[0].hotkey, None);
        assert_eq!(loaded.warnings.len(), 1);
        assert_eq!(loaded.warnings[0].line, Some(3));
    }

    #[test]
    fn t9_hard_protected_program_is_refused() {
        for exe in ["explorer.exe", "CSRSS", "conhost.exe"] {
            let text = format!(
                "[[profile]]\nname = \"a\"\n[[profile.app]]\nexe = \"{exe}\"\naction = \"ignore\"\n"
            );
            let (line, message) = rejected(&text);
            assert_eq!(line, 4, "{exe}");
            assert!(message.contains("KS104"), "{message}");
        }
    }

    #[test]
    fn exe_and_paths_are_checked() {
        let app = |lines: &str| format!("[[profile]]\nname = \"a\"\n[[profile.app]]\n{lines}\n");
        let (_, message) = rejected(&app("exe = \"C:\\\\x\\\\a.exe\""));
        assert!(message.contains("must be a file name"));
        let (_, message) = rejected(&app("exe = \"\""));
        assert!(message.contains("empty"));
        let (line, message) = rejected(&app("exe = \"a.exe\"\npath = \"tools\\\\a.exe\""));
        assert_eq!(line, 5);
        assert!(message.contains("absolute"));
        let (line, message) = rejected(&app("exe = \"a.exe\"\npath = \"C:\\\\tools\\\\b.exe\""));
        assert_eq!(line, 5);
        assert!(message.contains("points to"));
    }

    #[test]
    fn all_problems_are_reported_together_in_line_order() {
        let rejected = parse(
            "[[profile]]\nname = \"neutral\"\n[[profile.app]]\nexe = \"a.exe\"\nargs = []\n\
             [[profile]]\nname = \"\"\n",
        )
        .unwrap_err();
        assert_eq!(rejected.code, Code::ConfigInvalid);
        let lines: Vec<_> = rejected.issues.iter().map(|i| i.line.unwrap()).collect();
        assert_eq!(lines, vec![2, 5, 7]);
    }

    #[test]
    fn u4_triggers_and_priority_are_read() {
        let config = ok(r#"
[[profile]]
name = "flight"
trigger = ["simulator", 'C:\Games\Sim\launcher.exe']
priority = 5

  [[profile.app]]
  exe = "browser.exe"

[[profile]]
name = "plain"
"#)
        .config;
        let flight = config.profile("flight").unwrap();
        assert_eq!(flight.priority, 5);
        assert_eq!(
            flight.triggers,
            vec![
                Trigger {
                    exe: "simulator.exe".into(),
                    path: None
                },
                Trigger {
                    exe: "launcher.exe".into(),
                    path: Some(PathBuf::from(r"C:\Games\Sim\launcher.exe"))
                },
            ]
        );
        assert_eq!(flight.triggers[1].text(), r"C:\Games\Sim\launcher.exe");
        let plain = config.profile("plain").unwrap();
        assert!(plain.triggers.is_empty());
        assert_eq!(plain.priority, 0);
    }

    #[test]
    fn u4_a_trigger_its_own_profile_stops_or_freezes_is_refused() {
        // Freezing is the default action.
        let (line, message) = rejected(
            "[[profile]]\nname = \"flight\"\ntrigger = [\"sim.exe\"]\n\
             [[profile.app]]\nexe = \"SIM.exe\"\n",
        );
        assert_eq!(line, 3);
        assert!(message.contains("freezes"), "{message}");
        let (_, message) = rejected(
            "[[profile]]\nname = \"flight\"\ntrigger = ['C:\\x\\sim.exe']\n\
             [[profile.app]]\nexe = \"sim.exe\"\naction = \"stop\"\n",
        );
        assert!(message.contains("stops"), "{message}");
        // Another program of the same name (other path), or one that is
        // started or ignored, is fine.
        ok(
            "[[profile]]\nname = \"flight\"\ntrigger = ['C:\\x\\sim.exe']\n\
            [[profile.app]]\nexe = \"sim.exe\"\npath = 'C:\\y\\sim.exe'\n\
            [[profile.app]]\nexe = \"tool.exe\"\npath = 'C:\\t\\tool.exe'\naction = \"start\"\n\
            [[profile.app]]\nexe = \"other.exe\"\naction = \"ignore\"\n",
        );
    }

    #[test]
    fn u4_triggers_are_validated() {
        let cases = [
            ("trigger = [\"\"]", "must not be empty"),
            ("trigger = ['sub\\sim.exe']", "absolute path"),
            ("trigger = ['C:\\games\\sim']", "file name"),
            (
                "trigger = [\"a.exe\", \"A\"]",
                "already a trigger of this profile",
            ),
            ("priority = 3000000000", "out of range"),
        ];
        for (line, expected) in cases {
            let (_, message) = rejected(&format!("[[profile]]\nname = \"p\"\n{line}\n"));
            assert!(message.contains(expected), "{line}: {message}");
        }
        let (line, message) = rejected(
            "[[profile]]\nname = \"a\"\ntrigger = [\"sim.exe\"]\n\
             [[profile]]\nname = \"b\"\ntrigger = [\"sim\"]\n",
        );
        assert_eq!(line, 6);
        assert!(message.contains("trigger of profile \"a\""), "{message}");
        // Two paths of one name are two programs.
        ok("[[profile]]\nname = \"a\"\ntrigger = ['C:\\a\\sim.exe', 'C:\\b\\sim.exe']\n");
        assert!(parse("[[profile]]\nname = \"a\"\ntrigger = \"sim.exe\"\n").is_err());
    }

    #[test]
    fn load_reports_a_missing_file() {
        let dir = tempfile::tempdir().unwrap();
        let rejected = load(&dir.path().join("config.toml")).unwrap_err();
        assert_eq!(rejected.code, Code::ConfigRead);
    }
}
