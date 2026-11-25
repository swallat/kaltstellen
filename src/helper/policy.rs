//! The policy file `helper.toml` in the helper's data folder.
//!
//! Only administrators and SYSTEM can write it (the folder gets an explicit
//! access list at install), the owner can read it. It holds the roles that
//! are on, the owner recorded at install, and the allow lists the later
//! milestones use (services M4e, elevated programs M4d). The allow lists are
//! changed only from an elevated prompt (owner decision of 2026-11-24).

use std::path::Path;

use serde::{Deserialize, Serialize};

use super::protocol::Roles;
use crate::error::{Code, Error, Result};

/// The largest policy file read, so a broken file cannot fill memory.
const MAX_SIZE: u64 = 256 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Policy {
    /// Version of this file's layout.
    pub version: u32,
    /// The SID of the user the helper serves, recorded at install; the
    /// pipe admits this user only.
    pub owner_sid: String,
    pub roles: Roles,
    #[serde(default)]
    pub allow: Allow,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Allow {
    /// Windows services the helper may stop and start (M4e).
    #[serde(default)]
    pub services: Vec<String>,
    /// Elevated programs of the owner it may freeze or end (M4d), by full
    /// path.
    #[serde(default)]
    pub programs: Vec<String>,
}

/// The most programs on the allow list, so a policy stays small.
pub const MAX_PROGRAMS: usize = 256;

/// Whether two Windows paths name the same file, ignoring case and the
/// kind of slash.
fn same_path(a: &str, b: &str) -> bool {
    let key = |p: &str| p.replace('/', "\\").to_lowercase();
    key(a) == key(b)
}

/// The text of an allow-list entry: a full path of an `.exe` on a drive,
/// without wildcards, relative parts, quotes or control characters, and not
/// a program kaltstellen never touches (the hard-protected list). Checked
/// again by the elevated step, so the unelevated caller cannot slip
/// anything in.
pub fn normalize_program(path: &str) -> Result<String> {
    let path = path.trim().replace('/', "\\");
    let bad = |why: &str| {
        Err(Error::new(
            Code::HelperPolicy,
            format!("{path:?} cannot go on the allow list: {why}"),
        ))
    };
    let bytes = path.as_bytes();
    if bytes.len() < 7
        || bytes.len() > 520
        || !bytes[0].is_ascii_alphabetic()
        || bytes[1] != b':'
        || bytes[2] != b'\\'
    {
        return bad("it must be a full path on a drive, like C:\\Tools\\tool.exe");
    }
    if path
        .chars()
        .any(|c| c.is_control() || "*?\"<>|".contains(c))
    {
        return bad("it has wildcards, quotes or control characters");
    }
    let mut parts = path[3..].split('\\').peekable();
    let mut file = "";
    while let Some(part) = parts.next() {
        if part.is_empty()
            || part == "."
            || part == ".."
            || part.ends_with(' ')
            || part.ends_with('.')
        {
            return bad("it has an empty or relative part");
        }
        if parts.peek().is_none() {
            file = part;
        }
    }
    if !file.to_lowercase().ends_with(".exe") {
        return bad("it must end in .exe");
    }
    if crate::protection::is_hard_protected(file) {
        return bad("kaltstellen never touches this program");
    }
    Ok(path)
}

impl Policy {
    /// The policy written at install: observing on, acting and services off
    /// until the administrator allows them.
    pub fn initial(owner_sid: &str) -> Policy {
        Policy {
            version: 1,
            owner_sid: owner_sid.to_owned(),
            roles: Roles {
                observe: true,
                act: false,
                services: false,
            },
            allow: Allow::default(),
        }
    }

    /// Adds an elevated program to the allow list; `false` if it was there.
    pub fn allow_program(&mut self, path: &str) -> Result<bool> {
        let path = normalize_program(path)?;
        if self.allow.programs.iter().any(|p| same_path(p, &path)) {
            return Ok(false);
        }
        if self.allow.programs.len() >= MAX_PROGRAMS {
            return Err(Error::new(
                Code::HelperPolicy,
                format!("the allow list holds {MAX_PROGRAMS} programs already"),
            ));
        }
        self.allow.programs.push(path);
        Ok(true)
    }

    /// Takes a program off the allow list; `false` if it was not there. The
    /// text is compared as given (case-insensitively), so an entry that
    /// became invalid can still be removed.
    pub fn disallow_program(&mut self, path: &str) -> bool {
        let before = self.allow.programs.len();
        self.allow.programs.retain(|p| !same_path(p, path.trim()));
        self.allow.programs.len() != before
    }

    /// Whether `path` (a process's full image path) is on the allow list.
    pub fn allows_program(&self, path: &str) -> bool {
        self.allow.programs.iter().any(|p| same_path(p, path))
    }

    pub fn parse(text: &str) -> Result<Policy> {
        let policy: Policy = toml::from_str(text)
            .map_err(|e| Error::new(Code::HelperPolicy, format!("helper.toml: {e}")))?;
        if policy.version != 1 {
            return Err(Error::new(
                Code::HelperPolicy,
                format!("helper.toml: version {} is not supported", policy.version),
            ));
        }
        if !super::valid_owner(&policy.owner_sid) {
            return Err(Error::new(
                Code::HelperPolicy,
                "helper.toml: owner_sid is not a SID",
            ));
        }
        Ok(policy)
    }

    pub fn to_toml(&self) -> String {
        let body = toml::to_string_pretty(self).expect("a policy is always valid TOML");
        format!("# kaltstellen background helper. Changed only from an elevated prompt.\n{body}")
    }

    pub fn read(path: &Path) -> Result<Policy> {
        let meta = std::fs::metadata(path)
            .map_err(|e| Error::new(Code::HelperPolicy, format!("{}: {e}", path.display())))?;
        if meta.len() > MAX_SIZE {
            return Err(Error::new(
                Code::HelperPolicy,
                format!("{}: larger than {MAX_SIZE} bytes", path.display()),
            ));
        }
        let text = std::fs::read_to_string(path)
            .map_err(|e| Error::new(Code::HelperPolicy, format!("{}: {e}", path.display())))?;
        Policy::parse(&text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SID: &str = "S-1-5-21-1-2-3-1001";

    #[test]
    fn the_initial_policy_round_trips() {
        let policy = Policy::initial(SID);
        let text = policy.to_toml();
        assert_eq!(Policy::parse(&text).unwrap(), policy);
        assert!(policy.roles.observe && !policy.roles.act && !policy.roles.services);
    }

    #[test]
    fn a_broken_policy_is_refused_not_guessed() {
        for text in [
            "",
            "version = 2\nowner_sid = \"S-1-5-18\"\n[roles]\nobserve=true\nact=false\nservices=false\n",
            "version = 1\nowner_sid = \"nobody\"\n[roles]\nobserve=true\nact=false\nservices=false\n",
            "version = 1\nowner_sid = \"S-1-5-18\"\nsurprise = 1\n[roles]\nobserve=true\nact=false\nservices=false\n",
            "version = 1\nowner_sid = \"S-1-5-18\"\n[roles]\nobserve=true\n",
        ] {
            let err = Policy::parse(text).unwrap_err();
            assert_eq!(err.code, Code::HelperPolicy, "{text:?}");
        }
    }

    #[test]
    fn a_file_is_read_from_disk_with_a_size_limit() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("helper.toml");
        std::fs::write(&path, Policy::initial(SID).to_toml()).unwrap();
        assert_eq!(Policy::read(&path).unwrap().owner_sid, SID);
        std::fs::write(&path, vec![b'#'; MAX_SIZE as usize + 1]).unwrap();
        assert_eq!(Policy::read(&path).unwrap_err().code, Code::HelperPolicy);
        assert_eq!(
            Policy::read(&dir.path().join("none.toml"))
                .unwrap_err()
                .code,
            Code::HelperPolicy
        );
    }

    #[test]
    fn m4d_allow_list_entries_are_full_exe_paths_only() {
        for good in [r"C:\Tools\tool.exe", "d:/games/Sim/sim.EXE"] {
            assert!(normalize_program(good).is_ok(), "{good}");
        }
        assert_eq!(
            normalize_program("d:/games/Sim/sim.exe").unwrap(),
            r"d:\games\Sim\sim.exe"
        );
        for bad in [
            "",
            "tool.exe",
            r"\\server\share\tool.exe",
            r"C:\Tools\*.exe",
            r"C:\Tools\..\Windows\tool.exe",
            r"C:\Tools\.\tool.exe",
            r"C:\Tools\tool.dll",
            r"C:\Tools\tool.exe ",
            "C:\\Tools\\\"tool\".exe",
            r"C:\Windows\explorer.exe",
            r"C:\Tools\\tool.exe",
        ] {
            // The one with trailing space is trimmed and fine; the rest not.
            let result = normalize_program(bad);
            if bad == r"C:\Tools\tool.exe " {
                assert!(result.is_ok());
            } else {
                assert!(result.is_err(), "{bad:?} was accepted");
            }
        }
    }

    #[test]
    fn m4d_allow_and_disallow_ignore_case_and_slashes() {
        let mut policy = Policy::initial(SID);
        assert!(policy.allow_program(r"C:\Tools\Tool.exe").unwrap());
        assert!(
            !policy.allow_program("c:/tools/tool.EXE").unwrap(),
            "same file"
        );
        assert!(policy.allows_program(r"c:\TOOLS\tool.exe"));
        assert!(!policy.allows_program(r"C:\Tools\other.exe"));
        assert!(!policy.disallow_program(r"C:\Tools\other.exe"));
        assert!(policy.disallow_program("C:/TOOLS/tool.exe"));
        assert!(policy.allow.programs.is_empty());
        // The list survives the file round trip.
        policy.allow_program(r"C:\Tools\Tool.exe").unwrap();
        assert_eq!(Policy::parse(&policy.to_toml()).unwrap(), policy);
    }

    #[test]
    fn m4d_the_allow_list_is_bounded() {
        let mut policy = Policy::initial(SID);
        for n in 0..MAX_PROGRAMS {
            policy
                .allow_program(&format!(r"C:\Tools\t{n}.exe"))
                .unwrap();
        }
        assert!(policy.allow_program(r"C:\Tools\one-more.exe").is_err());
    }
}
