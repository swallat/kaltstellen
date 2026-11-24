//! `kaltstellen helper status` without an installed helper (M4a): portable
//! use. It must report that cleanly and exit with 0. Nothing here installs,
//! starts or touches a service.

use std::process::Command;

const EXE: &str = env!("CARGO_BIN_EXE_kaltstellen");

#[test]
fn status_without_a_helper_says_so_and_succeeds() {
    let dir = tempfile::tempdir().unwrap();
    let output = Command::new(EXE)
        .args(["helper", "status"])
        .env("KALTSTELLEN_CONFIG_DIR", dir.path().join("config"))
        .env("KALTSTELLEN_DATA_DIR", dir.path().join("data"))
        .env("KALTSTELLEN_HELPER_PROGRAM_DIR", dir.path().join("program"))
        .env(
            "KALTSTELLEN_HELPER_DATA_DIR",
            dir.path().join("helper-data"),
        )
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "{text} {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(text.contains("not installed"), "{text}");
}

#[test]
fn run_outside_the_service_manager_fails_at_once() {
    // `helper run` outside the service control manager must fail at once
    // instead of serving anything.
    let dir = tempfile::tempdir().unwrap();
    let output = Command::new(EXE)
        .args(["helper", "run"])
        .env("KALTSTELLEN_HELPER_PROGRAM_DIR", dir.path().join("program"))
        .env(
            "KALTSTELLEN_HELPER_DATA_DIR",
            dir.path().join("helper-data"),
        )
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("KS1001"),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}
