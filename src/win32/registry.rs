//! String values under `HKEY_CURRENT_USER`, for start at sign-in (the
//! per-user Run key). Nothing outside `HKEY_CURRENT_USER` is touched.

use windows::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_NO_MORE_ITEMS, WIN32_ERROR};
use windows::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, KEY_ENUMERATE_SUB_KEYS, KEY_QUERY_VALUE, KEY_SET_VALUE,
    REG_OPTION_NON_VOLATILE, REG_SZ, REG_VALUE_TYPE, RegCloseKey, RegCreateKeyExW, RegDeleteTreeW,
    RegDeleteValueW, RegEnumKeyExW, RegOpenKeyExW, RegQueryValueExW, RegSetValueExW,
};
use windows::core::PCWSTR;

use super::wide;
use crate::error::{Code, Error, Result};

fn failed(what: &str, key: &str, status: WIN32_ERROR) -> Error {
    Error::new(
        Code::AutostartIo,
        format!(
            "cannot {what} HKCU\\{key}: {}",
            windows::core::Error::from_hresult(status.to_hresult()).message()
        ),
    )
}

struct Key(HKEY);

impl Drop for Key {
    fn drop(&mut self) {
        // SAFETY: opened by this module, closed once.
        let _ = unsafe { RegCloseKey(self.0) };
    }
}

fn open(subkey: &str, write: bool) -> Result<Option<Key>> {
    let name = wide(subkey);
    let mut key = HKEY::default();
    // SAFETY: NUL-terminated name, valid out pointer.
    let status = unsafe {
        if write {
            RegCreateKeyExW(
                HKEY_CURRENT_USER,
                PCWSTR(name.as_ptr()),
                None,
                PCWSTR::null(),
                REG_OPTION_NON_VOLATILE,
                KEY_SET_VALUE | KEY_QUERY_VALUE,
                None,
                &mut key,
                None,
            )
        } else {
            RegOpenKeyExW(
                HKEY_CURRENT_USER,
                PCWSTR(name.as_ptr()),
                None,
                // Listing subkeys (`subkeys`) needs a right of its own;
                // without it RegEnumKeyExW is refused (N1 icon registry).
                KEY_QUERY_VALUE | KEY_ENUMERATE_SUB_KEYS,
                &mut key,
            )
        }
    };
    if status == ERROR_FILE_NOT_FOUND && !write {
        return Ok(None);
    }
    if status.is_err() {
        return Err(failed("open", subkey, status));
    }
    Ok(Some(Key(key)))
}

/// The string `value` of `HKCU\subkey`; `None` if the key or value is
/// missing.
pub fn read(subkey: &str, value: &str) -> Result<Option<String>> {
    let Some(key) = open(subkey, false)? else {
        return Ok(None);
    };
    let name = wide(value);
    let mut kind = REG_VALUE_TYPE::default();
    let mut size = 0u32;
    // SAFETY: size query with valid out pointers.
    let status = unsafe {
        RegQueryValueExW(
            key.0,
            PCWSTR(name.as_ptr()),
            None,
            Some(&mut kind),
            None,
            Some(&mut size),
        )
    };
    if status == ERROR_FILE_NOT_FOUND {
        return Ok(None);
    }
    if status.is_err() {
        return Err(failed("read", subkey, status));
    }
    let mut data = vec![0u16; (size as usize).div_ceil(2) + 1];
    let mut bytes = (data.len() * 2) as u32;
    // SAFETY: the buffer holds `bytes` bytes.
    let status = unsafe {
        RegQueryValueExW(
            key.0,
            PCWSTR(name.as_ptr()),
            None,
            Some(&mut kind),
            Some(data.as_mut_ptr().cast::<u8>()),
            Some(&mut bytes),
        )
    };
    if status.is_err() {
        return Err(failed("read", subkey, status));
    }
    if kind != REG_SZ {
        return Ok(Some(String::new()));
    }
    let units = &data[..(bytes as usize / 2).min(data.len())];
    let end = units.iter().position(|&u| u == 0).unwrap_or(units.len());
    Ok(Some(String::from_utf16_lossy(&units[..end])))
}

/// Sets the string `value` of `HKCU\subkey`, creating the key if needed.
pub fn write(subkey: &str, value: &str, data: &str) -> Result<()> {
    let key = open(subkey, true)?.expect("created");
    let name = wide(value);
    let bytes: Vec<u8> = wide(data).iter().flat_map(|u| u.to_le_bytes()).collect();
    // SAFETY: NUL-terminated name; data with its terminating NUL.
    let status =
        unsafe { RegSetValueExW(key.0, PCWSTR(name.as_ptr()), None, REG_SZ, Some(&bytes)) };
    if status.is_err() {
        return Err(failed("write", subkey, status));
    }
    Ok(())
}

/// Removes `value` from `HKCU\subkey`; true if it was there.
pub fn delete(subkey: &str, value: &str) -> Result<bool> {
    let Some(key) = open(subkey, false)? else {
        return Ok(false);
    };
    // Reopen with write access only when there is something to delete.
    drop(key);
    let key = open(subkey, true)?.expect("created");
    let name = wide(value);
    // SAFETY: NUL-terminated name.
    let status = unsafe { RegDeleteValueW(key.0, PCWSTR(name.as_ptr())) };
    if status == ERROR_FILE_NOT_FOUND {
        return Ok(false);
    }
    if status.is_err() {
        return Err(failed("delete a value of", subkey, status));
    }
    Ok(true)
}

/// Names of the keys directly below `HKCU\subkey`; empty if it is missing.
pub fn subkeys(subkey: &str) -> Result<Vec<String>> {
    let Some(key) = open(subkey, false)? else {
        return Ok(Vec::new());
    };
    let mut names = Vec::new();
    for index in 0.. {
        let mut buffer = [0u16; 256];
        let mut len = buffer.len() as u32;
        // SAFETY: the buffer holds `len` units.
        let status = unsafe {
            RegEnumKeyExW(
                key.0,
                index,
                Some(windows::core::PWSTR(buffer.as_mut_ptr())),
                &mut len,
                None,
                None,
                None,
                None,
            )
        };
        if status == ERROR_NO_MORE_ITEMS {
            break;
        }
        if status.is_err() {
            // Not the end of the list: say so instead of returning a
            // shorter list that looks complete.
            return Err(failed("list", subkey, status));
        }
        names.push(String::from_utf16_lossy(&buffer[..len as usize]));
    }
    Ok(names)
}

/// Deletes `HKCU\subkey` with everything below it; for test keys only.
pub fn delete_tree(subkey: &str) -> Result<()> {
    let name = wide(subkey);
    // SAFETY: NUL-terminated name.
    let status = unsafe { RegDeleteTreeW(HKEY_CURRENT_USER, PCWSTR(name.as_ptr())) };
    if status.is_err() && status != ERROR_FILE_NOT_FOUND {
        return Err(failed("delete", subkey, status));
    }
    Ok(())
}

/// Whether `HKCU\subkey` exists.
pub fn exists(subkey: &str) -> Result<bool> {
    Ok(open(subkey, false)?.is_some())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn values_round_trip_in_a_test_key_that_is_removed_again() {
        let root = format!("Software\\kaltstellen-test-{}-registry", std::process::id());
        let key = format!("{root}\\Run");
        assert_eq!(read(&key, "v").unwrap(), None);
        assert!(!delete(&key, "v").unwrap());
        write(&key, "v", "\"C:\\x y\\k.exe\" tray").unwrap();
        assert_eq!(
            read(&key, "v").unwrap().as_deref(),
            Some("\"C:\\x y\\k.exe\" tray")
        );
        assert!(delete(&key, "v").unwrap());
        assert_eq!(read(&key, "v").unwrap(), None);
        delete_tree(&root).unwrap();
        assert!(!exists(&root).unwrap(), "nothing left behind");
    }

    /// N1 reads `NotifyIconSettings` this way: the subkeys must be listed,
    /// not silently come back empty (a key opened without the right to
    /// enumerate did exactly that).
    #[test]
    fn n1_subkeys_are_listed() {
        let root = format!("Software\\kaltstellen-test-{}-subkeys", std::process::id());
        assert_eq!(subkeys(&root).unwrap(), Vec::<String>::new());
        for name in ["1001", "1002", "1003"] {
            write(&format!("{root}\\{name}"), "ExecutablePath", "C:\\x\\a.exe").unwrap();
        }
        let mut names = subkeys(&root).unwrap();
        names.sort();
        assert_eq!(names, ["1001", "1002", "1003"]);
        assert_eq!(
            read(&format!("{root}\\1002"), "ExecutablePath")
                .unwrap()
                .as_deref(),
            Some("C:\\x\\a.exe")
        );
        delete_tree(&root).unwrap();
        assert!(!exists(&root).unwrap(), "nothing left behind");
    }
}
