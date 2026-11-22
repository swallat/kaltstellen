//! Command-line interface (U2). Exit codes follow spec E2.

use std::process::ExitCode;

use anyhow::Result;
use clap::{Parser, Subcommand};
use log::{LevelFilter, info, warn};

use crate::logging;
use crate::paths;
use crate::process::{self, ProcessInfo};

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
}

pub fn main() -> ExitCode {
    let cli = Cli::parse();
    init_logging();
    match run(cli) {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::FAILURE,
        Err(err) => {
            eprintln!("error: {err:#}");
            ExitCode::FAILURE
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
            let mut processes = process::list()?;
            processes.sort_by_key(|p| (p.name.to_lowercase(), p.pid));
            println!("{:>8} {:>8}  NAME", "PID", "PARENT");
            for p in processes {
                if filter
                    .as_ref()
                    .is_none_or(|f| p.name.to_lowercase().contains(f))
                {
                    println!("{:>8} {:>8}  {}", p.pid, p.parent_pid, p.name);
                }
            }
            Ok(true)
        }
        Command::Suspend { name, trim } => for_each(&name, "suspended", |p| {
            process::suspend(p.pid)?;
            if trim {
                process::trim_working_set(p.pid)?;
            }
            Ok(())
        }),
        Command::Resume { name } => for_each(&name, "resumed", |p| process::resume(p.pid)),
    }
}

fn for_each(name: &str, done: &str, action: impl Fn(&ProcessInfo) -> Result<()>) -> Result<bool> {
    let processes = process::find(name)?;
    if processes.is_empty() {
        eprintln!("no running process named {name}");
        return Ok(false);
    }
    let mut all_ok = true;
    for p in &processes {
        match action(p) {
            Ok(()) => {
                info!("{done} {} (pid {})", p.name, p.pid);
                println!("{done} {} (pid {})", p.name, p.pid);
            }
            Err(err) => {
                all_ok = false;
                warn!("failed {} (pid {}): {err:#}", p.name, p.pid);
                eprintln!("failed {} (pid {}): {err:#}", p.name, p.pid);
            }
        }
    }
    Ok(all_ok)
}
