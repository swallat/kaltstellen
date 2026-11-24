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
    if !super::valid_sid(owner_sid) {
        return Err(Error::new(
            Code::HelperService,
            format!("{owner_sid:?} is not a SID"),
        ));
    }
    if !service::is_elevated() {
        return Err(Error::new(
            Code::HelperNeedsAdmin,
            "installing the helper needs an elevated process",
        ));
    }
    let program =
        program_dir().ok_or_else(|| Error::new(Code::HelperService, "ProgramFiles is not set"))?;
    let data =
        data_dir().ok_or_else(|| Error::new(Code::HelperService, "ProgramData is not set"))?;
    let source = std::env::current_exe()
        .map_err(|e| Error::new(Code::HelperService, format!("own path: {e}")))?;
    let target = program.join(EXE_NAME);

    fs::create_dir_all(&program).map_err(|e| io_error("cannot create", &program, e))?;
    fs::create_dir_all(&data).map_err(|e| io_error("cannot create", &data, e))?;
    service::protect_data_dir(&data, owner_sid)?;
    write_policy(owner_sid)?;

    // A running service holds its file: stop it first, copy, start it again.
    service::stop(SERVICE_NAME)?;
    copy_exe(&source, &target)?;
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

/// Writes `helper.toml`: a new one, or the existing one with the owner
/// updated (roles and allow lists the administrator set stay).
fn write_policy(owner_sid: &str) -> Result<()> {
    let path = policy_file().ok_or_else(|| Error::new(Code::HelperService, "no data folder"))?;
    let policy = match Policy::read(&path) {
        Ok(mut existing) => {
            existing.owner_sid = owner_sid.to_owned();
            existing
        }
        Err(_) => Policy::initial(owner_sid),
    };
    fs::write(&path, policy.to_toml()).map_err(|e| io_error("cannot write", &path, e))
}

/// Puts `source` at `target` through a temporary file, so a failure leaves
/// the old copy. Nothing to do when they are the same file.
fn copy_exe(source: &Path, target: &Path) -> Result<()> {
    if let (Ok(a), Ok(b)) = (fs::canonicalize(source), fs::canonicalize(target))
        && a == b
    {
        return Ok(());
    }
    let temporary = target.with_extension("exe.new");
    let _ = fs::remove_file(&temporary);
    fs::copy(source, &temporary).map_err(|e| io_error("cannot copy to", &temporary, e))?;
    let wanted = fs::metadata(source)
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
    let _ = fs::remove_file(target);
    fs::rename(&temporary, target).map_err(|e| io_error("cannot put in place", target, e))
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
        match fs::remove_file(file) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => left_over.push(format!("{}: {e}", file.display())),
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

/// What `kaltstellen helper status` reports.
#[derive(Debug)]
pub struct Report {
    pub service: Option<service::Info>,
    pub installed_exe: Option<PathBuf>,
    pub policy: std::result::Result<Policy, String>,
}

pub fn report() -> Result<Report> {
    let exe = installed_exe().filter(|path| path.exists());
    let policy = match policy_file() {
        Some(path) => Policy::read(&path).map_err(|e| e.reason),
        None => Err("ProgramData is not set".to_owned()),
    };
    Ok(Report {
        service: service::query(SERVICE_NAME)?,
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
    match &report.service {
        None => lines
            .push("service: not installed (portable use; everything works without it)".to_owned()),
        Some(info) => {
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
                "policy: roles {} (not all are implemented yet)",
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
        copy_exe(&source, &target).unwrap();
        assert_eq!(fs::read(&target).unwrap(), b"new version");
        assert!(!dir.path().join("kaltstellen.exe.new").exists());
        // The same file is left alone.
        copy_exe(&target, &target).unwrap();
        assert_eq!(fs::read(&target).unwrap(), b"new version");
    }

    #[test]
    fn a_portable_copy_is_described_without_a_service() {
        let report = Report {
            service: None,
            installed_exe: None,
            policy: Err("no file".into()),
        };
        let lines = describe(&report, &Ok(None));
        assert_eq!(lines.len(), 1);
        assert!(lines[0].contains("not installed"));
    }
}
