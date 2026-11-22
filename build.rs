//! Embeds the program icon, the three tray icons and version information
//! into the Windows executable, and decides the version the program reports.

use std::process::Command;

const ICON: &str = "branding/app/kaltstellen.ico";
const MANIFEST: &str = "kaltstellen.manifest";

/// Tray icons with their resource IDs; `src/win32/tray.rs` loads them by
/// these numbers.
const TRAY_ICONS: [(&str, &str); 3] = [
    ("branding/tray/ico/kaltstellen-neutral.ico", "101"),
    ("branding/tray/ico/kaltstellen-aktiv.ico", "102"),
    ("branding/tray/ico/kaltstellen-fehler.ico", "103"),
];

/// The version the program reports (`--version`, updater).
///
/// A release build (`KALTSTELLEN_RELEASE` set) reports exactly the version
/// in Cargo.toml, which the release workflow has set from the tag.
///
/// Any other build is a development build. Cargo.toml only holds the
/// placeholder `0.0.0-dev`, so the version is derived from Git: the last
/// version tag, the number of commits since, and the commit hash.
///
/// | Situation | Reported |
/// | --- | --- |
/// | 5 commits after `v1.2.0` | `1.2.1-dev.5+1a2b3c4` |
/// | exactly on `v1.2.0` | `1.2.0+1a2b3c4` |
/// | 5 commits after `v1.3.0-rc.1` | `1.3.0-rc.1.dev.5+1a2b3c4` |
/// | no version tag yet (or a shallow clone) | `0.0.0-dev+1a2b3c4` |
/// | no Git at all (source archive) | `0.0.0-dev` |
///
/// The patch number is raised after a release so that a development build
/// sorts after the release it grew from and before the next one.
fn version() -> String {
    let package = std::env::var("CARGO_PKG_VERSION").expect("set by Cargo");
    if std::env::var_os("KALTSTELLEN_RELEASE").is_some() {
        return package;
    }
    // Re-run when HEAD moves or tags change, so the version is current.
    for path in ["logs/HEAD", "packed-refs", "refs/tags"] {
        if let Some(path) = git(&["rev-parse", "--git-path", path]) {
            println!("cargo:rerun-if-changed={path}");
        }
    }
    let described = git(&[
        "describe",
        "--tags",
        "--long",
        "--abbrev=7",
        "--match",
        "v[0-9]*",
    ]);
    if let Some(version) = described.as_deref().and_then(from_describe) {
        return version;
    }
    match git(&["rev-parse", "--short=7", "HEAD"]) {
        Some(hash) if is_hash(&hash) => format!("{package}+{hash}"),
        _ => package,
    }
}

/// Turns `v1.2.0-5-g1a2b3c4` (`git describe --long`) into a version.
fn from_describe(described: &str) -> Option<String> {
    let mut parts = described.rsplitn(3, '-');
    let hash = parts.next()?.strip_prefix('g')?;
    let commits: u64 = parts.next()?.parse().ok()?;
    let tag = parts.next()?.strip_prefix('v')?;
    if !is_hash(hash) {
        return None;
    }
    let (core, pre) = match tag.split_once('-') {
        Some((core, pre)) => (core, Some(pre)),
        None => (tag, None),
    };
    let numbers: Vec<u64> = core
        .split('.')
        .map(|n| n.parse().ok())
        .collect::<Option<_>>()?;
    let [major, minor, patch] = numbers[..] else {
        return None;
    };
    Some(match (commits, pre) {
        (0, _) => format!("{tag}+{hash}"),
        (n, Some(pre)) => format!("{major}.{minor}.{patch}-{pre}.dev.{n}+{hash}"),
        (n, None) => format!("{major}.{minor}.{}-dev.{n}+{hash}", patch + 1),
    })
}

fn is_hash(text: &str) -> bool {
    text.len() >= 7 && text.chars().all(|c| c.is_ascii_hexdigit())
}

fn git(args: &[&str]) -> Option<String> {
    let output = Command::new("git").args(args).output().ok()?;
    let text = String::from_utf8(output.stdout).ok()?;
    let text = text.trim();
    (output.status.success() && !text.is_empty()).then(|| text.to_owned())
}
fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    // UP1: the release workflow sets it; without it the build is a
    // development build that does not update itself.
    println!("cargo:rerun-if-env-changed=KALTSTELLEN_RELEASE");
    println!("cargo:rustc-env=KALTSTELLEN_VERSION={}", version());
    println!("cargo:rerun-if-changed={ICON}");
    println!("cargo:rerun-if-changed={MANIFEST}");
    for (path, _) in TRAY_ICONS {
        println!("cargo:rerun-if-changed={path}");
    }

    // The cfg of the *target*, not of the machine running this build script.
    if std::env::var_os("CARGO_CFG_WINDOWS").is_none() {
        return;
    }

    // File description, product name and version come from Cargo.toml.
    let mut res = winresource::WindowsResource::new();
    // The application icon first, with the lowest ID, so Explorer shows it.
    res.set_icon(ICON);
    // Per-monitor DPI awareness v2 and common controls v6.
    res.set_manifest_file(MANIFEST);
    for (path, id) in TRAY_ICONS {
        res.set_icon_with_id(path, id);
    }
    res.compile()
        .expect("failed to embed the icons; is the Windows SDK resource compiler installed?");
}
