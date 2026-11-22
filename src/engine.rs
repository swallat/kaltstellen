//! Switch engine (W1 to W6) and recovery (S3, S10).
//!
//! A switch runs four steps in the order of W1: resume, stop, suspend, start.
//! Each step matches processes at the moment it runs (W6). A failed action
//! never stops the switch; errors are collected (W2). The state is persisted
//! before a process is frozen and after every step, so a crash at any point
//! leaves a state file that recovery can undo.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::Duration;

use log::{info, warn};

use crate::config::{App, AppAction, Config, NEUTRAL, Settings};
use crate::error::{Code, Error, Result, Step};
use crate::process::{self, Access, Guest, ProcessInfo, Snapshot, StopOutcome, Target};
use crate::protection::{self, Protection};
use crate::state::{State, Stopped, Suspended};

/// W7: how long windows get to report minimised before the freeze.
const MINIMIZE_WAIT: Duration = Duration::from_secs(1);

#[derive(Debug, Clone, Copy, Default)]
pub struct Options {
    /// Confirms programs on the warn list (P6).
    pub force: bool,
}

/// Something a switch did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Event {
    pub step: Step,
    pub program: String,
    pub pid: Option<u32>,
}

/// What a switch or a recovery did and what went wrong.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Report {
    pub events: Vec<Event>,
    pub errors: Vec<Error>,
}

impl Report {
    fn done(&mut self, step: Step, program: &str, pid: Option<u32>) {
        match pid {
            Some(pid) => info!("{step} {program} (pid {pid})"),
            None => info!("{step} {program}"),
        }
        self.events.push(Event {
            step,
            program: program.to_owned(),
            pid,
        });
    }

    /// The same problem is reported once, even if two passes run into it.
    fn failed(&mut self, error: Error) {
        if !self.errors.contains(&error) {
            warn!("{error}");
            self.errors.push(error);
        }
    }
}

/// Identity of a program across profiles: entry name and optional path.
type Key = (String, Option<String>);

fn key(exe: &str, path: Option<&str>) -> Key {
    (
        exe.to_lowercase(),
        path.map(|p| p.replace('/', "\\").to_lowercase()),
    )
}

fn app_key(app: &App) -> Key {
    key(&app.exe, app.path.as_deref().and_then(Path::to_str))
}

/// W1 step 1: recorded suspended processes whose program the target profile
/// suspends as well stay frozen untouched; all others are resumed. Returns
/// `(stay, resume)`.
pub fn split_suspended(recorded: &[Suspended], target: &[App]) -> (Vec<Suspended>, Vec<Suspended>) {
    let frozen_by_target: HashSet<Key> = target
        .iter()
        .filter(|app| matches!(app.action, AppAction::Suspend { .. }))
        .map(app_key)
        .collect();
    recorded
        .iter()
        .cloned()
        .partition(|record| frozen_by_target.contains(&key(&record.exe, record.path.as_deref())))
}

/// Switches to the profile `target` (`neutral` included). `persist` is
/// called whenever the state must reach the disk.
///
/// Fails only if nothing could be attempted: unknown profile, or no process
/// list. Everything else ends up in the report.
pub fn switch(
    config: &Config,
    target: &str,
    state: &mut State,
    options: Options,
    persist: &mut dyn FnMut(&State) -> Result<()>,
) -> Result<Report> {
    switch_with(config, target, state, options, persist, &|| false)
}

/// [`switch`] that stops early once `interrupted` returns true (S5, console
/// control handler): the action on the current program finishes, later
/// programs and steps are skipped, the report gets `Interrupted` and the
/// active profile stays as it was. The caller then resumes what is
/// recorded, see [`recover`].
pub fn switch_with(
    config: &Config,
    target: &str,
    state: &mut State,
    options: Options,
    persist: &mut dyn FnMut(&State) -> Result<()>,
    interrupted: &dyn Fn() -> bool,
) -> Result<Report> {
    switch_guarded(
        config,
        target,
        state,
        options,
        persist,
        interrupted,
        &Vec::new,
    )
}

/// [`switch_with`] that also protects `guests()` and their ancestors like
/// kaltstellen itself (T16): the command line that asked the tray for this
/// switch. `guests` is asked again for every snapshot, so a caller added
/// while the switch runs is protected from the next step on.
pub fn switch_guarded(
    config: &Config,
    target: &str,
    state: &mut State,
    options: Options,
    persist: &mut dyn FnMut(&State) -> Result<()>,
    interrupted: &dyn Fn() -> bool,
    guests: &dyn Fn() -> Vec<Guest>,
) -> Result<Report> {
    let (name, apps): (String, &[App]) = if target.trim().eq_ignore_ascii_case(NEUTRAL) {
        (NEUTRAL.to_owned(), &[])
    } else {
        let profile = config.profile(target).ok_or_else(|| {
            Error::new(
                Code::UnknownProfile,
                format!("no profile named \"{}\"", target.trim()),
            )
        })?;
        (profile.name.clone(), &profile.apps)
    };
    let to_neutral = name == NEUTRAL;
    info!("switch from \"{}\" to \"{name}\"", state.active_profile);

    let mut run = Run {
        settings: &config.settings,
        options,
        state,
        persist,
        report: Report::default(),
        interrupted,
        halted: false,
        guests,
    };
    let snapshot = run.snapshot()?;
    run.drop_records_of_previous_boot(&snapshot);

    run.resume(&snapshot, apps);
    run.save();
    run.stop(apps)?;
    run.save();
    run.suspend(apps)?;
    run.save();
    run.start(apps, to_neutral)?;

    if run.halted {
        run.save();
        return Ok(run.report);
    }
    run.state.active_profile = name;
    run.state.boot = snapshot.boot_marker();
    run.save();
    Ok(run.report)
}

/// S3, L3: resumes everything recorded as suspended and returns to
/// `neutral`. Records of stopped programs are kept.
pub fn recover(state: &mut State, persist: &mut dyn FnMut(&State) -> Result<()>) -> Result<Report> {
    let settings = Settings::default();
    let mut run = Run {
        settings: &settings,
        options: Options::default(),
        state,
        persist,
        report: Report::default(),
        interrupted: &|| false,
        halted: false,
        guests: &Vec::new,
    };
    let snapshot = Snapshot::take()?;
    run.drop_records_of_previous_boot(&snapshot);
    run.resume(&snapshot, &[]);
    for event in &mut run.report.events {
        event.step = Step::Recover;
    }
    run.state.active_profile = NEUTRAL.to_owned();
    run.state.boot = snapshot.boot_marker();
    run.save();
    Ok(run.report)
}

struct Run<'a> {
    settings: &'a Settings,
    options: Options,
    state: &'a mut State,
    persist: &'a mut dyn FnMut(&State) -> Result<()>,
    report: Report,
    interrupted: &'a dyn Fn() -> bool,
    /// Protected like kaltstellen itself in every snapshot (T16).
    guests: &'a dyn Fn() -> Vec<Guest>,
    /// Set once `interrupted` returned true; everything after is skipped.
    halted: bool,
}

impl Run<'_> {
    /// A snapshot that protects the guests of this run.
    fn snapshot(&self) -> Result<Snapshot> {
        Ok(Snapshot::take()?.with_guests((self.guests)()))
    }

    /// Whether to stop here because of an interrupt.
    fn halt(&mut self) -> bool {
        if !self.halted && (self.interrupted)() {
            self.halted = true;
            self.report.failed(Error::new(
                Code::Interrupted,
                "interrupted; the remaining programs and steps were skipped",
            ));
        }
        self.halted
    }

    fn save(&mut self) {
        if let Err(err) = (self.persist)(self.state) {
            self.report.failed(err);
        }
    }

    /// S10: after a reboot no recorded process exists any more, and a PID
    /// may belong to something else.
    fn drop_records_of_previous_boot(&mut self, snapshot: &Snapshot) {
        let boot = snapshot.boot_marker();
        if self.state.boot != 0 && boot != 0 && self.state.boot != boot {
            if !self.state.suspended.is_empty() {
                info!(
                    "dropping {} suspended record(s) from before the last reboot",
                    self.state.suspended.len()
                );
            }
            self.state.suspended.clear();
        }
    }

    /// P6: a program on the warn list needs confirmation.
    fn confirmed(&mut self, app: &App, step: Step) -> bool {
        match protection::of(&app.exe) {
            Protection::Warn(category) if !self.options.force && !app.confirmed => {
                self.report.failed(
                    Error::new(
                        Code::WarnListUnconfirmed,
                        format!(
                            "is a {}; set confirmed = true in its entry or use --force",
                            category.description()
                        ),
                    )
                    .program(&app.exe)
                    .step(step),
                );
                false
            }
            _ => true,
        }
    }

    /// A7: the processes of an entry at this moment.
    fn resolve(&mut self, snapshot: &Snapshot, app: &App, step: Step) -> Vec<ProcessInfo> {
        match &app.path {
            None => snapshot.program(&app.exe),
            Some(path) => {
                let (processes, errors) = snapshot.program_at(&app.exe, path);
                for error in errors {
                    self.report.failed(error.step(step));
                }
                processes
            }
        }
    }

    fn resume(&mut self, snapshot: &Snapshot, target: &[App]) {
        let (mut stay, resume) = split_suspended(&self.state.suspended, target);
        for record in resume {
            match resume_record(snapshot, &record) {
                Ok(true) => self
                    .report
                    .done(Step::Resume, &record.exe, Some(record.pid)),
                // Gone, or the PID belongs to another process now (S6).
                Ok(false) => {}
                Err(err) => {
                    self.report.failed(err.step(Step::Resume));
                    // Still frozen: keep the record for the next attempt.
                    stay.push(record);
                }
            }
        }
        self.state.suspended = stay;
    }

    fn stop(&mut self, target: &[App]) -> Result<()> {
        let stops: Vec<&App> = target
            .iter()
            .filter(|app| matches!(app.action, AppAction::Stop { .. }))
            .collect();
        if stops.is_empty() {
            return Ok(());
        }
        if self.halt() {
            return Ok(());
        }
        let snapshot = self.snapshot()?;
        for app in stops {
            if self.halt() {
                break;
            }
            let AppAction::Stop {
                on_timeout,
                restart_on_neutral,
                args,
                cwd,
            } = &app.action
            else {
                continue;
            };
            if !self.confirmed(app, Step::Stop) {
                continue;
            }
            let processes = self.resolve(&snapshot, app, Step::Stop);
            if processes.is_empty() {
                continue;
            }
            let image = app.path.clone().or_else(|| image_of(&processes, &app.exe));
            let timeout = Duration::from_secs(u64::from(self.settings.stop_timeout_s));
            match process::stop(&processes, timeout, *on_timeout) {
                Ok(StopOutcome::NotRunning) => {}
                Ok(StopOutcome::Closed | StopOutcome::Killed) => {
                    self.report.done(Step::Stop, &app.exe, None);
                    let image = image.map(|p| p.to_string_lossy().into_owned());
                    self.state.stopped.retain(|s| {
                        key(&s.exe, s.image.as_deref()) != key(&app.exe, image.as_deref())
                    });
                    self.state.stopped.push(Stopped {
                        exe: app.exe.clone(),
                        image,
                        restart_on_neutral: *restart_on_neutral,
                        args: args.clone(),
                        cwd: cwd.as_ref().map(|c| c.to_string_lossy().into_owned()),
                    });
                }
                Err(err) => self.report.failed(err),
            }
        }
        Ok(())
    }

    fn suspend(&mut self, target: &[App]) -> Result<()> {
        let suspends: Vec<&App> = target
            .iter()
            .filter(|app| matches!(app.action, AppAction::Suspend { .. }))
            .collect();
        if suspends.is_empty() {
            return Ok(());
        }
        // W6: the second pass freezes processes that appeared meanwhile, for
        // example children spawned just before their parent froze.
        for pass in 0..2 {
            if self.halt() {
                return Ok(());
            }
            let snapshot = self.snapshot()?;
            for app in &suspends {
                if self.halt() {
                    return Ok(());
                }
                let AppAction::Suspend { trim } = app.action else {
                    continue;
                };
                if !self.confirmed(app, Step::Suspend) {
                    continue;
                }
                let processes: Vec<ProcessInfo> = self
                    .resolve(&snapshot, app, Step::Suspend)
                    .into_iter()
                    // W5: a process is suspended at most once.
                    .filter(|p| !self.is_recorded(p))
                    .collect();
                if processes.is_empty() {
                    continue;
                }
                if pass == 0 && self.settings.minimize_before_suspend {
                    let pids: Vec<u32> = processes.iter().map(|p| p.pid).collect();
                    process::minimize_windows(&pids, MINIMIZE_WAIT);
                }
                self.freeze(app, &processes, trim);
            }
        }
        Ok(())
    }

    fn is_recorded(&self, process: &ProcessInfo) -> bool {
        self.state
            .suspended
            .iter()
            .any(|s| s.pid == process.pid && s.created == process.created)
    }

    fn freeze(&mut self, app: &App, processes: &[ProcessInfo], trim: bool) {
        let mut targets = Vec::new();
        for info in processes {
            match Target::open(info, Access::Suspend) {
                Ok(Some(target)) => targets.push(target),
                Ok(None) => {}
                Err(err) => self.report.failed(err.step(Step::Suspend)),
            }
        }
        // Record first, then freeze: a crash in between leaves a record that
        // recovery resumes, never a frozen process nobody knows about.
        for target in &targets {
            let image = target.image_path().unwrap_or_default();
            self.state.suspended.push(Suspended {
                pid: target.info().pid,
                created: target.info().created,
                image: image.to_string_lossy().into_owned(),
                exe: app.exe.clone(),
                path: app.path.as_ref().map(|p| p.to_string_lossy().into_owned()),
            });
        }
        self.save();
        for target in &targets {
            let info = target.info();
            match target.suspend() {
                Ok(()) => {
                    self.report.done(Step::Suspend, &info.name, Some(info.pid));
                    if trim && let Err(err) = target.trim() {
                        self.report.failed(err);
                    }
                }
                Err(err) => {
                    self.report.failed(err);
                    self.state
                        .suspended
                        .retain(|s| !(s.pid == info.pid && s.created == info.created));
                }
            }
        }
    }

    fn start(&mut self, target: &[App], to_neutral: bool) -> Result<()> {
        if self.halt() {
            return Ok(());
        }
        let starts: Vec<&App> = target
            .iter()
            .filter(|app| matches!(app.action, AppAction::Start { .. }))
            .collect();
        let restarts: Vec<Stopped> = if to_neutral {
            // P5: honoured on a switch to neutral only; then the records go.
            std::mem::take(&mut self.state.stopped)
                .into_iter()
                .filter(|s| s.restart_on_neutral)
                .collect()
        } else {
            Vec::new()
        };
        if starts.is_empty() && restarts.is_empty() {
            return Ok(());
        }
        let snapshot = self.snapshot()?;
        for app in starts {
            if self.halt() {
                return Ok(());
            }
            let (AppAction::Start { args, cwd }, Some(path)) = (&app.action, &app.path) else {
                continue;
            };
            // A frozen process counts as running (W1).
            if !self.resolve(&snapshot, app, Step::Start).is_empty() {
                continue;
            }
            match process::start(path, args, cwd.as_deref()) {
                Ok(pid) => self.report.done(Step::Start, &app.exe, Some(pid)),
                Err(err) => self.report.failed(err),
            }
        }
        for stopped in restarts {
            let Some(image) = stopped.image.as_deref().map(PathBuf::from) else {
                self.report.failed(
                    Error::new(Code::StartFailed, "cannot restart: its path is unknown")
                        .program(&stopped.exe)
                        .step(Step::Start),
                );
                continue;
            };
            if !snapshot.program_at(&stopped.exe, &image).0.is_empty() {
                continue;
            }
            match process::start(&image, &stopped.args, stopped.cwd.as_deref().map(Path::new)) {
                Ok(pid) => self.report.done(Step::Start, &stopped.exe, Some(pid)),
                Err(err) => self.report.failed(err),
            }
        }
        Ok(())
    }
}

/// Resumes one recorded process. `Ok(false)` if it is not the recorded
/// process any more (S6, S10).
fn resume_record(snapshot: &Snapshot, record: &Suspended) -> Result<bool> {
    let Some(info) = snapshot.find(record.pid, record.created) else {
        return Ok(false);
    };
    let Some(target) = Target::open(info, Access::Suspend).map_err(|e| e.program(&record.exe))?
    else {
        return Ok(false);
    };
    if !record.image.is_empty()
        && let Ok(image) = target.image_path()
        && !process::same_path(&image, Path::new(&record.image))
    {
        return Ok(false);
    }
    target.resume()?;
    Ok(true)
}

/// Image path of the first process that is the program itself, not a child.
fn image_of(processes: &[ProcessInfo], exe: &str) -> Option<PathBuf> {
    processes
        .iter()
        .filter(|p| p.name.eq_ignore_ascii_case(exe))
        .find_map(|p| Target::open(p, Access::Query).ok()??.image_path().ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app(exe: &str, path: Option<&str>, action: AppAction) -> App {
        App::new(exe, path.map(PathBuf::from), action)
    }

    fn record(pid: u32, exe: &str, path: Option<&str>) -> Suspended {
        Suspended {
            pid,
            created: 1,
            image: String::new(),
            exe: exe.to_owned(),
            path: path.map(str::to_owned),
        }
    }

    fn pids(records: &[Suspended]) -> Vec<u32> {
        records.iter().map(|r| r.pid).collect()
    }

    #[test]
    fn w1_programs_the_target_also_suspends_stay_frozen() {
        let recorded = vec![
            record(1, "browser.exe", None),
            record(2, "Browser.exe", None),
            record(3, "editor.exe", None),
            record(4, "chat.exe", None),
        ];
        let target = vec![
            app("BROWSER.EXE", None, AppAction::Suspend { trim: true }),
            // Stopped by the target: must be resumed first.
            app(
                "editor.exe",
                None,
                AppAction::Stop {
                    on_timeout: crate::config::OnTimeout::Kill,
                    restart_on_neutral: false,
                    args: Vec::new(),
                    cwd: None,
                },
            ),
            app("chat.exe", None, AppAction::Ignore),
        ];
        let (stay, resume) = split_suspended(&recorded, &target);
        assert_eq!(pids(&stay), vec![1, 2]);
        assert_eq!(pids(&resume), vec![3, 4]);
    }

    #[test]
    fn w1_neutral_resumes_everything() {
        let recorded = vec![record(1, "a.exe", None), record(2, "b.exe", None)];
        let (stay, resume) = split_suspended(&recorded, &[]);
        assert!(stay.is_empty());
        assert_eq!(pids(&resume), vec![1, 2]);
    }

    #[test]
    fn w1_entries_with_paths_are_distinct_programs() {
        let recorded = vec![
            record(1, "x.exe", Some("C:\\one\\x.exe")),
            record(2, "x.exe", Some("C:\\two\\x.exe")),
            record(3, "x.exe", None),
        ];
        let target = vec![app(
            "x.exe",
            Some("c:/ONE/x.exe"),
            AppAction::Suspend { trim: false },
        )];
        let (stay, resume) = split_suspended(&recorded, &target);
        assert_eq!(pids(&stay), vec![1]);
        assert_eq!(pids(&resume), vec![2, 3]);
    }

    #[test]
    fn unknown_profile_is_refused_before_anything_happens() {
        let config = Config {
            settings: Settings::default(),
            profiles: Vec::new(),
        };
        let mut state = State::default();
        let mut saved = 0;
        let err = switch(&config, "nope", &mut state, Options::default(), &mut |_| {
            saved += 1;
            Ok(())
        })
        .unwrap_err();
        assert_eq!(err.code, Code::UnknownProfile);
        assert_eq!(saved, 0);
        assert_eq!(state, State::default());
    }
}
