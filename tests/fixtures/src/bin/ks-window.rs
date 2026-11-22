//! Windowed fixture:
//! `ks-window [--title <t>] [--heartbeat <file>] [--close-delay-ms <n>] [--alloc-mb <n>]`.
//!
//! Closes on `WM_CLOSE`, optionally after a delay (slow-stop scenario for W3).

use ks_fixtures::window::{self, OnClose};
use ks_fixtures::{Args, common_setup};

fn main() {
    let args = Args::from_env();
    common_setup(&args);
    let on_close = match args.number("--close-delay-ms") {
        Some(ms) => OnClose::Delay(ms.try_into().expect("delay too large")),
        None => OnClose::Close,
    };
    window::run(args.value("--title").unwrap_or("ks-window"), on_close);
}
