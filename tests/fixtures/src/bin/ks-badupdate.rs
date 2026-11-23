//! Fixture for the update tests (spec UP1): a "new version" that passes the
//! `--version` probe but whose tray does not start.
//!
//! `ks-badupdate --version` prints `kaltstellen <KS_FAKE_VERSION>`; any other
//! call ends at once with exit code 3.

fn main() {
    if std::env::args().nth(1).as_deref() == Some("--version") {
        let version = std::env::var("KS_FAKE_VERSION").unwrap_or_default();
        println!("kaltstellen {version}");
        return;
    }
    std::process::exit(3);
}
