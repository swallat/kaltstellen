//! The bridge between the web page of the main window and the Rust side
//! (WP10, spec M6; message format in `docs/gui-bridge.md`).
//!
//! The page is a thin view: it asks for state and sends intents. Every
//! message from it is untrusted input: size-limited, versioned, parsed
//! strictly (unknown fields refused) and checked again here, whatever the
//! page already checked. The page never touches files or processes itself;
//! the [`Host`] does the few things that need the outside world.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::document::{Document, EntryEdit, action_name};
pub use super::icons::Wanted;
use super::model::{self, AddCheck, Candidate};
use crate::config::{App, AppAction, Config, NEUTRAL, OnTimeout};
use crate::protection::{self, Protection};
use crate::unassigned::Unassigned;

pub const VERSION: u64 = 1;
/// Longest message the page may send.
pub const MAX_MESSAGE: usize = 64 * 1024;
/// Longest single text field (names, paths, arguments).
const MAX_TEXT: usize = 4096;

/// What the bridge needs from the outside world. The real host lives in
/// `gui::mod`; tests use a fake.
pub trait Host {
    fn active_profile(&self) -> String;
    fn running(&self) -> Vec<Candidate>;
    fn unassigned(&self, config: &Config, profile: &str) -> Vec<Unassigned>;
    fn dismiss(&self, profile: &str, exe: &str) -> Result<(), String>;
    /// `None` when the Run key cannot be read.
    fn autostart(&self) -> Option<bool>;
    fn set_autostart(&self, on: bool) -> Result<(), String>;
    /// The file dialog for an executable.
    fn pick_executable(&self) -> Option<PathBuf>;
    /// Starts `kaltstellen switch PROFILE`; the result comes back later
    /// through [`Bridge::applied`].
    fn apply(&self, profile: &str);
    /// UP1: what is known about updates.
    fn update_info(&self) -> UpdateInfo;
    fn set_update_mode(&self, mode: crate::update::Mode) -> Result<(), String>;
    /// Starts a check; the end comes back through [`Bridge::checked`].
    fn check_updates(&self);
    /// Opens the visible update (a console window running `kaltstellen
    /// update`), as the tray's `Install update…` does.
    fn install_update(&self) -> Result<(), String>;
    /// M2: the icon of each program as a PNG `data:` URL, `None` where
    /// there is none, in the order asked.
    fn icons(&self, wanted: &[Wanted], size: u32) -> Vec<Option<String>>;
}

/// Most programs one `icons` request may name.
const MAX_ICONS: usize = 256;

/// One program of an `icons` request.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IconProgram {
    pub exe: String,
    #[serde(default)]
    pub path: Option<String>,
}

/// UP1 in the main window.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct UpdateInfo {
    /// The installed version.
    pub version: String,
    /// `auto`, `manual` or `off`.
    pub mode: &'static str,
    /// `KALTSTELLEN_UPDATE` decides; the window cannot change the mode.
    pub mode_locked: bool,
    /// A newer version found by the last check.
    pub available: Option<String>,
    /// Why this copy cannot install it here.
    pub blocked: Option<String>,
    /// The available version could not be installed here before: it is not
    /// installed automatically, only with `install_update`.
    pub failed: bool,
    /// Seconds since 1970 of the last check.
    pub last_check: Option<u64>,
    /// Why the last check failed.
    pub error: Option<String>,
    /// A check started from the window runs.
    pub checking: bool,
}

/// Intents and queries of the page.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum Request {
    GetState,
    CreateProfile {
        name: String,
    },
    CopyProfile {
        from: String,
        to: String,
    },
    RenameProfile {
        from: String,
        to: String,
    },
    DeleteProfile {
        name: String,
    },
    SetHotkey {
        profile: String,
        hotkey: Option<String>,
    },
    CheckProgram {
        exe: String,
    },
    AddEntry {
        profile: String,
        exe: String,
        #[serde(default)]
        path: Option<String>,
        #[serde(default)]
        confirmed: bool,
    },
    UpdateEntry {
        profile: String,
        index: usize,
        entry: EntryEdit,
    },
    RemoveEntry {
        profile: String,
        index: usize,
    },
    PickExecutable,
    Running,
    Unassigned {
        profile: String,
    },
    Dismiss {
        profile: String,
        exe: String,
    },
    Apply {
        profile: String,
    },
    SetAutostart {
        on: bool,
    },
    SetUpdateMode {
        mode: String,
    },
    CheckUpdates,
    InstallUpdate,
    Icons {
        programs: Vec<IconProgram>,
        /// Device pixels: the page's size times its DPI scale.
        size: u32,
    },
}

/// One entry as the page shows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct EntryView {
    pub exe: String,
    pub path: Option<String>,
    pub action: &'static str,
    pub args: Vec<String>,
    pub cwd: Option<String>,
    pub on_timeout: &'static str,
    pub restart_on_neutral: bool,
    pub trim: Option<bool>,
    pub confirmed: bool,
    /// `none`, `warn` or `hard`.
    pub protection: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ProfileView {
    pub name: String,
    pub hotkey: Option<String>,
    pub active: bool,
    pub entries: Vec<EntryView>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct StateView {
    pub config_file: String,
    pub active: String,
    pub default_action: &'static str,
    pub default_trim: bool,
    pub profiles: Vec<ProfileView>,
    pub autostart: Option<bool>,
    pub applying: bool,
    pub status: String,
    pub update: UpdateInfo,
}

fn protection_name(exe: &str) -> &'static str {
    match protection::of(exe) {
        Protection::None => "none",
        Protection::Warn(_) => "warn",
        Protection::Hard => "hard",
    }
}

fn entry_view(app: &App) -> EntryView {
    let path_text = |p: &Option<PathBuf>| p.as_ref().map(|p| p.to_string_lossy().into_owned());
    let mut view = EntryView {
        exe: app.exe.clone(),
        path: path_text(&app.path),
        action: "ignore",
        args: Vec::new(),
        cwd: None,
        on_timeout: "kill",
        restart_on_neutral: false,
        trim: None,
        confirmed: app.confirmed,
        protection: protection_name(&app.exe),
    };
    match &app.action {
        AppAction::Start { args, cwd } => {
            view.action = "start";
            view.args = args.clone();
            view.cwd = path_text(cwd);
        }
        AppAction::Stop {
            on_timeout,
            restart_on_neutral,
            args,
            cwd,
        } => {
            view.action = "stop";
            view.on_timeout = if *on_timeout == OnTimeout::Keep {
                "keep"
            } else {
                "kill"
            };
            view.restart_on_neutral = *restart_on_neutral;
            view.args = args.clone();
            view.cwd = path_text(cwd);
        }
        AppAction::Suspend { trim } => {
            view.action = "suspend";
            view.trim = Some(*trim);
        }
        AppAction::Ignore => {}
    }
    view
}

/// Checks text fields of a request before anything else looks at them.
fn check_text(texts: &[&str]) -> Result<(), String> {
    for text in texts {
        if text.len() > MAX_TEXT {
            return Err("a text field is too long".to_owned());
        }
        if text.chars().any(|c| c.is_control()) {
            return Err("a text field contains control characters".to_owned());
        }
    }
    Ok(())
}

/// Requests that change the configuration file.
fn is_edit(request: &Request) -> bool {
    matches!(
        request,
        Request::CreateProfile { .. }
            | Request::CopyProfile { .. }
            | Request::RenameProfile { .. }
            | Request::DeleteProfile { .. }
            | Request::SetHotkey { .. }
            | Request::AddEntry { .. }
            | Request::UpdateEntry { .. }
            | Request::RemoveEntry { .. }
    )
}

fn check_path(path: Option<&str>) -> Result<(), String> {
    if let Some(path) = path.filter(|p| !p.trim().is_empty())
        && !Path::new(path.trim()).is_absolute()
    {
        return Err(format!("\"{path}\" is not a full path"));
    }
    Ok(())
}

pub struct Bridge<H: Host> {
    document: Document,
    host: H,
    status: String,
    /// M1: delete this profile once the switch to `neutral` is done.
    pending_delete: Option<String>,
    applying: bool,
    checking: bool,
    /// The document before the edit that is being handled.
    snapshot: Option<Document>,
}

impl<H: Host> Bridge<H> {
    pub fn new(document: Document, host: H) -> Self {
        let status = "Changes are saved at once.".to_owned();
        Self {
            document,
            host,
            status,
            pending_delete: None,
            applying: false,
            checking: false,
            snapshot: None,
        }
    }

    pub fn document(&self) -> &Document {
        &self.document
    }

    pub fn state(&self) -> StateView {
        let active = self.host.active_profile();
        let config = self.document.config();
        StateView {
            config_file: self.document.path().display().to_string(),
            default_action: action_name(config.settings.default_action),
            default_trim: config.settings.trim,
            profiles: config
                .profiles
                .iter()
                .map(|p| ProfileView {
                    name: p.name.clone(),
                    hotkey: p.hotkey.map(|h| h.to_string()),
                    active: p.name.eq_ignore_ascii_case(&active),
                    entries: p.apps.iter().map(entry_view).collect(),
                })
                .collect(),
            active,
            autostart: self.host.autostart(),
            applying: self.applying,
            status: self.status.clone(),
            update: UpdateInfo {
                checking: self.checking,
                ..self.host.update_info()
            },
        }
    }

    /// Handles one raw message from the page and returns the reply.
    pub fn handle(&mut self, raw: &str) -> String {
        let (id, result) = match self.parse(raw) {
            Ok((id, request)) => {
                // An edit whose save fails is taken back (see `saved`).
                self.snapshot = is_edit(&request).then(|| self.document.clone());
                let result = self.dispatch(request);
                self.snapshot = None;
                (id, result)
            }
            Err((id, error)) => (id, Err(error)),
        };
        let reply = match result {
            Ok(data) => {
                let mut reply = json!({"v": VERSION, "id": id, "ok": true, "state": self.state()});
                if let Some(data) = data {
                    reply["data"] = data;
                }
                reply
            }
            Err(error) => {
                json!({"v": VERSION, "id": id, "ok": false, "error": error, "state": self.state()})
            }
        };
        reply.to_string()
    }

    fn parse(&self, raw: &str) -> Result<(u64, Request), (u64, String)> {
        if raw.len() > MAX_MESSAGE {
            return Err((0, "message too long".to_owned()));
        }
        let mut value: Value =
            serde_json::from_str(raw).map_err(|e| (0, format!("not a JSON message: {e}")))?;
        let object = value
            .as_object_mut()
            .ok_or((0, "a message is a JSON object".to_owned()))?;
        let id = object.remove("id").and_then(|v| v.as_u64()).unwrap_or(0);
        match object.remove("v").and_then(|v| v.as_u64()) {
            Some(VERSION) => {}
            Some(other) => {
                return Err((
                    id,
                    format!("bridge version {other} is not supported (this is {VERSION})"),
                ));
            }
            None => return Err((id, "message without version".to_owned())),
        }
        let request =
            serde_json::from_value(value).map_err(|e| (id, format!("malformed request: {e}")))?;
        Ok((id, request))
    }

    fn saved(
        &mut self,
        what: String,
        result: crate::error::Result<()>,
    ) -> Result<Option<Value>, String> {
        let result = result.and_then(|()| self.document.save());
        match result {
            Ok(()) => {
                self.status = format!("{what}; saved.");
                Ok(None)
            }
            Err(err) => {
                if err.reason.contains("changed by another program")
                    && let Ok(fresh) = Document::open(self.document.path())
                {
                    // The file on disk wins; the edit has to be repeated.
                    self.document = fresh;
                    self.status = format!("{err}. Reloaded; please repeat the change.");
                } else {
                    // Not saved: the window shows the file as it is, and a
                    // later edit does not write this one by the way.
                    if let Some(before) = self.snapshot.take() {
                        self.document = before;
                    }
                    self.status = format!("{err}. Not saved.");
                }
                Err(self.status.clone())
            }
        }
    }

    fn dispatch(&mut self, request: Request) -> Result<Option<Value>, String> {
        // M1: while a profile waits to be deleted, no profile may be edited,
        // so the one that is deleted is the one that was meant.
        if let Some(name) = &self.pending_delete
            && is_edit(&request)
        {
            return Err(format!(
                "\"{name}\" is being deleted; wait until the switch to neutral has ended"
            ));
        }
        match request {
            Request::GetState => Ok(None),
            Request::CreateProfile { name } => {
                check_text(&[&name])?;
                let result = self.document.add_profile(&name);
                self.saved(format!("Profile \"{}\" created", name.trim()), result)
            }
            Request::CopyProfile { from, to } => {
                check_text(&[&from, &to])?;
                let result = self.document.copy_profile(&from, &to);
                self.saved(
                    format!("Profile \"{from}\" copied to \"{}\"", to.trim()),
                    result,
                )
            }
            Request::RenameProfile { from, to } => {
                check_text(&[&from, &to])?;
                let result = self.document.rename_profile(&from, &to);
                self.saved(
                    format!("Profile \"{from}\" renamed to \"{}\"", to.trim()),
                    result,
                )
            }
            Request::DeleteProfile { name } => {
                check_text(&[&name])?;
                if self.document.config().profile(&name).is_none() {
                    return Err(format!("no profile named \"{name}\""));
                }
                if self.applying {
                    // The running switch may be the one to this profile.
                    return Err(format!(
                        "a switch is running; delete \"{}\" when it has ended",
                        name.trim()
                    ));
                }
                if self.host.active_profile().eq_ignore_ascii_case(name.trim()) {
                    // M1: switch to neutral first, delete when that worked.
                    self.start_apply(NEUTRAL)?;
                    self.pending_delete = Some(name.trim().to_owned());
                    self.status =
                        format!("Switching to neutral before deleting \"{}\"…", name.trim());
                    return Ok(None);
                }
                let result = self.document.delete_profile(&name);
                self.saved(format!("Profile \"{}\" deleted", name.trim()), result)
            }
            Request::SetHotkey { profile, hotkey } => {
                check_text(&[&profile, hotkey.as_deref().unwrap_or("")])?;
                let result = self.document.set_hotkey(&profile, hotkey.as_deref());
                let what = match hotkey.as_deref().map(str::trim).filter(|h| !h.is_empty()) {
                    Some(h) => format!("Hotkey of \"{profile}\" set to {h}"),
                    None => format!("Hotkey of \"{profile}\" removed"),
                };
                self.saved(what, result)
            }
            Request::CheckProgram { exe } => {
                check_text(&[&exe])?;
                let check = match model::check_add(exe.trim()) {
                    AddCheck::Allowed => json!({"protection": "none"}),
                    AddCheck::Confirm(question) => {
                        json!({"protection": "warn", "message": question})
                    }
                    AddCheck::Refused(reason) => json!({"protection": "hard", "message": reason}),
                };
                Ok(Some(check))
            }
            Request::AddEntry {
                profile,
                exe,
                path,
                confirmed,
            } => {
                check_text(&[&profile, &exe, path.as_deref().unwrap_or("")])?;
                check_path(path.as_deref())?;
                let exe = exe.trim().to_owned();
                // M4 again here: the page may have skipped its own check.
                match model::check_add(&exe) {
                    AddCheck::Refused(reason) => {
                        self.status = reason.clone();
                        return Err(reason);
                    }
                    AddCheck::Confirm(_) if !confirmed => {
                        return Err(format!("{exe} is on the warn list and needs confirmation"));
                    }
                    _ => {}
                }
                let warn = matches!(protection::of(&exe), Protection::Warn(_));
                let path = path.filter(|p| !p.trim().is_empty()).map(PathBuf::from);
                let result = self.document.add_entry(
                    &profile,
                    &exe,
                    path.as_deref(),
                    None,
                    confirmed && warn,
                );
                self.saved(format!("{exe} added to \"{profile}\""), result)
            }
            Request::UpdateEntry {
                profile,
                index,
                entry,
            } => {
                let mut texts = vec![profile.as_str(), entry.exe.as_str()];
                texts.extend(entry.path.as_deref());
                texts.extend(entry.cwd.as_deref());
                texts.extend(entry.args.iter().map(String::as_str));
                check_text(&texts)?;
                if entry.args.len() > 64 {
                    return Err("too many arguments".to_owned());
                }
                check_path(entry.path.as_deref())?;
                check_path(entry.cwd.as_deref())?;
                if let AddCheck::Refused(reason) = model::check_add(entry.exe.trim()) {
                    return Err(reason);
                }
                let touches = matches!(
                    entry.action,
                    crate::config::Action::Stop | crate::config::Action::Suspend
                );
                if touches
                    && !entry.confirmed
                    && matches!(protection::of(entry.exe.trim()), Protection::Warn(_))
                {
                    return Err(format!(
                        "{} is on the warn list; confirm it to let kaltstellen touch it",
                        entry.exe.trim()
                    ));
                }
                let result = self.document.update_entry(&profile, index, &entry);
                self.saved(
                    format!("{} in \"{profile}\" changed", entry.exe.trim()),
                    result,
                )
            }
            Request::RemoveEntry { profile, index } => {
                check_text(&[&profile])?;
                let result = self.document.remove_entry(&profile, index);
                self.saved(format!("Entry removed from \"{profile}\""), result)
            }
            Request::PickExecutable => {
                let picked = self.host.pick_executable().and_then(|path| {
                    let exe = path.file_name()?.to_string_lossy().into_owned();
                    Some(json!({
                        "exe": exe,
                        "path": path.to_string_lossy(),
                        "protection": protection_name(&exe),
                    }))
                });
                Ok(Some(picked.unwrap_or(Value::Null)))
            }
            Request::Running => {
                let list: Vec<Value> = self
                    .host
                    .running()
                    .iter()
                    .map(|c| {
                        json!({
                            "exe": c.exe,
                            "private_bytes": c.private_bytes,
                            "protection": protection_name(&c.exe),
                        })
                    })
                    .collect();
                Ok(Some(Value::Array(list)))
            }
            Request::Unassigned { profile } => {
                check_text(&[&profile])?;
                let list: Vec<Value> = self
                    .host
                    .unassigned(self.document.config(), &profile)
                    .iter()
                    .map(|u| json!({"exe": u.exe, "path": u.path, "new": u.new}))
                    .collect();
                Ok(Some(Value::Array(list)))
            }
            Request::Dismiss { profile, exe } => {
                check_text(&[&profile, &exe])?;
                self.host.dismiss(&profile, &exe)?;
                self.status = format!("{exe} dismissed for \"{profile}\".");
                Ok(None)
            }
            Request::Apply { profile } => {
                check_text(&[&profile])?;
                let profile = profile.trim();
                if !profile.eq_ignore_ascii_case(NEUTRAL)
                    && self.document.config().profile(profile).is_none()
                {
                    return Err(format!("no profile named \"{profile}\""));
                }
                self.start_apply(profile)?;
                self.status = format!("Applying \"{profile}\"…");
                Ok(None)
            }
            Request::SetUpdateMode { mode } => {
                check_text(&[&mode])?;
                let parsed = crate::update::Mode::parse(&mode)
                    .filter(|m| m.name() == mode.trim())
                    .ok_or_else(|| format!("unknown update mode \"{mode}\""))?;
                if self.host.update_info().mode_locked {
                    return Err("KALTSTELLEN_UPDATE decides the update mode".to_owned());
                }
                self.host.set_update_mode(parsed)?;
                self.status = format!("Update mode {}.", parsed.name());
                if parsed.checks() && !self.checking {
                    self.checking = true;
                    self.host.check_updates();
                }
                Ok(None)
            }
            Request::CheckUpdates => {
                if self.checking {
                    return Err("a check is still running".to_owned());
                }
                self.checking = true;
                self.status = "Checking for updates…".to_owned();
                self.host.check_updates();
                Ok(None)
            }
            Request::InstallUpdate => {
                let info = self.host.update_info();
                let Some(version) = info.available else {
                    return Err("no newer version is known; check first".to_owned());
                };
                if let Some(how) = info.blocked {
                    return Err(format!("this copy cannot update itself: {how}"));
                }
                self.host.install_update()?;
                self.status = format!("Installing {version} in a separate window…");
                Ok(None)
            }
            Request::Icons { programs, size } => {
                if programs.len() > MAX_ICONS {
                    return Err(format!("at most {MAX_ICONS} icons per request"));
                }
                if !(16..=crate::win32::icon::MAX_SIZE).contains(&size) {
                    return Err(format!(
                        "icon size {size} is outside 16 to {}",
                        crate::win32::icon::MAX_SIZE
                    ));
                }
                // The page never names a file of its own: a path is used only
                // if an entry of the configuration has it.
                let configured: Vec<&Path> = self
                    .document
                    .config()
                    .profiles
                    .iter()
                    .flat_map(|p| &p.apps)
                    .filter_map(|a| a.path.as_deref())
                    .collect();
                let mut wanted = Vec::with_capacity(programs.len());
                for program in programs {
                    check_text(&[&program.exe, program.path.as_deref().unwrap_or_default()])?;
                    check_path(program.path.as_deref())?;
                    let path = program
                        .path
                        .map(|p| PathBuf::from(p.trim()))
                        .filter(|p| configured.iter().any(|c| crate::process::same_path(c, p)));
                    wanted.push(Wanted {
                        exe: program.exe,
                        path,
                    });
                }
                let icons = self.host.icons(&wanted, size);
                Ok(Some(Value::Array(
                    icons
                        .into_iter()
                        .map(|icon| icon.map_or(Value::Null, Value::String))
                        .collect(),
                )))
            }
            Request::SetAutostart { on } => {
                self.host.set_autostart(on)?;
                self.status = format!("Start at sign-in {}.", if on { "on" } else { "off" });
                Ok(None)
            }
        }
    }

    fn start_apply(&mut self, profile: &str) -> Result<(), String> {
        if self.applying {
            return Err("a switch is still running".to_owned());
        }
        self.applying = true;
        self.host.apply(profile);
        Ok(())
    }

    /// The check started by `check_updates` ended; returns the event.
    pub fn checked(&mut self) -> String {
        self.checking = false;
        let info = self.host.update_info();
        self.status = match (&info.error, &info.available) {
            (Some(error), _) => format!("Update check failed: {error}"),
            (None, Some(version)) => format!("Version {version} is available."),
            (None, None) => format!("kaltstellen {} is the newest version.", info.version),
        };
        json!({"v": VERSION, "event": "update_checked", "state": self.state()}).to_string()
    }

    /// The switch started by `apply` ended; returns the event for the page.
    pub fn applied(&mut self, target: &str, code: u8, stderr: &str) -> String {
        self.applying = false;
        self.status = model::applied(target, code, stderr);
        if let Some(name) = self.pending_delete.take() {
            if target == NEUTRAL && code <= crate::exit::ERRORS {
                self.snapshot = Some(self.document.clone());
                let result = self.document.delete_profile(&name);
                let _ = self.saved(format!("Profile \"{name}\" deleted"), result);
                self.snapshot = None;
            } else {
                self.status = format!("{} Profile \"{name}\" not deleted.", self.status);
            }
        }
        json!({"v": VERSION, "event": "applied", "state": self.state()}).to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::rc::Rc;

    #[derive(Clone, Default)]
    struct Fake {
        active: Rc<RefCell<String>>,
        applied: Rc<RefCell<Vec<String>>>,
        dismissed: Rc<RefCell<Vec<String>>>,
        autostart: Rc<RefCell<Option<bool>>>,
        pick: Option<PathBuf>,
        update: Rc<RefCell<UpdateInfo>>,
        checks: Rc<RefCell<usize>>,
        installs: Rc<RefCell<usize>>,
    }

    impl Host for Fake {
        fn active_profile(&self) -> String {
            let active = self.active.borrow().clone();
            if active.is_empty() {
                NEUTRAL.to_owned()
            } else {
                active
            }
        }
        fn running(&self) -> Vec<Candidate> {
            vec![Candidate {
                exe: "anydesk.exe".into(),
                private_bytes: 1 << 20,
                protection: protection::of("anydesk.exe"),
            }]
        }
        fn unassigned(&self, _config: &Config, profile: &str) -> Vec<Unassigned> {
            vec![Unassigned {
                exe: format!("{profile}-x.exe"),
                path: None,
                new: true,
            }]
        }
        fn dismiss(&self, profile: &str, exe: &str) -> Result<(), String> {
            self.dismissed.borrow_mut().push(format!("{profile}/{exe}"));
            Ok(())
        }
        fn autostart(&self) -> Option<bool> {
            *self.autostart.borrow()
        }
        fn set_autostart(&self, on: bool) -> Result<(), String> {
            *self.autostart.borrow_mut() = Some(on);
            Ok(())
        }
        fn pick_executable(&self) -> Option<PathBuf> {
            self.pick.clone()
        }
        fn apply(&self, profile: &str) {
            self.applied.borrow_mut().push(profile.to_owned());
        }
        fn update_info(&self) -> UpdateInfo {
            self.update.borrow().clone()
        }
        fn set_update_mode(&self, mode: crate::update::Mode) -> Result<(), String> {
            self.update.borrow_mut().mode = mode.name();
            Ok(())
        }
        fn check_updates(&self) {
            *self.checks.borrow_mut() += 1;
        }
        fn install_update(&self) -> Result<(), String> {
            *self.installs.borrow_mut() += 1;
            Ok(())
        }
        fn icons(&self, wanted: &[Wanted], size: u32) -> Vec<Option<String>> {
            // An icon for programs with a path, none otherwise.
            wanted
                .iter()
                .map(|w| {
                    w.path
                        .as_ref()
                        .map(|p| format!("data:{size}:{}:{}", w.exe, p.display()))
                })
                .collect()
        }
    }

    #[test]
    fn m2_icons_in_order_with_null_for_none_and_strict_input() {
        let dir = tempfile::tempdir().unwrap();
        let (mut b, _) = bridge(&dir);
        let added = send(
            &mut b,
            json!({"v": 1, "id": 9, "type": "add_entry", "profile": "focus", "exe": "a.exe", "path": "C:\\A\\a.exe"}),
        );
        assert_eq!(added["ok"], true, "{added}");
        let reply = send(
            &mut b,
            json!({"v": 1, "id": 1, "type": "icons", "size": 40, "programs": [
                {"exe": "a.exe", "path": "c:\\a\\A.EXE"},
                {"exe": "b.exe"},
                {"exe": "c.exe", "path": "  "},
                // Not in the configuration: the page cannot make the host
                // open a file or a share of its choice.
                {"exe": "d.exe", "path": "C:\\Other\\d.exe"},
                {"exe": "e.exe", "path": "\\\\server\\share\\e.exe"}
            ]}),
        );
        assert_eq!(reply["ok"], true, "{reply}");
        assert_eq!(
            reply["data"],
            json!(["data:40:a.exe:c:\\a\\A.EXE", null, null, null, null])
        );
        let refused = |b: &mut Bridge<Fake>, request: Value| {
            let reply = send(b, request);
            assert_eq!(reply["ok"], false, "{reply}");
        };
        // Relative paths, sizes out of range, unknown fields, too many.
        refused(
            &mut b,
            json!({"v": 1, "id": 2, "type": "icons", "size": 32,
                   "programs": [{"exe": "a.exe", "path": "a.exe"}]}),
        );
        refused(
            &mut b,
            json!({"v": 1, "id": 3, "type": "icons", "size": 8, "programs": []}),
        );
        refused(
            &mut b,
            json!({"v": 1, "id": 4, "type": "icons", "size": 97, "programs": []}),
        );
        refused(
            &mut b,
            json!({"v": 1, "id": 5, "type": "icons", "size": 32,
                   "programs": [{"exe": "a.exe", "file": "x"}]}),
        );
        let many: Vec<Value> = (0..=MAX_ICONS).map(|_| json!({"exe": "a"})).collect();
        refused(
            &mut b,
            json!({"v": 1, "id": 6, "type": "icons", "size": 32, "programs": many}),
        );
    }

    const CONFIG: &str =
        "# keep me\n[[profile]]\nname = \"focus\"\n\n  [[profile.app]]\n  exe = \"browser.exe\"\n";

    fn bridge(dir: &tempfile::TempDir) -> (Bridge<Fake>, Fake) {
        let file = dir.path().join("config.toml");
        std::fs::write(&file, CONFIG).unwrap();
        let fake = Fake {
            pick: Some(PathBuf::from("C:\\tools\\tool.exe")),
            ..Fake::default()
        };
        (
            Bridge::new(Document::open(&file).unwrap(), fake.clone()),
            fake,
        )
    }

    fn send(b: &mut Bridge<Fake>, message: Value) -> Value {
        serde_json::from_str(&b.handle(&message.to_string())).unwrap()
    }

    #[test]
    fn state_describes_profiles_and_entries() {
        let dir = tempfile::tempdir().unwrap();
        let (mut b, fake) = bridge(&dir);
        *fake.active.borrow_mut() = "focus".into();
        let reply = send(&mut b, json!({"v": 1, "id": 7, "type": "get_state"}));
        assert_eq!(reply["id"], 7);
        assert_eq!(reply["ok"], true);
        let profile = &reply["state"]["profiles"][0];
        assert_eq!(profile["name"], "focus");
        assert_eq!(profile["active"], true);
        assert_eq!(profile["entries"][0]["exe"], "browser.exe");
        assert_eq!(profile["entries"][0]["action"], "suspend");
        assert_eq!(profile["entries"][0]["trim"], true);
        assert_eq!(reply["state"]["default_action"], "suspend");
    }

    #[test]
    fn untrusted_messages_are_refused_without_effect() {
        let dir = tempfile::tempdir().unwrap();
        let (mut b, _) = bridge(&dir);
        let before = b.document().text();
        for message in [
            "not json".to_owned(),
            "[1, 2]".to_owned(),
            json!({"id": 1, "type": "get_state"}).to_string(),
            json!({"v": 2, "id": 1, "type": "get_state"}).to_string(),
            json!({"v": 1, "id": 1, "type": "format_disk"}).to_string(),
            json!({"v": 1, "id": 1, "type": "create_profile", "name": "x", "extra": 1}).to_string(),
            json!({"v": 1, "id": 1, "type": "create_profile", "name": "a\u{0}b"}).to_string(),
            json!({"v": 1, "id": 1, "type": "create_profile", "name": "x".repeat(5000)})
                .to_string(),
            "x".repeat(MAX_MESSAGE + 1),
        ] {
            let reply: Value = serde_json::from_str(&b.handle(&message)).unwrap();
            assert_eq!(reply["ok"], false, "{message:.60}");
        }
        assert_eq!(b.document().text(), before);
    }

    #[test]
    fn m1_profile_intents_edit_and_save_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let (mut b, _) = bridge(&dir);
        for message in [
            json!({"v": 1, "id": 1, "type": "create_profile", "name": "music"}),
            json!({"v": 1, "id": 2, "type": "copy_profile", "from": "focus", "to": "focus 2"}),
            json!({"v": 1, "id": 3, "type": "rename_profile", "from": "music", "to": "audio"}),
            json!({"v": 1, "id": 4, "type": "set_hotkey", "profile": "audio", "hotkey": "Ctrl+Shift+Alt+F3"}),
            json!({"v": 1, "id": 5, "type": "delete_profile", "name": "focus 2"}),
        ] {
            let reply = send(&mut b, message);
            assert_eq!(reply["ok"], true, "{reply}");
        }
        let file = std::fs::read_to_string(dir.path().join("config.toml")).unwrap();
        assert!(file.starts_with("# keep me"));
        assert!(file.contains("name = \"audio\"") && file.contains("Ctrl+Shift+Alt+F3"));
        assert!(!file.contains("focus 2"));
        let bad = send(
            &mut b,
            json!({"v": 1, "id": 6, "type": "set_hotkey", "profile": "audio", "hotkey": "Ctrl+Alt+Banana"}),
        );
        assert_eq!(bad["ok"], false);
        assert!(bad["error"].as_str().unwrap().contains("KS205"), "{bad}");
    }

    #[test]
    fn m4_protection_is_checked_again_on_the_rust_side() {
        let dir = tempfile::tempdir().unwrap();
        let (mut b, _) = bridge(&dir);
        let check = send(
            &mut b,
            json!({"v": 1, "id": 1, "type": "check_program", "exe": "anydesk.exe"}),
        );
        assert_eq!(check["data"]["protection"], "warn");
        let refused = send(
            &mut b,
            json!({"v": 1, "id": 2, "type": "add_entry", "profile": "focus", "exe": "explorer.exe"}),
        );
        assert_eq!(refused["ok"], false);
        let unconfirmed = send(
            &mut b,
            json!({"v": 1, "id": 3, "type": "add_entry", "profile": "focus", "exe": "anydesk.exe"}),
        );
        assert_eq!(unconfirmed["ok"], false);
        let relative = send(
            &mut b,
            json!({"v": 1, "id": 4, "type": "add_entry", "profile": "focus", "exe": "x.exe", "path": "x.exe"}),
        );
        assert_eq!(relative["ok"], false);
        let ok = send(
            &mut b,
            json!({"v": 1, "id": 5, "type": "add_entry", "profile": "focus", "exe": "anydesk.exe", "confirmed": true}),
        );
        assert_eq!(ok["ok"], true, "{ok}");
        assert_eq!(ok["state"]["profiles"][0]["entries"][1]["confirmed"], true);
        // Changing the action of a warn-list entry keeps needing confirmation.
        let entry = json!({"exe": "anydesk.exe", "action": "stop"});
        let refused = send(
            &mut b,
            json!({"v": 1, "id": 6, "type": "update_entry", "profile": "focus", "index": 1, "entry": entry}),
        );
        assert_eq!(refused["ok"], false);
        let entry = json!({"exe": "anydesk.exe", "action": "stop", "confirmed": true, "on_timeout": "keep"});
        let ok = send(
            &mut b,
            json!({"v": 1, "id": 7, "type": "update_entry", "profile": "focus", "index": 1, "entry": entry}),
        );
        assert_eq!(
            ok["state"]["profiles"][0]["entries"][1]["on_timeout"], "keep",
            "{ok}"
        );
        let removed = send(
            &mut b,
            json!({"v": 1, "id": 8, "type": "remove_entry", "profile": "focus", "index": 1}),
        );
        assert_eq!(
            removed["state"]["profiles"][0]["entries"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
    }

    #[test]
    fn host_queries_and_the_apply_cycle() {
        let dir = tempfile::tempdir().unwrap();
        let (mut b, fake) = bridge(&dir);
        let picked = send(&mut b, json!({"v": 1, "id": 1, "type": "pick_executable"}));
        assert_eq!(picked["data"]["exe"], "tool.exe");
        let running = send(&mut b, json!({"v": 1, "id": 2, "type": "running"}));
        assert_eq!(running["data"][0]["protection"], "warn");
        let list = send(
            &mut b,
            json!({"v": 1, "id": 3, "type": "unassigned", "profile": "focus"}),
        );
        assert_eq!(list["data"][0]["new"], true);
        send(
            &mut b,
            json!({"v": 1, "id": 4, "type": "dismiss", "profile": "focus", "exe": "y.exe"}),
        );
        assert_eq!(fake.dismissed.borrow().as_slice(), ["focus/y.exe"]);
        send(
            &mut b,
            json!({"v": 1, "id": 5, "type": "set_autostart", "on": true}),
        );
        assert_eq!(*fake.autostart.borrow(), Some(true));

        let applying = send(
            &mut b,
            json!({"v": 1, "id": 6, "type": "apply", "profile": "focus"}),
        );
        assert_eq!(applying["state"]["applying"], true);
        let twice = send(
            &mut b,
            json!({"v": 1, "id": 7, "type": "apply", "profile": "focus"}),
        );
        assert_eq!(twice["ok"], false, "one switch at a time");
        assert_eq!(fake.applied.borrow().as_slice(), ["focus"]);
        let event: Value = serde_json::from_str(&b.applied("focus", 0, "")).unwrap();
        assert_eq!(event["event"], "applied");
        assert_eq!(event["state"]["status"], "Applied \"focus\".");
        assert_eq!(event["state"]["applying"], false);
        let unknown = send(
            &mut b,
            json!({"v": 1, "id": 8, "type": "apply", "profile": "nope"}),
        );
        assert_eq!(unknown["ok"], false);
    }

    #[test]
    fn up1_update_mode_check_and_install_through_the_bridge() {
        let dir = tempfile::tempdir().unwrap();
        let (mut b, fake) = bridge(&dir);
        *fake.update.borrow_mut() = UpdateInfo {
            version: "1.0.0".into(),
            mode: "auto",
            ..UpdateInfo::default()
        };
        let state = send(&mut b, json!({"v": 1, "id": 1, "type": "get_state"}));
        assert_eq!(state["state"]["update"]["version"], "1.0.0");
        assert_eq!(state["state"]["update"]["mode"], "auto");
        // Only the three names; a mode that checks starts a check.
        let bad = send(
            &mut b,
            json!({"v": 1, "id": 2, "type": "set_update_mode", "mode": "on"}),
        );
        assert_eq!(bad["ok"], false);
        let manual = send(
            &mut b,
            json!({"v": 1, "id": 3, "type": "set_update_mode", "mode": "manual"}),
        );
        assert_eq!(manual["state"]["update"]["mode"], "manual");
        assert_eq!(manual["state"]["update"]["checking"], true);
        assert_eq!(*fake.checks.borrow(), 1);
        let again = send(&mut b, json!({"v": 1, "id": 4, "type": "check_updates"}));
        assert_eq!(again["ok"], false, "one check at a time");
        fake.update.borrow_mut().available = Some("1.1.0".into());
        let event: Value = serde_json::from_str(&b.checked()).unwrap();
        assert_eq!(event["event"], "update_checked");
        assert_eq!(event["state"]["status"], "Version 1.1.0 is available.");
        assert_eq!(event["state"]["update"]["checking"], false);
        let install = send(&mut b, json!({"v": 1, "id": 5, "type": "install_update"}));
        assert_eq!(install["ok"], true);
        assert_eq!(*fake.installs.borrow(), 1);
        // Not installable here: refused on the Rust side.
        fake.update.borrow_mut().blocked = Some("installed by scoop".into());
        let refused = send(&mut b, json!({"v": 1, "id": 6, "type": "install_update"}));
        assert_eq!(refused["ok"], false);
        // The environment decides: the window cannot change the mode.
        fake.update.borrow_mut().mode_locked = true;
        let locked = send(
            &mut b,
            json!({"v": 1, "id": 7, "type": "set_update_mode", "mode": "off"}),
        );
        assert_eq!(locked["ok"], false);
    }

    #[test]
    fn m1_deleting_the_active_profile_switches_to_neutral_first() {
        let dir = tempfile::tempdir().unwrap();
        let (mut b, fake) = bridge(&dir);
        *fake.active.borrow_mut() = "focus".into();
        let reply = send(
            &mut b,
            json!({"v": 1, "id": 1, "type": "delete_profile", "name": "focus"}),
        );
        assert_eq!(reply["ok"], true);
        assert_eq!(fake.applied.borrow().as_slice(), ["neutral"]);
        assert_eq!(b.document().config().profiles.len(), 1, "not yet deleted");
        *fake.active.borrow_mut() = "neutral".into();
        b.applied("neutral", 0, "");
        assert!(b.document().config().profiles.is_empty());
        let file = std::fs::read_to_string(dir.path().join("config.toml")).unwrap();
        assert!(!file.contains("focus"));
    }

    #[test]
    fn m1_no_profile_edit_while_a_delete_waits_and_no_delete_while_applying() {
        let dir = tempfile::tempdir().unwrap();
        let (mut b, fake) = bridge(&dir);
        *fake.active.borrow_mut() = "focus".into();
        let ok = |b: &mut Bridge<Fake>, request: Value| send(b, request)["ok"] == true;
        assert!(ok(
            &mut b,
            json!({"v": 1, "id": 1, "type": "delete_profile", "name": "focus"})
        ));
        // Renaming and creating again must not make the delete hit another
        // profile.
        assert!(!ok(
            &mut b,
            json!({"v": 1, "id": 2, "type": "rename_profile", "from": "focus", "to": "old"})
        ));
        assert!(!ok(
            &mut b,
            json!({"v": 1, "id": 3, "type": "create_profile", "name": "x"})
        ));
        *fake.active.borrow_mut() = "neutral".into();
        b.applied("neutral", 0, "");
        assert!(b.document().config().profiles.is_empty());
        // While a switch runs, a profile is not deleted (it may be the
        // target of that switch).
        assert!(ok(
            &mut b,
            json!({"v": 1, "id": 4, "type": "create_profile", "name": "play"})
        ));
        assert!(ok(
            &mut b,
            json!({"v": 1, "id": 5, "type": "apply", "profile": "play"})
        ));
        assert!(!ok(
            &mut b,
            json!({"v": 1, "id": 6, "type": "delete_profile", "name": "play"})
        ));
        b.applied("play", 0, "");
        assert_eq!(b.document().config().profiles.len(), 1);
    }

    #[test]
    fn m1_an_edit_that_cannot_be_saved_is_taken_back() {
        let dir = tempfile::tempdir().unwrap();
        let (mut b, _) = bridge(&dir);
        let file = dir.path().join("config.toml");
        let mut permissions = std::fs::metadata(&file).unwrap().permissions();
        permissions.set_readonly(true);
        std::fs::set_permissions(&file, permissions.clone()).unwrap();
        let reply = send(
            &mut b,
            json!({"v": 1, "id": 1, "type": "create_profile", "name": "music"}),
        );
        assert_eq!(reply["ok"], false, "{reply}");
        assert_eq!(reply["state"]["profiles"].as_array().unwrap().len(), 1);
        assert!(b.document().config().profile("music").is_none());
        // Writable again: the next edit does not bring the lost one along.
        #[allow(clippy::permissions_set_readonly_false)]
        permissions.set_readonly(false);
        std::fs::set_permissions(&file, permissions).unwrap();
        let reply = send(
            &mut b,
            json!({"v": 1, "id": 2, "type": "create_profile", "name": "games"}),
        );
        assert_eq!(reply["ok"], true, "{reply}");
        let text = std::fs::read_to_string(&file).unwrap();
        assert!(text.contains("games") && !text.contains("music"), "{text}");
    }
}
