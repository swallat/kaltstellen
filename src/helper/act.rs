//! The role *act* (M4d): suspend, resume and end elevated programs of the
//! owner that the administrator put on the allow list (docs/design-optional-helper.md,
//! "act"). The tray remains the decision maker; this is only the hands for
//! what it cannot reach.
//!
//! Safe code only. What touches processes is behind [`Os`], so the rules
//! (who may be acted on, what is recorded, when frozen programs are
//! released) are tested without a process.
//!
//! The rules, in short:
//! - the role must be on and the program on the allow list (full image
//!   path), owned by the owner recorded at install, in the Windows session
//!   of the caller, not hard-protected, never the helper or the caller;
//! - one open handle does the checks and the action (the process is the same
//!   one: pid and creation time, S6);
//! - everything frozen is written to `helper-state.json` before the process
//!   is touched, resumed when the caller that asked is gone (checked every
//!   few seconds), at service stop and after a crash at the next start.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use log::{info, warn};
use serde::{Deserialize, Serialize};

use super::policy::Policy;
use crate::protection;

/// The most programs frozen at once.
pub const MAX_FROZEN: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verb {
    Suspend,
    Resume,
    Terminate,
}

/// What the helper sees of a process before it acts on it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Inspected {
    /// Full image path.
    pub path: String,
    /// File name of the image.
    pub exe: String,
    /// The SID of the user the process runs as.
    pub owner_sid: String,
    pub session: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Done {
    Done,
    /// The process is not there (any more) or its pid belongs to another
    /// process now.
    Gone,
}

/// The operating system side. One call opens the process once, lets
/// `check` look at it and then does `verb` on that same handle.
pub trait Os: Send + Sync + std::fmt::Debug {
    fn run(
        &self,
        pid: u32,
        created: u64,
        verb: Verb,
        check: &dyn Fn(&Inspected) -> std::result::Result<(), String>,
    ) -> std::result::Result<Done, String>;
    /// The creation time of a process (`FILETIME`), if it exists.
    fn created_of(&self, pid: u32) -> Option<u64>;
    /// Whether the process with this pid and creation time still runs;
    /// `None` if that cannot be told now (nothing is forgotten then).
    fn alive(&self, pid: u32, created: u64) -> Option<bool>;
}

/// The asker: the process that connected to the pipe.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Caller {
    pub pid: u32,
    pub session: u32,
}

/// A program the helper froze.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Frozen {
    pub pid: u32,
    pub created: u64,
    /// Image path (empty until the freeze went through).
    pub path: String,
    /// Who asked; the program is released when this process is gone.
    pub client_pid: u32,
    pub client_created: u64,
}

#[derive(Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct StateFile {
    version: u32,
    frozen: Vec<Frozen>,
}

#[derive(Debug)]
pub struct Actor {
    owner_sid: String,
    role: bool,
    allowed: Policy,
    os: Arc<dyn Os>,
    file: Option<PathBuf>,
    own_pid: u32,
    frozen: Mutex<Vec<Frozen>>,
    /// Set at service stop (under the lock): nothing is frozen afterwards.
    stopped: std::sync::atomic::AtomicBool,
}

impl Actor {
    pub fn new(policy: &Policy, os: Arc<dyn Os>, file: Option<PathBuf>, own_pid: u32) -> Actor {
        Actor {
            owner_sid: policy.owner_sid.clone(),
            role: policy.roles.act,
            allowed: policy.clone(),
            os,
            file,
            own_pid,
            frozen: Mutex::new(Vec::new()),
            stopped: std::sync::atomic::AtomicBool::new(false),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Vec<Frozen>> {
        self.frozen.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn frozen_count(&self) -> usize {
        self.lock().len()
    }

    /// Writes the record. A failure is logged; the helper then still
    /// releases everything at stop and by the sweep.
    fn save(&self, list: &[Frozen]) -> std::result::Result<(), String> {
        let Some(file) = &self.file else {
            return Ok(());
        };
        if list.is_empty() {
            return match std::fs::remove_file(file) {
                Ok(()) => Ok(()),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(e) => Err(format!("cannot remove {}: {e}", file.display())),
            };
        }
        let state = StateFile {
            version: 1,
            frozen: list.to_vec(),
        };
        let text = serde_json::to_string_pretty(&state).map_err(|e| e.to_string())?;
        let temporary = file.with_extension("json.tmp");
        std::fs::write(&temporary, text)
            .and_then(|()| std::fs::rename(&temporary, file))
            .map_err(|e| {
                let _ = std::fs::remove_file(&temporary);
                format!("cannot write {}: {e}", file.display())
            })
    }

    /// The checks every verb needs, run on the opened process.
    fn check(&self, caller: Caller, found: &Inspected) -> std::result::Result<(), String> {
        if found.owner_sid != self.owner_sid {
            return Err("the program does not belong to the owner".to_owned());
        }
        if found.session != caller.session {
            return Err("the program runs in another Windows session than the caller".to_owned());
        }
        if protection::is_hard_protected(&found.exe) {
            return Err(format!("{} is never touched", found.exe));
        }
        if !self.allowed.allows_program(&found.path) {
            return Err(format!("{} is not on the allow list", found.path));
        }
        Ok(())
    }

    /// Does `verb` on the program `pid` (started at `created`) for `caller`.
    pub fn act(
        &self,
        verb: Verb,
        pid: u32,
        created: u64,
        caller: Caller,
    ) -> std::result::Result<(), String> {
        if !self.role {
            return Err("the role act is off".to_owned());
        }
        if pid <= 4 || pid == self.own_pid || pid == caller.pid {
            return Err("this process is never acted on".to_owned());
        }
        let mut list = self.lock();
        let known = list
            .iter()
            .position(|f| f.pid == pid && f.created == created);
        let check = |found: &Inspected| self.check(caller, found);
        match verb {
            Verb::Suspend => {
                if self.stopped.load(std::sync::atomic::Ordering::SeqCst) {
                    return Err("the helper is stopping".to_owned());
                }
                if known.is_some() {
                    return Ok(());
                }
                if list.len() >= MAX_FROZEN {
                    return Err(format!("{MAX_FROZEN} programs are frozen already"));
                }
                // Written before the process is touched: a crash in between
                // leaves a record that the next start resumes (resuming a
                // process that is not frozen does nothing).
                let client_created = self.os.created_of(caller.pid).unwrap_or(0);
                list.push(Frozen {
                    pid,
                    created,
                    path: String::new(),
                    client_pid: caller.pid,
                    client_created,
                });
                if let Err(e) = self.save(&list) {
                    list.pop();
                    return Err(e);
                }
                let outcome = self.os.run(pid, created, Verb::Suspend, &check);
                match outcome {
                    Ok(Done::Done) => {
                        info!("act: suspended pid {pid} for pid {}", caller.pid);
                        Ok(())
                    }
                    Ok(Done::Gone) => {
                        list.retain(|f| !(f.pid == pid && f.created == created));
                        let _ = self.save(&list);
                        Ok(())
                    }
                    Err(e) => {
                        list.retain(|f| !(f.pid == pid && f.created == created));
                        let _ = self.save(&list);
                        warn!("act: suspend pid {pid} refused or failed: {e}");
                        Err(e)
                    }
                }
            }
            Verb::Resume => {
                if known.is_none() {
                    return Err("the helper did not freeze this program".to_owned());
                }
                let outcome = self.os.run(pid, created, Verb::Resume, &check);
                match outcome {
                    Ok(_) => {
                        list.retain(|f| !(f.pid == pid && f.created == created));
                        if let Err(e) = self.save(&list) {
                            warn!("act: {e}");
                        }
                        info!("act: resumed pid {pid}");
                        Ok(())
                    }
                    Err(e) => {
                        warn!("act: resume pid {pid} failed: {e}");
                        Err(e)
                    }
                }
            }
            Verb::Terminate => {
                let outcome = self.os.run(pid, created, Verb::Terminate, &check);
                match outcome {
                    Ok(_) => {
                        list.retain(|f| !(f.pid == pid && f.created == created));
                        let _ = self.save(&list);
                        info!("act: ended pid {pid} for pid {}", caller.pid);
                        Ok(())
                    }
                    Err(e) => {
                        warn!("act: end pid {pid} refused or failed: {e}");
                        Err(e)
                    }
                }
            }
        }
    }

    /// Releases what the helper froze for a caller that is gone, and
    /// forgets programs that ended. Called every few seconds.
    pub fn sweep(&self) {
        let mut list = self.lock();
        if list.is_empty() {
            return;
        }
        let before = list.len();
        let mut keep = Vec::with_capacity(list.len());
        for entry in list.drain(..) {
            // What cannot be told stays on the list and is looked at again.
            match (
                self.os.alive(entry.pid, entry.created),
                self.os.alive(entry.client_pid, entry.client_created),
            ) {
                (Some(false), _) => info!("act: pid {} ended while frozen", entry.pid),
                (Some(true), Some(false)) => {
                    if !self.release(&entry, "its caller is gone") {
                        keep.push(entry);
                    }
                }
                _ => keep.push(entry),
            }
        }
        *list = keep;
        if list.len() != before
            && let Err(e) = self.save(&list)
        {
            warn!("act: {e}");
        }
    }

    /// Resumes everything (service stop, end of the session) and refuses
    /// new freezes from then on. What cannot be resumed stays on the record
    /// for the next start.
    pub fn release_all(&self) {
        let mut list = self.lock();
        self.stopped
            .store(true, std::sync::atomic::Ordering::SeqCst);
        let all = std::mem::take(&mut *list);
        for entry in all {
            if !self.release(&entry, "the helper stops") {
                list.push(entry);
            }
        }
        if let Err(e) = self.save(&list) {
            warn!("act: {e}");
        }
    }

    /// At start: resumes what a crash or a kill left frozen. The record is
    /// removed only when everything on it was released.
    pub fn recover(&self) {
        let Some(file) = &self.file else { return };
        let Ok(text) = std::fs::read_to_string(file) else {
            return;
        };
        match serde_json::from_str::<StateFile>(&text) {
            Ok(state) if state.version == 1 => {
                let left: Vec<Frozen> = state
                    .frozen
                    .into_iter()
                    .filter(|entry| !self.release(entry, "left over from before"))
                    .collect();
                if let Err(e) = self.save(&left) {
                    warn!("act: {e}");
                }
                // What stays is tried again by the sweep.
                *self.lock() = left;
            }
            Ok(_) | Err(_) => {
                warn!("act: {} is not a record this helper knows", file.display());
                let _ = std::fs::remove_file(file);
            }
        }
    }

    /// Resumes one entry; `false` if that failed and it must be kept.
    fn release(&self, entry: &Frozen, why: &str) -> bool {
        let always = |_: &Inspected| Ok(());
        match self.os.run(entry.pid, entry.created, Verb::Resume, &always) {
            Ok(Done::Done) => {
                info!("act: resumed pid {} ({why})", entry.pid);
                true
            }
            Ok(Done::Gone) => true,
            Err(e) => {
                warn!("act: cannot resume pid {} ({why}): {e}", entry.pid);
                false
            }
        }
    }
}

/// Where the record lives.
pub fn state_file(data_dir: &Path) -> PathBuf {
    data_dir.join("helper-state.json")
}

/// The real processes.
#[derive(Debug, Default)]
pub struct RealOs;

impl Os for RealOs {
    fn run(
        &self,
        pid: u32,
        created: u64,
        verb: Verb,
        check: &dyn Fn(&Inspected) -> std::result::Result<(), String>,
    ) -> std::result::Result<Done, String> {
        use crate::process::{Access, Snapshot, Target};
        let snapshot = Snapshot::take().map_err(|e| e.reason)?;
        let Some(info) = snapshot.find(pid, created) else {
            return Ok(Done::Gone);
        };
        let Some(target) = Target::open(info, Access::Stop).map_err(|e| e.reason)? else {
            return Ok(Done::Gone);
        };
        let path = target
            .image_path()
            .map_err(|e| e.reason)?
            .to_string_lossy()
            .into_owned();
        let exe = path.rsplit('\\').next().unwrap_or_default().to_owned();
        let found = Inspected {
            path,
            exe,
            owner_sid: target.owner_sid().map_err(|e| e.reason)?,
            session: crate::win32::pipe::session_of(pid).unwrap_or(u32::MAX),
        };
        check(&found)?;
        match verb {
            Verb::Suspend => target.suspend(),
            Verb::Resume => target.resume(),
            Verb::Terminate => target.terminate(),
        }
        .map_err(|e| e.reason)?;
        Ok(Done::Done)
    }

    fn created_of(&self, pid: u32) -> Option<u64> {
        crate::process::Snapshot::take()
            .ok()?
            .get(pid)
            .map(|p| p.created)
    }

    fn alive(&self, pid: u32, created: u64) -> Option<bool> {
        crate::process::Snapshot::take()
            .ok()
            .map(|s| s.find(pid, created).is_some())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    const OWNER: &str = "S-1-5-21-1-2-3-1001";
    const TOOL: &str = r"C:\Tools\tool.exe";

    #[derive(Debug, Default)]
    struct FakeOs {
        /// (pid, created) -> inspected, and whether it is frozen.
        processes: Mutex<HashMap<(u32, u64), (Inspected, bool)>>,
        fail_suspend: Mutex<bool>,
    }

    impl FakeOs {
        fn add(&self, pid: u32, created: u64, path: &str, owner: &str, session: u32) {
            let exe = path.rsplit('\\').next().unwrap().to_owned();
            self.processes.lock().unwrap().insert(
                (pid, created),
                (
                    Inspected {
                        path: path.to_owned(),
                        exe,
                        owner_sid: owner.to_owned(),
                        session,
                    },
                    false,
                ),
            );
        }
        fn frozen(&self, pid: u32, created: u64) -> bool {
            self.processes
                .lock()
                .unwrap()
                .get(&(pid, created))
                .is_some_and(|p| p.1)
        }
        fn exit(&self, pid: u32, created: u64) {
            self.processes.lock().unwrap().remove(&(pid, created));
        }
    }

    impl Os for FakeOs {
        fn run(
            &self,
            pid: u32,
            created: u64,
            verb: Verb,
            check: &dyn Fn(&Inspected) -> std::result::Result<(), String>,
        ) -> std::result::Result<Done, String> {
            let mut all = self.processes.lock().unwrap();
            let Some(entry) = all.get_mut(&(pid, created)) else {
                return Ok(Done::Gone);
            };
            check(&entry.0)?;
            match verb {
                Verb::Suspend => {
                    if *self.fail_suspend.lock().unwrap() {
                        return Err("access denied".to_owned());
                    }
                    entry.1 = true;
                }
                Verb::Resume => entry.1 = false,
                Verb::Terminate => {
                    all.remove(&(pid, created));
                }
            }
            Ok(Done::Done)
        }
        fn created_of(&self, pid: u32) -> Option<u64> {
            self.processes
                .lock()
                .unwrap()
                .keys()
                .find(|(p, _)| *p == pid)
                .map(|(_, c)| *c)
        }
        fn alive(&self, pid: u32, created: u64) -> Option<bool> {
            Some(self.processes.lock().unwrap().contains_key(&(pid, created)))
        }
    }

    fn policy(act: bool, allowed: &[&str]) -> Policy {
        let mut p = Policy::initial(OWNER);
        p.roles.act = act;
        for path in allowed {
            p.allow_program(path).unwrap();
        }
        p
    }

    fn setup(act: bool, allowed: &[&str]) -> (Actor, Arc<FakeOs>) {
        let os = Arc::new(FakeOs::default());
        // The tray (pid 50) and the program (pid 100).
        os.add(50, 5, r"C:\Kalt\kaltstellen.exe", OWNER, 1);
        os.add(100, 7, TOOL, OWNER, 1);
        let actor = Actor::new(&policy(act, allowed), os.clone(), None, 9);
        (actor, os)
    }

    const TRAY: Caller = Caller {
        pid: 50,
        session: 1,
    };

    #[test]
    fn m4d_the_role_must_be_on_and_the_program_allowed() {
        let (actor, os) = setup(false, &[TOOL]);
        assert!(
            actor
                .act(Verb::Suspend, 100, 7, TRAY)
                .unwrap_err()
                .contains("off")
        );
        let (actor, os2) = setup(true, &[]);
        assert!(
            actor
                .act(Verb::Suspend, 100, 7, TRAY)
                .unwrap_err()
                .contains("not on the allow list")
        );
        assert!(!os.frozen(100, 7) && !os2.frozen(100, 7));
        assert_eq!(actor.frozen_count(), 0, "a refusal leaves no record");
    }

    #[test]
    fn m4d_only_the_owners_programs_in_the_callers_session() {
        let (actor, os) = setup(true, &[TOOL]);
        os.add(101, 8, TOOL, "S-1-5-21-9-9-9-1002", 1);
        os.add(102, 9, TOOL, OWNER, 2);
        assert!(
            actor
                .act(Verb::Suspend, 101, 8, TRAY)
                .unwrap_err()
                .contains("owner")
        );
        assert!(
            actor
                .act(Verb::Suspend, 102, 9, TRAY)
                .unwrap_err()
                .contains("session")
        );
        assert!(actor.act(Verb::Terminate, 101, 8, TRAY).is_err());
        assert!(os.alive(101, 8) == Some(true) && os.alive(102, 9) == Some(true));
    }

    #[test]
    fn m4d_never_the_helper_the_caller_system_or_protected_programs() {
        let (actor, os) = setup(true, &[TOOL, r"C:\Windows\explorer.exe"]);
        os.add(200, 3, r"C:\Windows\explorer.exe", OWNER, 1);
        // Protected programs cannot even get on the list; a policy file
        // edited by hand is stopped here as well.
        assert!(
            actor
                .act(Verb::Suspend, 200, 3, TRAY)
                .unwrap_err()
                .contains("never")
        );
        for pid in [0, 4, 9, 50] {
            assert!(actor.act(Verb::Terminate, pid, 1, TRAY).is_err(), "{pid}");
        }
    }

    #[test]
    fn m4d_a_reused_pid_is_another_process() {
        let (actor, os) = setup(true, &[TOOL]);
        // Same pid, other creation time: gone, not frozen.
        assert!(actor.act(Verb::Suspend, 100, 999, TRAY).is_ok());
        assert!(!os.frozen(100, 7));
        assert_eq!(actor.frozen_count(), 0);
    }

    #[test]
    fn m4d_suspend_is_recorded_once_and_resume_only_for_what_it_froze() {
        let (actor, os) = setup(true, &[TOOL]);
        assert!(
            actor.act(Verb::Resume, 100, 7, TRAY).is_err(),
            "not frozen by the helper"
        );
        actor.act(Verb::Suspend, 100, 7, TRAY).unwrap();
        actor.act(Verb::Suspend, 100, 7, TRAY).unwrap();
        assert!(os.frozen(100, 7));
        assert_eq!(actor.frozen_count(), 1, "one record, one suspend count");
        actor.act(Verb::Resume, 100, 7, TRAY).unwrap();
        assert!(!os.frozen(100, 7));
        assert_eq!(actor.frozen_count(), 0);
    }

    #[test]
    fn m4d_a_failed_suspend_leaves_no_record() {
        let (actor, os) = setup(true, &[TOOL]);
        *os.fail_suspend.lock().unwrap() = true;
        assert!(actor.act(Verb::Suspend, 100, 7, TRAY).is_err());
        assert_eq!(actor.frozen_count(), 0);
    }

    #[test]
    fn m4d_the_sweep_releases_for_a_caller_that_is_gone() {
        let (actor, os) = setup(true, &[TOOL]);
        actor.act(Verb::Suspend, 100, 7, TRAY).unwrap();
        actor.sweep();
        assert!(os.frozen(100, 7), "the caller is still there");
        os.exit(50, 5);
        actor.sweep();
        assert!(!os.frozen(100, 7), "released");
        assert_eq!(actor.frozen_count(), 0);
    }

    #[test]
    fn m4d_a_program_that_ends_while_frozen_is_forgotten() {
        let (actor, os) = setup(true, &[TOOL]);
        actor.act(Verb::Suspend, 100, 7, TRAY).unwrap();
        os.exit(100, 7);
        actor.sweep();
        assert_eq!(actor.frozen_count(), 0);
    }

    #[test]
    fn m4d_stop_releases_everything_and_terminate_clears_the_record() {
        let (actor, os) = setup(true, &[TOOL]);
        os.add(103, 11, TOOL, OWNER, 1);
        actor.act(Verb::Suspend, 100, 7, TRAY).unwrap();
        actor.act(Verb::Suspend, 103, 11, TRAY).unwrap();
        actor.act(Verb::Terminate, 103, 11, TRAY).unwrap();
        assert_eq!(os.alive(103, 11), Some(false));
        assert_eq!(actor.frozen_count(), 1);
        actor.release_all();
        assert!(!os.frozen(100, 7));
        assert_eq!(actor.frozen_count(), 0);
    }

    #[test]
    fn m4d_the_record_is_written_and_recovered_after_a_crash() {
        let dir = std::env::temp_dir().join(format!("kaltstellen-act-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = state_file(&dir);
        let os = Arc::new(FakeOs::default());
        os.add(50, 5, r"C:\Kalt\kaltstellen.exe", OWNER, 1);
        os.add(100, 7, TOOL, OWNER, 1);
        let actor = Actor::new(&policy(true, &[TOOL]), os.clone(), Some(file.clone()), 9);
        actor.act(Verb::Suspend, 100, 7, TRAY).unwrap();
        let text = std::fs::read_to_string(&file).unwrap();
        assert!(text.contains("\"pid\": 100"), "{text}");
        // The helper dies; a new one starts and resumes what was left.
        let again = Actor::new(&policy(true, &[TOOL]), os.clone(), Some(file.clone()), 9);
        again.recover();
        assert!(!os.frozen(100, 7));
        assert!(!file.exists(), "the record is gone once released");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn m4d_after_stop_nothing_is_frozen_any_more() {
        let (actor, os) = setup(true, &[TOOL]);
        actor.act(Verb::Suspend, 100, 7, TRAY).unwrap();
        actor.release_all();
        assert!(actor.act(Verb::Suspend, 100, 7, TRAY).is_err());
        assert!(!os.frozen(100, 7));
        assert_eq!(actor.frozen_count(), 0);
    }

    #[test]
    fn m4d_a_foreign_record_is_not_trusted_blindly() {
        let dir = std::env::temp_dir().join(format!("kaltstellen-act2-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = state_file(&dir);
        std::fs::write(&file, "not json").unwrap();
        let (actor, _) = setup(true, &[TOOL]);
        let actor = Actor {
            file: Some(file.clone()),
            ..actor
        };
        actor.recover();
        assert!(!file.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
