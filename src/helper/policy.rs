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

    pub fn parse(text: &str) -> Result<Policy> {
        let policy: Policy = toml::from_str(text)
            .map_err(|e| Error::new(Code::HelperPolicy, format!("helper.toml: {e}")))?;
        if policy.version != 1 {
            return Err(Error::new(
                Code::HelperPolicy,
                format!("helper.toml: version {} is not supported", policy.version),
            ));
        }
        if !super::valid_sid(&policy.owner_sid) {
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
}
