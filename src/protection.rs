//! The two protection tiers of spec P6. The lists are data shipped with the
//! tool; they are not user-extendable in version 1.

/// Never touched, whatever the configuration says (A6): core Windows
/// processes, the shell, console hosts, error reporting, the desktop window
/// manager. Freezing one of these hangs the session or the machine.
const HARD: &[&str] = &[
    "system idle process",
    "system",
    "secure system",
    "registry",
    "memory compression",
    "smss.exe",
    "csrss.exe",
    "wininit.exe",
    "winlogon.exe",
    "services.exe",
    "lsass.exe",
    "lsaiso.exe",
    "svchost.exe",
    "fontdrvhost.exe",
    "dwm.exe",
    "userinit.exe",
    "logonui.exe",
    "lockapp.exe",
    "explorer.exe",
    "sihost.exe",
    "taskhostw.exe",
    "ctfmon.exe",
    "dllhost.exe",
    "runtimebroker.exe",
    "applicationframehost.exe",
    "shellexperiencehost.exe",
    "startmenuexperiencehost.exe",
    "searchhost.exe",
    "textinputhost.exe",
    "conhost.exe",
    "openconsole.exe",
    "werfault.exe",
    "werfaultsecure.exe",
    "wermgr.exe",
];

/// Allowed only after explicit confirmation (command line: `--force`).
const WARN: &[(&str, WarnCategory)] = &[
    ("mstsc.exe", WarnCategory::RemoteAccess),
    ("teamviewer.exe", WarnCategory::RemoteAccess),
    ("teamviewer_service.exe", WarnCategory::RemoteAccess),
    ("anydesk.exe", WarnCategory::RemoteAccess),
    ("rustdesk.exe", WarnCategory::RemoteAccess),
    ("parsecd.exe", WarnCategory::RemoteAccess),
    ("sshd.exe", WarnCategory::RemoteAccess),
    ("nvcontainer.exe", WarnCategory::GraphicsDriver),
    ("nvdisplay.container.exe", WarnCategory::GraphicsDriver),
    ("radeonsoftware.exe", WarnCategory::GraphicsDriver),
    ("amdrsserv.exe", WarnCategory::GraphicsDriver),
    ("atiesrxx.exe", WarnCategory::GraphicsDriver),
    ("igfxem.exe", WarnCategory::GraphicsDriver),
    ("codemeter.exe", WarnCategory::LicenceService),
    ("codemetercc.exe", WarnCategory::LicenceService),
    ("hasplms.exe", WarnCategory::LicenceService),
    ("lmgrd.exe", WarnCategory::LicenceService),
    ("msmpeng.exe", WarnCategory::Security),
    ("nissrv.exe", WarnCategory::Security),
    ("securityhealthservice.exe", WarnCategory::Security),
    ("securityhealthsystray.exe", WarnCategory::Security),
    ("smartscreen.exe", WarnCategory::Security),
    ("devenv.exe", WarnCategory::DeveloperTool),
    ("code.exe", WarnCategory::DeveloperTool),
    ("windowsterminal.exe", WarnCategory::DeveloperTool),
    ("wezterm-gui.exe", WarnCategory::DeveloperTool),
    ("cmd.exe", WarnCategory::DeveloperTool),
    ("powershell.exe", WarnCategory::DeveloperTool),
    ("pwsh.exe", WarnCategory::DeveloperTool),
    ("bash.exe", WarnCategory::DeveloperTool),
    ("wsl.exe", WarnCategory::DeveloperTool),
    ("wslhost.exe", WarnCategory::DeveloperTool),
    ("vmmem", WarnCategory::DeveloperTool),
    ("vmmemwsl", WarnCategory::DeveloperTool),
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WarnCategory {
    RemoteAccess,
    GraphicsDriver,
    LicenceService,
    Security,
    DeveloperTool,
}

impl WarnCategory {
    pub fn description(self) -> &'static str {
        match self {
            WarnCategory::RemoteAccess => "remote-access tool",
            WarnCategory::GraphicsDriver => "graphics driver component",
            WarnCategory::LicenceService => "licence service",
            WarnCategory::Security => "security software",
            WarnCategory::DeveloperTool => "developer tool or shell",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Protection {
    None,
    Warn(WarnCategory),
    Hard,
}

/// Protection tier of an executable name. Ignores case; a name without
/// extension is also tried with `.exe`.
pub fn of(exe: &str) -> Protection {
    let name = exe.trim().to_lowercase();
    let with_exe = format!("{name}.exe");
    let is = |entry: &str| entry == name || (!name.contains('.') && entry == with_exe);
    if HARD.iter().any(|entry| is(entry)) {
        Protection::Hard
    } else if let Some((_, category)) = WARN.iter().find(|(entry, _)| is(entry)) {
        Protection::Warn(*category)
    } else {
        Protection::None
    }
}

pub fn is_hard_protected(exe: &str) -> bool {
    of(exe) == Protection::Hard
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tiers() {
        assert_eq!(of("Explorer.EXE"), Protection::Hard);
        assert_eq!(of("csrss"), Protection::Hard);
        assert_eq!(of("System"), Protection::Hard);
        assert_eq!(of("conhost.exe"), Protection::Hard);
        assert_eq!(of("WerFault.exe"), Protection::Hard);
        assert_eq!(of("anydesk"), Protection::Warn(WarnCategory::RemoteAccess));
        assert_eq!(of("browser.exe"), Protection::None);
        // "system.exe" is not the kernel pseudo-process.
        assert_eq!(of("system.exe"), Protection::None);
    }

    #[test]
    fn lists_are_lower_case_and_disjoint() {
        for entry in HARD {
            assert_eq!(*entry, entry.to_lowercase());
            assert!(!WARN.iter().any(|(warn, _)| warn == entry), "{entry}");
        }
        for (entry, _) in WARN {
            assert_eq!(*entry, entry.to_lowercase());
        }
    }
}
