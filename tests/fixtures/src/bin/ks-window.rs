//! Windowed fixture: `ks-window [--title <t>] [--heartbeat <file>]
//! [--close-delay-ms <n> | --close-when <file>] [--alloc-mb <n>]`.
//!
//! Closes on `WM_CLOSE`, optionally after a delay, or once `<file>` exists
//! (slow-stop scenarios for W3; the file lets a test end the stop exactly
//! when it wants, at the latest after 60 s; `<file>.closing` is written
//! when `WM_CLOSE` arrives, so the test knows the stop has begun).

use ks_fixtures::window::{self, CLOSE_WHEN, OnClose};
use ks_fixtures::{Args, common_setup};

fn main() {
    let args = Args::from_env();
    common_setup(&args);
    let on_close = match (args.number("--close-delay-ms"), args.value("--close-when")) {
        (_, Some(file)) => {
            CLOSE_WHEN.set(file.into()).expect("set once");
            OnClose::WhenFile
        }
        (Some(ms), None) => OnClose::Delay(ms.try_into().expect("delay too large")),
        (None, None) => OnClose::Close,
    };
    window::run(args.value("--title").unwrap_or("ks-window"), on_close);
}
