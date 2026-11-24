//! The uninstall entry the per-user setup writes (`installer/kaltstellen.iss`,
//! spec Q2): after a self-update, the version shown in Windows' list of
//! installed apps follows, as long as the entry belongs to this copy.

use std::path::Path;

use crate::error::Result;
use crate::win32::registry;

/// The setup's `AppId`; never changes (also in `installer/kaltstellen.iss`).
pub const APP_ID: &str = "{0867F4F2-B652-4569-9343-104E8470E95F}";

/// The entry below `HKEY_CURRENT_USER` (Inno Setup appends `_is1`).
pub fn key() -> String {
    format!("Software\\Microsoft\\Windows\\CurrentVersion\\Uninstall\\{APP_ID}_is1")
}

/// Sets `DisplayVersion` of the entry at `key` to `version` if the entry
/// installed the folder `dir`; true if it did. A portable copy elsewhere
/// leaves an installed one alone.
pub fn follow_update(key: &str, dir: &Path, version: &str) -> Result<bool> {
    let Some(location) = registry::read(key, "InstallLocation")? else {
        return Ok(false);
    };
    if normalize(&location) != normalize(&dir.to_string_lossy()) {
        return Ok(false);
    }
    registry::write(key, "DisplayVersion", version)?;
    Ok(true)
}

fn normalize(path: &str) -> String {
    path.replace('/', "\\")
        .trim_end_matches('\\')
        .to_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn q2_the_setup_uses_the_same_app_id() {
        let script = include_str!("../../installer/kaltstellen.iss");
        // Inno Setup writes `{{` for a literal brace.
        assert!(
            script.contains(&format!("AppId={{{APP_ID}")),
            "AppId differs"
        );
        assert!(
            script.contains(&format!("{APP_ID}_is1")),
            "the key in [Code] differs"
        );
    }

    #[test]
    fn q2_the_entry_follows_an_update_of_its_own_folder_only() {
        let root = format!(
            "Software\\kaltstellen-test-{}-uninstall",
            std::process::id()
        );
        let key = format!("{root}\\{APP_ID}_is1");
        let dir = Path::new(r"C:\Users\x\AppData\Local\Programs\kaltstellen");
        // No entry: nothing to do, nothing created.
        let missing = follow_update(&key, dir, "1.0.1");
        let created = registry::exists(&key);
        registry::write(
            &key,
            "InstallLocation",
            r"c:\users\x\appdata\local\programs\kaltstellen\",
        )
        .unwrap();
        registry::write(&key, "DisplayVersion", "1.0.0").unwrap();
        let other = follow_update(&key, Path::new(r"D:\portable"), "1.0.1");
        let after_other = registry::read(&key, "DisplayVersion");
        let own = follow_update(&key, dir, "1.0.1");
        let after_own = registry::read(&key, "DisplayVersion");
        // Removed before the asserts, so a failure leaves nothing behind.
        registry::delete_tree(&root).unwrap();
        assert!(!registry::exists(&root).unwrap(), "nothing left behind");
        assert!(!missing.unwrap());
        assert!(!created.unwrap(), "a missing entry is not created");
        assert!(!other.unwrap());
        assert_eq!(after_other.unwrap().as_deref(), Some("1.0.0"));
        assert!(own.unwrap());
        assert_eq!(after_own.unwrap().as_deref(), Some("1.0.1"));
    }
}
