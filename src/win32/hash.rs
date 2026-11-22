//! SHA-256 through Windows' CNG (`BCryptHash`), for checking a downloaded
//! update against its published sum (spec UP1). No hashing crate needed.

use windows::Win32::Security::Cryptography::{BCRYPT_SHA256_ALG_HANDLE, BCryptHash};

/// The SHA-256 of `data`.
pub fn sha256(data: &[u8]) -> Result<[u8; 32], String> {
    let mut out = [0u8; 32];
    // SAFETY: the pseudo-handle needs no opening (Windows 10 and later);
    // input and output are live slices whose lengths are passed along.
    let status = unsafe { BCryptHash(BCRYPT_SHA256_ALG_HANDLE, None, data, &mut out) };
    if status.is_ok() {
        Ok(out)
    } else {
        Err(format!("SHA-256 failed (NTSTATUS 0x{:08x})", status.0))
    }
}

/// Lower-case hexadecimal, the form `sha256sum` writes.
pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_sums() {
        // FIPS 180-2 examples.
        assert_eq!(
            hex(&sha256(b"abc").unwrap()),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(
            hex(&sha256(b"").unwrap()),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        let million = vec![b'a'; 1_000_000];
        assert_eq!(
            hex(&sha256(&million).unwrap()),
            "cdc76e5c9914fb9281a1c7e284d73e67f1809a48a497200e046d39ccc7112cd0"
        );
    }
}
