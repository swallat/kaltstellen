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
use super::model::{self, AddCheck, Candidate, Failure};
use crate::config::{App, AppAction, Config, NEUTRAL, OnTimeout};
use crate::program_info::View;
use crate::protection::{self, Protection};
pub use crate::tray::client::Delivery;
use crate::tray::protocol::{LastSwitch, TrayStatus};
use crate::unassigned::Unassigned;

pub const VERSION: u64 = 1;
/// Longest message the page may send.
pub const MAX_MESSAGE: usize = 64 * 1024;
/// Longest single text field (names, paths, arguments).
const MAX_TEXT: usize = 4096;
/// U4: trigger programs per profile the window may set.
const MAX_TRIGGERS: usize = 64;

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
    /// UP1: through the running tray (pipe `set_update_mode`), which writes
    /// it and checks at once if the mode checks; without one, or when it
    /// does not take it, written to `settings.json` here.
    fn set_update_mode(&self, mode: crate::update::Mode) -> Result<Delivery, String>;
    /// U4: whether `settings.json` has automatic switching paused.
    fn triggers_paused(&self) -> bool;
    /// U4: pauses or resumes automatic switching through the running tray
    /// (pipe `set_automation`), which takes it over at once and writes it;
    /// without one, or when it does not take it, written here.
    fn set_automation(&self, paused: bool) -> Result<Delivery, String>;
    /// Starts a check; the end comes back through [`Bridge::checked`].
    fn check_updates(&self);
    /// Opens the visible update (a console window running `kaltstellen
    /// update`), as the tray's `Install update…` does.
    fn install_update(&self) -> Result<(), String>;
    /// M2: the icon of each program as a PNG `data:` URL, `None` where
    /// there is none, in the order asked.
    fn icons(&self, wanted: &[Wanted], size: u32) -> Vec<Option<String>>;
    /// M2, N1: what each program is (version resource, signature), `None`
    /// where no file can be read, in the order asked. Reading happens in
    /// the background; what is not known yet is `pending`, and the host
    /// sends the `program_info` event (see [`Bridge::program_info_ready`])
    /// when there is more.
    fn program_info(&self, wanted: &[Wanted]) -> Vec<Option<View>>;
    /// The running tray's status: M5 the open errors of the last switch,
    /// U4 whether automatic switching is paused. `Ok(None)` without a
    /// tray, `Err` when the tray did not answer in time (what is shown
    /// stays as it is).
    fn tray_status(&self) -> Result<Option<TrayStatus>, String>;
}

/// Most programs one `icons` request may name.
const MAX_ICONS: usize = 256;
/// M2: most files one drop may bring.
const MAX_DROPPED: usize = 64;

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
    /// The available version could not be installed here last time;
    /// `install_update` still installs it.
    pub failed: bool,
    /// After such a failure: when the automatic install tries it again
    /// (seconds since 1970), or `null`.
    pub retry_at: Option<u64>,
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
    SetTriggers {
        profile: String,
        triggers: Vec<String>,
        #[serde(default)]
        priority: i64,
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
    /// M2: files dropped onto the window. Their paths are not in the
    /// message: the host reads them from the dropped file objects.
    DropFiles,
    /// M2: the page has added what it could of a drop; the summary goes
    /// into the status line, so a later refresh of the state keeps it.
    DropDone {
        profile: String,
        added: usize,
        not_added: Vec<String>,
    },
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
    /// U4: `Automatic switching` on or paused.
    SetAutomation {
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
    ProgramInfo {
        programs: Vec<IconProgram>,
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
    /// M5: markers for errors of the last switch.
    pub errors: Vec<Failure>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ProfileView {
    pub name: String,
    pub hotkey: Option<String>,
    pub active: bool,
    pub entries: Vec<EntryView>,
    /// U4: trigger programs as written (name or full path).
    pub triggers: Vec<String>,
    pub priority: i32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct StateView {
    pub config_file: String,
    pub active: String,
    pub default_action: &'static str,
    pub default_trim: bool,
    pub profiles: Vec<ProfileView>,
    pub autostart: Option<bool>,
    /// U4: `Automatic switching`.
    pub automation: AutomationView,
    pub applying: bool,
    pub status: String,
    pub update: UpdateInfo,
}

/// U4 in the main window (moved from the tray menu, owner 2026-11-23).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AutomationView {
    /// Not paused: what the running tray reported at the last refresh,
    /// else what `settings.json` says.
    pub on: bool,
    /// Some profile of the file names triggers; without one the switch
    /// does nothing yet and the page shows it disabled with a hint.
    pub triggers: bool,
    /// Set while a running tray did not take the value shown (an older
    /// tray): it keeps its own until it restarts.
    pub note: Option<String>,
}

fn protection_name(exe: &str) -> &'static str {
    match protection::of(exe) {
        Protection::None => "none",
        Protection::Warn(_) => "warn",
        Protection::Hard => "hard",
    }
}

fn entry_view(app: &App, errors: Vec<Failure>) -> EntryView {
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
        errors,
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

/// The end of the status line for a setting (U4, UP1): when it takes
/// effect.
fn later(delivery: &Delivery) -> String {
    match delivery {
        // The tray's notes: taken over, but for example not saved.
        Delivery::Tray(status) if !status.notes.is_empty() => {
            format!(". {}", status.notes.join(" "))
        }
        Delivery::Tray(_) => ".".to_owned(),
        Delivery::Saved => "; saved. The tray uses it when it starts.".to_owned(),
        Delivery::OlderTray(_) => {
            "; saved. The running tray keeps its own setting until it restarts.".to_owned()
        }
        Delivery::Unanswered(_) => {
            "; saved, but the running tray did not answer in time.".to_owned()
        }
    }
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
            | Request::SetTriggers { .. }
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
    /// M5: the open errors of the last switch as the tray reported them
    /// at the last refresh (`get_state`, the end of a switch).
    last_switch: Option<LastSwitch>,
    /// U4: whether the running tray had automatic switching paused at the
    /// last refresh; `None` without a tray or while it has no triggers
    /// (then `settings.json` is shown).
    tray_paused: Option<bool>,
    /// U4: the value saved for a running tray that did not take it (an
    /// older tray); shown, with a note, until no tray runs or the tray
    /// reports it.
    held_back: Option<bool>,
    /// M2: the paths of the files attached to the message being handled.
    dropped: Vec<PathBuf>,
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
            last_switch: None,
            tray_paused: None,
            held_back: None,
            dropped: Vec::new(),
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
                    entries: p
                        .apps
                        .iter()
                        .map(|app| {
                            let errors = model::failures(
                                self.last_switch.as_ref(),
                                &p.name,
                                &app.exe,
                                app.path.as_deref(),
                            );
                            entry_view(app, errors)
                        })
                        .collect(),
                    triggers: p.triggers.iter().map(|t| t.text()).collect(),
                    priority: p.priority,
                })
                .collect(),
            active,
            autostart: self.host.autostart(),
            automation: AutomationView {
                on: !self
                    .held_back
                    .or(self.tray_paused)
                    .unwrap_or_else(|| self.host.triggers_paused()),
                note: self.held_back.map(|_| {
                    "Saved; the running tray keeps its own setting until it restarts.".to_owned()
                }),
                triggers: config.profiles.iter().any(|p| !p.triggers.is_empty()),
            },
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
        self.handle_with_files(raw, Vec::new())
    }

    /// [`Bridge::handle`] for a message that came with files (M2): the
    /// host read `files` from the file objects the page attached. Only
    /// `drop_files` may carry them.
    pub fn handle_with_files(&mut self, raw: &str, files: Vec<PathBuf>) -> String {
        let parsed = self.parse(raw).and_then(|(id, request)| {
            if !files.is_empty() && request != Request::DropFiles {
                return Err((id, "only drop_files may carry files".to_owned()));
            }
            Ok((id, request))
        });
        self.dropped = files;
        let (id, result) = match parsed {
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
            Request::GetState => {
                // M5: asked here and when a switch ends, not with every
                // reply: it is a round trip to the tray.
                self.refresh_tray();
                Ok(None)
            }
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
            Request::SetTriggers {
                profile,
                triggers,
                priority,
            } => {
                // U4: the same checks as for entries; the configuration
                // rules decide the rest (own profile, duplicates, range).
                let mut texts = vec![profile.as_str()];
                texts.extend(triggers.iter().map(String::as_str));
                check_text(&texts)?;
                if triggers.len() > MAX_TRIGGERS {
                    return Err("too many trigger programs".to_owned());
                }
                for trigger in &triggers {
                    if trigger.contains(['\\', '/', ':']) {
                        check_path(Some(trigger))?;
                    }
                }
                let result = self.document.set_triggers(&profile, &triggers, priority);
                let count = triggers.iter().filter(|t| !t.trim().is_empty()).count();
                let what = match count {
                    0 => format!("Triggers of \"{profile}\" removed"),
                    1 => format!("1 trigger of \"{profile}\" set, priority {priority}"),
                    n => format!("{n} triggers of \"{profile}\" set, priority {priority}"),
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
            Request::DropFiles => {
                // M2: like `pick_executable` for each file; the page then
                // adds them with `add_entry`, which checks everything again.
                let dropped = std::mem::take(&mut self.dropped);
                if dropped.is_empty() {
                    return Err("no file was dropped (drop files from Explorer)".to_owned());
                }
                if dropped.len() > MAX_DROPPED {
                    return Err(format!("at most {MAX_DROPPED} files at once"));
                }
                let mut files = Vec::new();
                let mut refused = Vec::new();
                for path in dropped {
                    match model::check_dropped(&path, MAX_TEXT) {
                        Ok(exe) => files.push(json!({
                            "path": path.to_string_lossy(),
                            "protection": protection_name(&exe),
                            "exe": exe,
                        })),
                        Err(reason) => refused.push(reason),
                    }
                }
                Ok(Some(json!({"files": files, "refused": refused})))
            }
            Request::DropDone {
                profile,
                added,
                not_added,
            } => {
                if added > MAX_DROPPED || not_added.len() > MAX_DROPPED {
                    return Err(format!("at most {MAX_DROPPED} files at once"));
                }
                let mut texts = vec![profile.as_str()];
                texts.extend(not_added.iter().map(String::as_str));
                check_text(&texts)?;
                let profile = profile.trim();
                let total = added + not_added.len();
                self.status = match (added, not_added.is_empty()) {
                    (1, true) => format!("1 program added to \"{profile}\"; saved."),
                    (_, true) => format!("{added} programs added to \"{profile}\"; saved."),
                    (_, false) => format!(
                        "{added} of {total} added to \"{profile}\". Not added: {}",
                        not_added.join("; ")
                    ),
                };
                Ok(None)
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
                let delivery = self.host.set_update_mode(parsed)?;
                self.status = format!("Update mode {}{}", parsed.name(), later(&delivery));
                // UP1: a mode that checks checks at once. A running tray
                // that took the mode checks too (its result decides on an
                // automatic install); this check is the window's own, so
                // the window can say what it found. [assumed]
                if parsed.checks() && !self.checking {
                    self.checking = true;
                    self.host.check_updates();
                }
                Ok(None)
            }
            Request::SetAutomation { on } => {
                let delivery = self.host.set_automation(!on)?;
                // What the tray does now; without a tray (or one that did
                // not take it), the file just written is shown.
                self.tray_paused = match &delivery {
                    Delivery::Tray(_) => Some(!on),
                    Delivery::Saved | Delivery::OlderTray(_) | Delivery::Unanswered(_) => None,
                };
                // An older tray keeps its own state: the window goes on
                // showing what was saved, with a note, instead of flipping
                // back at the next refresh. [assumed] A tray that did not
                // answer in time may well have it; the next refresh says.
                self.held_back = match &delivery {
                    Delivery::OlderTray(_) => Some(!on),
                    Delivery::Tray(_) | Delivery::Saved | Delivery::Unanswered(_) => None,
                };
                self.status = format!(
                    "Automatic switching {}{}",
                    if on { "on" } else { "paused" },
                    later(&delivery)
                );
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
                let wanted = self.wanted(programs)?;
                let icons = self.host.icons(&wanted, size);
                Ok(Some(Value::Array(
                    icons
                        .into_iter()
                        .map(|icon| icon.map_or(Value::Null, Value::String))
                        .collect(),
                )))
            }
            Request::ProgramInfo { programs } => {
                if programs.len() > MAX_ICONS {
                    return Err(format!("at most {MAX_ICONS} programs per request"));
                }
                let wanted = self.wanted(programs)?;
                Ok(Some(Value::Array(
                    self.host
                        .program_info(&wanted)
                        .into_iter()
                        .map(|view| view.map_or(Value::Null, |v| json!(v)))
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

    /// The programs of an `icons` or `program_info` request. The page never
    /// names a file of its own: a path is used only if an entry of the
    /// configuration has it.
    fn wanted(&self, programs: Vec<IconProgram>) -> Result<Vec<Wanted>, String> {
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
        Ok(wanted)
    }

    /// M2: the host read more program information; the page asks again for
    /// what was `pending`. No state: nothing of it changed.
    pub fn program_info_ready(&self) -> String {
        json!({"v": VERSION, "event": "program_info"}).to_string()
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

    /// M5 `[assumed]`: only an answer of the tray replaces the markers; a
    /// tray that did not answer in time leaves the last known ones. U4:
    /// the same for automatic switching.
    fn refresh_tray(&mut self) {
        if let Ok(status) = self.host.tray_status() {
            self.tray_paused = status
                .as_ref()
                .and_then(|s| s.automation.as_ref().map(|a| a.paused));
            // The saved value counts again once no tray runs or the tray
            // reports it.
            if status.is_none() || self.tray_paused == self.held_back {
                self.held_back = None;
            }
            self.last_switch = status.and_then(|s| s.last_switch);
        }
    }

    /// The switch started by `apply` ended; returns the event for the page.
    pub fn applied(&mut self, target: &str, code: u8, stderr: &str) -> String {
        self.applying = false;
        self.status = model::applied(target, code, stderr);
        self.refresh_tray();
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
        last_switch: Rc<RefCell<Option<LastSwitch>>>,
        tray_silent: Rc<std::cell::Cell<bool>>,
        /// No tray runs (`tray_status` is `Ok(None)`).
        no_tray: Rc<std::cell::Cell<bool>>,
        /// The tray's `automation.paused`; `None`: it has no triggers.
        tray_paused: Rc<std::cell::Cell<Option<bool>>>,
        /// `triggers_paused` in `settings.json`.
        saved_paused: Rc<std::cell::Cell<bool>>,
        /// How a setting arrives: 0 the tray takes it, 1 no tray, 2 an
        /// older tray that does not know the command, 3 the tray takes it but
        /// cannot save it, 4 the tray does not answer in time.
        delivery: Rc<std::cell::Cell<u8>>,
    }

    impl Fake {
        fn deliver(&self, tray_takes: impl FnOnce()) -> Delivery {
            match self.delivery.get() {
                0 => {
                    tray_takes();
                    Delivery::Tray(Box::default())
                }
                1 => Delivery::Saved,
                3 => {
                    tray_takes();
                    Delivery::Tray(Box::new(TrayStatus {
                        notes: vec![
                            "warning: taken over by the tray for this session, but not saved: disk full"
                                .into(),
                        ],
                        ..TrayStatus::default()
                    }))
                }
                2 => Delivery::OlderTray("KS505 unknown command".into()),
                _ => Delivery::Unanswered("KS501 no answer".into()),
            }
        }
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
        fn set_update_mode(&self, mode: crate::update::Mode) -> Result<Delivery, String> {
            self.update.borrow_mut().mode = mode.name();
            Ok(self.deliver(|| {}))
        }
        fn triggers_paused(&self) -> bool {
            self.saved_paused.get()
        }
        fn set_automation(&self, paused: bool) -> Result<Delivery, String> {
            // The tray writes the file itself, the others here: the file
            // has it either way, except when the tray could not save it.
            if self.delivery.get() != 3 {
                self.saved_paused.set(paused);
            }
            Ok(self.deliver(|| {
                if self.tray_paused.get().is_some() {
                    self.tray_paused.set(Some(paused));
                }
            }))
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
        fn program_info(&self, wanted: &[Wanted]) -> Vec<Option<View>> {
            // Known for programs with a path, pending for running ones.
            wanted
                .iter()
                .map(|w| match &w.path {
                    Some(p) => Some(View {
                        description: Some(format!("{} at {}", w.exe, p.display())),
                        signature: "unsigned",
                        ..View::default()
                    }),
                    None if w.exe == "running.exe" => Some(View {
                        signature: "pending",
                        pending: true,
                        ..View::default()
                    }),
                    None => None,
                })
                .collect()
        }
        fn tray_status(&self) -> Result<Option<TrayStatus>, String> {
            if self.tray_silent.get() {
                return Err("no answer".into());
            }
            if self.no_tray.get() {
                return Ok(None);
            }
            Ok(Some(TrayStatus {
                state: crate::state::State::default(),
                running: None,
                waiting: None,
                automation: self.tray_paused.get().map(|paused| {
                    crate::tray::protocol::AutomationStatus {
                        paused,
                        ..Default::default()
                    }
                }),
                last_switch: self.last_switch.borrow().clone(),
                notes: Vec::new(),
            }))
        }
    }

    #[test]
    fn m2_dropped_files_are_checked_and_added_like_chosen_ones() {
        let dir = tempfile::tempdir().unwrap();
        let (mut b, _) = bridge(&dir);
        let file = |name: &str| {
            let path = dir.path().join(name);
            std::fs::write(&path, b"MZ").unwrap();
            path
        };
        let tool = file("tool.exe");
        let any = file("anydesk.exe");
        let notes = file("notes.txt");
        std::fs::create_dir(dir.path().join("folder")).unwrap();
        let drop = |b: &mut Bridge<Fake>, files: Vec<PathBuf>| -> Value {
            let raw = json!({"v": 1, "id": 1, "type": "drop_files"}).to_string();
            serde_json::from_str(&b.handle_with_files(&raw, files)).unwrap()
        };
        let reply = drop(
            &mut b,
            vec![
                tool.clone(),
                any.clone(),
                notes,
                dir.path().join("folder"),
                PathBuf::from("relative.exe"),
            ],
        );
        assert_eq!(reply["ok"], true, "{reply}");
        let files = reply["data"]["files"].as_array().unwrap();
        assert_eq!(files.len(), 2, "{reply}");
        assert_eq!(files[0]["exe"], "tool.exe");
        assert_eq!(files[0]["path"], tool.to_str().unwrap());
        assert_eq!(files[0]["protection"], "none");
        assert_eq!(files[1]["protection"], "warn");
        let refused = reply["data"]["refused"].as_array().unwrap();
        assert_eq!(refused.len(), 3, "{reply}");
        assert!(refused[0].as_str().unwrap().contains("not a program"));
        assert!(refused[1].as_str().unwrap().contains("folder"));
        assert!(refused[2].as_str().unwrap().contains("full path"));
        // Nothing is added by the drop itself: the page adds each file with
        // `add_entry`, which checks protection, C1 to C8 and duplicates.
        assert_eq!(b.document().config().profiles[0].apps.len(), 1);
        let add = |b: &mut Bridge<Fake>, exe: &str, path: &Path, confirmed: bool| {
            send(
                b,
                json!({"v": 1, "id": 2, "type": "add_entry", "profile": "focus",
                       "exe": exe, "path": path, "confirmed": confirmed}),
            )["ok"]
                == true
        };
        assert!(add(&mut b, "tool.exe", &tool, false));
        assert!(!add(&mut b, "tool.exe", &tool, false), "a duplicate (C2)");
        assert!(!add(&mut b, "anydesk.exe", &any, false), "warn list");
        assert!(add(&mut b, "anydesk.exe", &any, true));

        // The page cannot name paths itself, and only a drop carries files.
        let named = send(
            &mut b,
            json!({"v": 1, "id": 3, "type": "drop_files", "paths": ["C:\\x.exe"]}),
        );
        assert_eq!(named["ok"], false);
        assert!(
            drop(&mut b, Vec::new())["error"]
                .as_str()
                .unwrap()
                .contains("no file")
        );
        let raw = json!({"v": 1, "id": 4, "type": "create_profile", "name": "x"}).to_string();
        let smuggled: Value =
            serde_json::from_str(&b.handle_with_files(&raw, vec![tool.clone()])).unwrap();
        assert_eq!(smuggled["ok"], false);
        assert!(b.document().config().profile("x").is_none());
        let many = vec![tool; MAX_DROPPED + 1];
        assert_eq!(drop(&mut b, many)["ok"], false);

        // The page's summary goes into the bridge's status, so a refresh
        // of the state (focus) shows it still.
        let done = send(
            &mut b,
            json!({"v": 1, "id": 5, "type": "drop_done", "profile": "focus", "added": 1,
                   "not_added": ["notes.txt is not a program (.exe)", "x.exe cannot be found"]}),
        );
        assert_eq!(done["ok"], true, "{done}");
        let summary = "1 of 3 added to \"focus\". Not added: notes.txt is not a program (.exe); x.exe cannot be found";
        assert_eq!(done["state"]["status"], summary);
        let later = send(&mut b, json!({"v": 1, "id": 6, "type": "get_state"}));
        assert_eq!(later["state"]["status"], summary);
        let done = send(
            &mut b,
            json!({"v": 1, "id": 7, "type": "drop_done", "profile": "focus", "added": 2, "not_added": []}),
        );
        assert_eq!(
            done["state"]["status"],
            "2 programs added to \"focus\"; saved."
        );
        let bad = send(
            &mut b,
            json!({"v": 1, "id": 8, "type": "drop_done", "profile": "focus", "added": 0,
                   "not_added": ["a\u{0}b"]}),
        );
        assert_eq!(bad["ok"], false);
    }

    #[test]
    fn m5_markers_follow_the_trays_errors_at_each_refresh() {
        use crate::tray::protocol::ErrorLine;
        let dir = tempfile::tempdir().unwrap();
        let (mut b, fake) = bridge(&dir);
        *fake.active.borrow_mut() = "focus".into();
        *fake.last_switch.borrow_mut() = Some(LastSwitch {
            profile: "focus".into(),
            errors: vec![ErrorLine {
                code: "KS105".into(),
                program: "browser.exe".into(),
                path: None,
                text: "KS105 [suspend] browser.exe: access denied".into(),
            }],
        });
        let errors = |reply: &Value| reply["state"]["profiles"][0]["entries"][0]["errors"].clone();
        // Not asked yet: other replies do not go to the tray.
        let reply = send(&mut b, json!({"v": 1, "id": 1, "type": "running"}));
        assert_eq!(errors(&reply), json!([]));
        let reply = send(&mut b, json!({"v": 1, "id": 2, "type": "get_state"}));
        assert_eq!(
            errors(&reply),
            json!([{"code": "KS105", "label": "freeze failed",
                    "text": "KS105 [suspend] browser.exe: access denied"}])
        );
        // Kept until the next refresh...
        *fake.last_switch.borrow_mut() = None;
        let reply = send(&mut b, json!({"v": 1, "id": 3, "type": "running"}));
        assert_eq!(errors(&reply).as_array().unwrap().len(), 1);
        // ...and through a refresh the tray did not answer in time.
        fake.tray_silent.set(true);
        let reply = send(&mut b, json!({"v": 1, "id": 4, "type": "get_state"}));
        assert_eq!(errors(&reply).as_array().unwrap().len(), 1);
        fake.tray_silent.set(false);
        // An answer replaces them, as the end of a switch does.
        let reply = send(&mut b, json!({"v": 1, "id": 5, "type": "get_state"}));
        assert_eq!(errors(&reply), json!([]));
        *fake.last_switch.borrow_mut() = Some(LastSwitch {
            profile: "focus".into(),
            errors: vec![ErrorLine {
                code: "KS105".into(),
                program: "browser.exe".into(),
                path: None,
                text: "x".into(),
            }],
        });
        send(&mut b, json!({"v": 1, "id": 6, "type": "get_state"}));
        *fake.last_switch.borrow_mut() = None;
        let event: Value = serde_json::from_str(&b.applied("focus", 0, "")).unwrap();
        assert_eq!(errors(&event), json!([]));
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

    #[test]
    fn m2_program_info_only_for_configured_paths_with_pending_and_strict_input() {
        let dir = tempfile::tempdir().unwrap();
        let (mut b, _) = bridge(&dir);
        let added = send(
            &mut b,
            json!({"v": 1, "id": 9, "type": "add_entry", "profile": "focus", "exe": "a.exe", "path": "C:\\A\\a.exe"}),
        );
        assert_eq!(added["ok"], true, "{added}");
        let reply = send(
            &mut b,
            json!({"v": 1, "id": 1, "type": "program_info", "programs": [
                {"exe": "a.exe", "path": "c:\\a\\A.EXE"},
                {"exe": "running.exe"},
                // Not in the configuration: never read.
                {"exe": "d.exe", "path": "C:\\Other\\d.exe"},
                {"exe": "e.exe", "path": "\\\\server\\share\\e.exe"}
            ]}),
        );
        assert_eq!(reply["ok"], true, "{reply}");
        let data = reply["data"].as_array().unwrap();
        assert_eq!(data.len(), 4);
        assert_eq!(data[0]["description"], "a.exe at c:\\a\\A.EXE");
        assert_eq!(data[0]["signature"], "unsigned");
        assert_eq!(data[0]["pending"], false);
        assert_eq!(data[1]["signature"], "pending");
        assert_eq!(data[1]["pending"], true);
        assert_eq!((&data[2], &data[3]), (&Value::Null, &Value::Null));
        // Every field is there for the page.
        let keys: Vec<&str> = data[0]
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        for key in [
            "description",
            "product",
            "company",
            "version",
            "signature",
            "publisher",
            "catalog",
            "problem",
            "pending",
        ] {
            assert!(keys.contains(&key), "{key}");
        }
        let event: Value = serde_json::from_str(&b.program_info_ready()).unwrap();
        assert_eq!(event, json!({"v": 1, "event": "program_info"}));
        let refused = |b: &mut Bridge<Fake>, request: Value| {
            let reply = send(b, request);
            assert_eq!(reply["ok"], false, "{reply}");
        };
        refused(
            &mut b,
            json!({"v": 1, "id": 2, "type": "program_info", "programs": [{"exe": "a.exe", "path": "a.exe"}]}),
        );
        refused(
            &mut b,
            json!({"v": 1, "id": 3, "type": "program_info", "programs": [], "size": 32}),
        );
        let many: Vec<Value> = (0..=MAX_ICONS).map(|_| json!({"exe": "a"})).collect();
        refused(
            &mut b,
            json!({"v": 1, "id": 4, "type": "program_info", "programs": many}),
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
    fn u4_triggers_are_set_through_the_bridge_with_untrusted_input_checks() {
        let dir = tempfile::tempdir().unwrap();
        let (mut b, _) = bridge(&dir);
        let reply = send(
            &mut b,
            json!({"v": 1, "id": 1, "type": "set_triggers", "profile": "focus",
                   "triggers": ["sim.exe", "C:\\Games\\launcher.exe"], "priority": 3}),
        );
        assert_eq!(reply["ok"], true, "{reply}");
        let focus = &reply["state"]["profiles"][0];
        assert_eq!(
            focus["triggers"],
            json!(["sim.exe", "C:\\Games\\launcher.exe"])
        );
        assert_eq!(focus["priority"], 3);
        let file = std::fs::read_to_string(dir.path().join("config.toml")).unwrap();
        assert!(
            file.starts_with("# keep me") && file.contains("priority = 3"),
            "{file}"
        );

        let before = b.document().text();
        let many: Vec<String> = (0..=MAX_TRIGGERS).map(|n| format!("t{n}.exe")).collect();
        for message in [
            // Frozen by its own profile (browser.exe is an entry).
            json!({"v": 1, "id": 2, "type": "set_triggers", "profile": "focus", "triggers": ["browser.exe"]}),
            json!({"v": 1, "id": 3, "type": "set_triggers", "profile": "focus", "triggers": ["a\u{7}.exe"]}),
            json!({"v": 1, "id": 4, "type": "set_triggers", "profile": "focus", "triggers": ["x".repeat(5000)]}),
            json!({"v": 1, "id": 5, "type": "set_triggers", "profile": "focus", "triggers": ["sub\\x.exe"]}),
            json!({"v": 1, "id": 6, "type": "set_triggers", "profile": "focus", "triggers": many}),
            json!({"v": 1, "id": 7, "type": "set_triggers", "profile": "focus", "triggers": [], "priority": 1_i64 << 40}),
            json!({"v": 1, "id": 8, "type": "set_triggers", "profile": "focus", "triggers": [], "priority": "high"}),
            json!({"v": 1, "id": 9, "type": "set_triggers", "profile": "nope", "triggers": []}),
            json!({"v": 1, "id": 10, "type": "set_triggers", "profile": "focus", "triggers": [], "x": 1}),
        ] {
            let reply = send(&mut b, message.clone());
            assert_eq!(reply["ok"], false, "{message}");
        }
        assert_eq!(b.document().text(), before);
        let reply = send(
            &mut b,
            json!({"v": 1, "id": 11, "type": "set_triggers", "profile": "focus", "triggers": []}),
        );
        assert_eq!(reply["ok"], true, "{reply}");
        assert_eq!(reply["state"]["profiles"][0]["triggers"], json!([]));
        assert!(!b.document().text().contains("trigger"));
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
        // Without a tray that takes it, the status line says when it
        // counts; a mode that checks still checks here.
        fake.update.borrow_mut().mode_locked = false;
        b.checked();
        fake.delivery.set(2);
        let later = send(
            &mut b,
            json!({"v": 1, "id": 8, "type": "set_update_mode", "mode": "auto"}),
        );
        assert_eq!(
            later["state"]["status"],
            "Update mode auto; saved. The running tray keeps its own setting until it restarts."
        );
        assert_eq!(*fake.checks.borrow(), 2);
    }

    #[test]
    fn u4_automatic_switching_through_the_bridge() {
        let dir = tempfile::tempdir().unwrap();
        let (mut b, fake) = bridge(&dir);
        let automation = |reply: &Value| reply["state"]["automation"].clone();
        // No profile has triggers: shown (on), the page disables it.
        let reply = send(&mut b, json!({"v": 1, "id": 1, "type": "get_state"}));
        assert_eq!(
            automation(&reply),
            json!({"on": true, "triggers": false, "note": null})
        );
        // Strict input: a bool named `on`, nothing else.
        for bad in [
            json!({"v": 1, "id": 2, "type": "set_automation"}),
            json!({"v": 1, "id": 2, "type": "set_automation", "on": "off"}),
            json!({"v": 1, "id": 2, "type": "set_automation", "paused": true}),
            json!({"v": 1, "id": 2, "type": "set_automation", "on": false, "x": 1}),
        ] {
            let reply = send(&mut b, bad.clone());
            assert_eq!(reply["ok"], false, "{bad}");
        }
        assert!(!fake.saved_paused.get(), "refused without effect");
        send(
            &mut b,
            json!({"v": 1, "id": 3, "type": "set_triggers", "profile": "focus",
                   "triggers": ["sim.exe"]}),
        );
        // A running tray with triggers: it takes it over at once, and the
        // window shows what the tray reports.
        fake.tray_paused.set(Some(false));
        let reply = send(
            &mut b,
            json!({"v": 1, "id": 4, "type": "set_automation", "on": false}),
        );
        assert_eq!(reply["ok"], true);
        assert_eq!(
            automation(&reply),
            json!({"on": false, "triggers": true, "note": null})
        );
        assert_eq!(reply["state"]["status"], "Automatic switching paused.");
        assert_eq!(fake.tray_paused.get(), Some(true));
        assert!(fake.saved_paused.get(), "persisted");
        // A refresh reads the tray again: switched on elsewhere (the
        // command line), the window follows.
        fake.tray_paused.set(Some(false));
        let reply = send(&mut b, json!({"v": 1, "id": 5, "type": "get_state"}));
        assert_eq!(automation(&reply)["on"], true);
        // A tray that does not answer in time: the last known state stays.
        fake.saved_paused.set(true);
        fake.tray_silent.set(true);
        let reply = send(&mut b, json!({"v": 1, "id": 6, "type": "get_state"}));
        assert_eq!(automation(&reply)["on"], true);
        fake.tray_silent.set(false);
        // No tray: settings.json is shown and written.
        fake.no_tray.set(true);
        fake.delivery.set(1);
        let reply = send(&mut b, json!({"v": 1, "id": 7, "type": "get_state"}));
        assert_eq!(automation(&reply)["on"], false, "from settings.json");
        let reply = send(
            &mut b,
            json!({"v": 1, "id": 8, "type": "set_automation", "on": true}),
        );
        assert_eq!(automation(&reply)["on"], true);
        assert!(!fake.saved_paused.get());
        assert_eq!(
            reply["state"]["status"],
            "Automatic switching on; saved. The tray uses it when it starts."
        );
        // An older tray that does not know the command (it is on): saved
        // here, and the window says when it counts.
        fake.no_tray.set(false);
        fake.tray_paused.set(Some(false));
        fake.delivery.set(2);
        let reply = send(
            &mut b,
            json!({"v": 1, "id": 9, "type": "set_automation", "on": false}),
        );
        assert_eq!(reply["ok"], true);
        assert!(fake.saved_paused.get());
        assert_eq!(
            reply["state"]["status"],
            "Automatic switching paused; saved. The running tray keeps its own setting until it restarts."
        );
        let note = json!("Saved; the running tray keeps its own setting until it restarts.");
        assert_eq!(
            automation(&reply),
            json!({"on": false, "triggers": true, "note": note})
        );
        // A refresh does not flip it back to the old tray's state.
        let reply = send(&mut b, json!({"v": 1, "id": 10, "type": "get_state"}));
        assert_eq!(automation(&reply)["on"], false);
        assert_eq!(automation(&reply)["note"], note);
        // Restarted, the tray has it: the note goes.
        fake.tray_paused.set(Some(true));
        let reply = send(&mut b, json!({"v": 1, "id": 11, "type": "get_state"}));
        assert_eq!(
            automation(&reply),
            json!({"on": false, "triggers": true, "note": null})
        );
        // Taken over but not saved: the tray's note is in the status line,
        // and nothing is written here.
        fake.delivery.set(3);
        let reply = send(
            &mut b,
            json!({"v": 1, "id": 12, "type": "set_automation", "on": true}),
        );
        assert_eq!(
            reply["state"]["status"],
            "Automatic switching on. warning: taken over by the tray for this session, but not saved: disk full"
        );
        assert_eq!(automation(&reply)["on"], true);
        // No answer in time: saved here, and said so; not the older-tray
        // wording, and no note (the next refresh shows what the tray does).
        fake.delivery.set(4);
        let reply = send(
            &mut b,
            json!({"v": 1, "id": 13, "type": "set_automation", "on": false}),
        );
        assert_eq!(reply["ok"], true);
        assert!(fake.saved_paused.get());
        assert_eq!(
            reply["state"]["status"],
            "Automatic switching paused; saved, but the running tray did not answer in time."
        );
        assert_eq!(automation(&reply)["note"], Value::Null);
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
