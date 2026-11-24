//! Installing, removing and inspecting the helper service (M4a). The
//! install and uninstall steps run elevated; the unelevated command line
//! starts them through a UAC prompt (see [`crate::cli`]).

use std::fs;
use std::path::{Path, PathBuf};

use log::info;

use super::policy::Policy;
use super::{EXE_NAME, PIPE_NAME, SERVICE_NAME, data_dir, installed_exe, policy_file, program_dir};
use crate::error::{Code, Error, Result};
use crate::win32::service::{self, State};

/// The only privilege the helper keeps in M4a (the right to be told of
/// changes in folders it opens). Later milestones add what the event
/// sessions and opening the owner's programs need, and nothing else.
const PRIVILEGES: &[&str] = &["SeChangeNotifyPrivilege"];

fn io_error(what: &str, path: &Path, err: std::io::Error) -> Error {
    Error::new(
        Code::HelperService,
        format!("{what} {}: {err}", path.display()),
    )
}

/// The command line the service control manager runs: the protected copy,
/// quoted, with the hidden `helper run`.
pub fn command_line(exe: &Path) -> String {
    format!("\"{}\" helper run", exe.display())
}

/// Installs or updates the helper for the user `owner_sid`: protected copy,
/// data folder with its access list, policy file, service. Needs an
/// elevated process.
pub fn install(owner_sid: &str) -> Result<()> {
    if !super::valid_owner(owner_sid) {
        return Err(Error::new(
            Code::HelperService,
            format!("{owner_sid:?} is not the SID of one user account"),
        ));
    }
    if !service::is_elevated() {
        return Err(Error::new(
            Code::HelperNeedsAdmin,
            "installing the helper needs an elevated process",
        ));
    }
    let program = program_dir()
        .ok_or_else(|| Error::new(Code::HelperService, "ProgramFiles is not known"))?;
    let data =
        data_dir().ok_or_else(|| Error::new(Code::HelperService, "ProgramData is not known"))?;
    // The source is where the running image is now, not the path it was
    // started from, and it is opened at once and held against writers: a
    // file put at that path later is not what is copied.
    let source = service::running_image()?;
    let input = open_source(&source)?;
    let target = program.join(EXE_NAME);

    // Any user can create a folder in ProgramData first and would keep the
    // right to change its access list. Use only folders that SYSTEM or the
    // administrators own.
    for dir in [&program, &data] {
        if dir.exists() && !service::folder_is_trusted(dir)? {
            return Err(Error::new(
                Code::HelperService,
                format!(
                    "{} exists but is not owned by SYSTEM or the administrators (or is a link); remove it as an administrator and install again",
                    dir.display()
                ),
            ));
        }
    }
    fs::create_dir_all(&program).map_err(|e| io_error("cannot create", &program, e))?;
    fs::create_dir_all(&data).map_err(|e| io_error("cannot create", &data, e))?;
    // At once, before anything is put in it: the default access list of
    // ProgramData lets users add files to a new folder.
    service::protect_data_dir(&data, owner_sid)?;
    remove_foreign_files(&data)?;

    // A running service holds its file: stop it first, copy, then set the
    // rest up and start it again.
    service::stop(SERVICE_NAME)?;
    copy_exe(input, &source, &target)?;
    write_policy(owner_sid)?;
    service::install(
        SERVICE_NAME,
        "kaltstellen background helper",
        "Optional helper of kaltstellen: watches program starts and network use and acts on elevated programs, only as the administrator allowed.",
        &command_line(&target),
        PRIVILEGES,
    )?;
    info!("helper installed from {} for {owner_sid}", source.display());
    Ok(())
}

/// Removes files in the data folder that a standard user created or that
/// belong to someone else than SYSTEM or the administrators: a user owns
/// what they created whatever its access list says.
fn remove_foreign_files(data: &Path) -> Result<()> {
    let entries = fs::read_dir(data).map_err(|e| io_error("cannot list", data, e))?;
    for entry in entries.flatten() {
        let path = entry.path();
        if !service::owner_is_trusted(&path).unwrap_or(false) {
            fs::remove_file(&path)
                .or_else(|_| fs::remove_dir_all(&path))
                .map_err(|e| {
                    io_error(
                        "cannot remove a file not owned by an administrator:",
                        &path,
                        e,
                    )
                })?;
        }
    }
    Ok(())
}

/// Writes `helper.toml`: a new one, or the existing one with the owner
/// updated (roles and allow lists the administrator set stay; files of
/// other owners were removed before). A file that exists but cannot be read
/// is not replaced silently: the administrator removes it on purpose.
fn write_policy(owner_sid: &str) -> Result<()> {
    let path = policy_file().ok_or_else(|| Error::new(Code::HelperService, "no data folder"))?;
    let policy = if path.exists() {
        let mut existing = Policy::read(&path).map_err(|e| {
            Error::new(
                Code::HelperPolicy,
                format!("{e}; fix or delete the file and install again"),
            )
        })?;
        existing.owner_sid = owner_sid.to_owned();
        existing
    } else {
        Policy::initial(owner_sid)
    };
    fs::write(&path, policy.to_toml()).map_err(|e| io_error("cannot write", &path, e))
}

fn open_source(source: &Path) -> Result<fs::File> {
    use std::os::windows::fs::OpenOptionsExt;
    const FILE_SHARE_READ: u32 = 1;
    fs::OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ)
        .open(source)
        .map_err(|e| io_error("cannot open", source, e))
}

/// Puts the opened `input` (the file at `source`) at `target` through a
/// temporary file in the same folder, so a failure leaves the old copy, and
/// moves it over the old copy in one step. Only the length of what was
/// written is compared with the source. Nothing to do when source and
/// target are the same file.
fn copy_exe(mut input: fs::File, source: &Path, target: &Path) -> Result<()> {
    use std::io::{Read, Write};
    if let (Ok(a), Ok(b)) = (fs::canonicalize(source), fs::canonicalize(target))
        && a == b
    {
        return Ok(());
    }
    let temporary = target.with_extension("exe.new");
    let _ = fs::remove_file(&temporary);
    let mut write = || -> std::io::Result<()> {
        let mut output = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)?;
        let mut buffer = vec![0u8; 256 * 1024];
        loop {
            let read = input.read(&mut buffer)?;
            if read == 0 {
                break;
            }
            output.write_all(&buffer[..read])?;
        }
        output.sync_all()
    };
    if let Err(e) = write() {
        let _ = fs::remove_file(&temporary);
        return Err(io_error("cannot copy to", &temporary, e));
    }
    let wanted = input
        .metadata()
        .map_err(|e| io_error("cannot read", source, e))?
        .len();
    let got = fs::metadata(&temporary)
        .map_err(|e| io_error("cannot read", &temporary, e))?
        .len();
    if wanted != got {
        let _ = fs::remove_file(&temporary);
        return Err(Error::new(
            Code::HelperService,
            format!("the copy has {got} bytes, expected {wanted}"),
        ));
    }
    fs::rename(&temporary, target).map_err(|e| {
        let _ = fs::remove_file(&temporary);
        io_error("cannot put in place", target, e)
    })
}

/// Stops and removes the service and the files the install made. Needs an
/// elevated process. Returns false if there was nothing to remove.
pub fn uninstall() -> Result<bool> {
    if !service::is_elevated() {
        return Err(Error::new(
            Code::HelperNeedsAdmin,
            "removing the helper needs an elevated process",
        ));
    }
    let removed = service::remove(SERVICE_NAME)?;
    let mut files: Vec<PathBuf> = Vec::new();
    if let Some(exe) = installed_exe() {
        files.push(exe.with_extension("exe.new"));
        files.push(exe);
    }
    if let Some(data) = data_dir() {
        files.push(data.join("helper.toml"));
        for name in ["kaltstellen.log", "kaltstellen.log.1", "kaltstellen.log.2"] {
            files.push(data.join(name));
        }
    }
    let mut left_over = Vec::new();
    for file in &files {
        if let Err(e) = remove_file_waiting(file) {
            left_over.push(format!("{}: {e}", file.display()));
        }
    }
    // Only empty folders go; anything else in them is not ours.
    for dir in [program_dir(), data_dir()].into_iter().flatten() {
        let _ = fs::remove_dir(dir);
    }
    if !left_over.is_empty() {
        return Err(Error::new(
            Code::HelperService,
            format!(
                "service removed, but not all files: {}",
                left_over.join("; ")
            ),
        ));
    }
    info!("helper removed");
    Ok(removed)
}

/// Removes `path`; a file that is not there is fine. The service process
/// can still hold its executable for a moment after the service reports
/// stopped, so a file in use is tried again for up to 15 s.
fn remove_file_waiting(path: &Path) -> std::io::Result<()> {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
    loop {
        match fs::remove_file(path) {
            Ok(()) => return Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(e) if std::time::Instant::now() >= deadline => return Err(e),
            Err(_) => std::thread::sleep(std::time::Duration::from_millis(250)),
        }
    }
}

/// What `kaltstellen helper status` reports.
#[derive(Debug)]
pub struct Report {
    pub service: Option<service::Info>,
    /// Why the service state could not be read (no access, for instance in
    /// a session without an interactive sign-in).
    pub service_error: Option<String>,
    pub installed_exe: Option<PathBuf>,
    pub policy: std::result::Result<Policy, String>,
}

pub fn report() -> Result<Report> {
    let exe = installed_exe().filter(|path| path.exists());
    let policy = match policy_file() {
        Some(path) => Policy::read(&path).map_err(|e| e.reason),
        None => Err("ProgramData is not set".to_owned()),
    };
    let (service, service_error) = match service::query(SERVICE_NAME) {
        Ok(info) => (info, None),
        Err(err) => (None, Some(err.to_string())),
    };
    Ok(Report {
        service,
        service_error,
        installed_exe: exe,
        policy,
    })
}

/// The lines of `helper status`.
pub fn describe(
    report: &Report,
    answer: &std::result::Result<Option<super::protocol::Reply>, Error>,
) -> Vec<String> {
    let mut lines = Vec::new();
    match (&report.service, &report.service_error) {
        (None, Some(reason)) => lines.push(format!("service: state unknown ({reason})")),
        (None, None) => lines
            .push("service: not installed (portable use; everything works without it)".to_owned()),
        (Some(info), _) => {
            let state = match info.state {
                State::Stopped => "stopped",
                State::Starting => "starting",
                State::Running => "running",
                State::Stopping => "stopping",
                State::Other => "in another state",
            };
            match info.pid {
                Some(pid) => lines.push(format!("service: {state} (process {pid})")),
                None => lines.push(format!("service: {state}")),
            }
        }
    }
    if let Some(exe) = &report.installed_exe {
        lines.push(format!("protected copy: {}", exe.display()));
    }
    match &report.policy {
        Ok(policy) => {
            let roles = &policy.roles;
            let on: Vec<&str> = [
                (roles.observe, "observe"),
                (roles.act, "act"),
                (roles.services, "services"),
            ]
            .iter()
            .filter(|(on, _)| *on)
            .map(|(_, name)| *name)
            .collect();
            lines.push(format!(
                "policy: roles {} (only observe is implemented)",
                if on.is_empty() {
                    "none".to_owned()
                } else {
                    on.join(", ")
                }
            ));
        }
        Err(reason) if report.service.is_some() => lines.push(format!("policy: {reason}")),
        Err(_) => {}
    }
    match answer {
        Ok(Some(reply)) => {
            let version = reply.version.as_deref().unwrap_or("?");
            lines.push(format!("pipe {PIPE_NAME}: answers, version {version}"));
            if let Some(network) = reply.network {
                lines.push(format!(
                    "network counters: {}",
                    if network { "running" } else { "not running" }
                ));
            }
            if let Some(active) = &reply.active {
                lines.push(format!(
                    "observe (process events): {}",
                    if active.observe {
                        "running"
                    } else {
                        "not running"
                    }
                ));
            }
            if let Some(note) = super::client::version_note(version) {
                lines.push(note);
            }
        }
        Ok(None) => {
            if report.service.is_some() {
                lines.push(format!(
                    "pipe {PIPE_NAME}: no answer (service not running?)"
                ));
            }
        }
        Err(err) => lines.push(format!("pipe {PIPE_NAME}: {err}")),
    }
    lines
}

/// What the window's settings show about the helper: the same facts as
/// `helper status`, as data.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Summary {
    /// `not_installed`, `stopped`, `starting`, `running`, `stopping`,
    /// `other` or `unknown` (the service state could not be read).
    pub state: String,
    /// Whether the pipe answered as the helper.
    pub answers: bool,
    /// Version of the running helper.
    pub version: Option<String>,
    /// Roles switched on in the policy file.
    pub roles: Vec<String>,
    /// Anything the page should say besides the state: why the state is
    /// unknown, an outdated helper, a pipe that is not the helper.
    pub notes: Vec<String>,
}

/// [`Summary`] of a [`Report`] and the answer of the pipe.
pub fn summarize(
    report: &Report,
    answer: &std::result::Result<Option<super::protocol::Reply>, Error>,
) -> Summary {
    let mut notes = Vec::new();
    let state = match (&report.service, &report.service_error) {
        (None, Some(reason)) => {
            notes.push(format!("the service state cannot be read: {reason}"));
            "unknown"
        }
        (None, None) => "not_installed",
        (Some(info), _) => match info.state {
            State::Stopped => "stopped",
            State::Starting => "starting",
            State::Running => "running",
            State::Stopping => "stopping",
            State::Other => "other",
        },
    };
    let roles = match &report.policy {
        Ok(policy) => [
            (policy.roles.observe, "observe"),
            (policy.roles.act, "act"),
            (policy.roles.services, "services"),
        ]
        .iter()
        .filter(|(on, _)| *on)
        .map(|(_, name)| (*name).to_owned())
        .collect(),
        Err(reason) => {
            if report.service.is_some() {
                notes.push(format!("policy: {reason}"));
            }
            Vec::new()
        }
    };
    let (answers, version) = match answer {
        Ok(Some(reply)) => {
            let version = reply.version.clone();
            if let Some(note) = super::client::version_note(version.as_deref().unwrap_or("?")) {
                notes.push(note);
            }
            (true, version)
        }
        Ok(None) => (false, None),
        Err(err) => {
            notes.push(err.to_string());
            (false, None)
        }
    };
    Summary {
        state: state.to_owned(),
        answers,
        version,
        roles,
        notes,
    }
}

/// The current [`Summary`]: reads the service state and asks the pipe.
pub fn summary() -> Summary {
    match report() {
        Ok(report) => {
            let answer = super::client::ask(super::protocol::Command::Status);
            summarize(&report, &answer)
        }
        Err(err) => Summary {
            state: "unknown".to_owned(),
            notes: vec![err.to_string()],
            ..Summary::default()
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_service_runs_the_quoted_protected_copy() {
        let line = command_line(Path::new(r"C:\Program Files\kaltstellen\kaltstellen.exe"));
        assert_eq!(
            line,
            r#""C:\Program Files\kaltstellen\kaltstellen.exe" helper run"#
        );
    }

    #[test]
    fn a_copy_replaces_the_old_file_and_leaves_no_temporary() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("source.exe");
        let target = dir.path().join("kaltstellen.exe");
        fs::write(&source, b"new version").unwrap();
        fs::write(&target, b"old").unwrap();
        copy_exe(open_source(&source).unwrap(), &source, &target).unwrap();
        assert_eq!(fs::read(&target).unwrap(), b"new version");
        assert!(!dir.path().join("kaltstellen.exe.new").exists());
        // The same file is left alone.
        copy_exe(open_source(&target).unwrap(), &target, &target).unwrap();
        assert_eq!(fs::read(&target).unwrap(), b"new version");
    }

    #[test]
    fn a_missing_file_is_removed_without_waiting() {
        let dir = tempfile::tempdir().unwrap();
        let started = std::time::Instant::now();
        remove_file_waiting(&dir.path().join("none.exe")).unwrap();
        let file = dir.path().join("there.exe");
        fs::write(&file, b"x").unwrap();
        remove_file_waiting(&file).unwrap();
        assert!(!file.exists());
        assert!(started.elapsed() < std::time::Duration::from_secs(5));
    }

    #[test]
    fn a_portable_copy_is_described_without_a_service() {
        let report = Report {
            service: None,
            service_error: None,
            installed_exe: None,
            policy: Err("no file".into()),
        };
        let lines = describe(&report, &Ok(None));
        assert_eq!(lines.len(), 1);
        assert!(lines[0].contains("not installed"));
    }
}
