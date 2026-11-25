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

use crate::config::{App, AppAction, Config, NEUTRAL, ServiceEntry, Settings};
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
    /// M4d: elevated programs the engine cannot open are handed to the
    /// background helper (role act). Only for a long-lived caller (the
    /// tray): the helper thaws what it froze when its asker is gone.
    pub helper: bool,
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
    /// A preview (H4): problems are collected but not written to the log.
    quiet: bool,
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
            if !self.quiet {
                warn!("{error}");
            }
            self.errors.push(error);
        }
    }
}

/// What a preview says will happen to a program (H4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// The step would act on it.
    Will,
    /// Nothing to do: already so, or not running.
    NoChange,
    /// The step would fail or is refused; the error is in [`Plan::problems`].
    Problem,
}

/// One program in one step of a preview, in the order of W1.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanItem {
    pub step: Step,
    pub program: String,
    pub outcome: Outcome,
    pub text: String,
    /// Processes of the program that the step would touch.
    pub processes: usize,
    /// Their private working set together, in bytes.
    pub private_bytes: u64,
}

/// What a switch would do, without doing it (H4). Built by the same steps
/// as the switch, so the two cannot drift apart.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    pub profile: String,
    pub items: Vec<PlanItem>,
    /// The errors the switch would collect (P7, P6, a missing file).
    pub problems: Vec<Error>,
}

impl Plan {
    /// The plan as text, one line per program in the order of W1, then the
    /// problems the switch would report.
    pub fn lines(&self) -> Vec<String> {
        let mut lines = vec![format!("switch to \"{}\" would:", self.profile)];
        for item in &self.items {
            let mark = match item.outcome {
                Outcome::Will => "+",
                Outcome::NoChange => "=",
                Outcome::Problem => "!",
            };
            lines.push(format!(
                "  {mark} {} {}: {}",
                item.step, item.program, item.text
            ));
        }
        if self.items.is_empty() {
            lines.push("  (nothing)".to_owned());
        }
        for problem in &self.problems {
            lines.push(format!("problem: {problem}"));
        }
        lines
    }

    /// How many items of `step` have `outcome`.
    pub fn count(&self, step: Step, outcome: Outcome) -> usize {
        self.items
            .iter()
            .filter(|i| i.step == step && i.outcome == outcome)
            .count()
    }

    /// Private memory of the programs the suspend and stop steps would touch.
    pub fn private_bytes(&self) -> u64 {
        self.items
            .iter()
            .filter(|i| i.outcome == Outcome::Will && matches!(i.step, Step::Stop | Step::Suspend))
            .map(|i| i.private_bytes)
            .sum()
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
    let (name, apps, services) = resolve_target(config, target)?;
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
        dry: false,
        plan: Vec::new(),
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
    run.services(services, to_neutral);

    if run.halted {
        run.save();
        return Ok(run.report);
    }
    run.state.active_profile = name;
    run.state.boot = snapshot.boot_marker();
    run.save();
    Ok(run.report)
}

/// The name and entries of the profile a switch goes to.
fn resolve_target<'c>(
    config: &'c Config,
    target: &str,
) -> Result<(String, &'c [App], &'c [ServiceEntry])> {
    if target.trim().eq_ignore_ascii_case(NEUTRAL) {
        return Ok((NEUTRAL.to_owned(), &[], &[]));
    }
    let profile = config.profile(target).ok_or_else(|| {
        Error::new(
            Code::UnknownProfile,
            format!("no profile named \"{}\"", target.trim()),
        )
    })?;
    Ok((profile.name.clone(), &profile.apps, &profile.services))
}

/// H4: what [`switch_guarded`] would do, in the same steps, without doing
/// it. Works on a copy of `state`, writes nothing, sends no signal and
/// minimises no window. To predict P7 it opens the processes the real step
/// would open, with the same rights, and closes them again at once.
pub fn preview(
    config: &Config,
    target: &str,
    state: &State,
    options: Options,
    guests: &dyn Fn() -> Vec<Guest>,
) -> Result<Plan> {
    let (name, apps, services) = resolve_target(config, target)?;
    let to_neutral = name == NEUTRAL;
    let mut state = state.clone();
    let mut persist = |_: &State| Ok(());
    let mut run = Run {
        settings: &config.settings,
        options,
        state: &mut state,
        persist: &mut persist,
        report: Report {
            quiet: true,
            ..Report::default()
        },
        interrupted: &|| false,
        halted: false,
        guests,
        dry: true,
        plan: Vec::new(),
    };
    let snapshot = run.snapshot()?;
    run.drop_records_of_previous_boot(&snapshot);
    run.resume(&snapshot, apps);
    run.stop(apps)?;
    run.suspend(apps)?;
    run.start(apps, to_neutral)?;
    run.services(services, to_neutral);
    Ok(Plan {
        profile: name,
        items: run.plan,
        problems: run.report.errors,
    })
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
        dry: false,
        plan: Vec::new(),
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
    /// H4: build a plan instead of acting.
    dry: bool,
    plan: Vec<PlanItem>,
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

    /// H4: one line of the plan.
    fn planned(
        &mut self,
        step: Step,
        program: &str,
        outcome: Outcome,
        text: impl Into<String>,
        processes: &[ProcessInfo],
    ) {
        let text = text.into();
        // The second pass of W6 runs into the same refusal again.
        if outcome == Outcome::Problem
            && self.plan.iter().any(|i| {
                i.step == step && i.program == program && i.outcome == outcome && i.text == text
            })
        {
            return;
        }
        self.plan.push(PlanItem {
            step,
            program: program.to_owned(),
            outcome,
            text,
            processes: processes.len(),
            private_bytes: processes.iter().map(|p| p.private_bytes).sum(),
        });
    }

    /// H4: opens `processes` with `access` as the real step would and
    /// closes them again; a refusal (P7) is collected like in the real step.
    /// Returns the processes that can be acted on and how many were refused;
    /// one that has ended meanwhile is neither (the real step skips it).
    fn probe(
        &mut self,
        processes: &[ProcessInfo],
        access: Access,
        step: Step,
    ) -> (Vec<ProcessInfo>, usize) {
        let mut usable = Vec::new();
        let mut refused = 0;
        for info in processes {
            match Target::open(info, access) {
                Ok(Some(_)) => usable.push(info.clone()),
                Ok(None) => {}
                Err(err) => {
                    refused += 1;
                    self.report.failed(err.step(step));
                }
            }
        }
        (usable, refused)
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
        // Services mostly come up again by themselves after a reboot.
        if self.state.boot != 0 && boot != 0 && self.state.boot != boot {
            self.state.stopped_services.clear();
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
                if self.dry {
                    self.planned(
                        step,
                        &app.exe,
                        Outcome::Problem,
                        "on the warn list, not confirmed: skipped",
                        &[],
                    );
                }
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
        if self.dry {
            for record in &stay {
                self.planned(
                    Step::Resume,
                    &record.exe,
                    Outcome::NoChange,
                    "stays frozen: the profile freezes it too",
                    &[],
                );
            }
            for record in &resume {
                let Some(info) = snapshot.find(record.pid, record.created).cloned() else {
                    continue;
                };
                let (usable, refused) =
                    self.probe(std::slice::from_ref(&info), Access::Suspend, Step::Resume);
                if usable.is_empty() {
                    let (outcome, text) = if refused > 0 {
                        (Outcome::Problem, "cannot be resumed")
                    } else {
                        (Outcome::NoChange, "not running any more")
                    };
                    self.planned(Step::Resume, &record.exe, outcome, text, &[]);
                } else {
                    self.planned(
                        Step::Resume,
                        &record.exe,
                        Outcome::Will,
                        "will resume",
                        &usable,
                    );
                }
            }
            self.state.suspended = stay;
            return;
        }
        for record in resume {
            match resume_record(snapshot, &record, self.options.helper) {
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
                if self.dry {
                    self.planned(Step::Stop, &app.exe, Outcome::NoChange, "not running", &[]);
                }
                continue;
            }
            if self.dry {
                let (usable, refused) = self.probe(&processes, Access::Stop, Step::Stop);
                if usable.is_empty() {
                    let (outcome, text) = if refused > 0 {
                        (Outcome::Problem, "cannot be stopped")
                    } else {
                        (Outcome::NoChange, "not running")
                    };
                    self.planned(Step::Stop, &app.exe, outcome, text, &[]);
                } else {
                    let after = match on_timeout {
                        crate::config::OnTimeout::Kill => "ended if it does not close",
                        crate::config::OnTimeout::Keep => "left running if it does not close",
                    };
                    let text = format!(
                        "will stop: closes within {} s, otherwise {after}",
                        self.settings.stop_timeout_s
                    );
                    self.planned(Step::Stop, &app.exe, Outcome::Will, text, &usable);
                }
                continue;
            }
            // Where it was started from and with which arguments, read now,
            // while it runs: that is what the restart on neutral uses unless
            // the entry names its own arguments.
            let launch = process::launch_of(&processes, &app.exe, app.path.as_deref());
            if *restart_on_neutral && launch.as_ref().is_some_and(|l| !l.args_known) {
                warn!(
                    "{}: its command line cannot be read; it will be restarted without arguments",
                    app.exe
                );
            }
            let image = app
                .path
                .clone()
                .or_else(|| launch.as_ref().map(|l| l.image.clone()))
                .or_else(|| image_of(&processes, &app.exe));
            let args = if args.is_empty() {
                launch.map(|l| l.args).unwrap_or_default()
            } else {
                args.clone()
            };
            let timeout = Duration::from_secs(u64::from(self.settings.stop_timeout_s));
            // M4d: what is elevated is ended by the helper (no graceful close
            // for it: `stop` is `terminate` there).
            let (processes, ended) = if self.options.helper {
                end_elevated(processes)
            } else {
                (processes, 0)
            };
            let result = if processes.is_empty() {
                Ok(StopOutcome::NotRunning)
            } else {
                process::stop(&processes, timeout, *on_timeout)
            };
            let result = match result {
                Ok(StopOutcome::NotRunning) if ended > 0 => Ok(StopOutcome::Killed),
                other => other,
            };
            // An elevated program ended by the helper counts as stopped even
            // if another process of the program then reports an error.
            let result = match result {
                Err(err) if ended > 0 => {
                    self.report.failed(err);
                    Ok(StopOutcome::Killed)
                }
                other => other,
            };
            match result {
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
                        args,
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
                    if self.dry && pass == 0 {
                        let running = self.resolve(&snapshot, app, Step::Suspend);
                        let (outcome, text) = if running.is_empty() {
                            (Outcome::NoChange, "not running")
                        } else {
                            (Outcome::NoChange, "no change: already frozen")
                        };
                        self.planned(Step::Suspend, &app.exe, outcome, text, &[]);
                    }
                    continue;
                }
                if self.dry {
                    self.freeze_dry(app, &processes);
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

    /// H4: what [`Run::freeze`] would do. The processes are recorded in the
    /// copy of the state, so the second pass sees them as frozen (W5).
    fn freeze_dry(&mut self, app: &App, processes: &[ProcessInfo]) {
        let (usable, refused) = self.probe(processes, Access::Suspend, Step::Suspend);
        if usable.is_empty() {
            let (outcome, text) = if refused > 0 {
                (Outcome::Problem, "cannot be frozen")
            } else {
                (Outcome::NoChange, "not running")
            };
            self.planned(Step::Suspend, &app.exe, outcome, text, &[]);
            return;
        }
        for info in &usable {
            self.state.suspended.push(Suspended {
                pid: info.pid,
                created: info.created,
                image: String::new(),
                exe: app.exe.clone(),
                path: app.path.as_ref().map(|p| p.to_string_lossy().into_owned()),
            });
        }
        let text = if self.settings.minimize_before_suspend {
            "will freeze: windows are minimised first"
        } else {
            "will freeze"
        };
        self.planned(Step::Suspend, &app.exe, Outcome::Will, text, &usable);
    }

    fn is_recorded(&self, process: &ProcessInfo) -> bool {
        self.state
            .suspended
            .iter()
            .any(|s| s.pid == process.pid && s.created == process.created)
    }

    fn freeze(&mut self, app: &App, processes: &[ProcessInfo], trim: bool) {
        let mut targets = Vec::new();
        let mut elevated = Vec::new();
        for info in processes {
            match Target::open(info, Access::Suspend) {
                Ok(Some(target)) => targets.push(target),
                Ok(None) => {}
                Err(err) if self.options.helper && err.code == Code::NeedsAdmin => {
                    elevated.push((info.clone(), err));
                }
                Err(err) => self.report.failed(err.step(Step::Suspend)),
            }
        }
        self.freeze_elevated(app, elevated);
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

    /// M4d: freezes elevated programs through the helper. The record is
    /// written first, as for a program frozen directly; the original
    /// refusal is reported when the helper does not do it (not installed,
    /// role off, program not on the allow list).
    fn freeze_elevated(&mut self, app: &App, elevated: Vec<(ProcessInfo, Error)>) {
        for (info, refusal) in elevated {
            self.state.suspended.push(Suspended {
                pid: info.pid,
                created: info.created,
                image: String::new(),
                exe: app.exe.clone(),
                path: app.path.as_ref().map(|p| p.to_string_lossy().into_owned()),
            });
            self.save();
            match crate::helper::client::ask_act(
                crate::helper::protocol::Command::Suspend,
                info.pid,
                info.created,
            ) {
                Ok(Some(_)) => self.report.done(Step::Suspend, &info.name, Some(info.pid)),
                // Lost connection or timeout: the helper may have frozen it
                // already, so the record stays and a later resume tries.
                Err(err) if !crate::helper::client::was_refused(&err) => {
                    warn!("helper: {err}");
                    self.report.failed(refusal.step(Step::Suspend));
                }
                other => {
                    if let Err(err) = other {
                        info!("helper: {err}");
                    }
                    self.state
                        .suspended
                        .retain(|s| !(s.pid == info.pid && s.created == info.created));
                    self.report.failed(refusal.step(Step::Suspend));
                }
            }
        }
        self.save();
    }

    /// H4: what starting `exe` from `path` would do. Whether it needs
    /// elevation is not known in advance (the manifest is not read).
    fn start_dry(&mut self, exe: &str, path: &Path) {
        if path.is_file() {
            self.planned(
                Step::Start,
                exe,
                Outcome::Will,
                format!("will start {}", path.display()),
                &[],
            );
        } else {
            let mut error = Error::new(
                Code::StartFailed,
                format!("{} was not found", path.display()),
            )
            .program(exe)
            .step(Step::Start);
            error.path = Some(path.to_string_lossy().into_owned());
            self.report.failed(error);
            self.planned(
                Step::Start,
                exe,
                Outcome::Problem,
                format!("{} was not found", path.display()),
                &[],
            );
        }
    }

    /// M4e: stops and starts Windows services through the helper. Services
    /// this tool stopped are started again on a switch to neutral (the
    /// record goes when that worked, or when there is nothing to restore:
    /// the service runs already or is gone); a `stop` entry is stopped if
    /// it runs and recorded, a `start` entry is started if it does not run.
    /// The record is saved right after each change.
    fn services(&mut self, target: &[ServiceEntry], to_neutral: bool) {
        use crate::config::ServiceAction;
        if self.halt() {
            return;
        }
        let restarts: Vec<String> = if to_neutral {
            self.state.stopped_services.clone()
        } else {
            Vec::new()
        };
        for name in restarts {
            if self.halt() {
                return;
            }
            let done = match self.service_step(&name, true) {
                Service::Asked(reached) => reached,
                Service::Already | Service::Gone => true,
                Service::Failed => false,
            };
            if done && !self.dry {
                self.state
                    .stopped_services
                    .retain(|s| !s.eq_ignore_ascii_case(&name));
                self.save();
            }
        }
        for entry in target {
            if self.halt() {
                return;
            }
            match entry.action {
                ServiceAction::Stop => {
                    // A recorded service that runs again (started by hand or
                    // by a reboot) is stopped again like any other.
                    if matches!(self.service_step(&entry.name, false), Service::Asked(_))
                        && !self.dry
                        && !self
                            .state
                            .stopped_services
                            .iter()
                            .any(|s| s.eq_ignore_ascii_case(&entry.name))
                    {
                        self.state.stopped_services.push(entry.name.clone());
                        self.save();
                    }
                }
                ServiceAction::Start => {
                    self.service_step(&entry.name, true);
                }
            }
        }
    }

    /// One service step: ask for the state of the service, then the helper
    /// to change it if needed (nothing in a preview, which only plans).
    fn service_step(&mut self, name: &str, start: bool) -> Service {
        use crate::win32::service::State as Svc;
        let step = if start { Step::Start } else { Step::Stop };
        let now = match crate::win32::service::query(name) {
            Ok(Some(info)) => info.state,
            Ok(None) => {
                self.report.failed(
                    Error::new(Code::ServiceFailed, "there is no such service")
                        .program(name)
                        .step(step),
                );
                return Service::Gone;
            }
            Err(err) => {
                self.report.failed(err.step(step));
                return Service::Failed;
            }
        };
        let wanted = if start { Svc::Running } else { Svc::Stopped };
        if now == wanted {
            if self.dry {
                let text = if start {
                    "service already running"
                } else {
                    "service not running"
                };
                self.planned(step, name, Outcome::NoChange, text, &[]);
            }
            return Service::Already;
        }
        // On its way already (maybe by an earlier request of ours): wait
        // for it, do not ask again.
        if (start && now == Svc::Starting) || (!start && now == Svc::Stopping) {
            return Service::Asked(false);
        }
        if self.dry {
            let text = if start {
                "will start the service (needs the helper)"
            } else {
                "will stop the service (needs the helper)"
            };
            self.planned(step, name, Outcome::Will, text, &[]);
            return Service::Asked(false);
        }
        let command = if start {
            crate::helper::protocol::Command::StartService
        } else {
            crate::helper::protocol::Command::StopService
        };
        match crate::helper::client::ask_service(command, name) {
            Ok(Some(reply)) => {
                self.report.done(step, name, None);
                Service::Asked(reply.reached == Some(true))
            }
            Ok(None) => {
                self.report.failed(
                    Error::new(
                        Code::ServiceFailed,
                        "needs the background helper (not installed or not running)",
                    )
                    .program(name)
                    .step(step),
                );
                Service::Failed
            }
            Err(err) => {
                self.report.failed(
                    Error::new(Code::ServiceFailed, err.reason)
                        .program(name)
                        .step(step),
                );
                Service::Failed
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
                if self.dry {
                    self.planned(
                        Step::Start,
                        &app.exe,
                        Outcome::NoChange,
                        "already running",
                        &[],
                    );
                }
                continue;
            }
            if self.dry {
                self.start_dry(&app.exe, path);
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
            if self.dry {
                self.start_dry(&stopped.exe, &image);
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

/// What a service step came to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Service {
    /// The helper was asked (or it is on its way already); whether the
    /// wanted state was reached by now.
    Asked(bool),
    /// It was in the wanted state.
    Already,
    /// There is no such service.
    Gone,
    /// The step failed (reported).
    Failed,
}

/// Resumes one recorded process. `Ok(false)` if it is not the recorded
/// process any more (S6, S10).
fn resume_record(snapshot: &Snapshot, record: &Suspended, helper: bool) -> Result<bool> {
    let Some(info) = snapshot.find(record.pid, record.created) else {
        return Ok(false);
    };
    let target = match Target::open(info, Access::Suspend) {
        Ok(Some(target)) => target,
        Ok(None) => return Ok(false),
        // An elevated program frozen by the helper is thawed by it.
        Err(err) if helper && err.code == Code::NeedsAdmin => {
            return match crate::helper::client::ask_act(
                crate::helper::protocol::Command::Resume,
                info.pid,
                info.created,
            ) {
                Ok(Some(_)) => Ok(true),
                _ => Err(err.program(&record.exe)),
            };
        }
        Err(err) => return Err(err.program(&record.exe)),
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

/// M4d: ends the processes that need administrator rights through the
/// helper. Returns the others and how many were ended; one the helper
/// refuses stays in the list so the normal step reports the refusal.
fn end_elevated(processes: Vec<ProcessInfo>) -> (Vec<ProcessInfo>, usize) {
    let mut rest = Vec::new();
    let mut ended = 0;
    for info in processes {
        let elevated = matches!(
            Target::open(&info, Access::Stop),
            Err(ref err) if err.code == Code::NeedsAdmin
        );
        if elevated
            && matches!(
                crate::helper::client::ask_act(
                    crate::helper::protocol::Command::Terminate,
                    info.pid,
                    info.created,
                ),
                Ok(Some(_))
            )
        {
            info!("ended {} (pid {}) through the helper", info.name, info.pid);
            ended += 1;
        } else {
            rest.push(info);
        }
    }
    (rest, ended)
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

    fn config_with(apps: Vec<App>) -> Config {
        Config {
            settings: Settings::default(),
            profiles: vec![crate::config::Profile {
                name: "focus".to_owned(),
                hotkey: None,
                apps,
                triggers: Vec::new(),
                priority: 0,
                watch: false,
                services: Vec::new(),
            }],
        }
    }

    #[test]
    fn preview_lists_the_steps_and_changes_nothing() {
        let config = config_with(vec![
            app(
                "kaltstellen-test-absent.exe",
                None,
                AppAction::Suspend { trim: false },
            ),
            App::new(
                "kaltstellen-test-tool.exe",
                Some(PathBuf::from("C:\\kaltstellen-test-absent\\tool.exe")),
                AppAction::Start {
                    args: Vec::new(),
                    cwd: None,
                },
            ),
        ]);
        let state = State::default();
        let plan = preview(&config, "focus", &state, Options::default(), &Vec::new).unwrap();
        assert_eq!(plan.profile, "focus");
        assert_eq!(plan.count(Step::Suspend, Outcome::NoChange), 1);
        assert_eq!(plan.count(Step::Start, Outcome::Problem), 1);
        assert_eq!(plan.problems.len(), 1);
        assert_eq!(plan.problems[0].code, Code::StartFailed);
        assert_eq!(state, State::default());
        let lines = plan.lines();
        assert_eq!(lines[0], "switch to \"focus\" would:");
        assert!(lines.iter().any(|l| l.starts_with("problem: ")));
    }

    #[test]
    fn preview_of_neutral_with_nothing_held_is_empty() {
        let config = config_with(Vec::new());
        let plan = preview(
            &config,
            "neutral",
            &State::default(),
            Options::default(),
            &Vec::new,
        )
        .unwrap();
        assert!(plan.items.is_empty());
        assert!(plan.problems.is_empty());
    }

    #[test]
    fn preview_refuses_an_unknown_profile() {
        let config = config_with(Vec::new());
        let err = preview(
            &config,
            "nope",
            &State::default(),
            Options::default(),
            &Vec::new,
        )
        .unwrap_err();
        assert_eq!(err.code, Code::UnknownProfile);
    }
}
