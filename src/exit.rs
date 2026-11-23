//! Exit codes of spec E2, shared by the in-process command line and the
//! answers the tray sends over its pipe.

use crate::error::{Code, Error};

pub const OK: u8 = 0;
/// Finished with collected errors.
pub const ERRORS: u8 = 1;
pub const CONFIG_INVALID: u8 = 2;
/// Superseded by a later request (W3).
pub const SUPERSEDED: u8 = 3;
pub const UNKNOWN_PROFILE: u8 = 4;
/// Tray not reachable, or the in-process lock is held by another switch.
pub const UNREACHABLE: u8 = 5;

/// The exit code of a switch or recovery. `ran` is false when nothing could
/// be attempted (unknown profile, lock held, no process list).
pub fn of(ran: bool, errors: &[Error]) -> u8 {
    if !ran {
        let has = |code| errors.iter().any(|e| e.code == code);
        if has(Code::UnknownProfile) {
            UNKNOWN_PROFILE
        } else if has(Code::StateLocked) || has(Code::TrayUnreachable) {
            UNREACHABLE
        } else {
            ERRORS
        }
    } else if errors.is_empty() {
        OK
    } else {
        ERRORS
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn e2_codes_follow_the_errors() {
        let err = |code| Error::new(code, "x");
        assert_eq!(of(true, &[]), OK);
        assert_eq!(of(true, &[err(Code::SuspendFailed)]), ERRORS);
        assert_eq!(of(false, &[err(Code::UnknownProfile)]), UNKNOWN_PROFILE);
        assert_eq!(of(false, &[err(Code::StateLocked)]), UNREACHABLE);
        assert_eq!(of(false, &[err(Code::Snapshot)]), ERRORS);
    }
}
