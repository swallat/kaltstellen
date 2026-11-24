//! The optional background helper (M4a; docs/design-optional-helper.md).
//!
//! A Windows service, `kaltstellen-helper`, that runs a protected copy of
//! this executable (`kaltstellen helper run`). The tray stays unelevated and
//! keeps the policy; the helper is the privileged side. Without it
//! everything works as before: nothing here is reached unless the owner ran
//! `kaltstellen helper install`.
//!
//! This module is safe code only: file locations, the policy file, the
//! pipe protocol and what the service answers. The service control manager,
//! the access lists and the elevated relaunch are in `win32::service`.
//!
//! M4a answers `hello` and `status`. The roles *observe*, *act* and
//! *services* are switched in the policy file but not implemented yet
//! (M4b to M4e).

pub mod client;
pub mod events;
pub mod policy;
pub mod protocol;
pub mod run;
pub mod setup;

use std::path::PathBuf;

use crate::update;
use crate::update::version::Version;
use crate::win32::service::Known;

/// Name of the Windows service.
pub const SERVICE_NAME: &str = "kaltstellen-helper";
/// Name of the helper's pipe (spec: `\\.\pipe\kaltstellen-helper`).
pub const PIPE_NAME: &str = r"\\.\pipe\kaltstellen-helper";
/// File name of the protected copy.
pub const EXE_NAME: &str = "kaltstellen.exe";

/// The folder of the protected copy: `%ProgramFiles%\kaltstellen`,
/// writable by administrators only. `KALTSTELLEN_HELPER_PROGRAM_DIR`
/// redirects it in debug builds (tests only).
pub fn program_dir() -> Option<PathBuf> {
    redirected("KALTSTELLEN_HELPER_PROGRAM_DIR", Known::ProgramFiles)
}

/// The helper's data folder: `%ProgramData%\kaltstellen` (policy file, `kaltstellen.log`,
/// later the state record). `KALTSTELLEN_HELPER_DATA_DIR` redirects it for
/// tests, in debug builds.
pub fn data_dir() -> Option<PathBuf> {
    redirected("KALTSTELLEN_HELPER_DATA_DIR", Known::ProgramData)
}

fn redirected(override_var: &str, known: Known) -> Option<PathBuf> {
    // The redirection is for tests with a built-in debug executable only.
    // An elevated process inherits the caller's environment, so a release
    // build must not let it decide where a SYSTEM service's files go.
    if cfg!(debug_assertions)
        && let Some(dir) = std::env::var_os(override_var).filter(|v| !v.is_empty())
    {
        return Some(PathBuf::from(dir));
    }
    crate::win32::service::known_folder(known).map(|base| base.join("kaltstellen"))
}

/// The protected copy of the executable.
pub fn installed_exe() -> Option<PathBuf> {
    program_dir().map(|dir| dir.join(EXE_NAME))
}

/// The policy file `helper.toml`.
pub fn policy_file() -> Option<PathBuf> {
    data_dir().map(|dir| dir.join("helper.toml"))
}

/// The version of this executable (`0.0.0-dev+hash` in development builds).
pub fn version() -> &'static str {
    update::CURRENT
}

/// How the tray's version relates to the installed helper's.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Freshness {
    Same,
    /// The helper is older than this executable: the owner should run
    /// `kaltstellen helper install` again (the helper never updates itself).
    Older,
    /// The helper is newer than this executable.
    Newer,
    /// One of the versions could not be compared (development build).
    Unknown,
}

/// Compares the helper's version with `ours`.
pub fn freshness(helper: &str, ours: &str) -> Freshness {
    if helper == ours {
        return Freshness::Same;
    }
    let (Some(h), Some(o)) = (Version::parse(helper), Version::parse(ours)) else {
        return Freshness::Unknown;
    };
    // Development builds (`0.0.0-dev+hash`) have no order.
    if h.pre == "dev" || o.pre == "dev" {
        return Freshness::Unknown;
    }
    match h.cmp(&o) {
        std::cmp::Ordering::Less => Freshness::Older,
        std::cmp::Ordering::Greater => Freshness::Newer,
        std::cmp::Ordering::Equal => Freshness::Same,
    }
}

/// Is `sid` a well-formed SID string (`S-1-5-21-…`)? Checked before one is
/// put into an access list (the elevated installer gets it as an argument).
pub fn valid_sid(sid: &str) -> bool {
    let Some(rest) = sid.strip_prefix("S-1-") else {
        return false;
    };
    let parts: Vec<&str> = rest.split('-').collect();
    parts.len() >= 2
        && parts.len() <= 15
        && parts
            .iter()
            .all(|p| !p.is_empty() && p.len() <= 20 && p.bytes().all(|b| b.is_ascii_digit()))
}

/// Is `sid` acceptable as the user the helper serves? One account: a
/// local, domain or Microsoft account SID (`S-1-5-21-a-b-c-RID` with RID of
/// 1000 or more) or an Entra ID account (`S-1-12-1-a-b-c-d`), numbers
/// without leading zeros and within 32 bits. Not a group, not a built-in
/// account.
pub fn valid_owner(sid: &str) -> bool {
    if !valid_sid(sid) {
        return false;
    }
    let numbers: Option<Vec<u32>> = sid["S-1-".len()..]
        .split('-')
        .map(|part| {
            if part.len() > 1 && part.starts_with('0') {
                None
            } else {
                part.parse().ok()
            }
        })
        .collect();
    match numbers.as_deref() {
        Some([5, 21, _, _, _, rid]) => *rid >= 1000,
        Some([12, 1, _, _, _, _]) => true,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_owner_is_one_account_not_a_group_everybody_is_in() {
        assert!(valid_owner("S-1-5-21-1004336348-1177238915-682003330-1001"));
        assert!(valid_owner("S-1-12-1-1-2-3-4"));
        for broad in [
            "S-1-1-0",
            "S-1-5-11",
            "S-1-5-32-545",
            "S-1-5-32-546",
            "S-1-5-18",
            "S-1-5-4",
            "S-1-5-21-1-2-3-513",
            "S-1-5-21-1-2-3-999",
            "S-1-5-21-1-2-3-01001",
            "S-1-5-21-1-2-3-99999999999",
            "S-1-5-21-1-2-1001",
        ] {
            assert!(!valid_owner(broad), "{broad}");
        }
    }

    #[test]
    fn sids_are_checked_before_they_reach_an_access_list() {
        assert!(valid_sid("S-1-5-21-1004336348-1177238915-682003330-1001"));
        assert!(valid_sid("S-1-5-18"));
        for bad in [
            "",
            "S-1-",
            "S-1-5",
            "S-2-5-18",
            "S-1-5-18)(A;;GA;;;WD",
            "S-1-5--18",
            "s-1-5-18",
            "S-1-5-18 ",
        ] {
            assert!(!valid_sid(bad), "{bad:?}");
        }
    }

    #[test]
    fn freshness_tells_when_the_helper_is_older() {
        assert_eq!(freshness("1.2.0", "1.2.0"), Freshness::Same);
        assert_eq!(freshness("1.1.0", "1.2.0"), Freshness::Older);
        assert_eq!(freshness("1.3.0", "1.2.0"), Freshness::Newer);
        assert_eq!(freshness("0.0.0-dev+abc", "1.2.0"), Freshness::Unknown);
    }
}
