//! Command-line interface (U2). Exit codes follow spec E2.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::Result;
use clap::{Parser, Subcommand};
use log::{LevelFilter, info, warn};

use crate::config;
use crate::engine;
use crate::error::{Code, Error};
use crate::exit;
use crate::logging;
use crate::paths;
use crate::process::{Access, Snapshot, Target};
use crate::program_info;
use crate::protection::{self, Protection};
use crate::state::{Lock, State, Store};
use crate::tray::protocol::{self, Body, Finished};
use crate::tray::{Endpoints, client};
use crate::update;
use crate::win32::console::Interrupts;

/// Put background apps on ice: suspend, resume and trim Windows processes.
#[derive(Parser)]
#[command(version = update::CURRENT, about, disable_version_flag = true)]
struct Cli {
    /// Print the version (`kaltstellen <version>`); `-V` works too
    #[arg(short = 'v', short_alias = 'V', long, action = clap::ArgAction::Version)]
    version: (),

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// List running processes, optionally only those containing FILTER
    List {
        filter: Option<String>,
        /// Also show what each program is: description and publisher (from
        /// its file only; checking signatures takes a moment)
        #[arg(long)]
        info: bool,
    },
    /// Freeze all processes with the given executable name
    Suspend {
        /// Executable name, e.g. "notepad" or "notepad.exe"
        name: String,
        /// Also move the frozen processes' memory out of RAM
        #[arg(long)]
        trim: bool,
    },
    /// Continue all processes with the given executable name
    Resume {
        /// Executable name, e.g. "notepad" or "notepad.exe"
        name: String,
    },
    /// Validate the configuration file and show what it defines
    CheckConfig {
        /// File to check instead of %APPDATA%\kaltstellen\config.toml
        file: Option<PathBuf>,
    },
    /// Switch to a profile; "neutral" brings everything back
    Switch {
        /// Profile name from the configuration, or "neutral"
        profile: String,
        /// Confirm programs on the warn list (remote access, drivers, ...)
        #[arg(long)]
        force: bool,
        /// With a tray running: return once the tray has queued the request
        #[arg(long)]
        no_wait: bool,
    },
    /// Show how two profiles differ: programs only one has, other actions
    /// or options, hotkey, triggers and priority
    Compare {
        /// First profile
        a: String,
        /// Second profile
        b: String,
    },
    /// Show the active profile and what kaltstellen has put on ice
    Status {
        /// Also list running programs without an entry in the active profile
        #[arg(long)]
        unassigned: bool,
    },
    /// Resume everything recorded as frozen, e.g. after a crash
    Recover {
        /// With a tray running: return once the tray has queued the request
        #[arg(long)]
        no_wait: bool,
    },
    /// End the running tray as its menu's Quit does (resumes what it froze)
    Quit,
    /// Run the notification-area icon with the profile menu
    Tray,
    /// Open the main window: profiles, programs and their actions
    Gui {
        /// Capture the window as PNG into this file and exit (review)
        #[arg(long, hide = true)]
        screenshot: Option<PathBuf>,
        /// View to open before the capture: entry, add, confirm, new, settings
        #[arg(long, hide = true)]
        view: Option<String>,
        /// Colour scheme for the capture: light or dark
        #[arg(long, hide = true)]
        theme: Option<String>,
        /// Run the bridge requests in this JSON file through the page (tests)
        #[arg(long, hide = true)]
        drive: Option<PathBuf>,
        /// Where --drive writes the replies
        #[arg(long, hide = true)]
        drive_out: Option<PathBuf>,
    },
    /// Automatic switching by trigger programs: show, or switch on or off
    Automation {
        #[arg(value_enum)]
        switch: Option<OnOff>,
    },
    /// Start the tray at sign-in: show, or switch on or off
    Autostart {
        #[arg(value_enum)]
        switch: Option<OnOff>,
    },
    /// Install the newest release from GitHub, or only check for it
    Update {
        /// Only check and report, do not install
        #[arg(long, conflicts_with = "force")]
        check: bool,
        /// Install the newest release even if it is not newer
        #[arg(long)]
        force: bool,
        /// Update mode: auto (install by itself while neutral), manual
        /// (check daily, install on request) or off (never check)
        #[arg(long, value_enum, conflicts_with_all = ["check", "force", "auto"])]
        mode: Option<UpdateModeArg>,
        /// Earlier form of --mode: on means auto, off means off
        #[arg(long, hide = true, value_enum, conflicts_with_all = ["check", "force"])]
        auto: Option<OnOff>,
        /// With --check: print nothing (the tray's daily check)
        #[arg(long, hide = true, requires = "check")]
        quiet: bool,
        /// Started from the tray menu: wait for Enter after a failure
        #[arg(long, hide = true)]
        from_tray: bool,
        /// The tray's automatic install: quits the tray only if it is idle
        /// in neutral, skips a version that could not be installed here recently
        #[arg(long, hide = true, conflicts_with_all = ["check", "force", "from_tray"])]
        unattended: bool,
    },
}

#[derive(Clone, Copy, clap::ValueEnum)]
enum UpdateModeArg {
    Auto,
    Manual,
    Off,
}

#[derive(Clone, Copy, clap::ValueEnum)]
enum OnOff {
    On,
    Off,
}

pub fn main() -> ExitCode {
    // A double-click on the executable: no arguments and a console of its
    // own. The help would flash by in a window that closes at once, so it
    // opens the main window and runs the tray (which lets go of the
    // console; one already running stays, this one ends) [assumed].
    if std::env::args_os().len() == 1 && crate::win32::console::is_own() {
        init_logging();
        info!("started without arguments and without a terminal: main window and tray");
        return ExitCode::from(crate::tray::main_and_window());
    }
    let cli = Cli::parse();
    init_logging();
    let code = match cli.command {
        Command::CheckConfig { file } => check_config(file),
        Command::Switch {
            profile,
            force,
            no_wait,
        } => switch(&profile, force, !no_wait),
        Command::Compare { a, b } => compare(&a, &b),
        Command::Status { unassigned } => {
            let code = status();
            if unassigned && code == exit::OK {
                status_unassigned()
            } else {
                code
            }
        }
        Command::Recover { no_wait } => recover(!no_wait),
        Command::Quit => quit(),
        Command::Tray => crate::tray::main(),
        Command::Gui {
            screenshot,
            view,
            theme,
            drive,
            drive_out,
        } => crate::gui::main(crate::gui::Args {
            screenshot,
            view,
            theme: match theme.as_deref() {
                Some("dark") => Some(crate::gui::GuiTheme::Dark),
                Some("light") => Some(crate::gui::GuiTheme::Light),
                _ => None,
            },
            drive,
            drive_out,
        }),
        Command::Autostart { switch } => autostart(switch),
        Command::Automation { switch } => automation(switch),
        Command::Update {
            check,
            force,
            mode,
            auto,
            quiet,
            from_tray,
            unattended,
        } => {
            let mode = mode
                .map(|m| match m {
                    UpdateModeArg::Auto => update::Mode::Auto,
                    UpdateModeArg::Manual => update::Mode::Manual,
                    UpdateModeArg::Off => update::Mode::Off,
                })
                .or(auto.map(|a| match a {
                    OnOff::On => update::Mode::Auto,
                    OnOff::Off => update::Mode::Off,
                }));
            let code = match (mode, check) {
                (Some(mode), _) => update_mode(mode),
                (None, true) => update_check(quiet),
                (None, false) => update_install(force, unattended),
            };
            if from_tray {
                finish_console(code);
            }
            code
        }
        command => match run(command) {
            Ok(true) => 0,
            Ok(false) => exit::ERRORS,
            Err(err) => {
                eprintln!("error: {err:#}");
                exit::ERRORS
            }
        },
    };
    ExitCode::from(code)
}

fn store() -> Result<Store, u8> {
    paths::data_dir().map(Store::new).ok_or_else(|| {
        eprintln!("error: no data directory (LOCALAPPDATA unset)");
        exit::ERRORS
    })
}

/// L1 without a tray: the switch runs in this process and holds the state
/// lock for its duration.
fn locked_store() -> Result<(Store, Lock), u8> {
    let store = store()?;
    match store.lock() {
        Ok(lock) => Ok((store, lock)),
        Err(err) => {
            eprintln!("error: {err}");
            Err(if err.code == Code::StateLocked {
                exit::UNREACHABLE
            } else {
                exit::ERRORS
            })
        }
    }
}

fn load_state(store: &Store) -> State {
    let (state, problem) = store.load();
    if let Some(problem) = problem {
        warn!("{problem}");
        eprintln!("warning: {problem}");
    }
    state
}

fn print_rejected(file: &Path, rejected: &config::Rejected) {
    warn!("{}: {rejected}", file.display());
    for line in config::rejected_lines(file, rejected) {
        eprintln!("{line}");
    }
}

fn print_finished(finished: &Finished) -> u8 {
    let (out, err) = finished.render();
    print!("{out}");
    eprint!("{err}");
    finished.exit_code
}

/// L1: with a tray running in this session, the request goes through its
/// pipe and queue (W3). `None` when no tray runs: the caller works
/// in-process.
fn through_tray(command: protocol::Command) -> Option<u8> {
    let waits = command.waits();
    let endpoints = match Endpoints::current() {
        Ok(endpoints) => endpoints,
        Err(err) => {
            warn!("{err}; working without the tray");
            return None;
        }
    };
    let body = match client::request(&endpoints, command, || {
        eprintln!("waiting for the tray's running switch");
    }) {
        Ok(None) => return None,
        Ok(Some(body)) => body,
        Err(err) => {
            warn!("{err}");
            eprintln!("error: {err}");
            return Some(exit::UNREACHABLE);
        }
    };
    Some(match body {
        Body::Accepted { waiting } if !waits => {
            println!("{} by the tray", if waiting { "queued" } else { "started" });
            exit::OK
        }
        Body::Done(finished) => print_finished(&finished),
        Body::Superseded { by } => {
            eprintln!(
                "error: {}: superseded by a later request for \"{by}\"",
                Code::Superseded
            );
            exit::SUPERSEDED
        }
        Body::Rejected { exit_code, lines } => {
            for line in lines {
                eprintln!("{line}");
            }
            exit_code
        }
        Body::Status(status) => {
            print_status(&status.state);
            if let Some(running) = status.running {
                println!("tray: running {running}");
            }
            if let Some(waiting) = status.waiting {
                println!("tray: waiting {waiting}");
            }
            if let Some(automation) = status.automation {
                for line in automation.lines(&status.state.active_profile) {
                    println!("{line}");
                }
            }
            exit::OK
        }
        Body::Accepted { .. } => {
            eprintln!(
                "error: {}: unexpected answer from the tray",
                Code::TrayProtocol
            );
            exit::UNREACHABLE
        }
    })
}

fn switch(target: &str, force: bool, wait: bool) -> u8 {
    if let Some(code) = through_tray(protocol::Command::Switch {
        profile: target.to_owned(),
        force,
        wait,
    }) {
        return code;
    }
    let (store, _lock) = match locked_store() {
        Ok(locked) => locked,
        Err(code) => return code,
    };
    let config = match config::for_switch(paths::config_file().as_deref(), target) {
        Ok((config, notes)) => {
            for note in notes {
                eprintln!("{note}");
            }
            config
        }
        Err(lines) => {
            for line in lines {
                eprintln!("{line}");
            }
            return exit::CONFIG_INVALID;
        }
    };
    let mut state = load_state(&store);
    let options = engine::Options { force };
    // S5: Ctrl+C, Ctrl+Break or closing the console must not leave anything
    // frozen; they stop the switch early and everything is resumed below.
    let interrupts = match Interrupts::catch() {
        Ok(interrupts) => Some(interrupts),
        Err(err) => {
            warn!("{err}");
            None
        }
    };
    let interrupted = || interrupts.as_ref().is_some_and(Interrupts::requested);
    let mut persist = |s: &State| store.save(s);
    let finished = match engine::switch_with(
        &config,
        target,
        &mut state,
        options,
        &mut persist,
        &interrupted,
    ) {
        Ok(report) => {
            let mut finished =
                Finished::new(true, &state.active_profile, &report.events, &report.errors);
            if report.errors.iter().any(|e| e.code == Code::Interrupted) {
                info!("switch interrupted; resuming what is recorded as frozen");
                match engine::recover(&mut state, &mut persist) {
                    Ok(recovered) => {
                        finished
                            .events
                            .extend(recovered.events.iter().map(protocol::EventLine::from));
                        finished
                            .errors
                            .extend(recovered.errors.iter().map(ToString::to_string));
                    }
                    Err(err) => finished.errors.push(err.to_string()),
                }
                finished.active_profile = Some(state.active_profile.clone());
                finished.exit_code = exit::ERRORS;
            }
            finished
        }
        Err(err) => {
            let mut finished = Finished::new(false, &state.active_profile, &[], &[err]);
            finished.profiles = config.profiles.iter().map(|p| p.name.clone()).collect();
            finished
        }
    };
    if let Some(interrupts) = &interrupts {
        interrupts.finished();
    }
    print_finished(&finished)
}

/// L2 from the command line: the tray of this data directory resumes what
/// it froze and ends, as with Quit in its menu (the pipe request the
/// updater uses). Without a tray there is nothing to end.
fn quit() -> u8 {
    let endpoints = match Endpoints::current() {
        Ok(endpoints) => endpoints,
        Err(err) => {
            eprintln!("error: {err}");
            return exit::UNREACHABLE;
        }
    };
    match client::request(&endpoints, protocol::Command::Quit, || {
        eprintln!("waiting for the tray's running switch");
    }) {
        Ok(None) => {
            println!("no tray runs");
            exit::OK
        }
        Ok(Some(Body::Done(finished))) => print_finished(&finished),
        Ok(Some(Body::Rejected { exit_code, lines })) => {
            for line in lines {
                eprintln!("{line}");
            }
            exit_code
        }
        Ok(Some(other)) => {
            eprintln!("error: unexpected answer of the tray: {other:?}");
            exit::UNREACHABLE
        }
        // The tray may end before its reply is written (as the updater
        // knows): what counts is that it ends.
        Err(err) => {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
            while crate::win32::instance::Instance::exists(&endpoints.mutex) {
                if std::time::Instant::now() >= deadline {
                    eprintln!("error: {err}");
                    return exit::UNREACHABLE;
                }
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
            println!("the tray has ended");
            exit::OK
        }
    }
}

fn recover(wait: bool) -> u8 {
    if let Some(code) = through_tray(protocol::Command::Recover { wait }) {
        return code;
    }
    let (store, _lock) = match locked_store() {
        Ok(locked) => locked,
        Err(code) => return code,
    };
    let mut state = load_state(&store);
    let finished = match engine::recover(&mut state, &mut |s| store.save(s)) {
        Ok(report) => Finished::new(true, &state.active_profile, &report.events, &report.errors),
        Err(err) => Finished::new(false, &state.active_profile, &[], &[err]),
    };
    print_finished(&finished)
}

/// Start at sign-in (WP7): the per-user Run key points at this executable
/// with `tray`.
fn autostart(switch: Option<OnOff>) -> u8 {
    let at = crate::autostart::Location::current();
    let exe = match std::env::current_exe() {
        Ok(exe) => exe,
        Err(err) => {
            eprintln!(
                "error: {}: cannot find this executable: {err}",
                Code::AutostartIo
            );
            return exit::ERRORS;
        }
    };
    let result = match switch {
        Some(OnOff::On) => crate::autostart::enable(&at, &exe),
        Some(OnOff::Off) => crate::autostart::disable(&at).map(|_| ()),
        None => Ok(()),
    };
    if let Err(err) = result {
        eprintln!("error: {err}");
        return exit::ERRORS;
    }
    // An explicit choice answers the tray's one-time question (L8): a tray
    // started later (by the installer, say) does not ask again.
    if switch.is_some()
        && let Some(dir) = paths::config_dir()
        && let Err(err) = crate::settings::edit(&dir, |s| s.autostart_asked = true)
    {
        warn!("autostart: {err}");
    }
    match crate::autostart::state(&at, &exe) {
        Ok(crate::autostart::State::On) => println!("start at sign-in: on"),
        Ok(crate::autostart::State::Off) => println!("start at sign-in: off"),
        Ok(crate::autostart::State::Other(command)) => {
            println!("start at sign-in: on, but for another program: {command}");
        }
        Err(err) => {
            eprintln!("error: {err}");
            return exit::ERRORS;
        }
    }
    exit::OK
}

/// A note for a setting the running tray did not take over at once, or
/// took over with a note of its own (not saved).
fn delivery_note(delivery: &client::Delivery) -> Option<String> {
    match delivery {
        client::Delivery::Tray(status) if !status.notes.is_empty() => Some(status.notes.join(" ")),
        client::Delivery::Tray(_) | client::Delivery::Saved => None,
        client::Delivery::OlderTray(reason) => Some(format!(
            "saved, but the running tray is older and did not take it over ({reason}); it keeps its own setting until it restarts"
        )),
        client::Delivery::Unanswered(reason) => Some(format!(
            "saved, but the running tray did not answer in time ({reason})"
        )),
    }
}

/// U4: whether some profile of the configuration in effect names triggers;
/// `None` when the file cannot be read (then nothing is said about it).
fn triggers_configured() -> Option<bool> {
    config::for_switch(paths::config_file().as_deref(), config::NEUTRAL)
        .ok()
        .map(|(config, _)| config.profiles.iter().any(|p| !p.triggers.is_empty()))
}

/// `update --mode auto|manual|off` (UP1). With a tray running, the tray
/// writes the mode and checks at once if the mode checks (pipe
/// `set_update_mode`); without one, the file is written here.
fn update_mode(mode: update::Mode) -> u8 {
    let Some(dir) = paths::config_dir() else {
        eprintln!("error: no configuration directory (APPDATA unset)");
        return exit::ERRORS;
    };
    let delivery = match client::set_update_mode(mode, false) {
        Ok(delivery) => delivery,
        Err(err) => {
            eprintln!("error: {err}");
            return exit::ERRORS;
        }
    };
    info!("update: mode {}", mode.name());
    println!("update mode: {}", mode.name());
    if let Some(note) = delivery_note(&delivery) {
        println!("note: {note}");
    }
    if update::current_mode(Some(&dir)).1 {
        println!("note: {} is set and takes precedence", update::MODE_ENV);
    }
    exit::OK
}

/// `automation [on|off]` (U4): shows or sets automatic switching. Set: the
/// running tray takes it over at once (pipe `set_automation`) and writes
/// `triggers_paused`; without one, the file is written here. Shown: what
/// the running tray does, else what `settings.json` says.
fn automation(switch: Option<OnOff>) -> u8 {
    let (paused, note) = match switch {
        Some(switch) => {
            let paused = matches!(switch, OnOff::Off);
            match client::set_automation(paused, false) {
                Ok(delivery) => (paused, delivery_note(&delivery)),
                Err(err) => {
                    eprintln!("error: {err}");
                    return exit::ERRORS;
                }
            }
        }
        None => {
            let from_tray = Endpoints::current()
                .ok()
                .and_then(|e| client::request(&e, protocol::Command::Status, || {}).ok())
                .flatten()
                .and_then(|body| match body {
                    Body::Status(status) => status.automation.map(|a| a.paused),
                    _ => None,
                });
            let paused = from_tray.unwrap_or_else(|| {
                paths::config_dir().is_some_and(|dir| crate::settings::load(&dir).0.triggers_paused)
            });
            (paused, None)
        }
    };
    println!(
        "automatic switching: {}",
        if paused { "paused" } else { "on" }
    );
    if let Some(note) = note {
        println!("note: {note}");
    }
    if triggers_configured() == Some(false) {
        println!(
            "note: no profile has triggers yet; automatic switching does nothing until one has"
        );
    }
    exit::OK
}

/// `update --check [--quiet]` (UP1): checks now, also in mode `off`.
fn update_check(quiet: bool) -> u8 {
    let source = update::Source::current();
    let release = match update::check(&update::WinHttp, &source, paths::data_dir().as_deref()) {
        Ok(Some(release)) => release,
        Ok(None) => {
            info!("update: no release published yet");
            if !quiet {
                println!(
                    "no release is published for this channel yet; kaltstellen {} is the newest version",
                    update::CURRENT
                );
            }
            return exit::OK;
        }
        Err(err) => {
            warn!("update: {err}");
            if !quiet {
                eprintln!("error: {err}");
            }
            return exit::ERRORS;
        }
    };
    info!("update: newest release {}", release.version);
    if quiet {
        return exit::OK;
    }
    if update::version::newer(update::CURRENT, &release.version) {
        println!(
            "version {} is available (installed: {}): {}",
            release.version,
            update::CURRENT,
            release.page
        );
        let blocked = update::may_install(update::RELEASE_BUILD, &source)
            .map_err(|e| e.reason)
            .and_then(|()| {
                let exe = update::running_exe().map_err(|e| e.reason)?;
                update::placement(&exe, &update::program_files())
            });
        match blocked {
            Ok(()) => println!("install it with: kaltstellen update"),
            Err(how) => println!("this copy cannot update itself: {how}"),
        }
    } else {
        println!("kaltstellen {} is the newest version", update::CURRENT);
    }
    exit::OK
}

/// `update [--force]` (UP1): installs the newest release. `unattended` is
/// the tray's automatic install (see [`update::session::System`]).
fn update_install(force: bool, unattended: bool) -> u8 {
    let source = update::Source::current();
    let refused = |err: Error| {
        warn!("update: {err}");
        eprintln!("error: {err}");
        exit::ERRORS
    };
    if let Err(err) = update::may_install(update::RELEASE_BUILD, &source) {
        return refused(err);
    }
    let exe = match update::running_exe() {
        Ok(exe) => exe,
        Err(err) => return refused(err),
    };
    if let Err(how) = update::placement(&exe, &update::program_files()) {
        return refused(Error::new(
            Code::UpdateNotHere,
            format!("this copy cannot update itself: {how}"),
        ));
    }
    let mut session = match update::session::System::current(unattended) {
        Ok(session) => session,
        Err(err) => return refused(err),
    };
    let _exclusive = match session.exclusive() {
        Ok(lock) => lock,
        Err(err) => return refused(err),
    };
    let release = match update::check(&update::WinHttp, &source, paths::data_dir().as_deref()) {
        Ok(Some(release)) => release,
        // `--force` asks for a reinstall; without a release there is none.
        Ok(None) if force => {
            return refused(Error::new(
                Code::UpdateRelease,
                "no release is published for this channel yet; nothing to install",
            ));
        }
        Ok(None) => {
            println!(
                "no release is published for this channel yet; kaltstellen {} is the newest version",
                update::CURRENT
            );
            return exit::OK;
        }
        Err(err) => return refused(err),
    };
    if !force && !update::version::newer(update::CURRENT, &release.version) {
        println!("kaltstellen {} is the newest version", update::CURRENT);
        return exit::OK;
    }
    let data_dir = paths::data_dir();
    if unattended
        && let Some(dir) = &data_dir
        && update::record::load(dir)
            .0
            .held_back(&release.version, update::record::now())
    {
        return refused(Error::new(
            Code::UpdateStart,
            format!(
                "version {} could not be installed here recently; it is tried again later, or now on request",
                release.version
            ),
        ));
    }
    info!(
        "update: installing {} over {} ({})",
        release.version,
        update::CURRENT,
        exe.display()
    );
    let result = update::install::install(
        &update::WinHttp,
        &release,
        std::env::consts::ARCH,
        &exe,
        &mut session,
        &mut |line| {
            info!("update: {line}");
            println!("{line}");
        },
    );
    match result {
        Ok(done) => {
            info!("update: installed {}", done.version);
            println!("updated to version {}", done.version);
            // Installed by the setup: Windows' list of apps shows the new
            // version too.
            if let Some(dir) = exe.parent() {
                let key = update::uninstall_entry::key();
                match update::uninstall_entry::follow_update(&key, dir, &done.version) {
                    Ok(true) => info!("update: uninstall entry now says {}", done.version),
                    Ok(false) => {}
                    Err(err) => warn!("update: {err}"),
                }
            }
            if done.tray_restarted {
                println!(
                    "the tray runs the new version; its menu offers the profile that was active"
                );
            }
            if let Some(profile) = done.recovered_profile {
                println!(
                    "profile \"{profile}\" was switched off for the update; apply it again with: kaltstellen switch {profile}"
                );
            }
            exit::OK
        }
        // A version that did not start was recorded by the session (UP1).
        Err(err) => refused(err),
    }
}

/// The console the tray opened for an update: keep a failure readable.
fn finish_console(code: u8) {
    if code == exit::OK {
        std::thread::sleep(std::time::Duration::from_secs(3));
    } else {
        println!("press Enter to close this window");
        let _ = std::io::stdin().read_line(&mut String::new());
    }
}

fn status() -> u8 {
    if let Some(code) = through_tray(protocol::Command::Status) {
        return code;
    }
    let store = match store() {
        Ok(store) => store,
        Err(code) => return code,
    };
    print_status(&load_state(&store));
    exit::OK
}

/// N1: the running programs without an entry in the active profile.
fn status_unassigned() -> u8 {
    let Some(data) = paths::data_dir() else {
        return exit::ERRORS;
    };
    let active = Store::new(&data)
        .peek()
        .map_or_else(|| config::NEUTRAL.to_owned(), |s| s.active_profile);
    let config = match config::for_switch(paths::config_file().as_deref(), &active) {
        Ok((config, _)) => config,
        Err(lines) => {
            for line in lines {
                eprintln!("{line}");
            }
            return exit::CONFIG_INVALID;
        }
    };
    let list = crate::unassigned::refresh(&data, &config, &active);
    // M2, N1: what each program is, from the file of its running process,
    // else the path the icon registry gave.
    let wanted: Vec<crate::gui::icons::Wanted> = list
        .iter()
        .map(|u| crate::gui::icons::Wanted {
            exe: u.exe.clone(),
            path: u.path.as_ref().map(PathBuf::from),
        })
        .collect();
    let running = Snapshot::take()
        .map(|s| crate::gui::icons::running_paths(&s, &wanted))
        .unwrap_or_default();
    let files: Vec<Option<PathBuf>> = wanted
        .iter()
        .map(|w| crate::gui::icons::file_of(w, &running))
        .collect();
    let about = program_info::read_many(files.iter().flatten().map(PathBuf::as_path));
    println!("unassigned in \"{active}\": {}", list.len());
    for (item, file) in list.into_iter().zip(&files) {
        let text = file
            .as_deref()
            .and_then(|f| program_info::summary_of(&about, f));
        println!(
            "  {}{}{}{}",
            item.exe,
            if item.new { " (new)" } else { "" },
            item.path.map_or(String::new(), |p| format!(", {p}")),
            after_name(text)
        );
    }
    exit::OK
}

/// M2: the description and publisher after a program in `list --info` and
/// `status --unassigned`, two spaces after what was there before, so the
/// existing columns stay as they were; nothing when nothing is known.
fn after_name(text: Option<String>) -> String {
    text.map_or_else(String::new, |t| format!("  {t}"))
}

/// The image path of a process, if it can be opened with query rights.
fn image_of(p: &crate::process::ProcessInfo) -> Option<PathBuf> {
    if p.pid == 0 {
        return None;
    }
    match Target::open(p, Access::Query) {
        Ok(Some(target)) => target.image_path().ok(),
        _ => None,
    }
}

/// One frozen process for `status`: its own executable name, and the entry
/// it was frozen for when that is another program (a child process, A7).
fn frozen_line(entry: &str, image: &str, pid: u32, gone: bool) -> String {
    let name = image.rsplit(['\\', '/']).next().filter(|n| !n.is_empty());
    // The same rule the matcher uses (A7), so the entry's own process
    // never shows as part of itself.
    let entry_exe = crate::process::with_extension(entry);
    let mut line = match name {
        Some(name) if !name.eq_ignore_ascii_case(&entry_exe) => {
            format!("{name} (pid {pid}, part of {entry})")
        }
        _ => format!("{entry} (pid {pid})"),
    };
    if gone {
        line.push_str(", no longer running");
    }
    line
}

fn print_status(state: &State) {
    let snapshot = Snapshot::take().ok();
    println!("active profile: {}", state.active_profile);
    println!("frozen: {}", state.suspended.len());
    for record in &state.suspended {
        let gone = snapshot
            .as_ref()
            .is_some_and(|s| s.find(record.pid, record.created).is_none());
        println!(
            "  {}",
            frozen_line(&record.exe, &record.image, record.pid, gone)
        );
    }
    println!("stopped: {}", state.stopped.len());
    for record in &state.stopped {
        println!(
            "  {}{}",
            record.exe,
            if record.restart_on_neutral {
                ", restarts on neutral"
            } else {
                ""
            }
        );
    }
}

/// Prints problems as `file:line: message`, the form editors can jump to.
/// `kaltstellen compare A B` (H6): prints the differences; exit code 0 also
/// when the profiles differ, 4 for an unknown profile, 2 for an invalid
/// configuration.
fn compare(a: &str, b: &str) -> u8 {
    let Some(file) = paths::config_file() else {
        eprintln!("error: no configuration directory (APPDATA unset)");
        return exit::CONFIG_INVALID;
    };
    let loaded = match config::load(&file) {
        Ok(loaded) => loaded,
        Err(rejected) => {
            for line in config::rejected_lines(&file, &rejected) {
                eprintln!("{line}");
            }
            return exit::CONFIG_INVALID;
        }
    };
    let find = |name: &str| loaded.config.profile(name);
    let (Some(first), Some(second)) = (find(a), find(b)) else {
        for name in [a, b] {
            if find(name).is_none() {
                eprintln!(
                    "error: {}: no profile named \"{name}\"",
                    Code::UnknownProfile
                );
            }
        }
        return exit::UNKNOWN_PROFILE;
    };
    for line in crate::compare::compare(first, second).lines(&first.name, &second.name) {
        println!("{line}");
    }
    exit::OK
}

fn check_config(file: Option<PathBuf>) -> u8 {
    let Some(file) = file.or_else(paths::config_file) else {
        eprintln!("error: no configuration directory (APPDATA unset); name a file");
        return exit::CONFIG_INVALID;
    };
    let location = |issue: &config::Issue| match issue.line {
        Some(line) => format!("{}:{line}", file.display()),
        None => file.display().to_string(),
    };
    match config::load(&file) {
        Ok(loaded) => {
            for warning in &loaded.warnings {
                eprintln!("{}: warning: {}", location(warning), warning.message);
            }
            for profile in &loaded.config.profiles {
                for app in &profile.apps {
                    let touches = !matches!(
                        app.action,
                        config::AppAction::Ignore | config::AppAction::Start { .. }
                    );
                    if let (true, Protection::Warn(category)) =
                        (touches && !app.confirmed, protection::of(&app.exe))
                    {
                        eprintln!(
                            "{}: warning: profile \"{}\": {} is a {}; it is skipped unless its entry has confirmed = true (or --force)",
                            file.display(),
                            profile.name,
                            app.exe,
                            category.description()
                        );
                    }
                }
            }
            println!("{}: ok", file.display());
            for profile in &loaded.config.profiles {
                let hotkey = profile
                    .hotkey
                    .map_or(String::new(), |h| format!(", hotkey {h}"));
                // U4: shown only where set, so files without triggers
                // print as before.
                let triggers = if profile.triggers.is_empty() {
                    String::new()
                } else {
                    let names: Vec<String> = profile.triggers.iter().map(|t| t.text()).collect();
                    format!(
                        ", trigger {} (priority {})",
                        names.join(", "),
                        profile.priority
                    )
                };
                println!(
                    "  profile \"{}\": {} program(s){hotkey}{triggers}",
                    profile.name,
                    profile.apps.len()
                );
            }
            0
        }
        Err(rejected) => {
            print_rejected(&file, &rejected);
            exit::CONFIG_INVALID
        }
    }
}

/// Logging is best effort: kaltstellen keeps working without a log file.
fn init_logging() {
    let Some(dir) = paths::data_dir() else {
        eprintln!("warning: no data directory (LOCALAPPDATA unset); logging disabled");
        return;
    };
    if let Err(err) = logging::init(&dir, LevelFilter::Info) {
        eprintln!(
            "warning: {}: cannot open log in {}: {err}",
            crate::error::Code::LogSetup,
            dir.display()
        );
    }
}

/// Returns `Ok(false)` if the command ran but did not fully succeed.
fn run(command: Command) -> Result<bool> {
    match command {
        Command::List { filter, info } => {
            let filter = filter.map(|f| f.to_lowercase());
            let snapshot = Snapshot::take()?;
            let mut processes: Vec<_> = snapshot
                .processes()
                .iter()
                .filter(|p| {
                    filter
                        .as_ref()
                        .is_none_or(|f| p.name.to_lowercase().contains(f))
                })
                .cloned()
                .collect();
            processes.sort_by_key(|p| (p.name.to_lowercase(), p.pid));
            // M2: with `--info`, what each program is, after its name, where
            // its file can be read (processes of other users usually cannot
            // be opened). Without it nothing is opened or read, so the list
            // stays fast and as it was `[assumed]`.
            let images: Vec<Option<PathBuf>> = if info {
                processes.iter().map(image_of).collect()
            } else {
                vec![None; processes.len()]
            };
            let about = if info {
                program_info::read_many(images.iter().flatten().map(PathBuf::as_path))
            } else {
                std::collections::HashMap::new()
            };
            println!("{:>8} {:>8} {:>7}  NAME", "PID", "PARENT", "SESSION");
            for (p, image) in processes.iter().zip(&images) {
                let text = image
                    .as_deref()
                    .and_then(|i| program_info::summary_of(&about, i));
                println!(
                    "{:>8} {:>8} {:>7}  {}{}",
                    p.pid,
                    p.parent_pid,
                    p.session_id,
                    p.name,
                    after_name(text)
                );
            }
            Ok(true)
        }
        Command::Suspend { name, trim } => for_each(&name, "suspended", |target| {
            target.suspend()?;
            if trim {
                target.trim()?;
            }
            Ok(())
        }),
        Command::Resume { name } => for_each(&name, "resumed", Target::resume),
        Command::CheckConfig { .. }
        | Command::Switch { .. }
        | Command::Compare { .. }
        | Command::Status { .. }
        | Command::Recover { .. }
        | Command::Quit
        | Command::Tray
        | Command::Gui { .. }
        | Command::Autostart { .. }
        | Command::Automation { .. }
        | Command::Update { .. } => unreachable!("handled in main"),
    }
}

/// Applies `action` to every process of the program `name`: the processes
/// with that executable name in this session and their children (A7).
fn for_each(
    name: &str,
    done: &str,
    action: impl Fn(&Target) -> crate::error::Result<()>,
) -> Result<bool> {
    if protection::is_hard_protected(name) {
        return Err(Error::new(
            Code::HardProtected,
            "protected system program; it is never touched",
        )
        .program(name)
        .into());
    }
    let processes = Snapshot::take()?.program(name);
    let mut touched = 0;
    let mut all_ok = true;
    for p in &processes {
        let result = match Target::open(p, Access::Suspend) {
            // Exited since the snapshot: not an error (A7).
            Ok(None) => continue,
            Ok(Some(target)) => action(&target),
            Err(err) => Err(err),
        };
        touched += 1;
        match result {
            Ok(()) => {
                info!("{done} {} (pid {})", p.name, p.pid);
                println!("{done} {} (pid {})", p.name, p.pid);
            }
            Err(err) => {
                all_ok = false;
                warn!("failed pid {}: {err}", p.pid);
                eprintln!("failed pid {}: {err}", p.pid);
            }
        }
    }
    if touched == 0 {
        eprintln!("no running process named {name}");
        return Ok(false);
    }
    Ok(all_ok)
}

#[cfg(test)]
mod tests {
    use super::frozen_line;

    #[test]
    fn status_names_the_frozen_process_and_its_entry() {
        assert_eq!(
            frozen_line("app.exe", r"C:\A\App.exe", 7, false),
            "app.exe (pid 7)"
        );
        assert_eq!(frozen_line("app", r"C:\A\app.exe", 7, false), "app (pid 7)");
        // A child frozen with the program (A7) shows its own name.
        assert_eq!(
            frozen_line("host.exe", r"C:\A\helper.exe", 8, true),
            "helper.exe (pid 8, part of host.exe), no longer running"
        );
        // Entries with another extension or spaces match like the matcher.
        assert_eq!(
            frozen_line("saver.scr", r"C:\A\saver.scr", 5, false),
            "saver.scr (pid 5)"
        );
        assert_eq!(
            frozen_line(" app ", r"C:\A\app.exe", 6, false),
            " app  (pid 6)"
        );
        // Records written before the image was known.
        assert_eq!(frozen_line("app.exe", "", 9, false), "app.exe (pid 9)");
    }
}
