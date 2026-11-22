//! Embeds the program icon, the three tray icons and version information
//! into the Windows executable.

const ICON: &str = "branding/app/kaltstellen.ico";
const MANIFEST: &str = "kaltstellen.manifest";

/// Tray icons with their resource IDs; `src/win32/tray.rs` loads them by
/// these numbers.
const TRAY_ICONS: [(&str, &str); 3] = [
    ("branding/tray/ico/kaltstellen-neutral.ico", "101"),
    ("branding/tray/ico/kaltstellen-aktiv.ico", "102"),
    ("branding/tray/ico/kaltstellen-fehler.ico", "103"),
];

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    // UP1: the release workflow sets it; without it the build is a
    // development build that does not update itself.
    println!("cargo:rerun-if-env-changed=KALTSTELLEN_RELEASE");
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
