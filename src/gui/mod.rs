//! The main window (`kaltstellen gui`, WP10; spec section 8 and M6): a
//! WebView2 page embedded in the executable, in a process of its own,
//! started from the tray and ended when closed, so Q1 holds for the tray.
//! The page is a view; [`bridge::Bridge`] validates every intent and edits
//! the configuration file through [`document::Document`]. Profiles are
//! applied by running `kaltstellen switch`, which goes through the tray's
//! pipe when a tray runs.

pub mod about;
pub mod assets;
pub mod bridge;
pub mod document;
pub mod icons;
pub mod model;

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::thread;

use log::{info, warn};
use serde_json::Value;

use crate::config::{Config, NEUTRAL};
use crate::error::Code;
use crate::exit;
use crate::paths;
use crate::process::Snapshot;
use crate::state::Store;
use crate::unassigned::Unassigned;
use crate::win32::webview::{self, Answer, Options, Page, Theme, Waker};
use crate::win32::{console, dialog, instance::Instance};
use bridge::{Bridge, Host};
use document::Document;
use model::Candidate;

pub use webview::Theme as GuiTheme;

/// Keeps a started `kaltstellen switch` from opening a console window.
const CREATE_NO_WINDOW: u32 = 0x0800_0000;
/// How long `--screenshot` and `--drive` may take.
const REVIEW_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(90);
/// Client size at 96 DPI.
const SIZE: (i32, i32) = (1040, 760);

/// Hidden options for review and tests.
#[derive(Debug, Clone, Default)]
pub struct Args {
    /// Capture the page as PNG into this file and exit.
    pub screenshot: Option<PathBuf>,
    /// Open a view before the capture: `entry`, `add`, `confirm`, `new`.
    pub view: Option<String>,
    pub theme: Option<Theme>,
    /// Run these bridge requests (JSON array) through the page, then exit.
    pub drive: Option<PathBuf>,
    /// Where `--drive` writes the replies (JSON).
    pub drive_out: Option<PathBuf>,
}

/// Text for a missing WebView2 runtime (spec M6).
pub fn runtime_missing_message(config: Option<&Path>) -> String {
    format!(
        "error: {}: the Microsoft Edge WebView2 Runtime is not installed, and the kaltstellen \
         window needs it. It is part of Windows 11; on other systems install the Evergreen \
         runtime from Microsoft. Without it, edit the configuration file {} and use \
         `kaltstellen check-config`, `kaltstellen switch` and the tray menu.",
        Code::WebViewMissing,
        config.map_or_else(|| "config.toml".to_owned(), |p| p.display().to_string())
    )
}

/// `kaltstellen gui`. Returns the exit code.
pub fn main(args: Args) -> u8 {
    let started = std::time::Instant::now();
    let file = paths::config_file();
    if webview::runtime_version().is_none() {
        eprintln!("{}", runtime_missing_message(file.as_deref()));
        warn!("gui: WebView2 runtime missing");
        return exit::ERRORS;
    }
    let (Some(file), Some(data)) = (file, paths::data_dir()) else {
        eprintln!("error: no configuration or data directory (APPDATA, LOCALAPPDATA unset)");
        return exit::ERRORS;
    };
    let mutex = crate::tray::Endpoints::current()
        .map(|e| e.mutex.replace("kaltstellen-tray", "kaltstellen-gui"))
        .unwrap_or_else(|_| "Local\\kaltstellen-gui".to_owned());
    let _instance = match Instance::acquire(&mutex) {
        Ok(Some(instance)) => instance,
        Ok(None) => {
            // M6: a second start brings the open window to the front.
            if webview::bring_to_front(&mutex) {
                eprintln!("the kaltstellen window is already open; brought it to the front");
            } else {
                eprintln!("the kaltstellen window is already open in this session");
            }
            return exit::OK;
        }
        Err(err) => {
            eprintln!("error: {err}");
            return exit::ERRORS;
        }
    };
    let document = match Document::open(&file) {
        Ok(document) => document,
        Err(lines) => {
            // The window edits valid files only; check-config tells why.
            for line in &lines {
                eprintln!("{line}");
            }
            return exit::CONFIG_INVALID;
        }
    };
    let drive = match &args.drive {
        Some(path) => match std::fs::read_to_string(path) {
            Ok(text) if serde_json::from_str::<Vec<Value>>(&text).is_ok() => Some(text),
            _ => {
                eprintln!("error: --drive needs a file with a JSON array of requests");
                return exit::ERRORS;
            }
        },
        None => None,
    };
    console::detach_if_own();
    info!("gui: started (pid {})", std::process::id());
    let review = args.screenshot.is_some() || drive.is_some();
    let mut query = Vec::new();
    if let Some(view) = &args.view {
        query.push(format!(
            "view={}",
            view.chars()
                .filter(char::is_ascii_alphanumeric)
                .collect::<String>()
        ));
    }
    let options = Options {
        title: "kaltstellen".to_owned(),
        size: SIZE,
        // Under the data directory, never next to the executable.
        user_data: data.join("webview2"),
        dev_tools: cfg!(debug_assertions) && !review,
        theme: args.theme,
        query: query.join("&"),
        no_activate: review,
        instance: mutex.clone(),
    };
    let result = webview::run(options, assets::get, |waker| {
        let (results, inbox) = channel();
        if review {
            // A capture or a test run must end even if the page hangs.
            let watchdog = results.clone();
            let waker = waker.clone();
            thread::spawn(move || {
                thread::sleep(REVIEW_TIMEOUT);
                let _ = watchdog.send(Applied {
                    kind: Kind::Watchdog,
                    target: String::new(),
                    code: exit::ERRORS,
                    stderr: String::new(),
                });
                waker.wake();
            });
        }
        let host = RealHost {
            data: data.clone(),
            waker: waker.clone(),
            results: results.clone(),
            icons: std::cell::RefCell::default(),
            about: {
                let results = results.clone();
                let waker = waker.clone();
                about::Reader::new(move || {
                    let _ = results.send(Applied {
                        kind: Kind::Info,
                        target: String::new(),
                        code: 0,
                        stderr: String::new(),
                    });
                    waker.wake();
                })
            },
        };
        Box::new(GuiPage {
            bridge: Bridge::new(document, host),
            inbox,
            screenshot: args.screenshot.clone(),
            drive,
            drive_out: args.drive_out.clone(),
            started,
        })
    });
    match result {
        Ok(()) => {
            info!("gui: ended");
            exit::OK
        }
        Err(err) => {
            warn!("{err}");
            eprintln!("error: {err}");
            exit::ERRORS
        }
    }
}

/// Result of a `kaltstellen switch` run in the background.
/// What a background job of the window reports.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Switch,
    /// `kaltstellen update --check --quiet` ended.
    Check,
    /// The review watchdog: the page never got ready.
    Watchdog,
    /// M2: the reader knows more about programs.
    Info,
}

struct Applied {
    kind: Kind,
    target: String,
    code: u8,
    stderr: String,
}

struct RealHost {
    data: PathBuf,
    waker: Waker,
    results: Sender<Applied>,
    /// M2: icons read so far, by file and size.
    icons: std::cell::RefCell<icons::Cache>,
    /// M2, N1: program information, read in the background.
    about: about::Reader,
}

impl Host for RealHost {
    fn active_profile(&self) -> String {
        Store::new(&self.data)
            .peek()
            .map_or_else(|| NEUTRAL.to_owned(), |s| s.active_profile)
    }

    fn running(&self) -> Vec<Candidate> {
        Snapshot::take()
            .map(|s| model::candidates(s.processes(), s.own_session().unwrap_or(0)))
            .unwrap_or_default()
    }

    fn icons(&self, wanted: &[bridge::Wanted], size: u32) -> Vec<Option<String>> {
        let started = std::time::Instant::now();
        let running = Snapshot::take()
            .map(|s| icons::running_paths(&s, wanted))
            .unwrap_or_default();
        let mut cache = self.icons.borrow_mut();
        let before = cache.len();
        let found: Vec<Option<String>> = wanted
            .iter()
            .map(|w| {
                icons::file_of(w, &running)
                    .and_then(|file| cache.get(&file, size, crate::win32::icon::of_file))
            })
            .collect();
        info!(
            "gui: {} icons at {size} px ({} found, {} read) in {} ms; cache {} icons, {} KiB",
            wanted.len(),
            found.iter().filter(|i| i.is_some()).count(),
            cache.len().saturating_sub(before),
            started.elapsed().as_millis(),
            cache.len(),
            cache.bytes() / 1024
        );
        found
    }

    fn program_info(&self, wanted: &[bridge::Wanted]) -> Vec<Option<crate::program_info::View>> {
        let started = std::time::Instant::now();
        let running = Snapshot::take()
            .map(|s| icons::running_paths(&s, wanted))
            .unwrap_or_default();
        let files: Vec<Option<PathBuf>> =
            wanted.iter().map(|w| icons::file_of(w, &running)).collect();
        let views = self.about.views(&files);
        info!(
            "gui: program info for {} programs ({} known, {} pending) in {} ms",
            wanted.len(),
            views.iter().flatten().filter(|v| !v.pending).count(),
            views.iter().flatten().filter(|v| v.pending).count(),
            started.elapsed().as_millis()
        );
        views
    }

    fn tray_status(&self) -> Result<Option<crate::tray::protocol::TrayStatus>, String> {
        // M5: the tray keeps the error state (L9); without a tray there is
        // none, and the status line alone reports a switch. U4: the tray
        // knows whether automatic switching is paused.
        let endpoints = crate::tray::Endpoints::current().map_err(|e| e.to_string())?;
        crate::tray::client::status_now(&endpoints).map_err(|err| {
            warn!("gui: no status from the tray, markers kept: {err}");
            err.to_string()
        })
    }

    fn triggers_paused(&self) -> bool {
        paths::config_dir().is_some_and(|dir| crate::settings::load(&dir).0.triggers_paused)
    }

    fn set_automation(&self, paused: bool) -> Result<bridge::Delivery, String> {
        let delivery =
            crate::tray::client::set_automation(paused, true).map_err(|e| e.to_string())?;
        info!(
            "gui: automatic switching {} ({})",
            if paused { "paused" } else { "on" },
            delivery.how()
        );
        Ok(delivery)
    }

    fn unassigned(&self, config: &Config, profile: &str) -> Vec<Unassigned> {
        crate::unassigned::refresh(&self.data, config, profile)
    }

    fn dismiss(&self, profile: &str, exe: &str) -> Result<(), String> {
        crate::unassigned::dismiss(&self.data, profile, exe).map_err(|e| e.to_string())
    }

    fn autostart(&self) -> Option<bool> {
        let exe = std::env::current_exe().ok()?;
        crate::autostart::state(&crate::autostart::Location::current(), &exe)
            .ok()
            .map(|s| s == crate::autostart::State::On)
    }

    fn set_autostart(&self, on: bool) -> Result<(), String> {
        let at = crate::autostart::Location::current();
        if on {
            std::env::current_exe()
                .map_err(|e| e.to_string())
                .and_then(|exe| crate::autostart::enable(&at, &exe).map_err(|e| e.to_string()))
        } else {
            crate::autostart::disable(&at)
                .map(|_| ())
                .map_err(|e| e.to_string())
        }
    }

    fn pick_executable(&self) -> Option<PathBuf> {
        dialog::pick_executable(self.waker.window())
    }

    fn update_info(&self) -> bridge::UpdateInfo {
        let config_dir = paths::config_dir();
        let (mode, locked, offer) = crate::tray::update::menu_state(config_dir.as_deref());
        let record = crate::update::record::load(&self.data).0;
        bridge::UpdateInfo {
            version: crate::update::CURRENT.to_owned(),
            mode: mode.name(),
            mode_locked: locked,
            available: offer.as_ref().map(|o| o.version.clone()),
            failed: offer.as_ref().is_some_and(|o| o.failed),
            retry_at: offer.as_ref().and_then(|o| o.retry_at),
            blocked: offer.and_then(|o| o.blocked),
            last_check: record.last_check,
            error: record.error,
            checking: false,
        }
    }

    fn set_update_mode(&self, mode: crate::update::Mode) -> Result<bridge::Delivery, String> {
        let delivery =
            crate::tray::client::set_update_mode(mode, true).map_err(|e| e.to_string())?;
        info!("gui: update mode {} ({})", mode.name(), delivery.how());
        Ok(delivery)
    }

    fn check_updates(&self) {
        let exe = std::env::current_exe();
        let results = self.results.clone();
        let waker = self.waker.clone();
        thread::spawn(move || {
            use std::os::windows::process::CommandExt;
            let _ = exe.and_then(|exe| {
                Command::new(exe)
                    .args(["update", "--check", "--quiet"])
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .creation_flags(CREATE_NO_WINDOW)
                    .status()
            });
            let _ = results.send(Applied {
                kind: Kind::Check,
                target: String::new(),
                code: 0,
                stderr: String::new(),
            });
            waker.wake();
        });
    }

    fn install_update(&self) -> Result<(), String> {
        crate::tray::update::open_install();
        Ok(())
    }

    fn apply(&self, profile: &str) {
        let exe = std::env::current_exe();
        let results = self.results.clone();
        let waker = self.waker.clone();
        let target = profile.to_owned();
        thread::spawn(move || {
            use std::os::windows::process::CommandExt;
            let output = exe.and_then(|exe| {
                Command::new(exe)
                    // `--`: a profile name that starts with a dash is a
                    // name, not an option.
                    .args(["switch", "--", &target])
                    .stdin(Stdio::null())
                    .creation_flags(CREATE_NO_WINDOW)
                    .output()
            });
            let (code, stderr) = match output {
                Ok(o) => (
                    o.status.code().unwrap_or(1) as u8,
                    String::from_utf8_lossy(&o.stderr).into_owned(),
                ),
                Err(err) => (exit::UNREACHABLE, format!("error: {err}")),
            };
            let _ = results.send(Applied {
                kind: Kind::Switch,
                target,
                code,
                stderr,
            });
            waker.wake();
        });
    }
}

struct GuiPage {
    bridge: Bridge<RealHost>,
    inbox: Receiver<Applied>,
    screenshot: Option<PathBuf>,
    drive: Option<String>,
    drive_out: Option<PathBuf>,
    started: std::time::Instant,
}

impl GuiPage {
    /// The DevTools calls for the `drop_real` step the page names: drag
    /// enter, over and drop at its point, with that step's `paths`.
    fn drive_drop(&self, notice: &Value) -> Option<Answer> {
        let steps: Vec<Value> = serde_json::from_str(self.drive.as_deref()?).ok()?;
        let step = steps.get(usize::try_from(notice.get("step")?.as_u64()?).ok()?)?;
        if step.get("type")?.as_str()? != "drop_real" {
            return None;
        }
        let paths: Vec<&str> = step
            .get("paths")?
            .as_array()?
            .iter()
            .filter_map(Value::as_str)
            .collect();
        let (x, y) = (notice.get("x")?.as_f64()?, notice.get("y")?.as_f64()?);
        let calls = ["dragEnter", "dragOver", "drop"]
            .iter()
            .map(|kind| {
                let params = serde_json::json!({
                    "type": kind, "x": x, "y": y,
                    "data": {"items": [], "files": paths, "dragOperationsMask": 1},
                });
                ("Input.dispatchDragEvent".to_owned(), params.to_string())
            })
            .collect();
        Some(Answer::DevTools(calls))
    }

    fn finish(&self) -> Answer {
        match &self.screenshot {
            Some(path) => Answer::CaptureAndClose(path.clone()),
            None => Answer::Close,
        }
    }
}

impl Page for GuiPage {
    fn message(&mut self, json: &str, files: Vec<PathBuf>) -> Answer {
        // Two notices of the page itself; everything else is a request.
        let notice = serde_json::from_str::<Value>(json)
            .ok()
            .filter(|v| {
                v.get("v").and_then(Value::as_u64) == Some(bridge::VERSION) && v.get("id").is_none()
            })
            .and_then(|v| Some((v.get("type")?.as_str()?.to_owned(), v)));
        match notice {
            Some((kind, _)) if kind == "ready" => {
                info!(
                    "gui: page ready after {} ms",
                    self.started.elapsed().as_millis()
                );
                if let Some(steps) = &self.drive {
                    // Only the JSON array read at start goes into the script.
                    return Answer::Script(format!("globalThis.kaltstellenDrive({steps})"));
                }
                if self.screenshot.is_some() {
                    return self.finish();
                }
                Answer::Nothing
            }
            Some((kind, value)) if kind == "drive_done" && self.drive.is_some() => {
                if let Some(out) = &self.drive_out {
                    let results = value.get("results").cloned().unwrap_or(Value::Null);
                    if let Err(err) = std::fs::write(out, results.to_string()) {
                        warn!("gui: {}: {err}", out.display());
                    }
                }
                self.finish()
            }
            Some((kind, value)) if kind == "drive_drop" && self.drive.is_some() => {
                // M2 test driver: a drop of files on disk, as Explorer's
                // would arrive. The paths come from the drive file read at
                // start, never from the page; the page only says which step
                // and where.
                self.drive_drop(&value).unwrap_or(Answer::Nothing)
            }
            Some((kind, value)) if kind == "log" => {
                let message = value.get("message").and_then(Value::as_str).unwrap_or("");
                warn!(
                    "gui: page: {}",
                    message.chars().take(2000).collect::<String>()
                );
                Answer::Nothing
            }
            Some(_) => Answer::Nothing,
            None => Answer::Reply(self.bridge.handle_with_files(json, files)),
        }
    }

    fn wake(&mut self) -> Vec<Answer> {
        let mut answers = Vec::new();
        let mut info = false;
        while let Ok(applied) = self.inbox.try_recv() {
            if applied.kind == Kind::Info {
                // One event for all that arrived meanwhile.
                info = true;
                continue;
            }
            if applied.kind == Kind::Check {
                answers.push(Answer::Reply(self.bridge.checked()));
                continue;
            }
            if applied.kind == Kind::Watchdog {
                // The review watchdog: the page never got ready.
                warn!("gui: the page did not finish in time; closing");
                answers.push(Answer::Close);
                continue;
            }
            info!(
                "gui: switch to \"{}\" ended with {}",
                applied.target, applied.code
            );
            answers.push(Answer::Reply(self.bridge.applied(
                &applied.target,
                applied.code,
                &applied.stderr,
            )));
        }
        if info {
            answers.push(Answer::Reply(self.bridge.program_info_ready()));
        }
        answers
    }

    fn closing(&mut self) -> bool {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_missing_runtime_message_points_to_the_alternatives() {
        let text = runtime_missing_message(Some(Path::new("C:\\cfg\\config.toml")));
        assert!(text.contains("KS702"));
        assert!(text.contains("C:\\cfg\\config.toml"));
        assert!(text.contains("kaltstellen switch"));
    }
}
