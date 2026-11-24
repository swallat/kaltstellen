//! Updates from GitHub Releases (spec UP1), modelled on foresprint's
//! `selfupdate` package: find the newest release, download the executable
//! for this system, check its SHA-256 against `checksums.txt` of the same
//! release and replace the running executable.
//!
//! Everything that decides (versions, metadata, where installing is
//! allowed, the order of the install steps) is safe code here and is tested
//! offline: the network comes in through [`Fetch`] and the tray, the
//! processes and the version probe through [`install::Session`]. The real
//! implementations are [`WinHttp`] (over [`crate::win32::http`]) and
//! [`session::System`].

pub mod install;
pub mod record;
pub mod release;
pub mod session;
pub mod url;
pub mod version;

use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::error::{Code, Error, Result};
use crate::settings;
use release::Release;

/// The version of this executable.
pub const CURRENT: &str = env!("KALTSTELLEN_VERSION");
/// Built by the release workflow (`KALTSTELLEN_RELEASE=1`). Anything else is
/// a development build, which does not update itself (UP1 "Versions").
pub const RELEASE_BUILD: bool = option_env!("KALTSTELLEN_RELEASE").is_some();
/// Where releases are published.
pub const REPOSITORY: &str = "swallat/kaltstellen";
/// API address of [`REPOSITORY`].
pub const DEFAULT_SOURCE: &str = "https://api.github.com/repos/swallat/kaltstellen";
/// Replaces [`DEFAULT_SOURCE`] (tests, a mirror).
pub const SOURCE_ENV: &str = "KALTSTELLEN_UPDATE_URL";
/// `auto`, `manual` or `off` (`on`, the earlier name, means `auto`); wins
/// over `settings.json`.
pub const MODE_ENV: &str = "KALTSTELLEN_UPDATE";

const METADATA_LIMIT: usize = 4 << 20;
const CHECKSUMS_LIMIT: usize = 64 << 10;
/// Largest executable accepted.
pub const BINARY_LIMIT: usize = 64 << 20;
/// Per step of a request (name resolution, connect, send, each receive).
pub const TIMEOUT: Duration = Duration::from_secs(30);

/// Why a [`Fetch`] failed: the HTTP status of an answer other than `200`,
/// or none when there was no answer (network, refused URL, too large).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FetchError {
    pub status: Option<u32>,
    pub message: String,
}

impl FetchError {
    /// No answer at all, or one that could not be read.
    pub fn failed(message: impl Into<String>) -> FetchError {
        FetchError {
            status: None,
            message: message.into(),
        }
    }

    /// An answer with `status` (not `200`) for `url`.
    pub fn status(url: &str, status: u32) -> FetchError {
        FetchError {
            status: Some(status),
            message: format!("{url}: HTTP {status}"),
        }
    }
}

impl std::fmt::Display for FetchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl From<FetchError> for String {
    fn from(err: FetchError) -> String {
        err.message
    }
}

/// Fetches a URL. The real one is [`WinHttp`]; tests serve local data.
pub trait Fetch {
    /// The body of a `200` answer, at most `limit` bytes.
    fn get(
        &self,
        url: &str,
        accept: &str,
        limit: usize,
    ) -> std::result::Result<Vec<u8>, FetchError>;
}

/// HTTPS through WinHTTP; refuses URLs that [`url::Url`] refuses.
pub struct WinHttp;

impl Fetch for WinHttp {
    fn get(
        &self,
        url: &str,
        accept: &str,
        limit: usize,
    ) -> std::result::Result<Vec<u8>, FetchError> {
        let parsed = url::Url::parse(url).map_err(FetchError::failed)?;
        let target = crate::win32::http::Target {
            secure: parsed.secure,
            host: &parsed.host,
            port: parsed.port,
            path: &parsed.path,
        };
        let agent = format!("kaltstellen/{CURRENT}");
        let response = crate::win32::http::get(&target, &agent, accept, limit, TIMEOUT)
            .map_err(|e| FetchError::failed(format!("{url}: {e}")))?;
        if response.status == 200 {
            Ok(response.body)
        } else {
            Err(FetchError::status(url, response.status))
        }
    }
}

/// Where release information comes from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Source {
    /// `https://api.github.com/repos/<owner>/<name>` or its replacement.
    pub base: String,
    /// Named by [`SOURCE_ENV`]; a development build installs only then.
    pub named: bool,
}

impl Source {
    pub fn from_env(value: Option<String>) -> Source {
        match value.map(|v| v.trim().trim_end_matches('/').to_owned()) {
            Some(base) if !base.is_empty() => Source { base, named: true },
            _ => Source {
                base: DEFAULT_SOURCE.to_owned(),
                named: false,
            },
        }
    }

    pub fn current() -> Source {
        Source::from_env(std::env::var(SOURCE_ENV).ok())
    }
}

/// The newest release for a program of version `current`: `latest`, or
/// for a running pre-release the highest of the recent releases. `None`
/// when none is published yet (GitHub answers `latest` with 404 while there
/// is no stable release; the list may be empty): nothing newer, not an
/// error.
pub fn latest(fetch: &dyn Fetch, source: &Source, current: &str) -> Result<Option<Release>> {
    // A development build (`1.2.1-dev.5+hash`, see build.rs) is a pre-release
    // by its version, but it follows the stable channel like the release it
    // grew from.
    let development = current
        .split('+')
        .next()
        .and_then(|v| v.split_once('-'))
        .is_some_and(|(_, pre)| pre.split('.').any(|id| id == "dev"));
    let pre = !development && version::Version::parse(current).is_some_and(|v| v.is_pre_release());
    let url = if pre {
        format!("{}/releases?per_page=30", source.base)
    } else {
        format!("{}/releases/latest", source.base)
    };
    let json = match fetch.get(&url, "application/vnd.github+json", METADATA_LIMIT) {
        Ok(json) => json,
        // Only `latest` means "none yet" by 404 (a repository that is gone
        // looks the same; the list then tells, for a pre-release). For the
        // list, which is empty rather than missing, a 404 stays an error.
        Err(err) if !pre && err.status == Some(404) => return Ok(None),
        Err(err) => return Err(Error::new(Code::UpdateCheck, err)),
    };
    if pre {
        release::parse_list(&json)
    } else {
        release::parse_latest(&json).map(Some)
    }
    .map_err(|e| Error::new(Code::UpdateRelease, e))
}

/// Checks and writes the result to `update.json` in `data_dir` (also a
/// failure, so the tray tries again only a day later). `None`: no release
/// is published yet.
pub fn check(
    fetch: &dyn Fetch,
    source: &Source,
    data_dir: Option<&Path>,
) -> Result<Option<Release>> {
    let result = latest(fetch, source, CURRENT);
    if let Some(dir) = data_dir {
        let (mut rec, _) = record::load(dir);
        rec.last_check = Some(record::now());
        match &result {
            Ok(release) => {
                // None published (any more): nothing to offer.
                rec.latest = release.as_ref().map(|r| r.version.clone());
                rec.page = release.as_ref().map(|r| r.page.clone());
                rec.error = None;
            }
            Err(err) => rec.error = Some(err.to_string()),
        }
        // A failed version that is installed now, or older, is done with.
        if rec
            .failed
            .as_deref()
            .is_some_and(|failed| !version::newer(CURRENT, failed))
        {
            rec.failed = None;
            rec.failed_at = None;
            rec.failures = 0;
        }
        // The answer counts more than remembering it.
        if let Err(err) = record::save(dir, &rec) {
            log::warn!("{err}");
        }
    }
    result
}

/// UP1 modes (owner, 2026-11-23).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Check once a day and install by itself, but only while `neutral` is
    /// active and no switch runs or waits (the default).
    Auto,
    /// Check once a day and report; install on request.
    Manual,
    /// Never connect by itself.
    Off,
}

impl Mode {
    pub const ALL: [Mode; 3] = [Mode::Auto, Mode::Manual, Mode::Off];

    /// `auto`, `manual`, `off`; `on` (the earlier name for checking
    /// daily) means `auto` now.
    pub fn parse(text: &str) -> Option<Mode> {
        match text.trim().to_ascii_lowercase().as_str() {
            "auto" | "on" | "1" | "true" => Some(Mode::Auto),
            "manual" => Some(Mode::Manual),
            "off" | "0" | "false" => Some(Mode::Off),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Mode::Auto => "auto",
            Mode::Manual => "manual",
            Mode::Off => "off",
        }
    }

    /// Whether the tray checks by itself.
    pub fn checks(self) -> bool {
        self != Mode::Off
    }
}

/// The mode in effect and whether the environment variable set it. The
/// variable wins over the saved value; the default is [`Mode::Auto`].
pub fn resolve_mode(env: Option<&str>, saved: Option<&str>) -> (Mode, bool) {
    if let Some(mode) = env.and_then(Mode::parse) {
        return (mode, true);
    }
    (saved.and_then(Mode::parse).unwrap_or(Mode::Auto), false)
}

/// UP1: whether the tray may install by itself now: mode `auto`, the
/// update can be installed here, `neutral` is active, and no switch runs or
/// waits. Otherwise it waits for the next chance.
pub fn may_install_now(mode: Mode, installable: bool, neutral: bool, idle: bool) -> bool {
    mode == Mode::Auto && installable && neutral && idle
}

/// [`resolve_mode`] for this process.
pub fn current_mode(config_dir: Option<&Path>) -> (Mode, bool) {
    let saved = config_dir.and_then(|dir| settings::load(dir).0.update);
    resolve_mode(std::env::var(MODE_ENV).ok().as_deref(), saved.as_deref())
}

/// Whether this build may install from `source`, and why not.
pub fn may_install(release_build: bool, source: &Source) -> Result<()> {
    if release_build || source.named {
        Ok(())
    } else {
        Err(Error::new(
            Code::UpdateNotHere,
            format!(
                "a development build does not update itself; install a release from https://github.com/{REPOSITORY}/releases"
            ),
        ))
    }
}

/// Whether a copy at `exe` may replace itself, judged by its path (UP1
/// "Where"); the writable probe comes on top in [`install`]. The error says
/// how to update instead.
pub fn placement(exe: &Path, program_files: &[PathBuf]) -> std::result::Result<(), String> {
    let text = exe.to_string_lossy().to_lowercase().replace('/', "\\");
    let managed = [
        ("\\scoop\\apps\\", "scoop: scoop update kaltstellen"),
        ("\\winget\\", "winget: winget upgrade kaltstellen"),
        ("\\chocolatey\\", "Chocolatey: choco upgrade kaltstellen"),
    ];
    for (marker, how) in managed {
        if text.contains(marker) {
            return Err(format!("installed by {how}"));
        }
    }
    for dir in program_files {
        let dir = dir.to_string_lossy().to_lowercase().replace('/', "\\");
        let dir = dir.trim_end_matches('\\');
        if !dir.is_empty() && text.starts_with(&format!("{dir}\\")) {
            return Err(
                "installed below Program Files, which needs administrator rights; replace the file by hand"
                    .to_owned(),
            );
        }
    }
    Ok(())
}

/// `%ProgramFiles%`, `%ProgramFiles(x86)%` and `%ProgramW6432%`.
pub fn program_files() -> Vec<PathBuf> {
    ["ProgramFiles", "ProgramFiles(x86)", "ProgramW6432"]
        .iter()
        .filter_map(std::env::var_os)
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .collect()
}

/// The running executable with links resolved.
pub fn running_exe() -> Result<PathBuf> {
    let exe = std::env::current_exe().map_err(|e| {
        Error::new(
            Code::UpdateNotHere,
            format!("cannot find this executable: {e}"),
        )
    })?;
    Ok(std::fs::canonicalize(&exe)
        .map(|p| strip_verbatim(&p))
        .unwrap_or(exe))
}

/// `\\?\C:\x` from `canonicalize` back to `C:\x` (`\\?\UNC\` stays).
fn strip_verbatim(path: &Path) -> PathBuf {
    let text = path.to_string_lossy();
    match text.strip_prefix(r"\\?\") {
        Some(rest) if !rest.starts_with("UNC\\") => PathBuf::from(rest),
        _ => path.to_path_buf(),
    }
}

/// Whether the last known version (from `update.json`) is newer than this
/// one; for the tray menu.
pub fn known_newer(rec: &record::Record) -> Option<&str> {
    rec.latest
        .as_deref()
        .filter(|latest| version::newer(CURRENT, latest))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    /// Serves fixed answers by URL; an `Err` is a failure without an answer
    /// (network), a URL not listed is answered with 404.
    pub(crate) struct Fake(pub HashMap<String, std::result::Result<Vec<u8>, String>>);

    impl Fetch for Fake {
        fn get(
            &self,
            url: &str,
            _: &str,
            limit: usize,
        ) -> std::result::Result<Vec<u8>, FetchError> {
            match self.0.get(url) {
                Some(Ok(body)) if body.len() > limit => Err(FetchError::failed("too large")),
                Some(Ok(body)) => Ok(body.clone()),
                Some(Err(why)) => Err(FetchError::failed(why.clone())),
                None => Err(FetchError::status(url, 404)),
            }
        }
    }

    const LATEST: &str = "http://127.0.0.1:1/repos/o/r/releases/latest";
    const LIST: &str = "http://127.0.0.1:1/repos/o/r/releases?per_page=30";

    /// Both addresses fail without an answer, as with no network.
    fn offline() -> Fake {
        Fake(HashMap::from([
            (LATEST.to_owned(), Err("unreachable".to_owned())),
            (LIST.to_owned(), Err("unreachable".to_owned())),
        ]))
    }

    fn source() -> Source {
        Source::from_env(Some("http://127.0.0.1:1/repos/o/r/".into()))
    }

    /// A minimal HTTP/1.1 server on loopback: `path -> (status, extra
    /// header, body)`. Runs until the test process ends.
    fn serve(routes: Vec<(&'static str, u16, &'static str, Vec<u8>)>) -> String {
        use std::io::{BufRead, BufReader, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let mut reader = BufReader::new(stream);
                let mut first = String::new();
                if reader.read_line(&mut first).is_err() {
                    continue;
                }
                loop {
                    let mut line = String::new();
                    if reader.read_line(&mut line).unwrap_or(0) <= 2 {
                        break;
                    }
                }
                let path = first.split_whitespace().nth(1).unwrap_or("/").to_owned();
                let (status, header, body) = routes
                    .iter()
                    .find(|r| r.0 == path)
                    .map_or((404, "", Vec::new()), |r| (r.1, r.2, r.3.clone()));
                let mut stream = reader.into_inner();
                let head = format!(
                    "HTTP/1.1 {status} X\r\nContent-Length: {}\r\nConnection: close\r\n{header}\r\n",
                    body.len()
                );
                let _ = stream.write_all(head.as_bytes());
                let _ = stream.write_all(&body);
            }
        });
        base
    }

    #[test]
    fn up1_winhttp_against_a_loopback_server() {
        let base = serve(vec![
            ("/a", 200, "", b"hello".to_vec()),
            ("/big", 200, "", vec![b'x'; 300_000]),
            ("/moved", 302, "Location: /a\r\n", Vec::new()),
            ("/gone", 410, "", Vec::new()),
        ]);
        let get = |path: &str, limit| WinHttp.get(&format!("{base}{path}"), "*/*", limit);
        assert_eq!(get("/a", 100).unwrap(), b"hello");
        assert_eq!(get("/big", 1 << 20).unwrap().len(), 300_000);
        assert!(
            get("/big", 1000)
                .unwrap_err()
                .message
                .contains("larger than")
        );
        // GitHub answers downloads with a redirect.
        assert_eq!(get("/moved", 100).unwrap(), b"hello");
        let gone = get("/gone", 100).unwrap_err();
        assert!(gone.message.ends_with("HTTP 410"));
        assert_eq!(gone.status, Some(410));
        let nothing = get("/nothing", 100).unwrap_err();
        assert!(nothing.message.ends_with("HTTP 404"));
        assert_eq!(nothing.status, Some(404));
        assert_eq!(get("/big", 1000).unwrap_err().status, None);
        // Refused before any connection: plain http to another host.
        let refused = WinHttp.get("http://example.org/", "*/*", 10).unwrap_err();
        assert!(refused.message.contains("loopback"));
        assert_eq!(refused.status, None);
    }

    #[test]
    fn up1_source_default_and_override() {
        assert_eq!(
            Source::from_env(None),
            Source {
                base: DEFAULT_SOURCE.into(),
                named: false
            }
        );
        assert!(!Source::from_env(Some("  ".into())).named);
        assert_eq!(source().base, "http://127.0.0.1:1/repos/o/r");
        assert!(source().named);
    }

    #[test]
    fn up1_latest_uses_the_pre_release_list_only_for_a_pre_release() {
        let fake = Fake(HashMap::from([
            (
                "http://127.0.0.1:1/repos/o/r/releases/latest".to_owned(),
                Ok(br#"{"tag_name":"v1.0.0"}"#.to_vec()),
            ),
            (
                "http://127.0.0.1:1/repos/o/r/releases?per_page=30".to_owned(),
                Ok(br#"[{"tag_name":"v1.0.0"},{"tag_name":"v1.1.0-beta.1"}]"#.to_vec()),
            ),
        ]));
        let version = |current| latest(&fake, &source(), current).unwrap().unwrap().version;
        assert_eq!(version("0.9.0"), "1.0.0");
        assert_eq!(version("1.1.0-beta.0"), "1.1.0-beta.1");
        for current in ["0.9.0", "1.1.0-beta.0"] {
            assert_eq!(
                latest(&offline(), &source(), current).unwrap_err().code,
                Code::UpdateCheck
            );
        }
        let junk = Fake(HashMap::from([(
            "http://127.0.0.1:1/repos/o/r/releases/latest".to_owned(),
            Ok(b"<html>".to_vec()),
        )]));
        assert_eq!(
            latest(&junk, &source(), "0.9.0").unwrap_err().code,
            Code::UpdateRelease
        );
    }

    /// No release published yet: GitHub answers `latest` with 404 (also
    /// while there are only pre-releases) and the list with `[]`. That is
    /// "nothing newer", not a failed check.
    #[test]
    fn up1_no_release_yet_is_nothing_newer() {
        let none = Fake(HashMap::from([(LIST.to_owned(), Ok(b"[]".to_vec()))]));
        assert_eq!(latest(&none, &source(), "1.0.0").unwrap(), None);
        assert_eq!(latest(&none, &source(), "1.0.0-beta.1").unwrap(), None);
        // A 404 for the list is not "none yet": the list exists, empty.
        let no_list = Fake(HashMap::new());
        assert_eq!(
            latest(&no_list, &source(), "1.0.0-beta.1")
                .unwrap_err()
                .code,
            Code::UpdateCheck
        );
        // Other answers stay errors (rate limit, server error).
        for status in [403, 500] {
            struct Status(u32);
            impl Fetch for Status {
                fn get(
                    &self,
                    url: &str,
                    _: &str,
                    _: usize,
                ) -> std::result::Result<Vec<u8>, FetchError> {
                    Err(FetchError::status(url, self.0))
                }
            }
            assert_eq!(
                latest(&Status(status), &source(), "1.0.0")
                    .unwrap_err()
                    .code,
                Code::UpdateCheck
            );
        }
        // `check` records it as a check without error and forgets a
        // version it knew (a release that was withdrawn).
        let dir = tempfile::tempdir().unwrap();
        let rec = record::Record {
            latest: Some("9.9.9".to_owned()),
            page: Some("https://page".to_owned()),
            error: Some("KS801: earlier".to_owned()),
            ..Default::default()
        };
        record::save(dir.path(), &rec).unwrap();
        assert_eq!(check(&none, &source(), Some(dir.path())).unwrap(), None);
        let (rec, _) = record::load(dir.path());
        assert_eq!((rec.latest, rec.page, rec.error), (None, None, None));
        assert!(rec.last_check.is_some());
    }

    #[test]
    fn up1_check_records_success_and_failure() {
        let dir = tempfile::tempdir().unwrap();
        // `check` asks for this build's own version: a build on a
        // pre-release tag (`1.0.0-beta.1+hash`) reads the list, any other
        // (also a development build, `1.2.1-dev.5+hash`) reads `latest`.
        // Serve both.
        let fake = Fake(HashMap::from([
            (
                "http://127.0.0.1:1/repos/o/r/releases/latest".to_owned(),
                Ok(br#"{"tag_name":"v9.9.9","html_url":"https://page"}"#.to_vec()),
            ),
            (
                "http://127.0.0.1:1/repos/o/r/releases?per_page=30".to_owned(),
                Ok(br#"[{"tag_name":"v9.9.9","html_url":"https://page"}]"#.to_vec()),
            ),
        ]));
        check(&fake, &source(), Some(dir.path())).unwrap();
        let (rec, _) = record::load(dir.path());
        assert_eq!(rec.latest.as_deref(), Some("9.9.9"));
        assert_eq!(rec.page.as_deref(), Some("https://page"));
        assert_eq!(known_newer(&rec), Some("9.9.9"));
        assert!(rec.last_check.is_some());
        check(&offline(), &source(), Some(dir.path())).unwrap_err();
        let (rec, _) = record::load(dir.path());
        // The version found earlier stays known; the error is recorded.
        assert_eq!(rec.latest.as_deref(), Some("9.9.9"));
        assert!(rec.error.unwrap().starts_with("KS801"));
        // A failed version newer than this build is kept; one this build
        // already has (installed after all) is forgotten.
        record::mark_failed(dir.path(), "9.9.9", 1).unwrap();
        check(&fake, &source(), Some(dir.path())).unwrap();
        assert!(record::load(dir.path()).0.failed_before("9.9.9"));
        record::mark_failed(dir.path(), CURRENT, 1).unwrap();
        check(&fake, &source(), Some(dir.path())).unwrap();
        let (rec, _) = record::load(dir.path());
        assert_eq!((rec.failed, rec.failed_at, rec.failures), (None, None, 0));
    }

    #[test]
    fn up1_mode_environment_wins_default_auto() {
        assert_eq!(resolve_mode(None, None), (Mode::Auto, false));
        assert_eq!(resolve_mode(None, Some("off")), (Mode::Off, false));
        assert_eq!(resolve_mode(None, Some("manual")), (Mode::Manual, false));
        assert_eq!(resolve_mode(Some("OFF"), Some("on")), (Mode::Off, true));
        assert_eq!(resolve_mode(Some("1"), Some("off")), (Mode::Auto, true));
        assert_eq!(resolve_mode(Some("Manual"), None), (Mode::Manual, true));
        for mode in Mode::ALL {
            assert_eq!(Mode::parse(mode.name()), Some(mode));
        }
    }

    #[test]
    fn up1_automatic_install_only_while_neutral_and_idle() {
        assert!(may_install_now(Mode::Auto, true, true, true));
        assert!(
            !may_install_now(Mode::Auto, true, false, true),
            "a profile is active"
        );
        assert!(
            !may_install_now(Mode::Auto, true, true, false),
            "a switch runs or waits"
        );
        assert!(
            !may_install_now(Mode::Auto, false, true, true),
            "cannot install here"
        );
        assert!(!may_install_now(Mode::Manual, true, true, true));
        assert!(!may_install_now(Mode::Off, true, true, true));
        // Junk in the variable is ignored, junk in the file means default.
        assert_eq!(resolve_mode(Some("maybe"), Some("off")), (Mode::Off, false));
        assert_eq!(resolve_mode(None, Some("maybe")), (Mode::Auto, false));
    }

    #[test]
    fn up1_development_build_installs_only_from_a_named_source() {
        assert!(may_install(true, &Source::from_env(None)).is_ok());
        assert!(may_install(false, &source()).is_ok());
        assert_eq!(
            may_install(false, &Source::from_env(None))
                .unwrap_err()
                .code,
            Code::UpdateNotHere
        );
    }

    #[test]
    fn up1_placement_refuses_package_managers_and_program_files() {
        let pf = [
            PathBuf::from(r"C:\Program Files"),
            PathBuf::from(r"C:\Program Files (x86)\"),
        ];
        let ok = |p: &str| placement(Path::new(p), &pf);
        assert!(ok(r"C:\Tools\kaltstellen.exe").is_ok());
        assert!(ok(r"D:\Program Files Portable\kaltstellen.exe").is_ok());
        assert!(
            ok(r"C:\Users\u\scoop\apps\kaltstellen\current\kaltstellen.exe")
                .unwrap_err()
                .contains("scoop update")
        );
        assert!(
            ok(r"C:\Users\u\AppData\Local\Microsoft\WinGet\Packages\x\kaltstellen.exe")
                .unwrap_err()
                .contains("winget upgrade")
        );
        assert!(
            ok(r"C:\ProgramData\chocolatey\lib\kaltstellen\tools\kaltstellen.exe")
                .unwrap_err()
                .contains("choco")
        );
        assert!(
            ok(r"c:\program files\kaltstellen\kaltstellen.exe")
                .unwrap_err()
                .contains("administrator")
        );
        assert!(ok(r"C:\Program Files (x86)\k\kaltstellen.exe").is_err());
    }

    #[test]
    fn verbatim_prefix_is_removed_for_drive_paths_only() {
        assert_eq!(
            strip_verbatim(Path::new(r"\\?\C:\x\k.exe")),
            PathBuf::from(r"C:\x\k.exe")
        );
        assert_eq!(
            strip_verbatim(Path::new(r"\\?\UNC\srv\s\k.exe")),
            PathBuf::from(r"\\?\UNC\srv\s\k.exe")
        );
    }
}
