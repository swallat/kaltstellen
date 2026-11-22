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
    List { filter: Option<String> },
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
    /// Run the notification-area icon with the profile menu
    Tray,
    /// Open the main window: profiles, programs and their actions
    Gui {
        /// Capture the window as PNG into this file and exit (review)
        #[arg(long, hide = true)]
        screenshot: Option<PathBuf>,
        /// View to open before the capture: entry, add, confirm, new
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
    let cli = Cli::parse();
    init_logging();
    let code = match cli.command {
        Command::CheckConfig { file } => check_config(file),
        Command::Switch {
            profile,
            force,
            no_wait,
        } => switch(&profile, force, !no_wait),
        Command::Status { unassigned } => {
            let code = status();
            if unassigned && code == exit::OK {
                status_unassigned()
            } else {
                code
            }
        }
        Command::Recover { no_wait } => recover(!no_wait),
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
        Command::Update {
            check,
            force,
            mode,
            auto,
            quiet,
            from_tray,
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
                (None, false) => update_install(force),
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

/// `update --auto on|off` (UP1).
fn update_mode(mode: update::Mode) -> u8 {
    let Some(dir) = paths::config_dir() else {
        eprintln!("error: no configuration directory (APPDATA unset)");
        return exit::ERRORS;
    };
    if let Err(err) = crate::settings::edit(&dir, |s| s.update = Some(mode.name().to_owned())) {
        eprintln!("error: {err}");
        return exit::ERRORS;
    }
    info!("update: mode {}", mode.name());
    println!("update mode: {}", mode.name());
    if update::current_mode(Some(&dir)).1 {
        println!("note: {} is set and takes precedence", update::MODE_ENV);
    }
    exit::OK
}

/// `update --check [--quiet]` (UP1): checks now, also in mode `off`.
fn update_check(quiet: bool) -> u8 {
    let source = update::Source::current();
    let release = match update::check(&update::WinHttp, &source, paths::data_dir().as_deref()) {
        Ok(release) => release,
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

/// `update [--force]` (UP1): installs the newest release.
fn update_install(force: bool) -> u8 {
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
    let release = match update::check(&update::WinHttp, &source, paths::data_dir().as_deref()) {
        Ok(release) => release,
        Err(err) => return refused(err),
    };
    if !force && !update::version::newer(update::CURRENT, &release.version) {
        println!("kaltstellen {} is the newest version", update::CURRENT);
        return exit::OK;
    }
    let mut session = match update::session::System::current() {
        Ok(session) => session,
        Err(err) => return refused(err),
    };
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
    println!("unassigned in \"{active}\": {}", list.len());
    for item in list {
        println!(
            "  {}{}{}",
            item.exe,
            if item.new { " (new)" } else { "" },
            item.path.map_or(String::new(), |p| format!(", {p}"))
        );
    }
    exit::OK
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
            "  {} (pid {}){}",
            record.exe,
            record.pid,
            if gone { ", no longer running" } else { "" }
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
                println!(
                    "  profile \"{}\": {} program(s){hotkey}",
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
        Command::List { filter } => {
            let filter = filter.map(|f| f.to_lowercase());
            let snapshot = Snapshot::take()?;
            let mut processes = snapshot.processes().to_vec();
            processes.sort_by_key(|p| (p.name.to_lowercase(), p.pid));
            println!("{:>8} {:>8} {:>7}  NAME", "PID", "PARENT", "SESSION");
            for p in processes {
                if filter
                    .as_ref()
                    .is_none_or(|f| p.name.to_lowercase().contains(f))
                {
                    println!(
                        "{:>8} {:>8} {:>7}  {}",
                        p.pid, p.parent_pid, p.session_id, p.name
                    );
                }
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
        | Command::Status { .. }
        | Command::Recover { .. }
        | Command::Tray
        | Command::Gui { .. }
        | Command::Autostart { .. }
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
