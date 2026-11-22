//! Unsaved-changes fixture: `ks-dialog [--title <t>] [--heartbeat <file>]`.
//!
//! On `WM_CLOSE` it shows a Yes/No "unsaved changes" box and stays open, so
//! a graceful stop times out (A2, test T11).

use ks_fixtures::window::{self, OnClose};
use ks_fixtures::{Args, common_setup};

fn main() {
    let args = Args::from_env();
    common_setup(&args);
    window::run(
        args.value("--title").unwrap_or("ks-dialog"),
        OnClose::Confirm,
    );
}
