//! Command-line interface (U2). Exit codes follow spec E2.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::Result;
use clap::{Parser, Subcommand};
use log::{LevelFilter, info, warn};

use crate::config::{self, Config};
use crate::engine::{self, Report};
use crate::error::{Code, Error};
use crate::logging;
use crate::paths;
use crate::process::{Access, Snapshot, Target};
use crate::protection::{self, Protection};
use crate::state::{Lock, State, Store};

/// Put background apps on ice: suspend, resume and trim Windows processes.
#[derive(Parser)]
#[command(version, about)]
struct Cli {
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
    },
    /// Show the active profile and what kaltstellen has put on ice
    Status,
    /// Resume everything recorded as frozen, e.g. after a crash
    Recover,
    /// Run the notification-area icon with the profile menu
    Tray,
}

/// E2 exit codes.
const EXIT_ERRORS: u8 = 1;
const EXIT_CONFIG_INVALID: u8 = 2;
const EXIT_UNKNOWN_PROFILE: u8 = 4;
const EXIT_LOCKED: u8 = 5;

pub fn main() -> ExitCode {
    let cli = Cli::parse();
    init_logging();
    let code = match cli.command {
        Command::CheckConfig { file } => check_config(file),
        Command::Switch { profile, force } => switch(&profile, force),
        Command::Status => status(),
        Command::Recover => recover(),
        Command::Tray => crate::tray::main(),
        command => match run(command) {
            Ok(true) => 0,
            Ok(false) => EXIT_ERRORS,
            Err(err) => {
                eprintln!("error: {err:#}");
                EXIT_ERRORS
            }
        },
    };
    ExitCode::from(code)
}

fn store() -> Result<Store, u8> {
    paths::data_dir().map(Store::new).ok_or_else(|| {
        eprintln!("error: no data directory (LOCALAPPDATA unset)");
        EXIT_ERRORS
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
                EXIT_LOCKED
            } else {
                EXIT_ERRORS
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
    for issue in &rejected.issues {
        let line = issue.line.map_or(String::new(), |l| format!(":{l}"));
        eprintln!(
            "{}{line}: error {}: {}",
            file.display(),
            rejected.code,
            issue.message
        );
    }
}

/// The configuration for a switch. `neutral` needs none: bringing everything
/// back must work even when the file is missing.
fn config_for(target: &str) -> Result<Config, u8> {
    let neutral = target.trim().eq_ignore_ascii_case(config::NEUTRAL);
    let empty = Config {
        settings: config::Settings::default(),
        profiles: Vec::new(),
    };
    let Some(file) = paths::config_file() else {
        eprintln!("error: no configuration directory (APPDATA unset)");
        return if neutral {
            Ok(empty)
        } else {
            Err(EXIT_CONFIG_INVALID)
        };
    };
    match config::load(&file) {
        Ok(loaded) => {
            for warning in &loaded.warnings {
                eprintln!("{}: warning: {warning}", file.display());
            }
            Ok(loaded.config)
        }
        Err(rejected) if neutral && rejected.code == Code::ConfigRead => Ok(empty),
        Err(rejected) => {
            print_rejected(&file, &rejected);
            if neutral {
                eprintln!("warning: switching to neutral without the configuration");
                Ok(empty)
            } else {
                Err(EXIT_CONFIG_INVALID)
            }
        }
    }
}

fn print_report(report: &Report) -> u8 {
    for event in &report.events {
        match event.pid {
            Some(pid) => println!("{} {} (pid {pid})", event.step, event.program),
            None => println!("{} {}", event.step, event.program),
        }
    }
    for error in &report.errors {
        eprintln!("error: {error}");
    }
    if report.errors.is_empty() {
        0
    } else {
        EXIT_ERRORS
    }
}

fn switch(target: &str, force: bool) -> u8 {
    let (store, _lock) = match locked_store() {
        Ok(locked) => locked,
        Err(code) => return code,
    };
    let config = match config_for(target) {
        Ok(config) => config,
        Err(code) => return code,
    };
    let mut state = load_state(&store);
    let options = engine::Options { force };
    match engine::switch(&config, target, &mut state, options, &mut |s| store.save(s)) {
        Ok(report) => {
            let code = print_report(&report);
            println!("active profile: {}", state.active_profile);
            code
        }
        Err(err) if err.code == Code::UnknownProfile => {
            eprintln!("error: {err}");
            let names: Vec<&str> = config.profiles.iter().map(|p| p.name.as_str()).collect();
            eprintln!(
                "profiles: neutral{}{}",
                if names.is_empty() { "" } else { ", " },
                names.join(", ")
            );
            EXIT_UNKNOWN_PROFILE
        }
        Err(err) => {
            eprintln!("error: {err}");
            EXIT_ERRORS
        }
    }
}

fn recover() -> u8 {
    let (store, _lock) = match locked_store() {
        Ok(locked) => locked,
        Err(code) => return code,
    };
    let mut state = load_state(&store);
    match engine::recover(&mut state, &mut |s| store.save(s)) {
        Ok(report) => {
            let code = print_report(&report);
            println!("active profile: {}", state.active_profile);
            code
        }
        Err(err) => {
            eprintln!("error: {err}");
            EXIT_ERRORS
        }
    }
}

fn status() -> u8 {
    let store = match store() {
        Ok(store) => store,
        Err(code) => return code,
    };
    let state = load_state(&store);
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
    0
}

/// Prints problems as `file:line: message`, the form editors can jump to.
fn check_config(file: Option<PathBuf>) -> u8 {
    let Some(file) = file.or_else(paths::config_file) else {
        eprintln!("error: no configuration directory (APPDATA unset); name a file");
        return EXIT_CONFIG_INVALID;
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
            EXIT_CONFIG_INVALID
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
        | Command::Status
        | Command::Recover
        | Command::Tray => unreachable!("handled in main"),
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
