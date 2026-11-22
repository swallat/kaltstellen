//! Command-line interface (U2). Exit codes follow spec E2.

use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::Result;
use clap::{Parser, Subcommand};
use log::{LevelFilter, info, warn};

use crate::config;
use crate::error::{Code, Error};
use crate::logging;
use crate::paths;
use crate::process::{Access, Snapshot, Target};
use crate::protection;

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
}

/// E2 exit codes.
const EXIT_ERRORS: u8 = 1;
const EXIT_CONFIG_INVALID: u8 = 2;

pub fn main() -> ExitCode {
    let cli = Cli::parse();
    init_logging();
    if let Command::CheckConfig { file } = cli.command {
        return ExitCode::from(check_config(file));
    }
    match run(cli) {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::from(EXIT_ERRORS),
        Err(err) => {
            eprintln!("error: {err:#}");
            ExitCode::from(EXIT_ERRORS)
        }
    }
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
            warn!("{}: {rejected}", file.display());
            for issue in &rejected.issues {
                eprintln!(
                    "{}: error {}: {}",
                    location(issue),
                    rejected.code,
                    issue.message
                );
            }
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
fn run(cli: Cli) -> Result<bool> {
    match cli.command {
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
        Command::CheckConfig { .. } => unreachable!("handled in main"),
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
