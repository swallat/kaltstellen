//! N1 unassigned programs (WP11): programs that run and have no entry in a
//! profile. Detection is lazy: a snapshot is taken when the main window
//! opens or refreshes, after the tray applied a profile, and on
//! `kaltstellen status`. `seen.json` in the data directory remembers when a
//! program was first seen, when each profile last changed, and what was
//! dismissed.

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::fs;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::config::{Config, NEUTRAL, Profile};
use crate::error::{Code, Error, Result};
use crate::process::{self, Snapshot};
use crate::protection;
use crate::win32::registry;

/// Per-user registry of notification-area icons (Windows 11).
pub const NOTIFY_ICONS_KEY: &str = "Control Panel\\NotifyIconSettings";

/// A program seen running now.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Observed {
    /// Executable file name.
    pub exe: String,
    /// Full path when the source tells it (the icon registry does).
    pub path: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SeenProgram {
    pub exe: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// Seconds since 1970.
    pub first_seen: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProfileStamp {
    /// Fingerprint of the profile's entries.
    pub hash: String,
    /// When the fingerprint last changed, seconds since 1970.
    pub changed: u64,
}

/// `seen.json`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Seen {
    #[serde(default)]
    pub programs: Vec<SeenProgram>,
    #[serde(default)]
    pub profiles: BTreeMap<String, ProfileStamp>,
    /// Per profile (lower case), the executables (lower case) dismissed.
    #[serde(default)]
    pub dismissed: BTreeMap<String, BTreeSet<String>>,
    /// Per profile (lower case), the programs that started while it was
    /// active and have no entry in it (the tray notes them from the helper's
    /// process events); cleared when the profile is applied again.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub started: BTreeMap<String, Vec<SeenProgram>>,
}

/// At most this many started programs are kept per profile.
pub const STARTED_MAX: usize = 50;

/// One unassigned program of a profile.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unassigned {
    pub exe: String,
    pub path: Option<String>,
    /// First seen after the profile last changed.
    pub new: bool,
    /// Started while the profile was active (the helper reported it).
    pub started: bool,
}

impl Unassigned {
    pub fn row(&self) -> Vec<String> {
        vec![
            self.exe.clone(),
            if self.new { "new" } else { "unassigned" }.to_owned(),
            self.path.clone().unwrap_or_default(),
        ]
    }
}

pub fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

fn fingerprint(profile: &Profile) -> String {
    // FNV-1a over the debug form of the entries: stable within a build,
    // and a changed hash only makes programs look new once more.
    let text = format!("{:?}", profile.apps);
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in text.bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{hash:016x}")
}

impl Seen {
    /// Records programs not seen before.
    pub fn observe(&mut self, observed: &[Observed], now: u64) {
        for program in observed {
            let known = self
                .programs
                .iter()
                .any(|p| p.exe.eq_ignore_ascii_case(&program.exe));
            if !known {
                self.programs.push(SeenProgram {
                    exe: program.exe.clone(),
                    path: program.path.clone(),
                    first_seen: now,
                });
            }
        }
    }

    /// Notes when profiles changed (compared by their entries).
    pub fn note_profiles(&mut self, config: &Config, now: u64) {
        for profile in &config.profiles {
            let hash = fingerprint(profile);
            let stamp = self
                .profiles
                .entry(profile.name.to_lowercase())
                .or_default();
            if stamp.hash != hash {
                stamp.hash = hash;
                stamp.changed = now;
            }
        }
    }

    /// Notes that `exe` started while `profile` was active. `false` when it
    /// was noted before or the list is full.
    pub fn note_started(&mut self, profile: &str, exe: &str, path: Option<&str>, now: u64) -> bool {
        let list = self.started.entry(profile.to_lowercase()).or_default();
        if list.iter().any(|p| p.exe.eq_ignore_ascii_case(exe)) || list.len() >= STARTED_MAX {
            return false;
        }
        list.push(SeenProgram {
            exe: exe.to_owned(),
            path: path.map(str::to_owned),
            first_seen: now,
        });
        true
    }

    /// Forgets what started in `profile`; `true` when there was something.
    pub fn clear_started(&mut self, profile: &str) -> bool {
        self.started.remove(&profile.to_lowercase()).is_some()
    }

    pub fn dismiss(&mut self, profile: &str, exe: &str) {
        self.dismissed
            .entry(profile.to_lowercase())
            .or_default()
            .insert(exe.to_lowercase());
    }

    /// The observed programs without an entry in `profile` and not
    /// dismissed for it. `neutral` has none.
    pub fn unassigned(
        &self,
        config: &Config,
        profile: &str,
        observed: &[Observed],
    ) -> Vec<Unassigned> {
        if profile.trim().eq_ignore_ascii_case(NEUTRAL) {
            return Vec::new();
        }
        let Some(entries) = config.profile(profile) else {
            return Vec::new();
        };
        let decided: HashSet<String> = entries.apps.iter().map(|a| a.exe.to_lowercase()).collect();
        let dismissed = self.dismissed.get(&entries.name.to_lowercase());
        let changed = self
            .profiles
            .get(&entries.name.to_lowercase())
            .map_or(0, |s| s.changed);
        let started = self.started.get(&entries.name.to_lowercase());
        let mut list: Vec<Unassigned> = Vec::new();
        for program in observed {
            let key = program.exe.to_lowercase();
            if decided.contains(&key)
                || dismissed.is_some_and(|d| d.contains(&key))
                || list
                    .iter()
                    .any(|u| u.exe.eq_ignore_ascii_case(&program.exe))
            {
                continue;
            }
            let first_seen = self
                .programs
                .iter()
                .find(|p| p.exe.eq_ignore_ascii_case(&program.exe))
                .map_or(0, |p| p.first_seen);
            let started =
                started.is_some_and(|l| l.iter().any(|p| p.exe.eq_ignore_ascii_case(&program.exe)));
            list.push(Unassigned {
                exe: program.exe.clone(),
                path: program.path.clone(),
                new: first_seen > changed,
                started,
            });
        }
        // What started may have ended or have no window: still listed.
        for program in started.into_iter().flatten() {
            let key = program.exe.to_lowercase();
            if decided.contains(&key)
                || dismissed.is_some_and(|d| d.contains(&key))
                || list
                    .iter()
                    .any(|u| u.exe.eq_ignore_ascii_case(&program.exe))
            {
                continue;
            }
            list.push(Unassigned {
                exe: program.exe.clone(),
                path: program.path.clone(),
                new: false,
                started: true,
            });
        }
        list.sort_by(|a, b| {
            b.started
                .cmp(&a.started)
                .then(b.new.cmp(&a.new))
                .then(a.exe.to_lowercase().cmp(&b.exe.to_lowercase()))
        });
        list
    }
}

fn file(dir: &Path) -> std::path::PathBuf {
    dir.join("seen.json")
}

/// Reads `seen.json`; missing or unreadable is empty (the problem is
/// returned).
pub fn load(dir: &Path) -> (Seen, Option<Error>) {
    match fs::read_to_string(file(dir)) {
        Ok(text) => match serde_json::from_str(&text) {
            Ok(seen) => (seen, None),
            Err(err) => (
                Seen::default(),
                Some(Error::new(Code::SeenIo, format!("seen.json: {err}"))),
            ),
        },
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => (Seen::default(), None),
        Err(err) => (
            Seen::default(),
            Some(Error::new(Code::SeenIo, format!("seen.json: {err}"))),
        ),
    }
}

/// Writes `seen.json` atomically.
pub fn save(dir: &Path, seen: &Seen) -> Result<()> {
    let text =
        serde_json::to_string_pretty(seen).map_err(|e| Error::new(Code::SeenIo, e.to_string()))?;
    let tmp = dir.join("seen.json.tmp");
    fs::create_dir_all(dir)
        .and_then(|()| fs::write(&tmp, text))
        .and_then(|()| fs::rename(&tmp, file(dir)))
        .map_err(|e| Error::new(Code::SeenIo, format!("seen.json: {e}")))
}

/// The executable name of a path from the icon registry, which may start
/// with a known-folder GUID instead of a drive.
fn from_icon_path(path: &str) -> Option<Observed> {
    let exe = path.rsplit(['\\', '/']).next()?.trim();
    if exe.is_empty() || !exe.to_lowercase().ends_with(".exe") {
        return None;
    }
    let absolute = path.len() > 2 && path.as_bytes()[1] == b':';
    Some(Observed {
        exe: exe.to_owned(),
        path: absolute.then(|| path.to_owned()),
    })
}

/// The programs of the icon registry below `HKCU\key` (one subkey per icon
/// with its `ExecutablePath`) whose executable name is in `running`
/// (lower case). A registry that cannot be listed is logged, not hidden;
/// a single entry that cannot be read is skipped.
pub(crate) fn icon_programs(key: &str, running: &HashSet<String>) -> Vec<Observed> {
    let names = match registry::subkeys(key) {
        Ok(names) => names,
        Err(err) => {
            log::warn!("unassigned: {err}");
            return Vec::new();
        }
    };
    names
        .iter()
        .filter_map(|name| registry::read(&format!("{key}\\{name}"), "ExecutablePath").ok()?)
        .filter_map(|path| from_icon_path(&path))
        .filter(|program| running.contains(&program.exe.to_lowercase()))
        .collect()
}

/// N1 sources, read only: programs with a notification-area icon (registry,
/// Windows 11) that run now, plus processes of this session with a visible
/// top-level window. Hard-protected programs and kaltstellen itself are
/// left out.
pub fn snapshot() -> Vec<Observed> {
    let Ok(snapshot) = Snapshot::take() else {
        return Vec::new();
    };
    let session = snapshot.own_session().unwrap_or(0);
    let own = std::env::current_exe()
        .ok()
        .and_then(|p| p.file_name().map(|n| n.to_string_lossy().to_lowercase()));
    let running: HashSet<String> = snapshot
        .processes()
        .iter()
        .filter(|p| p.session_id == session)
        .map(|p| p.name.to_lowercase())
        .collect();
    let mut found = icon_programs(NOTIFY_ICONS_KEY, &running);
    let visible = process::pids_with_visible_windows();
    for p in snapshot.processes() {
        if p.session_id == session && visible.contains(&p.pid) {
            found.push(Observed {
                exe: p.name.clone(),
                path: None,
            });
        }
    }
    // One entry per name; one with a full path wins over one without (a
    // known-folder GUID form), so the path is not lost.
    found.sort_by_key(|o| o.path.is_none());
    let mut seen_names = HashSet::new();
    found.retain(|o| {
        let key = o.exe.to_lowercase();
        !protection::is_hard_protected(&o.exe)
            && own.as_deref() != Some(key.as_str())
            && seen_names.insert(key)
    });
    found.sort();
    found
}

/// Takes a snapshot, updates `seen.json` in `data_dir` and returns the
/// unassigned programs of `profile`.
pub fn refresh(data_dir: &Path, config: &Config, profile: &str) -> Vec<Unassigned> {
    let observed = snapshot();
    refresh_with(data_dir, config, profile, &observed, now())
}

/// [`refresh`] with injected observations, for tests.
pub fn refresh_with(
    data_dir: &Path,
    config: &Config,
    profile: &str,
    observed: &[Observed],
    now: u64,
) -> Vec<Unassigned> {
    let (mut seen, problem) = load(data_dir);
    if let Some(problem) = problem {
        log::warn!("{problem}");
    }
    seen.note_profiles(config, now);
    seen.observe(observed, now);
    if let Err(err) = save(data_dir, &seen) {
        log::warn!("{err}");
    }
    seen.unassigned(config, profile, observed)
}

/// Notes started programs of `profile` in `seen.json` of `data_dir`;
/// returns those that were not noted before.
pub fn note_started(
    data_dir: &Path,
    profile: &str,
    programs: &[(String, Option<String>)],
    now: u64,
) -> Result<Vec<String>> {
    let (mut seen, _) = load(data_dir);
    let fresh: Vec<String> = programs
        .iter()
        .filter(|(exe, path)| seen.note_started(profile, exe, path.as_deref(), now))
        .map(|(exe, _)| exe.clone())
        .collect();
    if !fresh.is_empty() {
        save(data_dir, &seen)?;
    }
    Ok(fresh)
}

/// Forgets the started programs of `profile` (it is applied again).
pub fn clear_started(data_dir: &Path, profile: &str) -> Result<()> {
    let (mut seen, _) = load(data_dir);
    if seen.clear_started(profile) {
        save(data_dir, &seen)?;
    }
    Ok(())
}

/// Dismisses `exe` for `profile` in `seen.json` of `data_dir`.
pub fn dismiss(data_dir: &Path, profile: &str, exe: &str) -> Result<()> {
    let (mut seen, _) = load(data_dir);
    seen.dismiss(profile, exe);
    save(data_dir, &seen)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config;

    fn observed(names: &[&str]) -> Vec<Observed> {
        names
            .iter()
            .map(|n| Observed {
                exe: (*n).to_owned(),
                path: None,
            })
            .collect()
    }

    fn config(text: &str) -> Config {
        config::parse(text).unwrap().config
    }

    const ONE: &str = "[[profile]]\nname = \"focus\"\n[[profile.app]]\nexe = \"browser.exe\"\n";
    const TWO: &str = "[[profile]]\nname = \"focus\"\n[[profile.app]]\nexe = \"browser.exe\"\n[[profile.app]]\nexe = \"chat.exe\"\naction = \"ignore\"\n";

    #[test]
    fn n1_new_until_the_profile_changes_entries_remove_the_marker() {
        let dir = tempfile::tempdir().unwrap();
        let c = config(ONE);
        // Running when the profile is first noted: unassigned, not new.
        let list = refresh_with(
            dir.path(),
            &c,
            "focus",
            &observed(&["browser.exe", "chat.exe"]),
            100,
        );
        assert_eq!(
            list,
            vec![Unassigned {
                exe: "chat.exe".into(),
                path: None,
                new: false,
                started: false
            }]
        );
        // Appears later: new.
        let list = refresh_with(
            dir.path(),
            &c,
            "focus",
            &observed(&["chat.exe", "Game.exe"]),
            200,
        );
        assert_eq!(
            list[0],
            Unassigned {
                exe: "Game.exe".into(),
                path: None,
                new: true,
                started: false
            }
        );
        assert!(!list[1].new);
        // An entry, even "ignore", is a decision: the program leaves the list.
        let c2 = config(TWO);
        let list = refresh_with(
            dir.path(),
            &c2,
            "focus",
            &observed(&["chat.exe", "game.exe"]),
            300,
        );
        assert_eq!(
            list,
            vec![Unassigned {
                exe: "game.exe".into(),
                path: None,
                new: false,
                started: false
            }],
            "the profile changed after game.exe was first seen"
        );
        assert!(refresh_with(dir.path(), &c2, "neutral", &observed(&["x.exe"]), 300).is_empty());
    }

    #[test]
    fn started_programs_come_first_stay_listed_and_are_cleared() {
        let dir = tempfile::tempdir().unwrap();
        let c = config(ONE);
        let noted = note_started(
            dir.path(),
            "Focus",
            &[
                ("chat.exe".to_owned(), Some("C:\\Apps\\chat.exe".to_owned())),
                ("gone.exe".to_owned(), None),
                ("browser.exe".to_owned(), None),
            ],
            50,
        )
        .unwrap();
        assert_eq!(noted, ["chat.exe", "gone.exe", "browser.exe"]);
        // Noted before: not reported again.
        let again =
            note_started(dir.path(), "focus", &[("CHAT.exe".to_owned(), None)], 60).unwrap();
        assert!(again.is_empty());
        // chat.exe runs, gone.exe does not; browser.exe has an entry.
        let list = refresh_with(
            dir.path(),
            &c,
            "focus",
            &observed(&["alpha.exe", "chat.exe"]),
            100,
        );
        let names: Vec<(&str, bool)> = list.iter().map(|u| (u.exe.as_str(), u.started)).collect();
        assert_eq!(
            names,
            [("chat.exe", true), ("gone.exe", true), ("alpha.exe", false)],
            "started first, also the one that is not running; entries decide"
        );
        // Another profile has none of it.
        assert!(note_started(dir.path(), "other", &[("chat.exe".to_owned(), None)], 70).is_ok());
        clear_started(dir.path(), "focus").unwrap();
        let list = refresh_with(dir.path(), &c, "focus", &observed(&["chat.exe"]), 120);
        assert!(list.iter().all(|u| !u.started));
        let (seen, _) = load(dir.path());
        assert!(seen.started.contains_key("other"), "only focus was cleared");
    }

    #[test]
    fn n1_dismiss_hides_a_program_for_that_profile_only() {
        let dir = tempfile::tempdir().unwrap();
        let c = config("[[profile]]\nname = \"a\"\n\n[[profile]]\nname = \"b\"\n");
        refresh_with(dir.path(), &c, "a", &observed(&["x.exe"]), 1);
        dismiss(dir.path(), "A", "X.exe").unwrap();
        assert!(refresh_with(dir.path(), &c, "a", &observed(&["x.exe"]), 2).is_empty());
        assert_eq!(
            refresh_with(dir.path(), &c, "b", &observed(&["x.exe"]), 2).len(),
            1
        );
        let (seen, problem) = load(dir.path());
        assert!(problem.is_none());
        assert!(seen.dismissed["a"].contains("x.exe"));
    }

    #[test]
    fn icon_registry_paths() {
        assert_eq!(
            from_icon_path("C:\\Program Files\\App\\app.exe"),
            Some(Observed {
                exe: "app.exe".into(),
                path: Some("C:\\Program Files\\App\\app.exe".into())
            })
        );
        assert_eq!(
            from_icon_path("{F38BF404-1D43-42F2-9305-67DE0B28FC23}\\explorer.exe"),
            Some(Observed {
                exe: "explorer.exe".into(),
                path: None
            })
        );
        assert_eq!(from_icon_path("C:\\x\\readme.txt"), None);
    }

    /// The icon registry as Windows 11 keeps it (one numbered subkey per
    /// icon, paths with a known-folder GUID or a drive), in a test key:
    /// running programs are found, others and entries without a path not.
    #[test]
    fn n1_icon_registry_finds_running_programs() {
        let root = format!("Software\\kaltstellen-test-{}-icons", std::process::id());
        let entries = [
            (
                "1",
                Some("{7C5A40EF-A0FB-4BFC-874A-C0F2E0B9FA8E}\\Steam\\steam.exe"),
            ),
            (
                "2",
                Some("C:\\Users\\u\\AppData\\Local\\Discord\\app-1\\Discord.exe"),
            ),
            (
                "3",
                Some("C:\\Users\\u\\AppData\\Roaming\\Zoom\\bin\\Zoom.exe"),
            ),
            ("4", None),
        ];
        for (name, path) in entries {
            let key = format!("{root}\\{name}");
            match path {
                Some(path) => registry::write(&key, "ExecutablePath", path).unwrap(),
                None => registry::write(&key, "Other", "x").unwrap(),
            }
        }
        let running: HashSet<String> = ["steam.exe", "discord.exe"]
            .iter()
            .map(|s| (*s).to_owned())
            .collect();
        let mut found: Vec<String> = icon_programs(&root, &running)
            .into_iter()
            .map(|o| o.exe)
            .collect();
        found.sort();
        registry::delete_tree(&root).unwrap();
        assert_eq!(found, ["Discord.exe", "steam.exe"]);
        assert!(!registry::exists(&root).unwrap(), "nothing left behind");
    }

    #[test]
    fn the_live_snapshot_leaves_out_protected_programs() {
        // Reads the real registry and window list, writes nothing.
        for program in snapshot() {
            assert!(!protection::is_hard_protected(&program.exe), "{program:?}");
        }
    }
}
