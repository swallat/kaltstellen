//! Installing a release (spec UP1 "Install steps"): download, verify,
//! stage next to the running executable, make sure nothing stays frozen,
//! swap by renaming, start the new tray, roll back if it does not start.
//!
//! The file operations work on the paths of [`Files`] only; everything that
//! touches processes (the tray, the version probe) goes through
//! [`Session`], so the tests run the whole sequence in a temporary folder
//! with a fake session.

use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use super::release::{self, CHECKSUMS, Release};
use super::{BINARY_LIMIT, CHECKSUMS_LIMIT, Fetch};
use crate::error::{Code, Error, Result};
use crate::win32::hash;

/// What the install needs from the system around it.
pub trait Session {
    /// Whether a tray runs in this session (same data directory).
    fn tray_running(&self) -> bool;
    /// Keeps the active profile as the offer for the next tray, asks the
    /// tray to quit (L2: everything suspended is resumed) and waits until
    /// it has ended. `KS808` if it does not.
    fn stop_tray(&mut self) -> Result<()>;
    /// Without a tray: resumes everything recorded as suspended (L3).
    /// Returns the profile that was active, if it was not `neutral`.
    fn recover(&mut self) -> Result<Option<String>>;
    /// Starts `exe tray` and waits until it answers on its pipe. A tray that
    /// does not answer in time is ended before this returns an error.
    fn start_tray(&mut self, exe: &Path) -> std::result::Result<(), String>;
    /// The first line `exe --version` prints.
    fn version_of(&self, exe: &Path) -> std::result::Result<String, Probe>;
    /// Installing `version` failed after it was downloaded and verified: it
    /// did not start here (KS806), or it could not be put in place
    /// (KS805). It is never installed automatically again. Called before
    /// the previous tray starts again, so that tray already knows.
    fn install_failed(&mut self, version: &str);
}

/// Why the `--version` probe of the staged file gave no answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Probe {
    /// It could not be started or ended without the expected output.
    Failed(String),
    /// It did not end in time.
    TimedOut(String),
}

/// The files of one install, all next to the running executable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Files {
    pub exe: PathBuf,
    /// The verified download, before the swap.
    pub new: PathBuf,
    /// The previous executable after the swap; kept as a way back.
    pub old: PathBuf,
    /// A new executable that did not start, kept for a bug report.
    pub failed: PathBuf,
    /// Created and removed to see whether the folder is writable.
    pub probe: PathBuf,
}

impl Files {
    pub fn for_exe(exe: &Path) -> Files {
        let stem = exe.file_stem().map_or_else(
            || "kaltstellen".to_owned(),
            |s| s.to_string_lossy().into_owned(),
        );
        let sibling = |suffix: &str| exe.with_file_name(format!("{stem}.{suffix}"));
        Files {
            exe: exe.to_path_buf(),
            new: sibling("new.exe"),
            old: sibling("old.exe"),
            failed: sibling("failed.exe"),
            probe: sibling("update-probe"),
        }
    }

    /// `<stem>.old-<n>.exe`: an older `.old.exe` that could not be removed
    /// because a process still runs from it.
    fn aside(&self, n: u64) -> PathBuf {
        let name = self
            .old
            .file_name()
            .map(|n| n.to_string_lossy().into_owned());
        let name = name.unwrap_or_default();
        let stem = name.strip_suffix(".old.exe").unwrap_or(&name);
        self.exe.with_file_name(format!("{stem}.old-{n}.exe"))
    }
}

/// What an install did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Installed {
    pub version: String,
    /// A tray was quit for the update and the new one answers.
    pub tray_restarted: bool,
    /// Without a tray: the profile that `recover` switched off.
    pub recovered_profile: Option<String>,
}

fn remove_if_present(path: &Path) -> std::io::Result<()> {
    match fs::remove_file(path) {
        Err(err) if err.kind() != ErrorKind::NotFound => Err(err),
        _ => Ok(()),
    }
}

/// Removes what an earlier update left behind; best effort.
fn clean_up(files: &Files) {
    let _ = remove_if_present(&files.new);
    let _ = remove_if_present(&files.failed);
    let _ = remove_if_present(&files.probe);
    let Some(dir) = files.exe.parent() else {
        return;
    };
    let Some(prefix) = files.aside(0).file_name().map(|n| {
        let n = n.to_string_lossy();
        n.strip_suffix("0.exe").unwrap_or(&n).to_lowercase()
    }) else {
        return;
    };
    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_lowercase();
            if name.starts_with(&prefix) && name.ends_with(".exe") {
                let _ = fs::remove_file(entry.path());
            }
        }
    }
}

/// UP1 "Where": the folder must be writable.
fn writable(files: &Files) -> Result<()> {
    let probe = fs::write(&files.probe, b"kaltstellen update probe")
        .and_then(|()| fs::remove_file(&files.probe));
    probe.map_err(|e| {
        Error::new(
            Code::UpdateNotHere,
            format!(
                "cannot write next to {}: {e}; replace the file by hand",
                files.exe.display()
            ),
        )
    })
}

/// Downloads the executable of `release` for `arch` and checks it against
/// `checksums.txt` of the same release.
pub fn download(fetch: &dyn Fetch, release: &Release, arch: &str) -> Result<Vec<u8>> {
    let name = release::asset_name(&release.version, arch);
    let unusable = |why: String| Error::new(Code::UpdateRelease, why);
    let url = release.assets.get(&name).ok_or_else(|| {
        unusable(format!(
            "release {} has no {name} for this system",
            release.version
        ))
    })?;
    let sums_url = release.assets.get(CHECKSUMS).ok_or_else(|| {
        unusable(format!(
            "release {} has no {CHECKSUMS}; an unchecked file is not installed",
            release.version
        ))
    })?;
    let sums = fetch
        .get(sums_url, "application/octet-stream", CHECKSUMS_LIMIT)
        .map_err(|e| Error::new(Code::UpdateCheck, e))?;
    let want = release::checksum_of(&String::from_utf8_lossy(&sums), &name).map_err(unusable)?;
    let binary = fetch
        .get(url, "application/octet-stream", BINARY_LIMIT)
        .map_err(|e| Error::new(Code::UpdateCheck, e))?;
    let got = hash::hex(&hash::sha256(&binary).map_err(|e| Error::new(Code::UpdateChecksum, e))?);
    if got != want {
        return Err(Error::new(
            Code::UpdateChecksum,
            format!(
                "{name}: SHA-256 {got} does not match {want} in {CHECKSUMS}; nothing was changed"
            ),
        ));
    }
    Ok(binary)
}

/// Installs `release` over `exe`. `say` gets one line per step for the
/// user. On an error nothing has changed, or the old executable is back
/// (and its tray started again when one was running).
pub fn install(
    fetch: &dyn Fetch,
    release: &Release,
    arch: &str,
    exe: &Path,
    session: &mut dyn Session,
    say: &mut dyn FnMut(String),
) -> Result<Installed> {
    let files = Files::for_exe(exe);
    clean_up(&files);
    writable(&files)?;

    say(format!("downloading version {} …", release.version));
    let binary = download(fetch, release, arch)?;
    say("checksum ok".to_owned());

    // Stage and see that it starts at all, before anything else changes.
    fs::write(&files.new, &binary).map_err(|e| {
        Error::new(
            Code::UpdateReplace,
            format!("cannot write {}: {e}", files.new.display()),
        )
    })?;
    let expected = format!("kaltstellen {}", release.version);
    match session.version_of(&files.new) {
        // Build metadata (`+<commit>`, which development builds append) is
        // not part of the version (semver) and is ignored here.
        Ok(line) if line.trim().split('+').next() == Some(expected.as_str()) => {}
        outcome => {
            let _ = fs::remove_file(&files.new);
            let timed_out = matches!(outcome, Err(Probe::TimedOut(_)));
            let what = match outcome {
                Ok(line) => format!("it reports \"{}\" instead of \"{expected}\"", line.trim()),
                Err(Probe::Failed(err) | Probe::TimedOut(err)) => err,
            };
            // A probe that only took too long (a virus scan of the new
            // file) says nothing about the version; it may be tried again.
            if !timed_out {
                session.install_failed(&release.version);
            }
            return Err(Error::new(
                Code::UpdateStart,
                format!("the new version does not start: {what}; nothing was changed"),
            ));
        }
    }

    // Nothing may stay frozen across the update.
    let tray = session.tray_running();
    let mut recovered_profile = None;
    let stopped = if tray {
        say("quitting the tray; frozen programs are resumed".to_owned());
        session.stop_tray()
    } else {
        session.recover().map(|profile| recovered_profile = profile)
    };
    if let Err(err) = stopped {
        let _ = fs::remove_file(&files.new);
        return Err(err);
    }

    if let Err(err) = swap(&files) {
        if tray {
            // The tray was quit already: without the record, the tray that
            // starts again would try the same install at once, and again.
            // Without a tray there is no such loop, and a file that was
            // only locked for a moment may be swapped next time.
            session.install_failed(&release.version);
            restart_old(session, &files.exe, say);
        }
        return Err(err);
    }
    say(format!("replaced {}", files.exe.display()));

    if tray {
        say("starting the new tray …".to_owned());
        if let Err(why) = session.start_tray(&files.exe) {
            roll_back(&files);
            session.install_failed(&release.version);
            restart_old(session, &files.exe, say);
            return Err(Error::new(
                Code::UpdateStart,
                format!(
                    "the new tray did not start ({why}); the previous version is back, the new one is kept as {}",
                    files.failed.display()
                ),
            ));
        }
    }
    Ok(Installed {
        version: release.version.clone(),
        tray_restarted: tray,
        recovered_profile,
    })
}

/// Renames `exe` to `old` and `new` to `exe`; undoes the first rename if
/// the second fails.
fn swap(files: &Files) -> Result<()> {
    let fail = |what: String| Error::new(Code::UpdateReplace, what);
    if remove_if_present(&files.old).is_err() {
        // A process (an open main window) still runs from it: renaming a
        // running executable works, removing it does not.
        let aside = files.aside(super::record::now());
        fs::rename(&files.old, &aside).map_err(|e| {
            let _ = fs::remove_file(&files.new);
            fail(format!("cannot move {} aside: {e}", files.old.display()))
        })?;
    }
    if let Err(err) = fs::rename(&files.exe, &files.old) {
        let _ = fs::remove_file(&files.new);
        return Err(fail(format!(
            "cannot rename {}: {err}; nothing was changed",
            files.exe.display()
        )));
    }
    if let Err(err) = fs::rename(&files.new, &files.exe) {
        let back = fs::rename(&files.old, &files.exe);
        let _ = fs::remove_file(&files.new);
        return Err(fail(match back {
            Ok(()) => {
                format!("cannot put the new version in place: {err}; the previous one is back")
            }
            Err(back) => format!(
                "cannot put the new version in place: {err}; and the previous one could not be renamed back from {}: {back}",
                files.old.display()
            ),
        }));
    }
    Ok(())
}

/// After the new tray failed: the new file becomes `failed`, `old` gets the
/// original name back.
fn roll_back(files: &Files) {
    let _ = remove_if_present(&files.failed);
    if fs::rename(&files.exe, &files.failed).is_err() {
        let _ = fs::remove_file(&files.exe);
    }
    let _ = fs::rename(&files.old, &files.exe);
}

fn restart_old(session: &mut dyn Session, exe: &Path, say: &mut dyn FnMut(String)) {
    match session.start_tray(exe) {
        Ok(()) => say("the previous tray runs again".to_owned()),
        Err(why) => say(format!(
            "the previous tray did not start again ({why}); start it with: kaltstellen tray"
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    use crate::update::tests::Fake;

    const OLD: &[u8] = b"old executable";
    const NEW: &[u8] = b"new executable";

    /// Records what the install asked for; the version probe reads the
    /// staged file and answers like the real executable would.
    #[derive(Default)]
    struct Recorder {
        tray: bool,
        refuse_quit: bool,
        /// Start attempts that fail, from the first.
        failing_starts: usize,
        calls: Vec<String>,
        /// What the staged file says to `--version`, by content.
        versions: HashMap<Vec<u8>, String>,
        /// The probe does not answer in time.
        slow_probe: bool,
    }

    impl Session for Recorder {
        fn tray_running(&self) -> bool {
            self.tray
        }
        fn stop_tray(&mut self) -> Result<()> {
            self.calls.push("stop".into());
            if self.refuse_quit {
                return Err(Error::new(Code::UpdateTray, "refused"));
            }
            self.tray = false;
            Ok(())
        }
        fn recover(&mut self) -> Result<Option<String>> {
            self.calls.push("recover".into());
            Ok(Some("play".into()))
        }
        fn start_tray(&mut self, exe: &Path) -> std::result::Result<(), String> {
            let content = fs::read(exe).unwrap_or_default();
            self.calls
                .push(format!("start {}", String::from_utf8_lossy(&content)));
            if self.failing_starts > 0 {
                self.failing_starts -= 1;
                return Err("ended at once".into());
            }
            self.tray = true;
            Ok(())
        }
        fn version_of(&self, exe: &Path) -> std::result::Result<String, Probe> {
            if self.slow_probe {
                return Err(Probe::TimedOut("no answer within 10 s".into()));
            }
            let content = fs::read(exe).map_err(|e| Probe::Failed(e.to_string()))?;
            self.versions
                .get(&content)
                .cloned()
                .ok_or_else(|| Probe::Failed("not a program".into()))
        }
        fn install_failed(&mut self, version: &str) {
            self.calls.push(format!("failed {version}"));
        }
    }

    fn recorder(tray: bool) -> Recorder {
        Recorder {
            tray,
            versions: HashMap::from([(NEW.to_vec(), "kaltstellen 2.0.0\r\n".to_owned())]),
            ..Recorder::default()
        }
    }

    struct World {
        dir: tempfile::TempDir,
        exe: PathBuf,
        release: Release,
        fake: Fake,
    }

    fn sum(data: &[u8]) -> String {
        hash::hex(&hash::sha256(data).unwrap())
    }

    /// A folder with `k.exe` and a release 2.0.0 serving `served` with the
    /// sum of `summed`.
    fn world(served: &[u8], summed: &[u8]) -> World {
        let dir = tempfile::tempdir().unwrap();
        let exe = dir.path().join("k.exe");
        fs::write(&exe, OLD).unwrap();
        let name = release::asset_name("2.0.0", "x86_64");
        let release = Release {
            version: "2.0.0".into(),
            page: "https://page".into(),
            assets: [
                (name.clone(), "https://dl/exe".to_owned()),
                (CHECKSUMS.to_owned(), "https://dl/sums".to_owned()),
            ]
            .into(),
        };
        let fake = Fake(HashMap::from([
            ("https://dl/exe".to_owned(), Ok(served.to_vec())),
            (
                "https://dl/sums".to_owned(),
                Ok(format!("{}  {name}\n", sum(summed)).into_bytes()),
            ),
        ]));
        World {
            dir,
            exe,
            release,
            fake,
        }
    }

    fn names(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    fn run(w: &World, session: &mut Recorder) -> Result<Installed> {
        let mut lines = Vec::new();
        install(&w.fake, &w.release, "x86_64", &w.exe, session, &mut |l| {
            lines.push(l)
        })
    }

    #[test]
    fn up1_with_a_tray_quits_swaps_and_restarts() {
        let w = world(NEW, NEW);
        let mut session = recorder(true);
        let done = run(&w, &mut session).unwrap();
        assert!(done.tray_restarted);
        assert_eq!(fs::read(&w.exe).unwrap(), NEW);
        assert_eq!(fs::read(w.dir.path().join("k.old.exe")).unwrap(), OLD);
        assert_eq!(session.calls, ["stop", "start new executable"]);
        assert_eq!(names(w.dir.path()), ["k.exe", "k.old.exe"]);
    }

    #[test]
    fn up1_without_a_tray_recovers_first() {
        let w = world(NEW, NEW);
        let mut session = recorder(false);
        let done = run(&w, &mut session).unwrap();
        assert_eq!(done.recovered_profile.as_deref(), Some("play"));
        assert!(!done.tray_restarted);
        assert_eq!(session.calls, ["recover"]);
        assert_eq!(fs::read(&w.exe).unwrap(), NEW);
    }

    #[test]
    fn up1_wrong_checksum_changes_nothing() {
        let w = world(b"tampered", NEW);
        let mut session = recorder(true);
        let err = run(&w, &mut session).unwrap_err();
        assert_eq!(err.code, Code::UpdateChecksum);
        assert!(session.calls.is_empty(), "the tray was touched");
        assert_eq!(names(w.dir.path()), ["k.exe"]);
        assert_eq!(fs::read(&w.exe).unwrap(), OLD);
    }

    #[test]
    fn up1_missing_file_or_checksum_is_refused() {
        let mut w = world(NEW, NEW);
        w.release.assets.remove(CHECKSUMS);
        assert_eq!(
            run(&w, &mut recorder(true)).unwrap_err().code,
            Code::UpdateRelease
        );
        let w = world(NEW, NEW);
        let mut session = recorder(true);
        let err = install(
            &w.fake,
            &w.release,
            "aarch64",
            &w.exe,
            &mut session,
            &mut |_| {},
        )
        .unwrap_err();
        assert_eq!(err.code, Code::UpdateRelease);
        assert!(err.reason.contains("kaltstellen-2.0.0-windows-aarch64.exe"));
        assert_eq!(names(w.dir.path()), ["k.exe"]);
    }

    #[test]
    fn up1_a_new_file_that_does_not_start_changes_nothing() {
        let w = world(NEW, NEW);
        let mut session = recorder(true);
        session.versions.clear();
        assert_eq!(run(&w, &mut session).unwrap_err().code, Code::UpdateStart);
        // Another version than the release claims: refused as well.
        session
            .versions
            .insert(NEW.to_vec(), "kaltstellen 1.9.0".into());
        assert_eq!(run(&w, &mut session).unwrap_err().code, Code::UpdateStart);
        // The tray was not touched; the version is remembered as failed.
        assert_eq!(session.calls, ["failed 2.0.0", "failed 2.0.0"]);
        assert_eq!(names(w.dir.path()), ["k.exe"]);
        // A probe that only took too long is not held against the version.
        session.calls.clear();
        session.slow_probe = true;
        assert_eq!(run(&w, &mut session).unwrap_err().code, Code::UpdateStart);
        assert!(session.calls.is_empty(), "{:?}", session.calls);
        assert_eq!(names(w.dir.path()), ["k.exe"]);
    }

    #[test]
    fn up1_a_failed_swap_after_the_tray_quit_is_remembered() {
        use std::os::windows::fs::OpenOptionsExt;
        let w = world(NEW, NEW);
        let mut session = recorder(true);
        // Another program holds the executable without FILE_SHARE_DELETE,
        // so it cannot be renamed.
        let _held = fs::OpenOptions::new()
            .read(true)
            .share_mode(1) // FILE_SHARE_READ
            .open(&w.exe)
            .unwrap();
        let err = run(&w, &mut session).unwrap_err();
        assert_eq!(err.code, Code::UpdateReplace, "{err}");
        // Recorded before the old tray starts again, so it does not retry.
        assert_eq!(
            session.calls,
            ["stop", "failed 2.0.0", "start old executable"]
        );
        assert_eq!(fs::read(&w.exe).unwrap(), OLD);
    }

    #[test]
    fn up1_a_tray_that_does_not_quit_stops_the_update() {
        let w = world(NEW, NEW);
        let mut session = recorder(true);
        session.refuse_quit = true;
        assert_eq!(run(&w, &mut session).unwrap_err().code, Code::UpdateTray);
        assert_eq!(names(w.dir.path()), ["k.exe"]);
        assert_eq!(fs::read(&w.exe).unwrap(), OLD);
    }

    #[test]
    fn up1_new_tray_fails_rolls_back_and_restarts_the_old_one() {
        let w = world(NEW, NEW);
        let mut session = recorder(true);
        session.failing_starts = 1;
        let err = run(&w, &mut session).unwrap_err();
        assert_eq!(err.code, Code::UpdateStart);
        assert_eq!(fs::read(&w.exe).unwrap(), OLD);
        assert_eq!(fs::read(w.dir.path().join("k.failed.exe")).unwrap(), NEW);
        assert_eq!(
            session.calls,
            [
                "stop",
                "start new executable",
                // Before the old tray runs, so it does not try again.
                "failed 2.0.0",
                "start old executable"
            ]
        );
        assert!(session.tray, "the old tray runs again");
        // The next attempt cleans the failed file up first.
        session.failing_starts = 0;
        session.tray = true;
        run(&w, &mut session).unwrap();
        assert_eq!(names(w.dir.path()), ["k.exe", "k.old.exe"]);
    }

    #[test]
    fn up1_second_update_replaces_the_kept_old_file() {
        let w = world(NEW, NEW);
        fs::write(w.dir.path().join("k.old.exe"), b"older").unwrap();
        fs::write(w.dir.path().join("k.old-17.exe"), b"oldest").unwrap();
        fs::write(w.dir.path().join("k.new.exe"), b"leftover").unwrap();
        run(&w, &mut recorder(false)).unwrap();
        assert_eq!(names(w.dir.path()), ["k.exe", "k.old.exe"]);
        assert_eq!(fs::read(w.dir.path().join("k.old.exe")).unwrap(), OLD);
    }

    #[test]
    fn up1_files_sit_next_to_the_executable() {
        let files = Files::for_exe(Path::new(r"C:\t\kaltstellen.exe"));
        assert_eq!(files.new, PathBuf::from(r"C:\t\kaltstellen.new.exe"));
        assert_eq!(files.old, PathBuf::from(r"C:\t\kaltstellen.old.exe"));
        assert_eq!(files.failed, PathBuf::from(r"C:\t\kaltstellen.failed.exe"));
        assert_eq!(files.aside(5), PathBuf::from(r"C:\t\kaltstellen.old-5.exe"));
    }
}
