//! Error type with stable codes (spec E1).
//!
//! Every error carries a stable [`Code`], optionally the program it concerns
//! and the switch step it happened in, plus a human-readable reason. Codes
//! are never renumbered or reused; retired codes stay reserved. The list is
//! mirrored in `docs/errors.md`.

use std::fmt;

/// Stable error codes. The numeric value is part of the public contract
/// (logs, scripts), so variants must never be renumbered.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u16)]
pub enum Code {
    // 1xx: process engine
    /// Taking the process snapshot failed.
    Snapshot = 101,
    /// The process runs at a higher integrity level (P7).
    NeedsAdmin = 102,
    /// The process is a protected process (PPL) and cannot be opened (P7).
    Protected = 103,
    /// The program is on the hard-protected list (P6, A6).
    HardProtected = 104,
    /// Suspending a process failed.
    SuspendFailed = 105,
    /// Resuming a process failed.
    ResumeFailed = 106,
    /// Trimming the working set failed.
    TrimFailed = 107,
    /// A program did not close within `stop_timeout_s` and `on_timeout = "keep"`.
    CloseTimeout = 108,
    /// Terminating a process failed.
    TerminateFailed = 109,
    /// Starting a program failed.
    StartFailed = 110,
    /// Starting a program failed because it requires elevation (P7).
    ElevationRequired = 111,
    /// Minimising the windows of a process failed (W7).
    MinimizeFailed = 112,
    /// A program on the warn list was selected without confirmation (P6).
    WarnListUnconfirmed = 113,
    /// Any other operating-system error while handling a process.
    ProcessOs = 199,

    // 2xx: configuration
    /// The configuration file could not be read.
    ConfigRead = 201,
    /// The configuration file is not valid TOML or has unknown keys (C3).
    ConfigSyntax = 202,
    /// The configuration file violates a rule C1 to C7.
    ConfigInvalid = 203,
    /// `settings.json` could not be read or written.
    SettingsIo = 204,

    // 3xx: state file
    /// The state file could not be read or written.
    StateIo = 301,
    /// The state file was corrupt and has been renamed (S9).
    StateCorrupt = 302,
    /// Another switch holds the state lock (E2 exit code 5).
    StateLocked = 303,

    // 4xx: switch engine
    /// The requested profile does not exist (E2 exit code 4).
    UnknownProfile = 401,
    /// The request was superseded by a later one (W3, E2 exit code 3).
    Superseded = 402,

    // 5xx: tray communication
    /// The tray instance could not be reached.
    TrayUnreachable = 501,

    // 9xx: internal
    /// Logging could not be set up.
    LogSetup = 901,
}

impl Code {
    /// All codes, for documentation and uniqueness checks.
    pub const ALL: &[Code] = &[
        Code::Snapshot,
        Code::NeedsAdmin,
        Code::Protected,
        Code::HardProtected,
        Code::SuspendFailed,
        Code::ResumeFailed,
        Code::TrimFailed,
        Code::CloseTimeout,
        Code::TerminateFailed,
        Code::StartFailed,
        Code::ElevationRequired,
        Code::MinimizeFailed,
        Code::WarnListUnconfirmed,
        Code::ProcessOs,
        Code::ConfigRead,
        Code::ConfigSyntax,
        Code::ConfigInvalid,
        Code::SettingsIo,
        Code::StateIo,
        Code::StateCorrupt,
        Code::StateLocked,
        Code::UnknownProfile,
        Code::Superseded,
        Code::TrayUnreachable,
        Code::LogSetup,
    ];

    pub fn number(self) -> u16 {
        self as u16
    }
}

impl fmt::Display for Code {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "KS{:03}", self.number())
    }
}

/// The step of a switch an error belongs to (W1 order plus auxiliary steps).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    Resume,
    Stop,
    Suspend,
    Start,
    Minimize,
    Trim,
    Recover,
}

impl fmt::Display for Step {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Step::Resume => "resume",
            Step::Stop => "stop",
            Step::Suspend => "suspend",
            Step::Start => "start",
            Step::Minimize => "minimize",
            Step::Trim => "trim",
            Step::Recover => "recover",
        })
    }
}

/// A kaltstellen error: stable code, optional program and step, reason.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Error {
    pub code: Code,
    pub program: Option<String>,
    pub step: Option<Step>,
    pub reason: String,
}

pub type Result<T, E = Error> = std::result::Result<T, E>;

impl Error {
    pub fn new(code: Code, reason: impl Into<String>) -> Self {
        Self {
            code,
            program: None,
            step: None,
            reason: reason.into(),
        }
    }

    pub fn program(mut self, program: impl Into<String>) -> Self {
        self.program = Some(program.into());
        self
    }

    pub fn step(mut self, step: Step) -> Self {
        self.step = Some(step);
        self
    }
}

impl fmt::Display for Error {
    /// `KS105 [suspend] browser.exe: access denied`
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.code)?;
        if let Some(step) = self.step {
            write!(f, " [{step}]")?;
        }
        if let Some(program) = &self.program {
            write!(f, " {program}")?;
        }
        write!(f, ": {}", self.reason)
    }
}

impl std::error::Error for Error {}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn codes_are_unique() {
        let numbers: HashSet<u16> = Code::ALL.iter().map(|c| c.number()).collect();
        assert_eq!(numbers.len(), Code::ALL.len());
    }

    #[test]
    fn display_contains_all_parts() {
        let err = Error::new(Code::SuspendFailed, "access denied")
            .program("browser.exe")
            .step(Step::Suspend);
        assert_eq!(
            err.to_string(),
            "KS105 [suspend] browser.exe: access denied"
        );
    }

    #[test]
    fn display_without_optional_parts() {
        let err = Error::new(Code::UnknownProfile, "no profile named \"x\"");
        assert_eq!(err.to_string(), "KS401: no profile named \"x\"");
    }

    #[test]
    fn docs_list_every_code() {
        let doc = include_str!("../docs/errors.md");
        for code in Code::ALL {
            assert!(
                doc.contains(&format!("`{code}`")),
                "{code} missing in docs/errors.md"
            );
        }
    }
}
