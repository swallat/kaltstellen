//! Safe wrappers around the Win32 user-interface APIs the tray app needs.
//!
//! Together with [`crate::process`] this tree is the only place allowed to
//! contain `unsafe` (spec Q4 as widened in WP7). Each module wraps one
//! concern and exposes safe functions only; all decisions (what the menu
//! shows, which icon, what a click does) are made by safe code in
//! [`crate::tray`].

pub mod console;
pub mod dialog;
pub mod instance;
pub mod pipe;
pub mod registry;
pub mod tray;
pub mod webview;

/// UTF-16, NUL-terminated, for `PCWSTR` arguments.
fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}
