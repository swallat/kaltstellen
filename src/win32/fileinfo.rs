//! M2, N1: what an executable is, from local sources only: its version
//! resource and its Authenticode signature (embedded or in a system
//! catalog).
//!
//! Both are read as data. The version resource is read with
//! `GetFileVersionInfoW`, which maps the file as a data file; no code of the
//! file runs. The signature is checked with `WinVerifyTrust` without UI,
//! without revocation checks and with URL retrieval limited to the local
//! cache, so no network access is requested `[assumed]`: Windows may still
//! refresh its root and disallowed certificate lists on its own (automatic
//! root update has no WinTrust flag); manual check 13e verifies that the
//! window and the command line cause no connection. Choosing fields, wording and
//! caching are safe code in [`crate::program_info`].

use std::fs::File;
use std::io::{Seek, SeekFrom};
use std::os::windows::io::AsRawHandle;
use std::path::Path;

use windows::Win32::Foundation::{
    GetLastError, HANDLE, HWND, TRUST_E_NOSIGNATURE, TRUST_E_PROVIDER_UNKNOWN,
    TRUST_E_SUBJECT_FORM_UNKNOWN,
};
use windows::Win32::Security::Cryptography::Catalog::{
    CATALOG_INFO, CryptCATAdminAcquireContext2, CryptCATAdminCalcHashFromFileHandle2,
    CryptCATAdminEnumCatalogFromHash, CryptCATAdminReleaseCatalogContext,
    CryptCATAdminReleaseContext, CryptCATCatalogInfoFromContext,
};
use windows::Win32::Security::Cryptography::{CERT_NAME_SIMPLE_DISPLAY_TYPE, CertGetNameStringW};
use windows::Win32::Security::WinTrust::{
    DRIVER_ACTION_VERIFY, WINTRUST_ACTION_GENERIC_VERIFY_V2, WINTRUST_CATALOG_INFO, WINTRUST_DATA,
    WINTRUST_DATA_0, WINTRUST_DATA_UNION_CHOICE, WINTRUST_FILE_INFO, WTD_CACHE_ONLY_URL_RETRIEVAL,
    WTD_CHOICE_CATALOG, WTD_CHOICE_FILE, WTD_DISABLE_MD2_MD4, WTD_REVOCATION_CHECK_NONE,
    WTD_REVOKE_NONE, WTD_STATEACTION_CLOSE, WTD_STATEACTION_VERIFY, WTD_UI_NONE,
    WTD_UICONTEXT_EXECUTE, WTHelperGetProvCertFromChain, WTHelperGetProvSignerFromChain,
    WTHelperProvDataFromStateData, WinVerifyTrust,
};
use windows::Win32::Storage::FileSystem::{
    GetFileVersionInfoSizeW, GetFileVersionInfoW, VerQueryValueW,
};
use windows::core::{PCWSTR, w};

use super::icon::on_a_local_disk;
use super::{wide, wide_path};

/// Largest version resource read; real ones are a few KiB.
const MAX_VERSION_BLOCK: u32 = 1 << 20;

/// The version resource of a file, as one block. Values are looked up with
/// [`VersionResource::string`].
pub struct VersionResource {
    /// `u32` so the block is aligned as `VerQueryValueW` expects.
    block: Vec<u32>,
}

impl VersionResource {
    /// The block of `path`, or `None` if it has none, cannot be read or is
    /// not on a fixed local disk.
    pub fn read(path: &Path) -> Option<Self> {
        if !on_a_local_disk(path) {
            return None;
        }
        let name = wide_path(path);
        // SAFETY: `name` is NUL-terminated and outlives the call.
        let size = unsafe { GetFileVersionInfoSizeW(PCWSTR(name.as_ptr()), None) };
        if size == 0 || size > MAX_VERSION_BLOCK {
            return None;
        }
        let mut block = vec![0u32; size.div_ceil(4) as usize];
        // SAFETY: `block` is writable for at least `size` bytes.
        unsafe {
            GetFileVersionInfoW(PCWSTR(name.as_ptr()), None, size, block.as_mut_ptr().cast())
        }
        .ok()?;
        Some(Self { block })
    }

    /// The raw value at `sub_block` and its length as `VerQueryValueW`
    /// reports it (bytes for binary values, characters for strings).
    fn query(&self, sub_block: &str) -> Option<(*const u8, usize)> {
        let sub = wide(sub_block);
        let mut value = std::ptr::null_mut();
        let mut len = 0u32;
        // SAFETY: the block came from GetFileVersionInfoW and lives as long
        // as `self`; `sub` is NUL-terminated; both out pointers are valid.
        let found = unsafe {
            VerQueryValueW(
                self.block.as_ptr().cast(),
                PCWSTR(sub.as_ptr()),
                &mut value,
                &mut len,
            )
        };
        (found.as_bool() && !value.is_null()).then_some((value.cast_const().cast(), len as usize))
    }

    /// `\VarFileInfo\Translation`: the language and code page pairs the
    /// file has string tables for.
    pub fn translations(&self) -> Vec<(u16, u16)> {
        self.query("\\VarFileInfo\\Translation")
            .and_then(|(pointer, len)| self.inside(pointer, len))
            .map(|raw| {
                raw.as_chunks::<4>()
                    .0
                    .iter()
                    .map(|c| {
                        (
                            u16::from_le_bytes([c[0], c[1]]),
                            u16::from_le_bytes([c[2], c[3]]),
                        )
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    /// `\StringFileInfo\<table>\<name>`, `table` as `040904B0`. `None` if
    /// missing; the text up to the first NUL otherwise.
    pub fn string(&self, table: &str, name: &str) -> Option<String> {
        let (pointer, chars) = self.query(&format!("\\StringFileInfo\\{table}\\{name}"))?;
        let raw = self.inside(pointer, chars.checked_mul(2)?)?;
        let units: Vec<u16> = raw
            .as_chunks::<2>()
            .0
            .iter()
            .map(|c| u16::from_le_bytes(*c))
            .take_while(|&u| u != 0)
            .collect();
        Some(String::from_utf16_lossy(&units))
    }

    /// The block as bytes.
    fn bytes(&self) -> &[u8] {
        // SAFETY: the `u32`s of the block seen as their bytes, borrowed
        // from `self`.
        unsafe { std::slice::from_raw_parts(self.block.as_ptr().cast(), self.block.len() * 4) }
    }

    /// The `len` bytes at `pointer`, if they lie inside the block: a value
    /// that points elsewhere is not trusted.
    fn inside(&self, pointer: *const u8, len: usize) -> Option<&[u8]> {
        let bytes = self.bytes();
        let offset = (pointer as usize).checked_sub(bytes.as_ptr() as usize)?;
        bytes.get(offset..offset.checked_add(len)?)
    }
}

/// What `WinVerifyTrust` says about a file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Trust {
    /// A valid signature, with the simple display name of the signer's
    /// certificate. `catalog`: the file is signed through a system catalog,
    /// not by a signature of its own.
    Valid {
        signer: Option<String>,
        catalog: bool,
    },
    /// A signature is there but does not verify; the `HRESULT`.
    Invalid { code: i32, catalog: bool },
    /// Neither a signature of its own nor in a catalog.
    Unsigned,
    /// Not checked: not on a fixed local disk, or the file cannot be opened.
    Unreadable,
}

/// Owns the open state of one `WinVerifyTrust` call and closes it.
struct State<'a> {
    data: &'a mut WINTRUST_DATA,
    action: windows::core::GUID,
}

impl State<'_> {
    /// The simple display name of the first signer's certificate.
    fn signer(&self) -> Option<String> {
        // SAFETY: the state handle comes from a verification that has not
        // been closed; every pointer is checked before it is read, and all
        // of them belong to that state, which outlives this call.
        unsafe {
            let provider = WTHelperProvDataFromStateData(self.data.hWVTStateData);
            if provider.is_null() {
                return None;
            }
            let signer = WTHelperGetProvSignerFromChain(provider, 0, false, 0);
            if signer.is_null() {
                return None;
            }
            let cert = WTHelperGetProvCertFromChain(signer, 0);
            if cert.is_null() || (*cert).pCert.is_null() {
                return None;
            }
            let context = (*cert).pCert;
            let len = CertGetNameStringW(context, CERT_NAME_SIMPLE_DISPLAY_TYPE, 0, None, None);
            if len <= 1 {
                return None;
            }
            let mut name = vec![0u16; len as usize];
            let len = CertGetNameStringW(
                context,
                CERT_NAME_SIMPLE_DISPLAY_TYPE,
                0,
                None,
                Some(&mut name),
            );
            (len > 1).then(|| String::from_utf16_lossy(&name[..len as usize - 1]))
        }
    }
}

impl Drop for State<'_> {
    fn drop(&mut self) {
        self.data.dwStateAction = WTD_STATEACTION_CLOSE;
        // SAFETY: closes the state of the verification done with the same
        // data; the structures it points to still live.
        unsafe {
            WinVerifyTrust(
                HWND(std::ptr::null_mut()),
                &mut self.action,
                (&raw mut *self.data).cast(),
            )
        };
    }
}

/// `WINTRUST_DATA` for a check without UI, revocation or network.
fn trust_data(choice: WINTRUST_DATA_UNION_CHOICE, subject: WINTRUST_DATA_0) -> WINTRUST_DATA {
    WINTRUST_DATA {
        cbStruct: std::mem::size_of::<WINTRUST_DATA>() as u32,
        dwUIChoice: WTD_UI_NONE,
        fdwRevocationChecks: WTD_REVOKE_NONE,
        dwUnionChoice: choice,
        Anonymous: subject,
        dwStateAction: WTD_STATEACTION_VERIFY,
        // No revocation, and URLs (certificates, CRLs) only from the cache:
        // no network access is requested (see the module comment).
        dwProvFlags: WTD_CACHE_ONLY_URL_RETRIEVAL | WTD_REVOCATION_CHECK_NONE | WTD_DISABLE_MD2_MD4,
        dwUIContext: WTD_UICONTEXT_EXECUTE,
        ..WINTRUST_DATA::default()
    }
}

/// Runs the verification; the state is closed when the result is dropped.
fn verify_with(data: &mut WINTRUST_DATA) -> (i32, State<'_>) {
    let mut action = WINTRUST_ACTION_GENERIC_VERIFY_V2;
    // SAFETY: `data` and everything it points to are valid for the call;
    // the state it opens is closed by `State`.
    let result = unsafe {
        WinVerifyTrust(
            HWND(std::ptr::null_mut()),
            &mut action,
            (&raw mut *data).cast(),
        )
    };
    (result, State { data, action })
}

/// The signature of `path`: its own (embedded), else a system catalog that
/// lists its hash. Hashes the whole file, which takes time for large
/// files; callers decide when.
pub fn verify(path: &Path) -> Trust {
    if !on_a_local_disk(path) {
        return Trust::Unreadable;
    }
    let Ok(mut file) = File::open(path) else {
        return Trust::Unreadable;
    };
    let name = wide_path(path);
    let handle = HANDLE(file.as_raw_handle());
    let mut info = WINTRUST_FILE_INFO {
        cbStruct: std::mem::size_of::<WINTRUST_FILE_INFO>() as u32,
        pcwszFilePath: PCWSTR(name.as_ptr()),
        hFile: handle,
        pgKnownSubject: std::ptr::null_mut(),
    };
    let mut data = trust_data(WTD_CHOICE_FILE, WINTRUST_DATA_0 { pFile: &mut info });
    let (result, state) = verify_with(&mut data);
    if result == 0 {
        return Trust::Valid {
            signer: state.signer(),
            catalog: false,
        };
    }
    // As Microsoft's sample: "no signature" only when the last error says
    // so too; otherwise a signature is there and could not be verified.
    // SAFETY: plain FFI call.
    let last = unsafe { GetLastError() }.to_hresult();
    let none = [
        TRUST_E_NOSIGNATURE,
        TRUST_E_SUBJECT_FORM_UNKNOWN,
        TRUST_E_PROVIDER_UNKNOWN,
    ];
    let unsigned = (result == TRUST_E_NOSIGNATURE.0 && none.contains(&last))
        || result == TRUST_E_SUBJECT_FORM_UNKNOWN.0
        || result == TRUST_E_PROVIDER_UNKNOWN.0;
    drop(state);
    if !unsigned {
        return Trust::Invalid {
            code: result,
            catalog: false,
        };
    }
    // Windows' own files are mostly signed through catalogs. SHA-256
    // catalogs first; the file is hashed with SHA-1 only when no SHA-256
    // catalog lists it.
    [w!("SHA256"), w!("SHA1")]
        .into_iter()
        .find_map(|algorithm| in_catalog(&name, &mut file, algorithm))
        .unwrap_or(Trust::Unsigned)
}

/// Owns a catalog admin context.
struct Admin(isize);

impl Drop for Admin {
    fn drop(&mut self) {
        // SAFETY: owned and released once.
        let _ = unsafe { CryptCATAdminReleaseContext(self.0, 0) };
    }
}

/// Owns the catalog context the enumeration is at (0: none).
struct Catalog<'a>(&'a Admin, isize);

impl Catalog<'_> {
    /// The next catalog that lists `hash` (the first one at 0). The call
    /// releases the current context itself.
    fn advance(&mut self, hash: &[u8]) {
        let mut previous = std::mem::take(&mut self.1);
        let previous = (previous != 0).then_some(&raw mut previous);
        // SAFETY: the admin context is open; `previous`, if given, is a
        // context of it that is ours, and the call releases it.
        self.1 = unsafe { CryptCATAdminEnumCatalogFromHash(self.0.0, hash, None, previous) };
    }
}

impl Drop for Catalog<'_> {
    fn drop(&mut self) {
        if self.1 != 0 {
            // SAFETY: owned and released once, before its admin context.
            let _ = unsafe { CryptCATAdminReleaseCatalogContext(self.0.0, self.1, 0) };
        }
    }
}

/// The catalog verdict for the file, if a catalog lists its `algorithm`
/// hash: valid if one of the catalogs that list it verifies, else the
/// result of the first. `name` is the NUL-terminated path of `file`.
fn in_catalog(name: &[u16], file: &mut File, algorithm: PCWSTR) -> Option<Trust> {
    // Each hash starts at the beginning of the file.
    file.seek(SeekFrom::Start(0)).ok()?;
    let handle = HANDLE(file.as_raw_handle());
    let mut admin = 0isize;
    // SAFETY: valid out pointer; `algorithm` is a static string.
    unsafe {
        CryptCATAdminAcquireContext2(
            &mut admin,
            Some(&DRIVER_ACTION_VERIFY),
            algorithm,
            None,
            None,
        )
    }
    .ok()?;
    let admin = Admin(admin);
    // Large enough for SHA-512.
    let mut hash = [0u8; 64];
    let mut len = hash.len() as u32;
    // SAFETY: `hash` is writable for `len` bytes; the handle is open.
    unsafe {
        CryptCATAdminCalcHashFromFileHandle2(
            admin.0,
            handle,
            &mut len,
            Some(hash.as_mut_ptr()),
            None,
        )
    }
    .ok()?;
    let hash = &mut hash[..(len as usize).min(64)];
    let tag = wide(&hash.iter().map(|b| format!("{b:02X}")).collect::<String>());
    let mut catalog = Catalog(&admin, 0);
    let mut first_failure = None;
    loop {
        catalog.advance(hash);
        if catalog.1 == 0 {
            return first_failure;
        }
        let mut catalog_info = CATALOG_INFO {
            cbStruct: std::mem::size_of::<CATALOG_INFO>() as u32,
            ..CATALOG_INFO::default()
        };
        // SAFETY: a catalog context of this admin context; valid out
        // pointer.
        if unsafe { CryptCATCatalogInfoFromContext(catalog.1, &mut catalog_info, 0) }.is_err() {
            continue;
        }
        let mut info = WINTRUST_CATALOG_INFO {
            cbStruct: std::mem::size_of::<WINTRUST_CATALOG_INFO>() as u32,
            dwCatalogVersion: 0,
            pcwszCatalogFilePath: PCWSTR(catalog_info.wszCatalogFile.as_ptr()),
            pcwszMemberTag: PCWSTR(tag.as_ptr()),
            pcwszMemberFilePath: PCWSTR(name.as_ptr()),
            hMemberFile: handle,
            pbCalculatedFileHash: hash.as_mut_ptr(),
            cbCalculatedFileHash: hash.len() as u32,
            pcCatalogContext: std::ptr::null_mut(),
            hCatAdmin: admin.0,
        };
        let mut data = trust_data(
            WTD_CHOICE_CATALOG,
            WINTRUST_DATA_0 {
                pCatalog: &mut info,
            },
        );
        let (result, state) = verify_with(&mut data);
        if result == 0 {
            return Some(Trust::Valid {
                signer: state.signer(),
                catalog: true,
            });
        }
        first_failure.get_or_insert(Trust::Invalid {
            code: result,
            catalog: true,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn system32(name: &str) -> std::path::PathBuf {
        let windir = std::env::var_os("SystemRoot").expect("SystemRoot");
        Path::new(&windir).join("System32").join(name)
    }

    #[test]
    fn m2_version_resource_of_a_system_file() {
        let block = VersionResource::read(&system32("kernel32.dll")).expect("kernel32 has one");
        let translations = block.translations();
        assert!(!translations.is_empty());
        let (lang, cp) = translations[0];
        let table = format!("{lang:04X}{cp:04X}");
        let company = block.string(&table, "CompanyName").unwrap();
        assert!(company.contains("Microsoft"), "{company}");
        assert!(block.string(&table, "FileVersion").is_some());
        assert_eq!(block.string(&table, "NoSuchValue"), None);
        assert_eq!(block.string("DEADBEEF", "CompanyName"), None);
        // Nothing for files without one, missing files and other places.
        let dir = tempfile::tempdir().unwrap();
        let junk = dir.path().join("junk.exe");
        std::fs::write(&junk, b"MZ not really a program").unwrap();
        assert!(VersionResource::read(&junk).is_none());
        assert!(VersionResource::read(Path::new(r"C:\no\such\program.exe")).is_none());
        assert!(VersionResource::read(Path::new(r"\\server\share\a.exe")).is_none());
    }

    #[test]
    fn m2_catalog_signed_system_files_are_valid_and_junk_is_unsigned() {
        for name in ["notepad.exe", "kernel32.dll"] {
            match verify(&system32(name)) {
                Trust::Valid { signer, catalog } => {
                    let signer = signer.unwrap();
                    assert!(signer.starts_with("Microsoft"), "{name}: {signer}");
                    // Windows' own files are catalog-signed on the runners
                    // too; either way it must be valid.
                    let _ = catalog;
                }
                other => panic!("{name}: {other:?}"),
            }
        }
        let dir = tempfile::tempdir().unwrap();
        let junk = dir.path().join("junk.exe");
        std::fs::write(&junk, b"MZ not really a program").unwrap();
        assert_eq!(verify(&junk), Trust::Unsigned);
        assert_eq!(
            verify(Path::new(r"C:\no\such\program.exe")),
            Trust::Unreadable
        );
        assert_eq!(
            verify(Path::new(r"\\server\share\a.exe")),
            Trust::Unreadable
        );
    }
}
