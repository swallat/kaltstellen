//! The role *services* (M4e): stop and start Windows services of the
//! allow list on request of the owner's tray. Safe code only; the service
//! control manager is behind [`Os`] so the rules are tested without one.
//!
//! The helper keeps no record: a service it stopped is started again by the
//! tray's own state (as for a stopped program), not by the helper.

use std::sync::Arc;
use std::time::Duration;

use log::{info, warn};

use super::policy::{Policy, normalize_service};

/// How long a request waits for the wanted state. The pipe conversation is
/// cut off after a few seconds; a service that needs longer is reported as
/// pending and the caller asks again.
pub const WAIT: Duration = Duration::from_millis(1800);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Want {
    Stop,
    Start,
}

/// The service control manager.
pub trait Os: Send + Sync + std::fmt::Debug {
    /// Asks for the state and waits up to `wait`. `Ok(None)`: no such
    /// service; `Ok(Some(reached))`.
    fn control(
        &self,
        name: &str,
        want: Want,
        wait: Duration,
    ) -> std::result::Result<Option<bool>, String>;
}

/// What came of a request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reached {
    /// The service is in the wanted state.
    Yes,
    /// Asked, but not there yet.
    Pending,
}

#[derive(Debug)]
pub struct Services {
    role: bool,
    policy: Policy,
    os: Arc<dyn Os>,
}

impl Services {
    pub fn new(policy: &Policy, os: Arc<dyn Os>) -> Services {
        Services {
            role: policy.roles.services,
            policy: policy.clone(),
            os,
        }
    }

    pub fn act(&self, want: Want, name: &str) -> std::result::Result<Reached, String> {
        if !self.role {
            return Err("the role services is off".to_owned());
        }
        // Checked again here, so a policy written by hand cannot reach a
        // service that is never touched.
        let name = normalize_service(name).map_err(|e| e.reason)?;
        if !self.policy.allows_service(&name) {
            warn!("services: {name} is not on the allow list");
            return Err(format!("{name} is not on the allow list"));
        }
        match self.os.control(&name, want, WAIT) {
            Ok(Some(true)) => {
                info!("services: {name} {}", past(want));
                Ok(Reached::Yes)
            }
            Ok(Some(false)) => {
                info!("services: {name} asked to {}, not there yet", verb(want));
                Ok(Reached::Pending)
            }
            Ok(None) => Err(format!("there is no service named {name}")),
            Err(e) => {
                warn!("services: {name}: {e}");
                Err(e)
            }
        }
    }
}

fn verb(want: Want) -> &'static str {
    match want {
        Want::Stop => "stop",
        Want::Start => "start",
    }
}

fn past(want: Want) -> &'static str {
    match want {
        Want::Stop => "stopped",
        Want::Start => "started",
    }
}

/// The real service control manager.
#[derive(Debug, Default)]
pub struct RealOs;

impl Os for RealOs {
    fn control(
        &self,
        name: &str,
        want: Want,
        wait: Duration,
    ) -> std::result::Result<Option<bool>, String> {
        crate::win32::service::control(name, want == Want::Start, wait).map_err(|e| e.reason)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    #[derive(Debug, Default)]
    struct FakeOs {
        asked: Mutex<Vec<(String, Want)>>,
        slow: bool,
    }

    impl Os for FakeOs {
        fn control(
            &self,
            name: &str,
            want: Want,
            _wait: Duration,
        ) -> std::result::Result<Option<bool>, String> {
            self.asked.lock().unwrap().push((name.to_owned(), want));
            if name.eq_ignore_ascii_case("missing") {
                return Ok(None);
            }
            if name.eq_ignore_ascii_case("broken") {
                return Err("access denied".to_owned());
            }
            Ok(Some(!self.slow))
        }
    }

    fn setup(role: bool, slow: bool) -> (Services, Arc<FakeOs>) {
        let mut policy = Policy::initial("S-1-5-21-1-2-3-1001");
        policy.roles.services = role;
        for name in ["Spooler", "missing", "broken"] {
            policy.allow_service(name).unwrap();
        }
        let os = Arc::new(FakeOs {
            slow,
            ..FakeOs::default()
        });
        (Services::new(&policy, os.clone()), os)
    }

    #[test]
    fn m4e_the_role_must_be_on_and_the_service_listed() {
        let (off, os) = setup(false, false);
        assert!(off.act(Want::Stop, "Spooler").unwrap_err().contains("off"));
        let (on, os2) = setup(true, false);
        assert_eq!(on.act(Want::Stop, "spooler").unwrap(), Reached::Yes);
        assert!(
            on.act(Want::Stop, "Themes")
                .unwrap_err()
                .contains("allow list")
        );
        assert!(os.asked.lock().unwrap().is_empty());
        assert_eq!(os2.asked.lock().unwrap().len(), 1, "only the listed one");
    }

    #[test]
    fn m4e_critical_services_are_refused_even_if_a_file_lists_them() {
        let mut policy = Policy::initial("S-1-5-21-1-2-3-1001");
        policy.roles.services = true;
        policy.allow.services.push("RpcSs".to_owned());
        policy.allow.services.push("kaltstellen-helper".to_owned());
        let os = Arc::new(FakeOs::default());
        let services = Services::new(&policy, os.clone());
        assert!(services.act(Want::Stop, "rpcss").is_err());
        assert!(services.act(Want::Stop, "kaltstellen-helper").is_err());
        assert!(os.asked.lock().unwrap().is_empty());
    }

    #[test]
    fn m4e_pending_missing_and_failing_services_are_told_apart() {
        let (slow, _) = setup(true, true);
        assert_eq!(slow.act(Want::Start, "Spooler").unwrap(), Reached::Pending);
        let (fast, _) = setup(true, false);
        assert!(
            fast.act(Want::Start, "missing")
                .unwrap_err()
                .contains("no service")
        );
        assert!(
            fast.act(Want::Stop, "broken")
                .unwrap_err()
                .contains("denied")
        );
    }
}
