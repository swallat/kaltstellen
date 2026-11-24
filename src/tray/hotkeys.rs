//! Global hotkeys of the tray (WP9; spec U3, C5, C7). Which hotkeys to hold
//! and what changed is decided here; `win32::tray` registers them on the
//! tray window with `MOD_NOREPEAT`.

use std::collections::{BTreeMap, HashSet};

use log::{info, warn};

use crate::config::Profile;
use crate::hotkey::Hotkey;

/// One hotkey the configuration asks for. `id` is the `WM_HOTKEY` ID: the
/// position of the profile in the file plus one, so it stays the same for
/// an unchanged profile.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Binding {
    pub id: i32,
    pub profile: String,
    pub hotkey: Hotkey,
}

/// The hotkeys to register for `profiles`. `Ctrl+Alt+<printable>` is left
/// out (C7: it would steal AltGr characters); the configuration check warns
/// about it already.
pub fn wanted(profiles: &[Profile]) -> Vec<Binding> {
    profiles
        .iter()
        .enumerate()
        .filter_map(|(n, profile)| {
            let hotkey = profile.hotkey?;
            if hotkey.steals_altgr() {
                return None;
            }
            Some(Binding {
                id: i32::try_from(n).ok()? + 1,
                profile: profile.name.clone(),
                hotkey,
            })
        })
        .collect()
}

/// `MOD_*` flags for `RegisterHotKey`, with `MOD_NOREPEAT` (C7).
pub fn modifier_flags(hotkey: &Hotkey) -> u32 {
    const MOD_ALT: u32 = 0x1;
    const MOD_CONTROL: u32 = 0x2;
    const MOD_SHIFT: u32 = 0x4;
    const MOD_WIN: u32 = 0x8;
    const MOD_NOREPEAT: u32 = 0x4000;
    let m = hotkey.modifiers;
    let mut flags = MOD_NOREPEAT;
    for (on, flag) in [
        (m.alt, MOD_ALT),
        (m.ctrl, MOD_CONTROL),
        (m.shift, MOD_SHIFT),
        (m.win, MOD_WIN),
    ] {
        if on {
            flags |= flag;
        }
    }
    flags
}

/// What is registered now, and which failures were reported already.
#[derive(Debug, Default)]
pub struct Registrar {
    held: BTreeMap<i32, Binding>,
    reported: HashSet<String>,
    /// Hotkeys that could not be registered, for the menu.
    pub unavailable: Vec<String>,
}

impl Registrar {
    /// Brings the registered hotkeys in line with `wanted`. `register`
    /// returns an error text when another program holds the hotkey.
    /// Failures are logged once per hotkey and profile (C5); a failed one is
    /// tried again at the next sync.
    pub fn sync(
        &mut self,
        wanted: &[Binding],
        register: &mut dyn FnMut(&Binding) -> Result<(), String>,
        unregister: &mut dyn FnMut(i32),
    ) {
        let stale: Vec<i32> = self
            .held
            .iter()
            .filter(|(id, held)| !wanted.iter().any(|w| w.id == **id && w == *held))
            .map(|(id, _)| *id)
            .collect();
        for id in stale {
            if let Some(old) = self.held.remove(&id) {
                unregister(id);
                info!(
                    "tray: hotkey {} for \"{}\" released",
                    old.hotkey, old.profile
                );
            }
        }
        self.unavailable.clear();
        for binding in wanted {
            if self.held.contains_key(&binding.id) {
                continue;
            }
            match register(binding) {
                Ok(()) => {
                    info!(
                        "tray: hotkey {} registered for \"{}\" (id {})",
                        binding.hotkey, binding.profile, binding.id
                    );
                    self.held.insert(binding.id, binding.clone());
                }
                Err(reason) => {
                    let key = format!("{}\u{0}{}", binding.hotkey, binding.profile);
                    if self.reported.insert(key) {
                        warn!(
                            "tray: hotkey {} for \"{}\" is not available ({reason}); \
                             the profile stays usable from the menu and the command line",
                            binding.hotkey, binding.profile
                        );
                    }
                    self.unavailable
                        .push(format!("{} ({})", binding.hotkey, binding.profile));
                }
            }
        }
    }

    /// The profile of a pressed hotkey.
    pub fn profile(&self, id: i32) -> Option<&str> {
        self.held.get(&id).map(|b| b.profile.as_str())
    }

    /// Releases everything (Quit).
    pub fn clear(&mut self, unregister: &mut dyn FnMut(i32)) {
        for id in std::mem::take(&mut self.held).into_keys() {
            unregister(id);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn profile(name: &str, hotkey: Option<&str>) -> Profile {
        Profile {
            name: name.to_owned(),
            hotkey: hotkey.map(|h| Hotkey::parse(h).unwrap()),
            apps: Vec::new(),
            triggers: Vec::new(),
            priority: 0,
            watch: false,
        }
    }

    #[test]
    fn c7_altgr_combinations_are_not_registered_and_ids_follow_the_file() {
        let profiles = vec![
            profile("a", Some("Ctrl+Shift+Alt+F1")),
            profile("b", None),
            profile("c", Some("Ctrl+Alt+7")),
            profile("d", Some("Win+F2")),
        ];
        let wanted = wanted(&profiles);
        let ids: Vec<(i32, &str)> = wanted.iter().map(|b| (b.id, b.profile.as_str())).collect();
        assert_eq!(ids, vec![(1, "a"), (4, "d")]);
        assert_eq!(modifier_flags(&wanted[0].hotkey), 0x4000 | 0x1 | 0x2 | 0x4);
        assert_eq!(modifier_flags(&wanted[1].hotkey), 0x4000 | 0x8);
    }

    #[test]
    fn c5_sync_registers_changes_only_and_reports_failures_once() {
        let mut r = Registrar::default();
        let mut log: Vec<String> = Vec::new();
        let taken = "Ctrl+Shift+Alt+F2";
        let run = |r: &mut Registrar, profiles: &[Profile], log: &mut Vec<String>| {
            let wanted = wanted(profiles);
            let mut registered = Vec::new();
            let mut released = Vec::new();
            r.sync(
                &wanted,
                &mut |b| {
                    if b.hotkey.to_string() == taken {
                        Err("taken".into())
                    } else {
                        registered.push(b.id);
                        Ok(())
                    }
                },
                &mut |id| released.push(id),
            );
            log.push(format!("+{registered:?} -{released:?}"));
        };
        let first = vec![
            profile("a", Some("Ctrl+Shift+Alt+F1")),
            profile("b", Some(taken)),
        ];
        run(&mut r, &first, &mut log);
        assert_eq!(r.profile(1), Some("a"));
        assert_eq!(r.profile(2), None);
        assert_eq!(r.unavailable, vec![format!("{taken} (b)")]);
        assert_eq!(r.reported.len(), 1);
        // Unchanged: nothing to do; the failed one is tried again silently.
        run(&mut r, &first, &mut log);
        assert_eq!(r.reported.len(), 1, "reported once");
        // Changed key for "a": released and registered anew.
        let second = vec![profile("a", Some("Ctrl+Shift+Alt+F3"))];
        run(&mut r, &second, &mut log);
        assert_eq!(log, vec!["+[1] -[]", "+[] -[]", "+[1] -[1]"]);
        assert!(r.unavailable.is_empty());
        let mut released = Vec::new();
        r.clear(&mut |id| released.push(id));
        assert_eq!(released, vec![1]);
        assert_eq!(r.profile(1), None);
    }
}
