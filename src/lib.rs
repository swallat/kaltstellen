//! kaltstellen: switch between app profiles on Windows by suspending,
//! stopping and starting processes.
//!
//! Module layout (implementation plan WP1):
//! - [`process`]: safe wrapper around the Win32 process APIs; the only module
//!   allowed to contain `unsafe` (spec Q4).
//! - [`config`]: TOML configuration and its validation (C1 to C7).
//! - [`engine`]: switch planner and executor (W1 to W6).
//! - [`state`]: state file and recovery (S1 to S10).
//! - [`cli`]: command-line interface (U2, E2).
//! - [`hotkey`]: hotkey syntax (C7).
//! - [`error`], [`logging`], [`paths`]: shared infrastructure.

// Q4: the compiler rejects `unsafe` everywhere except in `process`.
// CI additionally greps for `unsafe` outside `src/process.rs`.
#![deny(unsafe_code)]

pub mod cli;
pub mod config;
pub mod engine;
pub mod error;
pub mod hotkey;
pub mod logging;
pub mod paths;
#[allow(unsafe_code)]
pub mod process;
pub mod protection;
pub mod state;
