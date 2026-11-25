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

/// The most services on the allow list.
pub const MAX_SERVICES: usize = 64;

/// Services that are never stopped, listed or not: the helper itself and
/// the ones Windows cannot run without (lower case).
const NEVER_SERVICES: &[&str] = &[
    "kaltstellen-helper",
    "rpcss",
    "rpceptmapper",
    "dcomlaunch",
    "lsm",
    "samss",
    "eventlog",
    "plugplay",
    "power",
    "profsvc",
    "gpsvc",
    "wininit",
    "winlogon",
    "schedule",
    "wudfpf",
    "trustedinstaller",
    "windefend",
    "mpssvc",
    "bfe",
    "cryptsvc",
    "wscsvc",
    "sens",
    "winmgmt",
    "appinfo",
    "keyiso",
    "brokerinfrastructure",
    "usermanager",
    "dhcp",
    "dnscache",
    "nsi",
    "lanmanserver",
    "lanmanworkstation",
    "termservice",
    "systemeventsbroker",
    "timebrokersvc",
    "coremessagingregistrar",
    "securityhealthservice",
    "sense",
    "netlogon",
    "winrm",
    "sshd",
];

/// The key name of a service: no path characters, no control characters,
/// at most 256 characters, and not one of the services that are never touched.
pub fn normalize_service(name: &str) -> Result<String> {
    let name = name.trim();
    let bad = name.is_empty()
        || name.chars().count() > 256
        || name
            .chars()
            .any(|c| c.is_control() || matches!(c, '/' | '\\' | '"' | '*' | '?'));
    if bad {
        return Err(Error::new(
            Code::HelperPolicy,
            "a service is named by its short key name, like Spooler",
        ));
    }
    if NEVER_SERVICES.contains(&name.to_lowercase().as_str()) {
        return Err(Error::new(
            Code::HelperPolicy,
            format!("{name} is never stopped by the helper"),
        ));
    }
    Ok(name.to_owned())
}

/// The most programs on the allow list, so a policy stays small.
pub const MAX_PROGRAMS: usize = 256;

/// Whether two Windows paths name the same file, ignoring case and the
/// kind of slash.
fn same_path(a: &str, b: &str) -> bool {
    let key = |p: &str| p.replace('/', "\\").to_lowercase();
    key(a) == key(b)
}

/// Whether some part looks like an 8.3 short name (`EXPLOR~1.EXE`), which
/// would also get past the check for protected programs.
fn has_short_name(path: &str) -> bool {
    path.split('\\').any(|part| {
        part.as_bytes()
            .windows(2)
            .any(|pair| pair[0] == b'~' && pair[1].is_ascii_digit())
    })
}

/// Whether `text` is fit for a command line and the log: no control
/// characters or quotes, not ending in a backslash (it would swallow the
/// closing quote), at most 520 bytes.
pub fn plain_text(text: &str) -> bool {
    !text.is_empty()
        && text.len() <= 520
        && !text.ends_with('\\')
        && !text.chars().any(|c| c.is_control() || c == '"')
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
    if path[2..].contains(':') {
        return bad("it names a stream, not a file");
    }
    if has_short_name(&path) {
        return bad("it uses a short (8.3) name; give the long name");
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

    /// Puts a Windows service on the allow list; `false` if it was there.
    pub fn allow_service(&mut self, name: &str) -> Result<bool> {
        let name = normalize_service(name)?;
        if self
            .allow
            .services
            .iter()
            .any(|s| s.eq_ignore_ascii_case(&name))
        {
            return Ok(false);
        }
        if self.allow.services.len() >= MAX_SERVICES {
            return Err(Error::new(
                Code::HelperPolicy,
                format!("the allow list holds {MAX_SERVICES} services already"),
            ));
        }
        self.allow.services.push(name);
        Ok(true)
    }

    /// Takes a service off the allow list (text as given, ignoring case).
    pub fn disallow_service(&mut self, name: &str) -> bool {
        let before = self.allow.services.len();
        self.allow
            .services
            .retain(|s| !s.eq_ignore_ascii_case(name.trim()));
        self.allow.services.len() != before
    }

    pub fn allows_service(&self, name: &str) -> bool {
        self.allow
            .services
            .iter()
            .any(|s| s.eq_ignore_ascii_case(name))
    }

    /// Whether `path` (a process's full image path) is on the allow list.
    pub fn allows_program(&self, path: &str) -> bool {
        self.allow.programs.iter().any(|p| same_path(p, path))
    }

    pub fn parse(text: &str) -> Result<Policy> {
        let mut policy: Policy = toml::from_str(text)
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
        // Entries that could not be put on the list by the commands never
        // work (the helper checks again); they are not carried along.
        policy
            .allow
            .services
            .retain(|s| normalize_service(s).is_ok());
        policy.allow.services.truncate(MAX_SERVICES);
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
        for good in [
            r"C:\Tools\tool.exe",
            "d:/games/Sim/sim.EXE",
            "  C:\\Tools\\tool.exe ",
        ] {
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
            r"C:\Tools\a:b.exe",
            r"C:\Tools\x.exe:y.exe",
            r"C:\Windows\EXPLOR~1.EXE",
            r"C:\PROGRA~1\tool.exe",
            "C:\\Tools\\\"tool\".exe",
            r"C:\Windows\explorer.exe",
            r"C:\Tools\\tool.exe",
        ] {
            assert!(normalize_program(bad).is_err(), "{bad:?} was accepted");
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

    #[test]
    fn m4d_remove_text_must_be_plain() {
        assert!(plain_text(r"C:\Tools\tool.exe"));
        for bad in ["", r"C:\Tools\", "C:\\a\"b.exe", "C:\\a\nb.exe"] {
            assert!(!plain_text(bad), "{bad:?}");
        }
        assert!(!plain_text(&"a".repeat(521)));
    }

    #[test]
    fn m4e_services_are_key_names_never_critical_ones() {
        let mut policy = Policy::initial("S-1-5-21-1-2-3-1001");
        assert!(policy.allow_service("Spooler").unwrap());
        assert!(!policy.allow_service("spooler").unwrap());
        assert!(policy.allows_service("SPOOLER"));
        for bad in [
            "",
            "  ",
            r"C:\x",
            "a/b",
            "a\"b",
            "Wild*",
            "RpcSs",
            "kaltstellen-helper",
            "WinDefend",
        ] {
            assert!(policy.allow_service(bad).is_err(), "{bad:?}");
        }
        assert!(policy.disallow_service(" SPOOLER "));
        assert!(!policy.disallow_service("Spooler"));
        for i in 0..MAX_SERVICES {
            policy.allow_service(&format!("svc{i}")).unwrap();
        }
        assert!(policy.allow_service("one-too-many").is_err());
    }
}
