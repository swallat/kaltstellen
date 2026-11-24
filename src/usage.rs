//! H5 suggestions from use: what the owner does by hand after a profile was
//! applied. No watcher and no timer. Two lists are compared per activation:
//! **A**, taken when the apply ended ([`after`]), and **B**, taken before the
//! next switch ([`before`]). The results are counted in `usage.json` in the
//! data directory, and a result seen in at least 3 of the last 5 activations
//! becomes a suggestion (R12: a coarse guess, only ever a proposal).

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::fs;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::config::{App, AppAction, Config, NEUTRAL};
use crate::error::{Code, Error, Result};
use crate::process::Snapshot;
use crate::protection::{self, Protection};
use crate::state::State;
use crate::unassigned::{self, Observed};

/// Activations kept per profile.
const KEEP: usize = 10;
/// Activations looked at for a suggestion, and how often a result must
/// repeat in them.
const WINDOW: usize = 5;
const REPEATS: usize = 3;
/// Programs remembered per profile.
const MAX_PROGRAMS: usize = 200;

/// What happened to a program between A and B.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Finding {
    /// Ran at A, not at B, and kaltstellen did not stop it.
    Gone,
    /// Frozen by kaltstellen at A, the process is gone at B.
    GoneFrozen,
    /// Not running at A, running at B, not started by kaltstellen.
    Appeared,
    /// Stopped by kaltstellen, running again at B.
    CameBack,
    /// Ran at A and at B without an entry in the profile.
    Stayed,
}

/// A process kaltstellen froze, as recorded at A.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Frozen {
    pub exe: String,
    pub pid: u32,
    pub created: u64,
}

/// List A of the activation that is waiting for its B.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Pending {
    pub profile: String,
    /// Boot marker of the Windows session; another one at B drops it.
    pub boot: u64,
    /// Programs seen running at A (N1 sources), lower case names with path.
    pub running: Vec<Program>,
    pub frozen: Vec<Frozen>,
    /// Programs kaltstellen stopped (lower case).
    pub stopped: Vec<String>,
}

/// An [`Observed`] that can be stored.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Program {
    pub exe: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Activation {
    /// Seconds since 1970.
    pub at: u64,
    /// Lower case executable name to its result.
    pub results: BTreeMap<String, Finding>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProfileUsage {
    /// Oldest first, at most [`KEEP`].
    #[serde(default)]
    pub activations: Vec<Activation>,
    /// Path where known, per lower case name.
    #[serde(default)]
    pub paths: BTreeMap<String, String>,
    /// Lower case names the owner said never to suggest.
    #[serde(default)]
    pub never: BTreeSet<String>,
}

/// `usage.json`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Usage {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pending: Option<Pending>,
    /// Per profile (lower case).
    #[serde(default)]
    pub profiles: BTreeMap<String, ProfileUsage>,
}

/// What a suggestion proposes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Proposal {
    Stop,
    Suspend,
    Start,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Suggestion {
    pub exe: String,
    pub path: Option<String>,
    pub proposal: Proposal,
    /// In how many of the last `of` activations the result was seen.
    pub seen: usize,
    pub of: usize,
    pub reason: String,
}

impl Suggestion {
    pub fn action_name(&self) -> &'static str {
        match self.proposal {
            Proposal::Stop => "stop",
            Proposal::Suspend => "suspend",
            Proposal::Start => "start",
        }
    }
}

/// The entry of `profile` for `exe`, if any.
fn entry<'a>(apps: &'a [App], exe: &str) -> Option<&'a App> {
    apps.iter().find(|a| a.exe.eq_ignore_ascii_case(exe))
}

fn has_no_say(apps: &[App], exe: &str) -> bool {
    entry(apps, exe).is_none_or(|a| matches!(a.action, AppAction::Ignore))
}

/// Compares A with B. `now` is the N1 snapshot at B, `live` the processes
/// at B as (name lower case, pid, created).
pub fn compare(
    pending: &Pending,
    apps: &[App],
    now: &[Observed],
    live: &[(String, u32, u64)],
) -> BTreeMap<String, Finding> {
    let mut results = BTreeMap::new();
    let alive: HashSet<&str> = live.iter().map(|(n, _, _)| n.as_str()).collect();
    for frozen in &pending.frozen {
        let same = live
            .iter()
            .any(|(_, pid, created)| *pid == frozen.pid && *created == frozen.created);
        if !same {
            results.insert(frozen.exe.to_lowercase(), Finding::GoneFrozen);
        }
    }
    for exe in &pending.stopped {
        if alive.contains(exe.as_str()) {
            results.insert(exe.clone(), Finding::CameBack);
        }
    }
    let at_a: HashSet<String> = pending
        .running
        .iter()
        .map(|o| o.exe.to_lowercase())
        .collect();
    for program in &pending.running {
        let key = program.exe.to_lowercase();
        if !has_no_say(apps, &program.exe) || results.contains_key(&key) {
            continue;
        }
        let result = if alive.contains(key.as_str()) {
            Finding::Stayed
        } else {
            Finding::Gone
        };
        results.insert(key, result);
    }
    for program in now {
        let key = program.exe.to_lowercase();
        if !at_a.contains(&key)
            && has_no_say(apps, &program.exe)
            && !pending.stopped.contains(&key)
            && !results.contains_key(&key)
        {
            results.insert(key, Finding::Appeared);
        }
    }
    results
}

impl Usage {
    /// Adds the result of one activation of `profile`.
    pub fn record(
        &mut self,
        profile: &str,
        results: BTreeMap<String, Finding>,
        paths: &BTreeMap<String, String>,
        at: u64,
    ) {
        let usage = self.profiles.entry(profile.to_lowercase()).or_default();
        for key in results.keys() {
            if let Some(path) = paths.get(key) {
                usage.paths.insert(key.clone(), path.clone());
            }
        }
        usage.activations.push(Activation { at, results });
        let extra = usage.activations.len().saturating_sub(KEEP);
        usage.activations.drain(..extra);
        // At most MAX_PROGRAMS names: the oldest activations lose theirs
        // first.
        let names = |u: &ProfileUsage| -> BTreeSet<String> {
            u.activations
                .iter()
                .flat_map(|a| a.results.keys().cloned())
                .collect()
        };
        while names(usage).len() > MAX_PROGRAMS && usage.activations.len() > 1 {
            usage.activations.remove(0);
        }
        let known = names(usage);
        usage.paths.retain(|k, _| known.contains(k));
    }

    /// Suggestions for `profile` with its current entries.
    pub fn suggestions(&self, profile: &str, apps: &[App]) -> Vec<Suggestion> {
        let Some(usage) = self.profiles.get(&profile.to_lowercase()) else {
            return Vec::new();
        };
        let recent = &usage.activations[usage.activations.len().saturating_sub(WINDOW)..];
        let mut counts: BTreeMap<(&str, Finding), usize> = BTreeMap::new();
        for activation in recent {
            for (exe, result) in &activation.results {
                *counts.entry((exe.as_str(), *result)).or_default() += 1;
            }
        }
        let of = recent.len();
        let mut found = Vec::new();
        for ((exe, result), seen) in counts {
            if seen < REPEATS || usage.never.contains(exe) {
                continue;
            }
            if protection::of(exe) != Protection::None {
                continue;
            }
            let existing = entry(apps, exe);
            let path = usage.paths.get(exe).cloned();
            let (proposal, what) = match (result, existing.map(|a| &a.action)) {
                (Finding::Gone, None | Some(AppAction::Ignore)) => (
                    Proposal::Stop,
                    "was gone when you switched back (it ended or you closed it)",
                ),
                (Finding::GoneFrozen, Some(AppAction::Suspend { .. })) => {
                    (Proposal::Stop, "was frozen and gone when you switched back")
                }
                (Finding::Appeared, None | Some(AppAction::Ignore)) if path.is_some() => (
                    Proposal::Start,
                    "was running again when you switched back and was not started by kaltstellen",
                ),
                (Finding::CameBack, Some(AppAction::Stop { .. })) => (
                    Proposal::Suspend,
                    "was running again when you switched back after it was stopped",
                ),
                _ => continue,
            };
            found.push(Suggestion {
                exe: exe.to_owned(),
                path,
                proposal,
                seen,
                of,
                reason: format!("In {seen} of the last {of} activations {exe} {what}."),
            });
        }
        found
    }

    /// `Never for this program`.
    pub fn never(&mut self, profile: &str, exe: &str) {
        self.profiles
            .entry(profile.to_lowercase())
            .or_default()
            .never
            .insert(exe.to_lowercase());
    }

    /// Resets the counters of `exe` in `profile` (after `Add entry`).
    pub fn reset(&mut self, profile: &str, exe: &str) {
        let key = exe.to_lowercase();
        if let Some(usage) = self.profiles.get_mut(&profile.to_lowercase()) {
            for activation in &mut usage.activations {
                activation.results.remove(&key);
            }
        }
    }
}

fn file(dir: &Path) -> std::path::PathBuf {
    dir.join("usage.json")
}

/// Reads `usage.json`; missing or unreadable is empty (the problem is
/// returned, KS305).
pub fn load(dir: &Path) -> (Usage, Option<Error>) {
    match fs::read_to_string(file(dir)) {
        Ok(text) => match serde_json::from_str(&text) {
            Ok(usage) => (usage, None),
            Err(err) => (
                Usage::default(),
                Some(Error::new(Code::UsageIo, format!("usage.json: {err}"))),
            ),
        },
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => (Usage::default(), None),
        Err(err) => (
            Usage::default(),
            Some(Error::new(Code::UsageIo, format!("usage.json: {err}"))),
        ),
    }
}

/// Writes `usage.json` atomically.
pub fn save(dir: &Path, usage: &Usage) -> Result<()> {
    let text = serde_json::to_string_pretty(usage)
        .map_err(|e| Error::new(Code::UsageIo, e.to_string()))?;
    let tmp = dir.join("usage.json.tmp");
    fs::create_dir_all(dir)
        .and_then(|()| fs::write(&tmp, text))
        .and_then(|()| fs::rename(&tmp, file(dir)))
        .map_err(|e| Error::new(Code::UsageIo, format!("usage.json: {e}")))
}

/// `Forget what was learned`: deletes `usage.json`.
pub fn forget(dir: &Path) -> Result<()> {
    match fs::remove_file(file(dir)) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(Error::new(Code::UsageIo, format!("usage.json: {err}"))),
    }
}

/// Edits `usage.json` in `dir` (read, change, write).
pub fn edit(dir: &Path, change: impl FnOnce(&mut Usage)) -> Result<()> {
    let (mut usage, _) = load(dir);
    change(&mut usage);
    save(dir, &usage)
}

fn log_problem(result: Result<()>) {
    if let Err(err) = result {
        log::warn!("{err}");
    }
}

/// Before a switch or a recover: B of the activation that is waiting. Does
/// nothing when learning is off, when no A is waiting, when it belongs to
/// another profile than the active one, or to an earlier boot.
pub fn before(dir: &Path, learning: bool, config: &Config, state: &State) {
    let (mut usage, problem) = load(dir);
    if let Some(problem) = problem {
        log::warn!("{problem}");
    }
    let Some(pending) = usage.pending.take() else {
        return;
    };
    let Ok(snapshot) = Snapshot::take() else {
        return;
    };
    let same = pending.profile.eq_ignore_ascii_case(&state.active_profile)
        && pending.boot == snapshot.boot_marker();
    if learning
        && same
        && let Some(profile) = config.profile(&pending.profile)
    {
        let session = snapshot.own_session().unwrap_or(0);
        let live: Vec<(String, u32, u64)> = snapshot
            .processes()
            .iter()
            .filter(|p| p.session_id == session)
            .map(|p| (p.name.to_lowercase(), p.pid, p.created))
            .collect();
        let now = unassigned::snapshot();
        let results = compare(&pending, &profile.apps, &now, &live);
        let paths: BTreeMap<String, String> = now
            .iter()
            .filter_map(|o| Some((o.exe.to_lowercase(), o.path.clone()?)))
            .collect();
        usage.record(&pending.profile, results, &paths, unassigned::now());
    }
    log_problem(save(dir, &usage));
}

/// After a switch that ended on `state.active_profile`: A of the next
/// activation. `neutral` has no entries and learns nothing.
pub fn after(dir: &Path, learning: bool, state: &State) {
    if !learning {
        return;
    }
    let (mut usage, problem) = load(dir);
    if let Some(problem) = problem {
        log::warn!("{problem}");
    }
    usage.pending = None;
    if !state.active_profile.eq_ignore_ascii_case(NEUTRAL)
        && let Ok(snapshot) = Snapshot::take()
    {
        usage.pending = Some(Pending {
            profile: state.active_profile.clone(),
            boot: snapshot.boot_marker(),
            running: unassigned::snapshot()
                .into_iter()
                .map(|o| Program {
                    exe: o.exe,
                    path: o.path,
                })
                .collect(),
            frozen: state
                .suspended
                .iter()
                .map(|s| Frozen {
                    exe: s.exe.clone(),
                    pid: s.pid,
                    created: s.created,
                })
                .collect(),
            stopped: state.stopped.iter().map(|s| s.exe.to_lowercase()).collect(),
        });
    }
    log_problem(save(dir, &usage));
}

/// Drops the waiting A (sign-out, quit): that activation has no B.
pub fn drop_pending(dir: &Path) {
    let (mut usage, _) = load(dir);
    if usage.pending.take().is_some() {
        log_problem(save(dir, &usage));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{App, AppAction, OnTimeout};

    fn app(exe: &str, action: AppAction) -> App {
        App::new(exe, None, action)
    }

    fn stop() -> AppAction {
        AppAction::Stop {
            on_timeout: OnTimeout::Kill,
            restart_on_neutral: false,
            args: Vec::new(),
            cwd: None,
        }
    }

    fn observed(exe: &str) -> Observed {
        Observed {
            exe: exe.into(),
            path: Some(format!("C:\\p\\{exe}")),
        }
    }

    fn two(exe: &str) -> Program {
        Program {
            exe: exe.into(),
            path: None,
        }
    }

    fn pending() -> Pending {
        Pending {
            profile: "focus".into(),
            boot: 1,
            running: vec![two("chat.exe"), two("tool.exe"), two("edit.exe")],
            frozen: vec![Frozen {
                exe: "video.exe".into(),
                pid: 10,
                created: 100,
            }],
            stopped: vec!["sync.exe".into()],
        }
    }

    fn live(items: &[(&str, u32, u64)]) -> Vec<(String, u32, u64)> {
        items
            .iter()
            .map(|(n, p, c)| ((*n).to_owned(), *p, *c))
            .collect()
    }

    #[test]
    fn compare_tells_the_five_results_apart() {
        let apps = vec![
            app("video.exe", AppAction::Suspend { trim: false }),
            app("sync.exe", stop()),
            app(
                "edit.exe",
                AppAction::Start {
                    args: vec![],
                    cwd: None,
                },
            ),
        ];
        let now = vec![
            observed("tool.exe"),
            observed("new.exe"),
            observed("sync.exe"),
        ];
        let live = live(&[
            ("tool.exe", 1, 1),
            ("new.exe", 2, 2),
            ("sync.exe", 3, 3),
            ("edit.exe", 4, 4),
        ]);
        let got = compare(&pending(), &apps, &now, &live);
        assert_eq!(got["chat.exe"], Finding::Gone);
        assert_eq!(got["tool.exe"], Finding::Stayed);
        assert_eq!(got["new.exe"], Finding::Appeared);
        assert_eq!(got["sync.exe"], Finding::CameBack);
        assert_eq!(got["video.exe"], Finding::GoneFrozen);
        // an entry other than ignore: kaltstellen's business, not learned
        assert!(!got.contains_key("edit.exe"));
    }

    #[test]
    fn a_frozen_program_that_is_still_there_is_nothing() {
        let apps = vec![app("video.exe", AppAction::Suspend { trim: false })];
        let got = compare(&pending(), &apps, &[], &live(&[("video.exe", 10, 100)]));
        assert!(!got.contains_key("video.exe"));
        // same PID, other creation time: another process
        let got = compare(&pending(), &apps, &[], &live(&[("video.exe", 10, 999)]));
        assert_eq!(got["video.exe"], Finding::GoneFrozen);
    }

    fn record_n(usage: &mut Usage, n: usize, result: Finding) {
        for i in 0..n {
            let mut r = BTreeMap::new();
            r.insert("chat.exe".to_owned(), result);
            let mut paths = BTreeMap::new();
            paths.insert("chat.exe".to_owned(), "C:\\p\\chat.exe".to_owned());
            usage.record("Focus", r, &paths, i as u64);
        }
    }

    #[test]
    fn three_of_the_last_five_make_a_suggestion() {
        let mut usage = Usage::default();
        record_n(&mut usage, 2, Finding::Gone);
        assert!(usage.suggestions("focus", &[]).is_empty());
        record_n(&mut usage, 1, Finding::Gone);
        let got = usage.suggestions("FOCUS", &[]);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].proposal, Proposal::Stop);
        assert_eq!((got[0].seen, got[0].of), (3, 3));
        assert!(got[0].reason.contains("3 of the last 3"));
        // an entry that already decides the program ends the suggestion
        let apps = vec![app("chat.exe", stop())];
        assert!(usage.suggestions("focus", &apps).is_empty());
    }

    #[test]
    fn old_results_fall_out_of_the_window() {
        let mut usage = Usage::default();
        record_n(&mut usage, 3, Finding::Gone);
        record_n(&mut usage, 3, Finding::Stayed);
        // last 5: gone 2 (index 1,2), stayed 3 -> stayed has no suggestion
        assert!(usage.suggestions("focus", &[]).is_empty());
        assert_eq!(usage.profiles["focus"].activations.len(), 6);
    }

    #[test]
    fn keeps_ten_activations_and_never_beats_a_suggestion() {
        let mut usage = Usage::default();
        record_n(&mut usage, 12, Finding::Gone);
        assert_eq!(usage.profiles["focus"].activations.len(), KEEP);
        usage.never("Focus", "CHAT.exe");
        assert!(usage.suggestions("focus", &[]).is_empty());
    }

    #[test]
    fn appeared_needs_a_path_and_came_back_changes_the_stop() {
        let mut usage = Usage::default();
        for i in 0..3 {
            let mut r = BTreeMap::new();
            r.insert("tool.exe".to_owned(), Finding::Appeared);
            r.insert("sync.exe".to_owned(), Finding::CameBack);
            let mut paths = BTreeMap::new();
            if i == 2 {
                paths.insert("tool.exe".to_owned(), "C:\\p\\tool.exe".to_owned());
            }
            usage.record("focus", r, &paths, i);
        }
        let apps = vec![app("sync.exe", stop())];
        let got = usage.suggestions("focus", &apps);
        let by: BTreeMap<_, _> = got.iter().map(|s| (s.exe.as_str(), s.proposal)).collect();
        assert_eq!(by["tool.exe"], Proposal::Start);
        assert_eq!(by["sync.exe"], Proposal::Suspend);
        usage.reset("focus", "tool.exe");
        assert_eq!(usage.suggestions("focus", &apps).len(), 1);
    }

    #[test]
    fn warn_list_programs_are_never_suggested() {
        let mut usage = Usage::default();
        for i in 0..3 {
            let mut r = BTreeMap::new();
            r.insert("anydesk.exe".to_owned(), Finding::Gone);
            usage.record("focus", r, &BTreeMap::new(), i);
        }
        assert!(usage.suggestions("focus", &[]).is_empty());
    }

    #[test]
    fn at_most_200_programs_per_profile() {
        let mut usage = Usage::default();
        for i in 0..3 {
            let mut r = BTreeMap::new();
            for n in 0..120 {
                r.insert(format!("p{i}-{n}.exe"), Finding::Gone);
            }
            usage.record("focus", r, &BTreeMap::new(), i);
        }
        let total: BTreeSet<_> = usage.profiles["focus"]
            .activations
            .iter()
            .flat_map(|a| a.results.keys())
            .collect();
        assert!(total.len() <= MAX_PROGRAMS);
    }

    #[test]
    fn file_round_trip_and_problems() {
        let dir = tempfile_dir("usage");
        let (empty, problem) = load(&dir);
        assert!(problem.is_none() && empty == Usage::default());
        let mut usage = Usage::default();
        record_n(&mut usage, 3, Finding::Gone);
        usage.pending = Some(pending());
        save(&dir, &usage).unwrap();
        assert_eq!(load(&dir).0, usage);
        fs::write(dir.join("usage.json"), "{not json").unwrap();
        let (broken, problem) = load(&dir);
        assert_eq!(broken, Usage::default());
        assert_eq!(problem.unwrap().code, Code::UsageIo);
        forget(&dir).unwrap();
        forget(&dir).unwrap();
        assert!(!dir.join("usage.json").exists());
    }

    fn tempfile_dir(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("kaltstellen-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }
}
