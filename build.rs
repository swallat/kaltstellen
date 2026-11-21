//! Embeds the program icon and version information into the Windows executable.

const ICON: &str = "branding/app/kaltstellen.ico";

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed={ICON}");

    // The cfg of the *target*, not of the machine running this build script.
    if std::env::var_os("CARGO_CFG_WINDOWS").is_none() {
        return;
    }

    // File description, product name and version come from Cargo.toml.
    winresource::WindowsResource::new()
        .set_icon(ICON)
        .compile()
        .expect(
            "failed to embed the program icon; is the Windows SDK resource compiler installed?",
        );
}
