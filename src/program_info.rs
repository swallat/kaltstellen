//! M2, N1: what a program is, from local sources only (owner request
//! 2026-11-23): the description, product, company and version from its
//! version resource, and the publisher from its Authenticode signature if
//! that is valid. No network, no online database. The Win32 calls are in
//! [`crate::win32::fileinfo`]; this module chooses the fields, words the
//! verdicts, keeps a bounded cache for the main window and formats the
//! command-line text.

use std::collections::HashMap;
use std::path::Path;
use std::time::SystemTime;

use serde::Serialize;

use crate::win32::fileinfo::{Trust, VersionResource};

/// `[assumed]` Files larger than this are not hashed for the signature:
/// checking 1 GiB takes about 2 s on a fast SSD (measured about 0.4 s for
/// 238 MiB and 0.5 to 1.5 s for 369 MiB).
pub const MAX_VERIFY_BYTES: u64 = 1 << 30;
/// Longest text kept from a version resource or certificate.
const MAX_TEXT: usize = 200;
/// `[assumed]` Files the main window keeps information about; beyond that
/// its cache starts over.
pub const CACHE_ENTRIES: usize = 1024;

/// Strings of a version resource, by table (`040904B0`) and name.
pub trait Strings {
    fn translations(&self) -> Vec<(u16, u16)>;
    fn string(&self, table: &str, name: &str) -> Option<String>;
}

impl Strings for VersionResource {
    fn translations(&self) -> Vec<(u16, u16)> {
        VersionResource::translations(self)
    }
    fn string(&self, table: &str, name: &str) -> Option<String> {
        VersionResource::string(self, table, name)
    }
}

/// What the version resource says. The file can claim anything here; only
/// the signature's publisher is verified.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Version {
    pub description: Option<String>,
    pub product: Option<String>,
    pub company: Option<String>,
    pub version: Option<String>,
}

impl Version {
    fn is_empty(&self) -> bool {
        self.description.is_none()
            && self.product.is_none()
            && self.company.is_none()
            && self.version.is_none()
    }
}

/// The string tables to look in, in order: those the translation table
/// names, then U.S. English with Unicode, with Windows-1252 and without a
/// code page, then language neutral (many files have no translation table
/// or a wrong one).
pub fn tables(translations: &[(u16, u16)]) -> Vec<String> {
    let mut tables: Vec<String> = translations
        .iter()
        .map(|(lang, cp)| format!("{lang:04X}{cp:04X}"))
        .collect();
    for fallback in ["040904B0", "040904E4", "04090000", "000004B0"] {
        if !tables.iter().any(|t| t == fallback) {
            tables.push(fallback.to_owned());
        }
    }
    tables
}

/// The four fields, each from the first table that has it non-empty
/// `[assumed]`. `None` when there is none of them.
pub fn version(strings: &impl Strings) -> Option<Version> {
    let tables = tables(&strings.translations());
    let field = |name: &str| {
        tables
            .iter()
            .find_map(|t| strings.string(t, name).and_then(|s| clean(&s)))
    };
    let version = Version {
        description: field("FileDescription"),
        product: field("ProductName"),
        company: field("CompanyName"),
        version: field("FileVersion"),
    };
    (!version.is_empty()).then_some(version)
}

/// Text from a file for the page and the terminal: control characters
/// (escape sequences) and bidirectional overrides (which could turn text
/// around) removed, whitespace collapsed, at most [`MAX_TEXT`] characters.
/// `None` when nothing is left.
pub fn clean(text: &str) -> Option<String> {
    let bidi = |c: char| {
        matches!(
            c,
            '\u{200E}' | '\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}'
        )
    };
    let words: Vec<&str> = text
        .split(|c: char| c.is_whitespace() || c.is_control())
        .filter(|w| !w.is_empty())
        .collect();
    let joined: String = words.join(" ").chars().filter(|&c| !bidi(c)).collect();
    let joined = joined.trim();
    if joined.is_empty() {
        return None;
    }
    if joined.chars().count() > MAX_TEXT {
        let cut: String = joined.chars().take(MAX_TEXT - 1).collect();
        return Some(format!("{}…", cut.trim_end()));
    }
    Some(joined.to_owned())
}

/// The signature as shown.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Signature {
    /// Valid: the publisher is the signer certificate's name. `catalog`:
    /// signed through a Windows catalog, not by a signature of its own.
    Valid {
        publisher: Option<String>,
        catalog: bool,
    },
    /// A signature is there but does not verify.
    Invalid {
        reason: String,
    },
    Unsigned,
    /// Not checked, and why.
    Unknown(String),
}

impl Signature {
    pub fn from_trust(trust: Trust) -> Self {
        match trust {
            Trust::Valid { signer, catalog } => Signature::Valid {
                publisher: signer.and_then(|s| clean(&s)),
                catalog,
            },
            Trust::Invalid { code, .. } => Signature::Invalid {
                reason: reason(code),
            },
            Trust::Unsigned => Signature::Unsigned,
            Trust::Unreadable => Signature::Unknown("the file cannot be read".to_owned()),
        }
    }

    /// `valid`, `invalid`, `unsigned` or `unknown`.
    pub fn kind(&self) -> &'static str {
        match self {
            Signature::Valid { .. } => "valid",
            Signature::Invalid { .. } => "invalid",
            Signature::Unsigned => "unsigned",
            Signature::Unknown(_) => "unknown",
        }
    }
}

/// Why `WinVerifyTrust` refused a signature, in a few words.
pub fn reason(code: i32) -> String {
    let text = match code as u32 {
        0x8009_6010 => "the file was changed after signing",
        0x800B_0100 => "the signature cannot be read",
        0x800B_0109 => "its root certificate is not trusted here",
        0x800B_010A => "its certificate chain cannot be built from local data",
        0x800B_0101 => "its certificate has expired",
        0x800B_010C => "its certificate was revoked",
        0x800B_010D => "it is signed with a test certificate",
        0x800B_0110 => "its certificate is not for code signing",
        0x800B_0111 => "its certificate is marked as not trusted",
        0x800B_0004 => "it is not trusted",
        0x8009_6004 => "its certificate's signature is not valid",
        0x8009_6005 => "its time stamp is not valid",
        0x8009_2026 => "the security settings of this computer block it",
        other => return format!("error 0x{other:08X}"),
    };
    text.to_owned()
}

/// Everything known about one file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Info {
    pub version: Option<Version>,
    pub signature: Signature,
}

/// The signature of a file of `len` bytes, unless it is too large.
pub fn signature_of(path: &Path, len: u64) -> Signature {
    if len > MAX_VERIFY_BYTES {
        return Signature::Unknown(format!(
            "not checked: larger than {} MiB",
            MAX_VERIFY_BYTES >> 20
        ));
    }
    Signature::from_trust(crate::win32::fileinfo::verify(path))
}

/// Reads both, for the command line. `None` for a file that is not on a
/// fixed local disk (nothing is read from shares) or cannot be found.
pub fn read(path: &Path) -> Option<Info> {
    let len = file_stamp(path)?.len;
    Some(Info {
        version: VersionResource::read(path).and_then(|v| version(&v)),
        signature: signature_of(path, len),
    })
}

/// Reads each of `paths` once, on a few threads, since signature checks
/// hash whole files. By lowercase path; files that are not on a fixed local
/// disk or not found are missing.
pub fn read_many<'a>(paths: impl IntoIterator<Item = &'a Path>) -> HashMap<String, Info> {
    let mut seen = std::collections::HashSet::new();
    let unique: Vec<&Path> = paths.into_iter().filter(|p| seen.insert(key(p))).collect();
    let threads = std::thread::available_parallelism()
        .map_or(2, usize::from)
        .clamp(1, 8)
        .min(unique.len().max(1));
    let next = std::sync::atomic::AtomicUsize::new(0);
    let found = std::sync::Mutex::new(HashMap::new());
    std::thread::scope(|scope| {
        for _ in 0..threads {
            scope.spawn(|| {
                loop {
                    let i = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    let Some(path) = unique.get(i) else { break };
                    if let Some(info) = read(path) {
                        found
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner)
                            .insert(key(path), info);
                    }
                }
            });
        }
    });
    found
        .into_inner()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// The text for `path` from [`read_many`], if any.
pub fn summary_of(found: &HashMap<String, Info>, path: &Path) -> Option<String> {
    found.get(&key(path)).and_then(summary)
}

/// Size and time of a file: a changed file is read again.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Stamp {
    pub len: u64,
    pub modified: Option<SystemTime>,
}

/// The stamp of `path`, only for files on a fixed local disk; the check
/// comes first, so a share is never asked.
pub fn file_stamp(path: &Path) -> Option<Stamp> {
    if !crate::win32::icon::on_a_local_disk(path) {
        return None;
    }
    let meta = std::fs::metadata(path).ok().filter(|m| m.is_file())?;
    Some(Stamp {
        len: meta.len(),
        modified: meta.modified().ok(),
    })
}

/// One line for the command line: `Notepad, signed by Microsoft Windows`,
/// `Tool (Acme), unsigned`. The company (which the file claims) is named
/// only when no valid signature names a publisher. `None` when nothing is
/// known.
pub fn summary(info: &Info) -> Option<String> {
    let version = info.version.clone().unwrap_or_default();
    let mut text = version
        .description
        .clone()
        .or_else(|| version.product.clone())
        .unwrap_or_default();
    let valid = matches!(info.signature, Signature::Valid { .. });
    if !valid && let Some(company) = &version.company {
        if text.is_empty() {
            text = company.clone();
        } else if !text.contains(company.as_str()) {
            text = format!("{text} ({company})");
        }
    }
    let mark = match &info.signature {
        Signature::Valid {
            publisher: Some(p), ..
        } => format!("signed by {p}"),
        Signature::Valid {
            publisher: None, ..
        } => "signed".to_owned(),
        Signature::Invalid { reason } => format!("signature not valid: {reason}"),
        Signature::Unsigned => "unsigned".to_owned(),
        // Unreadable or too large: nothing worth a word on every line.
        Signature::Unknown(_) => String::new(),
    };
    match (text.is_empty(), mark.is_empty()) {
        (true, true) => None,
        (true, false) => Some(mark),
        (false, true) => Some(text),
        (false, false) => Some(format!("{text}, {mark}")),
    }
}

/// What the page gets for one program (`program_info`, docs/gui-bridge.md).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct View {
    pub description: Option<String>,
    pub product: Option<String>,
    pub company: Option<String>,
    pub version: Option<String>,
    /// `valid`, `invalid`, `unsigned`, `unknown`, or `pending` while it is
    /// being checked.
    pub signature: &'static str,
    /// Only for a valid signature.
    pub publisher: Option<String>,
    /// Valid through a Windows catalog.
    pub catalog: bool,
    /// Why the signature is not valid or was not checked.
    pub problem: Option<String>,
    /// Something is still being read; the host sends the `program_info`
    /// event when there is more.
    pub pending: bool,
}

/// What the main window knows about one file.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Entry {
    pub stamp: Option<Stamp>,
    /// `None`: not read yet; `Some(None)`: the file has none.
    pub version: Option<Option<Version>>,
    pub signature: Option<Signature>,
    /// Handed to the reader and not finished yet.
    pub queued: bool,
}

impl Entry {
    pub fn view(&self) -> View {
        let version = self.version.clone().flatten().unwrap_or_default();
        let mut view = View {
            description: version.description,
            product: version.product,
            company: version.company,
            version: version.version,
            signature: "pending",
            pending: self.version.is_none() || self.signature.is_none(),
            ..View::default()
        };
        if let Some(signature) = &self.signature {
            view.signature = signature.kind();
            match signature {
                Signature::Valid { publisher, catalog } => {
                    view.publisher = publisher.clone();
                    view.catalog = *catalog;
                }
                Signature::Invalid { reason } => view.problem = Some(reason.clone()),
                Signature::Unknown(why) => view.problem = Some(why.clone()),
                Signature::Unsigned => {}
            }
        }
        view
    }
}

/// The main window's information by file, bounded by [`CACHE_ENTRIES`].
/// Shared between the window thread (lookups) and the reader thread.
#[derive(Debug, Default)]
pub struct Cache {
    entries: HashMap<String, Entry>,
}

fn key(path: &Path) -> String {
    path.to_string_lossy().to_lowercase()
}

impl Cache {
    /// The entry of `path` for a file with `stamp`. A file that changed
    /// starts afresh. `true` in the second place: it must be handed to the
    /// reader (it was not, or it changed).
    pub fn lookup(&mut self, path: &Path, stamp: Stamp) -> (View, bool) {
        let key = key(path);
        let fresh = self
            .entries
            .get(&key)
            .is_some_and(|e| e.stamp == Some(stamp));
        if !fresh {
            if self.entries.len() >= CACHE_ENTRIES {
                // Only finished entries go; the reader still fills the rest.
                self.entries.retain(|_, e| e.queued);
            }
            self.entries.insert(
                key.clone(),
                Entry {
                    stamp: Some(stamp),
                    ..Entry::default()
                },
            );
        }
        let entry = self.entries.get_mut(&key).expect("just inserted");
        let wanted = entry.view().pending && !entry.queued;
        if wanted {
            entry.queued = true;
        }
        (entry.view(), wanted)
    }

    /// The reader: what is still missing for `path`, `None` if the entry
    /// is gone or changed meanwhile.
    pub fn missing(&self, path: &Path, stamp: Stamp) -> Option<(bool, bool)> {
        let entry = self
            .entries
            .get(&key(path))
            .filter(|e| e.stamp == Some(stamp))?;
        Some((entry.version.is_none(), entry.signature.is_none()))
    }

    pub fn set_version(&mut self, path: &Path, stamp: Stamp, version: Option<Version>) {
        if let Some(entry) = self.entry_mut(path, stamp) {
            entry.version = Some(version);
        }
    }

    pub fn set_signature(&mut self, path: &Path, stamp: Stamp, signature: Signature) {
        if let Some(entry) = self.entry_mut(path, stamp) {
            entry.signature = Some(signature);
            entry.queued = false;
        }
    }

    /// The reader gave up on `path` (the window is closing).
    pub fn unqueue(&mut self, path: &Path) {
        if let Some(entry) = self.entries.get_mut(&key(path)) {
            entry.queued = false;
        }
    }

    fn entry_mut(&mut self, path: &Path, stamp: Stamp) -> Option<&mut Entry> {
        self.entries
            .get_mut(&key(path))
            .filter(|e| e.stamp == Some(stamp))
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    /// A version resource made of tables.
    struct Fake {
        translations: Vec<(u16, u16)>,
        values: Vec<(&'static str, &'static str, &'static str)>,
    }

    impl Strings for Fake {
        fn translations(&self) -> Vec<(u16, u16)> {
            self.translations.clone()
        }
        fn string(&self, table: &str, name: &str) -> Option<String> {
            self.values
                .iter()
                .find(|(t, n, _)| *t == table && *n == name)
                .map(|(_, _, v)| (*v).to_owned())
        }
    }

    #[test]
    fn m2_tables_follow_the_translation_table_then_the_fallbacks() {
        assert_eq!(
            tables(&[(0x0407, 0x04B0), (0x0409, 0x04B0)]),
            ["040704B0", "040904B0", "040904E4", "04090000", "000004B0"]
        );
        assert_eq!(
            tables(&[]),
            ["040904B0", "040904E4", "04090000", "000004B0"]
        );
    }

    #[test]
    fn m2_each_field_from_the_first_table_that_has_it() {
        let fake = Fake {
            translations: vec![(0x0407, 0x04B0)],
            values: vec![
                ("040704B0", "FileDescription", "Editor"),
                ("040704B0", "CompanyName", "  "),
                ("040904E4", "CompanyName", "Acme\u{1b}[31m Inc."),
                ("040904E4", "ProductName", "Acme Suite"),
                ("000004B0", "FileVersion", "1.2.3.4"),
            ],
        };
        assert_eq!(
            version(&fake),
            Some(Version {
                description: Some("Editor".into()),
                product: Some("Acme Suite".into()),
                company: Some("Acme [31m Inc.".into()),
                version: Some("1.2.3.4".into()),
            })
        );
        // A wrong translation table: the fallbacks still find the strings.
        let fake = Fake {
            translations: vec![(0x0000, 0x0000)],
            values: vec![("040904B0", "FileDescription", "Tool")],
        };
        assert_eq!(version(&fake).unwrap().description.as_deref(), Some("Tool"));
        let empty = Fake {
            translations: vec![],
            values: vec![("040904B0", "FileDescription", " \t ")],
        };
        assert_eq!(version(&empty), None);
    }

    #[test]
    fn m2_text_from_files_is_cleaned_for_page_and_terminal() {
        assert_eq!(clean("  a\r\n b\tc  "), Some("a b c".into()));
        assert_eq!(clean("\u{1b}]0;title\u{7}x"), Some("]0;title x".into()));
        assert_eq!(clean("evil\u{202E}txt.exe"), Some("eviltxt.exe".into()));
        assert_eq!(clean("\u{0}\u{9b}"), None);
        let long = "x".repeat(500);
        let cut = clean(&long).unwrap();
        assert_eq!(cut.chars().count(), MAX_TEXT);
        assert!(cut.ends_with('…'));
    }

    #[test]
    fn m2_verdicts_and_reasons() {
        assert_eq!(
            Signature::from_trust(Trust::Valid {
                signer: Some(" Microsoft Windows ".into()),
                catalog: true
            }),
            Signature::Valid {
                publisher: Some("Microsoft Windows".into()),
                catalog: true
            }
        );
        let bad = Signature::from_trust(Trust::Invalid {
            code: 0x8009_6010_u32 as i32,
            catalog: false,
        });
        assert_eq!(
            bad,
            Signature::Invalid {
                reason: "the file was changed after signing".into()
            }
        );
        assert_eq!(bad.kind(), "invalid");
        assert_eq!(reason(0x8000_4005_u32 as i32), "error 0x80004005");
        assert_eq!(Signature::from_trust(Trust::Unsigned).kind(), "unsigned");
        assert_eq!(Signature::from_trust(Trust::Unreadable).kind(), "unknown");
    }

    fn info(version: Option<Version>, signature: Signature) -> Info {
        Info { version, signature }
    }

    #[test]
    fn m2_summary_names_the_publisher_only_when_valid() {
        let v = |d: Option<&str>, p: Option<&str>, c: Option<&str>| {
            Some(Version {
                description: d.map(Into::into),
                product: p.map(Into::into),
                company: c.map(Into::into),
                version: None,
            })
        };
        let valid = Signature::Valid {
            publisher: Some("Microsoft Windows".into()),
            catalog: true,
        };
        assert_eq!(
            summary(&info(
                v(Some("Notepad"), None, Some("Microsoft Corporation")),
                valid.clone()
            ))
            .unwrap(),
            "Notepad, signed by Microsoft Windows"
        );
        assert_eq!(
            summary(&info(
                v(Some("Tool"), None, Some("Acme")),
                Signature::Unsigned
            ))
            .unwrap(),
            "Tool (Acme), unsigned"
        );
        assert_eq!(
            summary(&info(
                v(None, Some("Acme Suite"), Some("Acme")),
                Signature::Unsigned
            ))
            .unwrap(),
            // The company is in the name already.
            "Acme Suite, unsigned"
        );
        assert_eq!(
            summary(&info(v(None, None, Some("Acme")), Signature::Unsigned)).unwrap(),
            "Acme, unsigned"
        );
        assert_eq!(
            summary(&info(
                None,
                Signature::Invalid {
                    reason: "the file was changed after signing".into()
                }
            ))
            .unwrap(),
            "signature not valid: the file was changed after signing"
        );
        assert_eq!(
            summary(&info(
                v(Some("Big"), None, None),
                Signature::Unknown("too large".into())
            ))
            .unwrap(),
            "Big"
        );
        assert_eq!(summary(&info(None, Signature::Unknown("x".into()))), None);
        assert_eq!(
            summary(&info(
                None,
                Signature::Valid {
                    publisher: None,
                    catalog: false
                }
            ))
            .unwrap(),
            "signed"
        );
    }

    #[test]
    fn m2_cache_hands_each_file_to_the_reader_once_and_rereads_changed_files() {
        let mut cache = Cache::default();
        let a = PathBuf::from(r"C:\A\a.exe");
        let stamp = Stamp {
            len: 10,
            modified: None,
        };
        let (view, wanted) = cache.lookup(&a, stamp);
        assert!(wanted && view.pending);
        assert_eq!(view.signature, "pending");
        // Asked again while the reader works: not handed over twice.
        let (_, wanted) = cache.lookup(Path::new(r"c:\a\A.EXE"), stamp);
        assert!(!wanted);
        assert_eq!(cache.missing(&a, stamp), Some((true, true)));
        cache.set_version(
            &a,
            stamp,
            Some(Version {
                description: Some("A".into()),
                ..Version::default()
            }),
        );
        let (view, _) = cache.lookup(&a, stamp);
        assert_eq!(view.description.as_deref(), Some("A"));
        assert!(view.pending);
        cache.set_signature(
            &a,
            stamp,
            Signature::Valid {
                publisher: Some("Acme".into()),
                catalog: false,
            },
        );
        let (view, wanted) = cache.lookup(&a, stamp);
        assert!(!view.pending && !wanted);
        assert_eq!(
            (view.signature, view.publisher.as_deref()),
            ("valid", Some("Acme"))
        );
        // A changed file starts afresh; late results for the old one are
        // dropped.
        let newer = Stamp { len: 11, ..stamp };
        let (view, wanted) = cache.lookup(&a, newer);
        assert!(wanted && view.pending && view.description.is_none());
        cache.set_signature(&a, stamp, Signature::Unsigned);
        assert_eq!(cache.missing(&a, newer), Some((true, true)));
        assert_eq!(cache.missing(&a, stamp), None);
        // Invalid signatures carry the reason, never a publisher.
        cache.set_version(&a, newer, None);
        cache.set_signature(&a, newer, Signature::Invalid { reason: "r".into() });
        let (view, _) = cache.lookup(&a, newer);
        assert_eq!(
            (view.signature, view.publisher, view.problem.as_deref()),
            ("invalid", None, Some("r"))
        );
    }

    #[test]
    fn m2_cache_is_bounded_and_keeps_what_the_reader_still_fills() {
        let mut cache = Cache::default();
        let stamp = Stamp {
            len: 1,
            modified: None,
        };
        for i in 0..CACHE_ENTRIES {
            let path = PathBuf::from(format!(r"C:\P\{i}.exe"));
            cache.lookup(&path, stamp);
            if i > 0 {
                cache.set_version(&path, stamp, None);
                cache.set_signature(&path, stamp, Signature::Unsigned);
            }
        }
        assert_eq!(cache.len(), CACHE_ENTRIES);
        cache.lookup(Path::new(r"C:\P\new.exe"), stamp);
        // The one still queued and the new one.
        assert_eq!(cache.len(), 2);
        assert!(cache.missing(Path::new(r"C:\P\0.exe"), stamp).is_some());
    }

    #[test]
    fn m2_read_many_reads_each_file_once_for_the_command_line() {
        let windir = std::env::var_os("SystemRoot").expect("SystemRoot");
        let notepad = Path::new(&windir).join("System32").join("notepad.exe");
        let upper = PathBuf::from(notepad.to_string_lossy().to_uppercase());
        let missing = Path::new(r"C:\no\such\program.exe");
        let found = read_many([notepad.as_path(), upper.as_path(), missing]);
        assert_eq!(found.len(), 1);
        let text = summary_of(&found, &upper).unwrap();
        assert!(text.contains(", signed by Microsoft"), "{text}");
        assert_eq!(summary_of(&found, missing), None);
        assert!(read_many([]).is_empty());
    }

    #[test]
    fn m2_real_files_unsigned_and_catalog_signed() {
        let windir = std::env::var_os("SystemRoot").expect("SystemRoot");
        let notepad = Path::new(&windir).join("System32").join("notepad.exe");
        let info = read(&notepad).expect("notepad");
        assert!(
            matches!(info.signature, Signature::Valid { publisher: Some(ref p), .. } if p.starts_with("Microsoft")),
            "{info:?}"
        );
        assert!(info.version.unwrap().company.unwrap().contains("Microsoft"));
        // A copy is still in the catalog (same hash); a changed copy is not,
        // and has no signature of its own: unsigned.
        let dir = tempfile::tempdir().unwrap();
        let copy = dir.path().join("notepad-copy.exe");
        std::fs::copy(&notepad, &copy).unwrap();
        assert!(matches!(
            read(&copy).unwrap().signature,
            Signature::Valid { catalog: true, .. }
        ));
        let mut bytes = std::fs::read(&copy).unwrap();
        let middle = bytes.len() / 2;
        bytes[middle] ^= 0xFF;
        std::fs::write(&copy, &bytes).unwrap();
        let changed = read(&copy).unwrap();
        assert_eq!(changed.signature, Signature::Unsigned);
        // The version resource is read as data and still there.
        assert!(changed.version.is_some());
        // Too large is not hashed.
        assert!(matches!(
            signature_of(&notepad, MAX_VERIFY_BYTES + 1),
            Signature::Unknown(_)
        ));
        // Not a local file, no file.
        assert_eq!(read(Path::new(r"\\server\share\a.exe")), None);
        assert_eq!(read(Path::new(r"C:\no\such\program.exe")), None);
        assert_eq!(read(dir.path()), None);
    }

    /// An embedded signature, if the machine has a program with one where
    /// Windows installs it (Edge or its WebView2 runtime; both are on the
    /// GitHub runners). Its copy verifies, a changed copy does not.
    #[test]
    fn m2_embedded_signature_valid_and_broken() {
        let Some(signed) = embedded_signed() else {
            eprintln!("no program with an embedded signature found; skipped");
            return;
        };
        let info = read(&signed).unwrap();
        match &info.signature {
            Signature::Valid {
                publisher: Some(p),
                catalog: false,
            } => assert!(p.starts_with("Microsoft"), "{p}"),
            other => panic!("{}: {other:?}", signed.display()),
        }
        let dir = tempfile::tempdir().unwrap();
        let copy = dir.path().join("signed.exe");
        let mut bytes = std::fs::read(&signed).unwrap();
        // A byte of the code, far from the signature at the end.
        bytes[0x1000] ^= 0xFF;
        std::fs::write(&copy, &bytes).unwrap();
        let broken = read(&copy).unwrap();
        assert_eq!(
            broken.signature,
            Signature::Invalid {
                reason: "the file was changed after signing".into()
            },
            "{broken:?}"
        );
        assert!(summary(&broken).unwrap().contains("signature not valid"));
    }

    fn embedded_signed() -> Option<PathBuf> {
        let x86 = std::env::var_os("ProgramFiles(x86)")?;
        let base = Path::new(&x86).join("Microsoft");
        let mut candidates = vec![
            base.join("Edge").join("Application").join("msedge.exe"),
            base.join("EdgeUpdate").join("MicrosoftEdgeUpdate.exe"),
        ];
        if let Ok(versions) = std::fs::read_dir(base.join("EdgeWebView").join("Application")) {
            candidates.extend(
                versions
                    .flatten()
                    .map(|d| d.path().join("msedgewebview2.exe")),
            );
        }
        candidates.into_iter().find(|p| p.is_file())
    }
}
