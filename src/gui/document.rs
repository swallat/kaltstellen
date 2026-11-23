//! The configuration file as the main window edits it (WP10, spec M1 to
//! M4). The file stays the source of truth: edits go through `toml_edit`,
//! which keeps the owner's comments and layout; every edit is checked with
//! [`config::parse`] before it is accepted, and a save writes atomically
//! and refuses to overwrite a file that changed on disk meanwhile.

use std::fs;
use std::path::{Path, PathBuf};

use toml_edit::{ArrayOfTables, DocumentMut, Item, Table, value};

use crate::config::{self, Action, Config, NEUTRAL};
use crate::error::{Code, Error};

fn error(reason: impl Into<String>) -> Error {
    Error::new(Code::ConfigWrite, reason)
}

/// One open configuration file.
#[derive(Debug, Clone)]
pub struct Document {
    path: PathBuf,
    doc: DocumentMut,
    /// The file's text when it was read or last saved (`None`: no file).
    on_disk: Option<String>,
    config: Config,
    /// What the configuration rules warn about in the current text; an
    /// edit that adds a warning is refused (it would be ignored, C3).
    warnings: Vec<String>,
    /// The file uses `\r\n`; kept when it is written.
    crlf: bool,
    /// The file starts with a byte order mark; kept when it is written.
    bom: bool,
}

/// `doc` as file text with the given line endings and byte order mark.
fn render(doc: &DocumentMut, crlf: bool, bom: bool) -> String {
    let mut text = doc.to_string();
    if crlf {
        text = text.replace("\r\n", "\n").replace('\n', "\r\n");
    }
    if bom {
        text.insert(0, '\u{feff}');
    }
    text
}

/// Keys that belong to an action (C6); the rest are removed when the action
/// of an entry changes.
fn options_of(action: Action) -> &'static [&'static str] {
    match action {
        Action::Start => &["args", "cwd"],
        Action::Stop => &[
            "on_timeout",
            "restart_on_neutral",
            "args",
            "cwd",
            "confirmed",
        ],
        Action::Suspend => &["trim", "confirmed"],
        Action::Ignore => &[],
    }
}

const ACTION_OPTIONS: &[&str] = &[
    "args",
    "cwd",
    "on_timeout",
    "restart_on_neutral",
    "trim",
    "confirmed",
];

impl Document {
    /// Reads `path`; a missing file is an empty configuration. A file that
    /// is not valid is refused with the lines `check-config` would print.
    pub fn open(path: &Path) -> Result<Document, Vec<String>> {
        let text = match fs::read_to_string(path) {
            Ok(text) => Some(text),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => None,
            Err(err) => return Err(vec![format!("{}: {err}", path.display())]),
        };
        let mut document = Self::from_text(path, text.as_deref().unwrap_or(""))?;
        document.on_disk = text;
        Ok(document)
    }

    /// For tests: a document over `text` that is not on disk yet.
    pub fn from_text(path: &Path, text: &str) -> Result<Document, Vec<String>> {
        let loaded =
            config::parse(text).map_err(|rejected| config::rejected_lines(path, &rejected))?;
        let bom = text.starts_with('\u{feff}');
        let body = text.strip_prefix('\u{feff}').unwrap_or(text);
        // CRLF only when every line ends so; a file with mixed endings is
        // written with `\n`, the form toml_edit keeps.
        let crlf = body.contains("\r\n") && !body.replace("\r\n", "").contains('\n');
        let doc = body
            .replace("\r\n", "\n")
            .parse::<DocumentMut>()
            .map_err(|e| vec![format!("{}: {e}", path.display())])?;
        Ok(Document {
            path: path.to_owned(),
            doc,
            on_disk: None,
            config: loaded.config,
            warnings: loaded.warnings.iter().map(|w| w.message.clone()).collect(),
            crlf,
            bom,
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The validated configuration of the current text.
    pub fn config(&self) -> &Config {
        &self.config
    }

    /// The text as it is written: with the line endings and the byte
    /// order mark of the file as it was read.
    pub fn text(&self) -> String {
        render(&self.doc, self.crlf, self.bom)
    }

    /// Whether the document differs from the file.
    pub fn is_dirty(&self) -> bool {
        self.on_disk.as_deref().unwrap_or("") != self.text()
    }

    /// Applies `change` to a copy and keeps it only if the result is still a
    /// valid configuration (C1 to C8). The error lists every problem.
    fn edit(
        &mut self,
        change: impl FnOnce(&mut DocumentMut) -> Result<(), String>,
    ) -> Result<(), Error> {
        let mut doc = self.doc.clone();
        change(&mut doc).map_err(error)?;
        let text = doc.to_string();
        match config::parse(&text) {
            Ok(loaded) => {
                let warnings: Vec<String> =
                    loaded.warnings.iter().map(|w| w.message.clone()).collect();
                // A hotkey that steals AltGr, for example, is only warned
                // about and then ignored: saving it would look like success.
                let new: Vec<&String> = warnings
                    .iter()
                    .filter(|w| !self.warnings.contains(w))
                    .collect();
                if !new.is_empty() {
                    let new: Vec<&str> = new.iter().map(|w| w.as_str()).collect();
                    return Err(error(new.join("; ")));
                }
                self.doc = doc;
                self.config = loaded.config;
                self.warnings = warnings;
                Ok(())
            }
            Err(rejected) => {
                let reasons: Vec<String> =
                    rejected.issues.iter().map(|i| i.message.clone()).collect();
                Err(error(reasons.join("; ")))
            }
        }
    }

    pub fn add_profile(&mut self, name: &str) -> Result<(), Error> {
        let name = checked_name(name)?;
        self.edit(|doc| {
            let mut table = Table::new();
            table.insert("name", value(name.as_str()));
            profiles_mut(doc)?.push(table);
            Ok(())
        })
    }

    /// Copies `from` with all its entries; the hotkey is not copied (C5).
    pub fn copy_profile(&mut self, from: &str, to: &str) -> Result<(), Error> {
        let to = checked_name(to)?;
        self.edit(|doc| {
            let profiles = profiles_mut(doc)?;
            let index = position(profiles, from)?;
            let mut copy = profiles.get(index).expect("found").clone();
            copy.insert("name", value(to.as_str()));
            copy.remove("hotkey");
            // Placed at the end of the file, not next to the original: the
            // entries keep the positions of the original's entries
            // otherwise, and would be written under the original. The
            // comments above the original's header stay with it.
            copy.set_position(None);
            copy.decor_mut().clear();
            copy.decor_mut().set_prefix("\n");
            if let Some(apps) = copy.get_mut("app").and_then(Item::as_array_of_tables_mut) {
                for app in apps.iter_mut() {
                    app.set_position(None);
                }
            }
            profiles.push(copy);
            Ok(())
        })
    }

    pub fn rename_profile(&mut self, from: &str, to: &str) -> Result<(), Error> {
        let to = checked_name(to)?;
        self.edit(|doc| {
            let profiles = profiles_mut(doc)?;
            let index = position(profiles, from)?;
            let table = profiles.get_mut(index).expect("found");
            // Keeps the comments of the old value.
            set_keep(table, "name", to.as_str());
            Ok(())
        })
    }

    /// Removes the profile. Switching away from it first, if it is active,
    /// is the caller's job (M1).
    pub fn delete_profile(&mut self, name: &str) -> Result<(), Error> {
        self.edit(|doc| {
            let profiles = profiles_mut(doc)?;
            let index = position(profiles, name)?;
            profiles.remove(index);
            if profiles.is_empty() {
                doc.remove("profile");
            }
            Ok(())
        })
    }

    /// Adds an entry; `action` `None` takes `settings.default_action`.
    pub fn add_entry(
        &mut self,
        profile: &str,
        exe: &str,
        path: Option<&Path>,
        action: Option<Action>,
        confirmed: bool,
    ) -> Result<(), Error> {
        self.edit(|doc| {
            let table = profile_mut(doc, profile)?;
            let mut entry = Table::new();
            entry.insert("exe", value(exe));
            if let Some(path) = path {
                entry.insert("path", value(path.to_string_lossy().as_ref()));
            }
            if let Some(action) = action {
                entry.insert("action", value(action_name(action)));
            }
            if confirmed {
                entry.insert("confirmed", value(true));
            }
            apps_mut(table)?.push(entry);
            Ok(())
        })
    }

    pub fn remove_entry(&mut self, profile: &str, index: usize) -> Result<(), Error> {
        self.edit(|doc| {
            let table = profile_mut(doc, profile)?;
            let apps = apps_mut(table)?;
            if index >= apps.len() {
                return Err(format!("profile \"{profile}\" has no entry {}", index + 1));
            }
            apps.remove(index);
            if apps.is_empty() {
                table.remove("app");
            }
            Ok(())
        })
    }

    /// M3: changes the action of an entry and drops options the new action
    /// does not take (C6). `confirmed` survives between stop and suspend.
    pub fn set_action(&mut self, profile: &str, index: usize, action: Action) -> Result<(), Error> {
        self.edit(|doc| {
            let table = profile_mut(doc, profile)?;
            let apps = apps_mut(table)?;
            let entry = apps
                .get_mut(index)
                .ok_or_else(|| format!("profile \"{profile}\" has no entry {}", index + 1))?;
            let keep = options_of(action);
            for key in ACTION_OPTIONS {
                if !keep.contains(key) {
                    entry.remove(key);
                }
            }
            if action == Action::Stop
                && !entry
                    .get("restart_on_neutral")
                    .and_then(Item::as_bool)
                    .unwrap_or(false)
            {
                entry.remove("args");
                entry.remove("cwd");
            }
            set_keep(entry, "action", action_name(action));
            Ok(())
        })
    }

    /// Sets `confirmed = true` on an entry (P6, after the user agreed).
    pub fn confirm(&mut self, profile: &str, index: usize) -> Result<(), Error> {
        self.edit(|doc| {
            let table = profile_mut(doc, profile)?;
            let entry = apps_mut(table)?
                .get_mut(index)
                .ok_or_else(|| format!("profile \"{profile}\" has no entry {}", index + 1))?;
            set_keep(entry, "confirmed", true);
            Ok(())
        })
    }

    /// Sets or removes the hotkey of a profile (C5, C7 checked by the
    /// configuration rules).
    pub fn set_hotkey(&mut self, profile: &str, hotkey: Option<&str>) -> Result<(), Error> {
        self.edit(|doc| {
            let table = profile_mut(doc, profile)?;
            match hotkey.map(str::trim).filter(|h| !h.is_empty()) {
                Some(hotkey) => {
                    set_keep(table, "hotkey", hotkey);
                }
                None => {
                    table.remove("hotkey");
                }
            }
            Ok(())
        })
    }

    /// Replaces the options of an entry with `edit` (M3, C6). Keys not
    /// touched by the edit keep their comments; options the action does not
    /// take are removed. `trim` is written only when it differs from
    /// `settings.trim`, `on_timeout` only when it is `keep`.
    pub fn update_entry(
        &mut self,
        profile: &str,
        index: usize,
        edit: &EntryEdit,
    ) -> Result<(), Error> {
        let default_trim = self.config.settings.trim;
        self.edit(|doc| {
            let table = profile_mut(doc, profile)?;
            let entry = apps_mut(table)?
                .get_mut(index)
                .ok_or_else(|| format!("profile \"{profile}\" has no entry {}", index + 1))?;
            set_keep(entry, "exe", edit.exe.trim());
            set_or_remove(entry, "path", edit.path.as_deref());
            set_keep(entry, "action", action_name(edit.action));
            // The options the edit asks for; every other one goes.
            let mut wanted: Vec<(&str, toml_edit::Value)> = Vec::new();
            let launch = match edit.action {
                Action::Start => true,
                Action::Stop => edit.restart_on_neutral,
                _ => false,
            };
            if launch {
                if !edit.args.is_empty() {
                    let args: toml_edit::Array = edit.args.iter().map(String::as_str).collect();
                    wanted.push(("args", args.into()));
                }
                if let Some(cwd) = edit.cwd.as_deref().map(str::trim).filter(|c| !c.is_empty()) {
                    wanted.push(("cwd", cwd.into()));
                }
            }
            match edit.action {
                Action::Stop => {
                    if edit.on_timeout == config::OnTimeout::Keep {
                        wanted.push(("on_timeout", "keep".into()));
                    }
                    if edit.restart_on_neutral {
                        wanted.push(("restart_on_neutral", true.into()));
                    }
                }
                Action::Suspend => {
                    if let Some(trim) = edit.trim
                        && trim != default_trim
                    {
                        wanted.push(("trim", trim.into()));
                    }
                }
                Action::Start | Action::Ignore => {}
            }
            if edit.confirmed && matches!(edit.action, Action::Stop | Action::Suspend) {
                wanted.push(("confirmed", true.into()));
            }
            for key in ACTION_OPTIONS {
                match wanted.iter().position(|(k, _)| k == key) {
                    Some(at) => {
                        let (_, new) = wanted.swap_remove(at);
                        set_keep(entry, key, new);
                    }
                    None => {
                        entry.remove(key);
                    }
                }
            }
            Ok(())
        })
    }

    /// Writes the document: validated again, atomically (temporary file,
    /// then rename), and only if the file on disk is still what was read.
    pub fn save(&mut self) -> Result<(), Error> {
        let text = self.text();
        config::parse(&text).map_err(|rejected| error(rejected.to_string()))?;
        let current = match fs::read_to_string(&self.path) {
            Ok(text) => Some(text),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => None,
            Err(err) => return Err(error(format!("{}: {err}", self.path.display()))),
        };
        if current != self.on_disk {
            return Err(error(format!(
                "{} was changed by another program; reload it before saving",
                self.path.display()
            )));
        }
        let dir = self.path.parent().unwrap_or(Path::new("."));
        let tmp = self.path.with_extension("toml.tmp");
        // A change that lands between the read above and the rename below
        // is overwritten; the window of time is a few milliseconds, and the
        // file has no lock that other editors would respect.
        fs::create_dir_all(dir)
            .and_then(|()| fs::write(&tmp, &text))
            .and_then(|()| fs::rename(&tmp, &self.path))
            .map_err(|e| {
                let _ = fs::remove_file(&tmp);
                error(format!("{}: {e}", self.path.display()))
            })?;
        self.on_disk = Some(text);
        Ok(())
    }
}

/// All options of one entry as the main window edits them.
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EntryEdit {
    pub exe: String,
    #[serde(default)]
    pub path: Option<String>,
    pub action: Action,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub cwd: Option<String>,
    #[serde(default = "kill")]
    pub on_timeout: config::OnTimeout,
    #[serde(default)]
    pub restart_on_neutral: bool,
    #[serde(default)]
    pub trim: Option<bool>,
    #[serde(default)]
    pub confirmed: bool,
}

fn kill() -> config::OnTimeout {
    config::OnTimeout::Kill
}

fn set_or_remove(table: &mut Table, key: &str, text: Option<&str>) {
    match text.map(str::trim).filter(|t| !t.is_empty()) {
        Some(text) => set_keep(table, key, text),
        None => {
            table.remove(key);
        }
    }
}

/// Whether two values are equal, ignoring how they are written.
fn same_value(a: &toml_edit::Value, b: &toml_edit::Value) -> bool {
    let plain = |v: &toml_edit::Value| {
        let mut v = v.clone();
        v.decor_mut().clear();
        v.to_string().trim().to_owned()
    };
    match (a, b) {
        (toml_edit::Value::String(a), toml_edit::Value::String(b)) => a.value() == b.value(),
        (toml_edit::Value::Boolean(a), toml_edit::Value::Boolean(b)) => a.value() == b.value(),
        (toml_edit::Value::Array(a), toml_edit::Value::Array(b)) => {
            a.len() == b.len() && a.iter().zip(b.iter()).all(|(x, y)| same_value(x, y))
        }
        _ => plain(a) == plain(b),
    }
}

/// Sets `key` to `new`. An existing key keeps its place and the comments
/// around it and its value; only the value itself changes.
fn set_keep(table: &mut Table, key: &str, new: impl Into<toml_edit::Value>) {
    let mut new: toml_edit::Value = new.into();
    match table.get_mut(key) {
        Some(item) => {
            if let Some(old) = item.as_value() {
                if same_value(old, &new) {
                    // Exactly as written: quotes, spacing, comments.
                    return;
                }
                *new.decor_mut() = old.decor().clone();
            }
            *item = Item::Value(new);
        }
        None => {
            table.insert(key, Item::Value(new));
        }
    }
}

pub fn action_name(action: Action) -> &'static str {
    match action {
        Action::Start => "start",
        Action::Stop => "stop",
        Action::Suspend => "suspend",
        Action::Ignore => "ignore",
    }
}

fn checked_name(name: &str) -> Result<String, Error> {
    let name = name.trim();
    if name.is_empty() {
        return Err(error("a profile needs a name"));
    }
    if name.eq_ignore_ascii_case(NEUTRAL) {
        return Err(error(
            "\"neutral\" is built in and cannot be used as a name",
        ));
    }
    Ok(name.to_owned())
}

fn profiles_mut(doc: &mut DocumentMut) -> Result<&mut ArrayOfTables, String> {
    let item = doc
        .entry("profile")
        .or_insert(Item::ArrayOfTables(ArrayOfTables::new()));
    item.as_array_of_tables_mut()
        .ok_or_else(|| "\"profile\" is not a list of tables".to_owned())
}

fn position(profiles: &ArrayOfTables, name: &str) -> Result<usize, String> {
    let wanted = name.trim().to_lowercase();
    profiles
        .iter()
        .position(|t| {
            t.get("name")
                .and_then(Item::as_str)
                .is_some_and(|n| n.trim().to_lowercase() == wanted)
        })
        .ok_or_else(|| format!("no profile named \"{name}\""))
}

fn profile_mut<'a>(doc: &'a mut DocumentMut, name: &str) -> Result<&'a mut Table, String> {
    let profiles = profiles_mut(doc)?;
    let index = position(profiles, name)?;
    Ok(profiles.get_mut(index).expect("found"))
}

/// The entries of a profile as a list of tables. An inline list
/// (`app = [{ exe = "a.exe" }, …]`, which the configuration accepts) is
/// turned into the same entries as `[[profile.app]]` tables first, so an
/// edit never loses one.
fn apps_mut(profile: &mut Table) -> Result<&mut ArrayOfTables, String> {
    let item = profile
        .entry("app")
        .or_insert(Item::ArrayOfTables(ArrayOfTables::new()));
    if item.as_array_of_tables().is_none() {
        let empty = item.as_array().is_some_and(toml_edit::Array::is_empty);
        let taken = std::mem::replace(item, Item::None);
        match taken.into_array_of_tables() {
            Ok(tables) => *item = Item::ArrayOfTables(tables),
            Err(_) if empty => *item = Item::ArrayOfTables(ArrayOfTables::new()),
            Err(original) => {
                *item = original;
                return Err("\"app\" of this profile is not a list of entries".to_owned());
            }
        }
    }
    Ok(item.as_array_of_tables_mut().expect("just made"))
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"# My profiles. Keep this comment.
[settings]
stop_timeout_s = 5   # short

[[profile]]
name = "focus"   # the main one
hotkey = "Ctrl+Shift+Alt+F1"

  # browser is heavy
  [[profile.app]]
  exe = "browser.exe"
  action = "suspend"
  trim = false
"#;

    fn doc() -> Document {
        Document::from_text(Path::new("C:\\cfg\\config.toml"), SAMPLE).unwrap()
    }

    fn names(d: &Document) -> Vec<String> {
        d.config().profiles.iter().map(|p| p.name.clone()).collect()
    }

    #[test]
    fn m1_create_copy_rename_delete_keep_comments() {
        let mut d = doc();
        d.add_profile("music").unwrap();
        d.copy_profile("focus", "focus copy").unwrap();
        assert_eq!(names(&d), vec!["focus", "music", "focus copy"]);
        let copy = &d.config().profiles[2];
        assert_eq!(copy.apps.len(), 1);
        assert_eq!(copy.hotkey, None, "the hotkey is not copied (C5)");
        d.rename_profile("MUSIC", "audio").unwrap();
        assert_eq!(names(&d), vec!["focus", "audio", "focus copy"]);
        d.delete_profile("focus copy").unwrap();
        let text = d.text();
        for kept in [
            "# My profiles. Keep this comment.",
            "# short",
            "# the main one",
            "# browser is heavy",
        ] {
            assert!(text.contains(kept), "lost {kept:?}:\n{text}");
        }
        assert!(text.starts_with(SAMPLE.lines().next().unwrap()));
    }

    #[test]
    fn invalid_edits_are_refused_and_change_nothing() {
        let mut d = doc();
        let before = d.text();
        assert!(
            d.add_profile("Focus")
                .unwrap_err()
                .reason
                .to_lowercase()
                .contains("focus"),
            "C1 duplicate"
        );
        assert!(d.add_profile("neutral").is_err());
        assert!(d.add_profile("  ").is_err());
        assert!(d.copy_profile("nope", "x").is_err());
        // C6: a start entry needs a path.
        assert!(d.set_action("focus", 0, Action::Start).is_err());
        // P6: hard-protected programs cannot be added (T9).
        assert!(
            d.add_entry("focus", "csrss.exe", None, None, false)
                .is_err()
        );
        assert_eq!(d.text(), before);
    }

    #[test]
    fn m3_entries_and_actions_follow_c6() {
        let mut d = doc();
        d.add_entry("focus", "chat.exe", None, Some(Action::Stop), false)
            .unwrap();
        d.add_entry(
            "focus",
            "tool.exe",
            Some(Path::new("C:\\tools\\tool.exe")),
            Some(Action::Start),
            false,
        )
        .unwrap();
        // Suspend -> stop: `trim` goes, the entry stays valid.
        d.set_action("focus", 0, Action::Stop).unwrap();
        let apps = &d.config().profiles[0].apps;
        assert_eq!(apps.len(), 3);
        assert!(matches!(apps[0].action, config::AppAction::Stop { .. }));
        assert!(!d.text().contains("trim = false"));
        d.set_action("focus", 0, Action::Ignore).unwrap();
        d.remove_entry("focus", 1).unwrap();
        let apps = &d.config().profiles[0].apps;
        assert_eq!(apps.len(), 2);
        assert_eq!(apps[1].exe, "tool.exe");
        assert!(d.remove_entry("focus", 5).is_err());
    }

    #[test]
    fn p6_confirmed_is_written_for_warn_list_programs() {
        let mut d = doc();
        d.add_entry("focus", "anydesk.exe", None, None, true)
            .unwrap();
        assert!(d.config().profiles[0].apps[1].confirmed);
        assert!(d.text().contains("confirmed = true"));
    }

    fn edit(action: Action) -> EntryEdit {
        EntryEdit {
            exe: "chat.exe".into(),
            path: None,
            action,
            args: Vec::new(),
            cwd: None,
            on_timeout: config::OnTimeout::Kill,
            restart_on_neutral: false,
            trim: None,
            confirmed: false,
        }
    }

    #[test]
    fn m3_update_entry_writes_the_options_of_the_action_only() {
        let mut d = doc();
        let stop = EntryEdit {
            on_timeout: config::OnTimeout::Keep,
            restart_on_neutral: true,
            args: vec!["--min".into(), "a b".into()],
            cwd: Some("C:\\chat".into()),
            trim: Some(true),
            ..edit(Action::Stop)
        };
        d.update_entry("focus", 0, &stop).unwrap();
        let app = &d.config().profiles[0].apps[0];
        assert_eq!(
            app.action,
            config::AppAction::Stop {
                on_timeout: config::OnTimeout::Keep,
                restart_on_neutral: true,
                args: vec!["--min".into(), "a b".into()],
                cwd: Some(PathBuf::from("C:\\chat")),
            }
        );
        assert!(!d.text().contains("trim"), "trim is not a stop option");
        assert!(
            d.text().contains("# browser is heavy"),
            "comment above stays"
        );
        // Without restart, args and cwd go (C6).
        d.update_entry(
            "focus",
            0,
            &EntryEdit {
                restart_on_neutral: false,
                ..stop.clone()
            },
        )
        .unwrap();
        assert!(!d.text().contains("--min"));
        // Suspend: trim only when it differs from settings.trim (true here).
        d.update_entry(
            "focus",
            0,
            &EntryEdit {
                trim: Some(true),
                ..edit(Action::Suspend)
            },
        )
        .unwrap();
        assert!(!d.text().contains("trim"));
        d.update_entry(
            "focus",
            0,
            &EntryEdit {
                trim: Some(false),
                ..edit(Action::Suspend)
            },
        )
        .unwrap();
        assert!(d.text().contains("trim = false"));
        // Start without path is refused, nothing changes.
        let before = d.text();
        assert!(d.update_entry("focus", 0, &edit(Action::Start)).is_err());
        assert_eq!(d.text(), before);
    }

    #[test]
    fn c5_c7_hotkeys_are_validated() {
        let mut d = doc();
        d.add_profile("music").unwrap();
        assert!(
            d.set_hotkey("music", Some("Ctrl+Shift+Alt+F1")).is_err(),
            "C5 duplicate"
        );
        assert!(
            d.set_hotkey("music", Some("Ctrl+Nope")).is_err(),
            "C7 syntax"
        );
        d.set_hotkey("music", Some("Ctrl+Shift+Alt+F2")).unwrap();
        assert_eq!(
            d.config().profiles[1].hotkey.unwrap().to_string(),
            "Ctrl+Shift+Alt+F2"
        );
        d.set_hotkey("music", None).unwrap();
        assert_eq!(d.config().profiles[1].hotkey, None);
    }

    #[test]
    fn save_is_atomic_and_refuses_a_file_changed_meanwhile() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("config.toml");
        let mut d = Document::open(&file).unwrap();
        assert!(d.config().profiles.is_empty(), "no file: empty");
        d.add_profile("focus").unwrap();
        assert!(d.is_dirty());
        d.save().unwrap();
        assert!(!d.is_dirty());
        assert!(!dir.path().join("config.toml.tmp").exists());
        let reread = Document::open(&file).unwrap();
        assert_eq!(names(&reread), vec!["focus"]);

        fs::write(&file, "[[profile]]\nname = \"other\"\n").unwrap();
        d.add_profile("music").unwrap();
        let err = d.save().unwrap_err();
        assert!(
            err.reason.contains("changed by another program"),
            "{}",
            err.reason
        );
        assert_eq!(
            fs::read_to_string(&file).unwrap(),
            "[[profile]]\nname = \"other\"\n"
        );

        fs::write(&file, "typo = 1\n").unwrap();
        assert!(
            Document::open(&file).is_err(),
            "an invalid file is not opened"
        );
    }

    const TWO: &str = r#"# about focus
[[profile]]
name = "focus"

  [[profile.app]]
  exe = "browser.exe"
  action = "suspend"

[[profile]]
name = "music"

  [[profile.app]]
  exe = "player.exe"
"#;

    #[test]
    fn m1_copy_of_a_profile_that_is_not_the_last() {
        let mut d = Document::from_text(Path::new("config.toml"), TWO).unwrap();
        d.copy_profile("focus", "focus 2").unwrap();
        let profiles = &d.config().profiles;
        let apps =
            |n: usize| -> Vec<&str> { profiles[n].apps.iter().map(|a| a.exe.as_str()).collect() };
        assert_eq!(names(&d), ["focus", "music", "focus 2"]);
        assert_eq!(apps(0), ["browser.exe"]);
        assert_eq!(apps(1), ["player.exe"]);
        assert_eq!(apps(2), ["browser.exe"], "{}", d.text());
        // The comment above the original stays there only.
        assert_eq!(d.text().matches("# about focus").count(), 1);
        assert!(
            d.text().trim_end().ends_with("action = \"suspend\""),
            "{}",
            d.text()
        );
    }

    #[test]
    fn m3_an_inline_list_of_entries_is_kept_when_edited() {
        let inline = "[[profile]]\nname = \"focus\"\napp = [ { exe = \"a.exe\" }, { exe = \"b.exe\", action = \"stop\" } ]\n\n[[profile]]\nname = \"music\"\n";
        let mut d = Document::from_text(Path::new("config.toml"), inline).unwrap();
        d.add_entry("focus", "c.exe", None, None, false).unwrap();
        let exes: Vec<&str> = d.config().profiles[0]
            .apps
            .iter()
            .map(|a| a.exe.as_str())
            .collect();
        assert_eq!(exes, ["a.exe", "b.exe", "c.exe"], "{}", d.text());
        assert!(d.config().profiles[1].apps.is_empty());
        d.set_action("focus", 1, Action::Suspend).unwrap();
        assert_eq!(names(&d), ["focus", "music"]);
    }

    #[test]
    fn m3_an_edit_keeps_the_comments_of_the_keys_it_does_not_change() {
        let text = "[[profile]]\nname = \"chat\"   # work chat\nhotkey = \"Ctrl+Shift+Alt+F2\" # the second\n\n  [[profile.app]]\n  # why it is here\n  exe = \"chat.exe\" # the chat\n  action = \"stop\" # why stop\n  # often hangs\n  on_timeout = \"keep\" # it hangs often\n";
        let mut d = Document::from_text(Path::new("config.toml"), text).unwrap();
        let unchanged = EntryEdit {
            exe: "chat.exe".into(),
            path: None,
            action: Action::Stop,
            args: Vec::new(),
            cwd: None,
            on_timeout: config::OnTimeout::Keep,
            restart_on_neutral: false,
            trim: None,
            confirmed: false,
        };
        d.update_entry("chat", 0, &unchanged).unwrap();
        assert_eq!(d.text(), text);
        d.set_hotkey("chat", Some("Ctrl+Shift+Alt+F3")).unwrap();
        d.set_action("chat", 0, Action::Stop).unwrap();
        d.rename_profile("chat", "talk").unwrap();
        for kept in [
            "# work chat",
            "# the second",
            "# why it is here",
            "# the chat",
            "# why stop",
            "# often hangs",
            "# it hangs often",
        ] {
            assert!(d.text().contains(kept), "{kept} lost:\n{}", d.text());
        }
    }

    #[test]
    fn m1_an_edit_that_would_only_be_warned_about_is_refused() {
        let mut d = doc();
        let err = d.set_hotkey("focus", Some("Ctrl+Alt+A")).unwrap_err();
        assert!(err.reason.contains("AltGr"), "{err}");
        assert_eq!(
            d.config().profiles[0]
                .hotkey
                .as_ref()
                .map(ToString::to_string),
            Some("Ctrl+Shift+Alt+F1".to_owned())
        );
    }

    #[test]
    fn save_keeps_line_endings_and_byte_order_mark() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("config.toml");
        let text = "\u{feff}# notes\r\n[[profile]]\r\nname = \"focus\"\r\n";
        fs::write(&file, text).unwrap();
        let mut d = Document::open(&file).unwrap();
        assert!(!d.is_dirty(), "unchanged right after reading");
        d.add_profile("music").unwrap();
        d.save().unwrap();
        let saved = fs::read_to_string(&file).unwrap();
        assert!(saved.starts_with('\u{feff}'), "{saved:?}");
        assert!(!saved.replace("\r\n", "").contains('\n'), "{saved:?}");
        assert!(saved.contains("name = \"music\"\r\n"), "{saved:?}");
        assert!(Document::open(&file).is_ok());
    }
}
