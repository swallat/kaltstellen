//! The tray app (`kaltstellen tray`, WP7): one instance per session with a
//! named pipe for the command line (L1), notification icon with three
//! states and a menu built from the configuration, switches on a worker
//! thread (W3), recovery at start (S3) and resume on Quit (L2).
//!
//! Everything here is safe code. The Win32 side lives in
//! [`crate::win32`]; this module decides what it shows.

pub mod client;
pub mod hotkeys;
pub mod model;
pub mod protocol;
pub mod server;
pub mod source;
pub mod starts;
pub mod triggers;
pub mod update;
pub mod worker;

use std::cell::Cell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

/// M4c: starts since the profile was applied still count when `watch` is
/// noticed within this many seconds of it.
const WATCH_NOTICE: u64 = 60;

/// M4c: the shortest time between two applications of a watching profile.
const WATCH_GAP: Duration = Duration::from_secs(30);

use log::{info, warn};

use crate::autostart;
use crate::config::{self, NEUTRAL};
use crate::error::{Code, Error};
use crate::exit;
use crate::paths;
use crate::process::{Access, ProcessInfo, Snapshot, Target};
use crate::settings;
use crate::state::Store;
use crate::win32::{console, instance::Instance, pipe, tray as window};
use model::{Command as MenuCommand, Decision, Guests, Icon, Menu, Status, SwitchError};
use protocol::{AutomationStatus, Body, Command, Finished, TrayStatus};
use server::Incoming;
use source::Source;
use worker::{Controller, Done, EngineRunner, Job};

/// FNV-1a of the data directory: stable across builds, unlike the standard
/// hasher.
fn dir_hash(data_dir: &Path) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in data_dir.to_string_lossy().to_lowercase().bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{hash:016x}")
}

/// Mutex of the tray instance in the session namespace (L1). With a
/// redirected data directory (tests, portable use) the name includes that
/// directory, so such an instance does not block the real one.
pub fn instance_name(data_dir: &Path, redirected: bool) -> String {
    const BASE: &str = "Local\\kaltstellen-tray";
    if redirected {
        format!("{BASE}-{}", dir_hash(data_dir))
    } else {
        BASE.to_owned()
    }
}

/// Pipe of the tray (L1). Pipe names are machine-wide, so the name carries
/// the user's SID and the session, plus the same data-directory suffix as
/// the mutex.
pub fn pipe_name(sid: &str, session: u32, data_dir: &Path, redirected: bool) -> String {
    let mut name = format!("\\\\.\\pipe\\kaltstellen-{sid}-{session}");
    if redirected {
        name.push('-');
        name.push_str(&dir_hash(data_dir));
    }
    name
}

/// Where the tray of this session and data directory can be found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Endpoints {
    pub mutex: String,
    pub pipe: String,
}

impl Endpoints {
    pub fn current() -> Result<Endpoints, Error> {
        let data = paths::data_dir()
            .ok_or_else(|| Error::new(Code::StateIo, "no data directory (LOCALAPPDATA unset)"))?;
        let redirected = std::env::var_os("KALTSTELLEN_DATA_DIR").is_some_and(|v| !v.is_empty());
        let sid = pipe::current_user_sid()?;
        Ok(Endpoints {
            mutex: instance_name(&data, redirected),
            pipe: pipe_name(&sid, pipe::current_session(), &data, redirected),
        })
    }
}

/// Starts `kaltstellen gui` as a process of its own, without a console
/// window (spec section 8). It inherits the environment, so a test tray
/// opens a test window.
fn open_window() {
    use std::os::windows::process::CommandExt;
    const DETACHED_PROCESS: u32 = 0x0000_0008;
    let started = std::env::current_exe().and_then(|exe| {
        std::process::Command::new(exe)
            .arg("gui")
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .creation_flags(DETACHED_PROCESS)
            .spawn()
    });
    match started {
        Ok(child) => info!("tray: main window started (pid {})", child.id()),
        Err(err) => warn!("tray: cannot start the main window: {err}"),
    }
}

fn lock(source: &Mutex<Source>) -> MutexGuard<'_, Source> {
    source.lock().unwrap_or_else(|e| e.into_inner())
}

/// `kaltstellen tray`. Returns the process exit code.
pub fn main() -> u8 {
    run(false)
}

/// A double-click on the executable: the tray, and the main window as well,
/// whether this process becomes the tray or one is already running. The
/// window opens only once this process owns the tray instance and its pipe
/// is up (or another tray runs); a tray that cannot start at all opens no
/// window. If only the pipe fails, the window opens and says the tray does
/// not answer.
pub fn main_and_window() -> u8 {
    run(true)
}

fn run(window: bool) -> u8 {
    let endpoints = match Endpoints::current() {
        Ok(endpoints) => endpoints,
        Err(err) => {
            eprintln!("error: {err}");
            return exit::ERRORS;
        }
    };
    let data = paths::data_dir().unwrap_or_default();
    let _instance = match Instance::acquire(&endpoints.mutex) {
        Ok(Some(instance)) => instance,
        Ok(None) => {
            info!("tray: another instance runs in this session; exiting");
            if window {
                open_window();
            }
            eprintln!("kaltstellen tray is already running in this session");
            return exit::OK;
        }
        Err(err) => {
            warn!("{err}");
            eprintln!("error: {err}");
            return exit::ERRORS;
        }
    };
    if console::detach_if_own() {
        info!("tray: started without a terminal; console released");
    } else {
        eprintln!("kaltstellen tray is running; use Quit in its menu to end it");
    }
    info!(
        "tray: started (pid {}), DPI awareness {}",
        std::process::id(),
        window::dpi_awareness()
    );

    let store = Store::new(&data);
    let config_file = paths::config_file();
    let config_dir = paths::config_dir();
    let source = Arc::new(Mutex::new(Source::new(config_file.clone())));
    let mut status = Status {
        // What a crashed instance left; recovery below makes it neutral.
        active: store
            .peek()
            .map_or_else(|| NEUTRAL.to_owned(), |s| s.active_profile),
        switching: Some(NEUTRAL.to_owned()),
        ..Status::default()
    };
    // S5: the profile of the last session is offered, not applied.
    let mut triggers_paused = false;
    if let Some(dir) = &config_dir {
        let (saved, problem) = settings::load(dir);
        if let Some(problem) = problem {
            warn!("tray: {problem}");
        }
        if let Some(last) = saved.last_profile {
            info!("tray: offering last profile \"{last}\"");
            status.offer = Some(last);
        }
        // U4, decided detail 7: the pause survives a restart.
        triggers_paused = saved.triggers_paused;
    }
    let listener = match pipe::Listener::bind(&endpoints.pipe) {
        Ok(listener) => {
            info!("tray: pipe ready");
            Some(listener)
        }
        Err(err) => {
            // The menu still works; the command line reports KS501.
            warn!("tray: {err}");
            status.pipe_error = Some(err.to_string());
            None
        }
    };
    // The window talks to the pipe: it opens once that is bound.
    if window {
        open_window();
    }
    let names = {
        let mut source = lock(&source);
        source.refresh();
        status.config_error = source.problem().map(str::to_owned);
        source.profile_names()
    };
    let exit_code = Rc::new(Cell::new(exit::OK));

    let initial_icon = status.icon();
    info!("tray: icon {}", icon_name(initial_icon));
    let result = window::run(initial_icon, &status.tooltip(), |waker| {
        let (done_tx, done_rx) = mpsc::channel();
        let job_waker = waker.clone();
        let mut controller = Controller::spawn(
            EngineRunner::new(store.clone(), Arc::clone(&source)),
            move |done| {
                if done_tx.send(done).is_ok() {
                    job_waker.wake();
                }
            },
        );
        let (incoming_tx, incoming_rx) = mpsc::channel();
        if let Some(listener) = listener {
            let pipe_waker = waker.clone();
            server::spawn(listener, incoming_tx, move || pipe_waker.wake());
        }
        // S3: nothing stays frozen after a crash; then `neutral`.
        controller.request(Job::Recover);
        let mut app = App {
            controller,
            done: done_rx,
            incoming: incoming_rx,
            status,
            source: Arc::clone(&source),
            config_file: config_file.clone(),
            store: store.clone(),
            exit_code: Rc::clone(&exit_code),
            shown: Some(initial_icon),
            running: Vec::new(),
            config_dir: config_dir.clone(),
            waiting: Vec::new(),
            menu: Menu::default(),
            hotkeys: hotkeys::Registrar::default(),
            updates: update::Updates::new(waker.clone()),
            waker: waker.clone(),
            autostart_answer: None,
            unreadable_logged: std::collections::HashSet::new(),
            update_started: None,
            update_waiting: None,
            update_held: None,
            update_deferred: false,
            shown_menu: None,
            session_ending: false,
            automation: triggers::Automation::new(trigger_debounce(), triggers_paused),
            polling: false,
            starts: starts::StartWatch::default(),
            starts_setting_read: None,
            notify_new: false,
            watch_reapplied: None,
            watch_due: false,
            watch_streak: 0,
            was_watching: false,
            images: HashMap::new(),
            notice: None,
            balloons: model::Balloons::default(),
            balloons_stale: window::message_duration()
                .map_or(BALLOON_STALE_FALLBACK, |d| d + BALLOON_STALE_MARGIN),
            polls: 0,
            shown_tip: String::new(),
        };
        app.refresh_autostart();
        app.ask_autostart_once();
        app.refresh_update();
        app.schedule_update();
        app.sync_hotkeys();
        if app.automation.paused() {
            info!("tray: automatic switching is paused");
        }
        app.arm_poll();
        app.menu = Menu::build(&names, &app.status);
        Box::new(app)
    });
    match result {
        Ok(()) => {
            info!("tray: ended");
            exit_code.get()
        }
        Err(err) => {
            warn!("{err}");
            eprintln!("error: {err}");
            exit::ERRORS
        }
    }
}

/// A command-line client waiting for the result of a job.
struct Waiter {
    reply: Sender<Body>,
    /// Lines shown before the result (configuration warnings).
    notes: Vec<String>,
}

impl Waiter {
    /// Sending fails when the client went away (`--no-wait`); that is fine.
    fn send(&self, body: Body) {
        let _ = self.reply.send(body);
    }
}

struct App {
    controller: Controller,
    done: Receiver<Done>,
    incoming: Receiver<Incoming>,
    status: Status,
    source: Arc<Mutex<Source>>,
    config_file: Option<PathBuf>,
    store: Store,
    /// The menu last shown; `WM_COMMAND` IDs refer to it.
    menu: Menu,
    exit_code: Rc<Cell<u8>>,
    /// The icon state last drawn.
    shown: Option<Icon>,
    /// Clients of the running job, and of the waiting one (W3).
    running: Vec<Waiter>,
    waiting: Vec<Waiter>,
    /// Where `settings.json` lives.
    config_dir: Option<PathBuf>,
    hotkeys: hotkeys::Registrar,
    /// UP1: the daily check and its child process.
    updates: update::Updates,
    /// Wakes this window thread from other threads.
    waker: window::Waker,
    /// UP1: the version whose automatic install was started, and the one
    /// already reported as waiting (each is logged once).
    update_started: Option<String>,
    update_waiting: Option<String>,
    /// The failed version the automatic install leaves alone, and until
    /// when; logged once.
    update_held: Option<(String, u64)>,
    /// UP1: this tray refused `quit_if_idle` to the running automatic
    /// install, which therefore gives up; it is started again later.
    update_deferred: bool,
    /// The menu as it was shown last; a chosen ID is resolved against it.
    shown_menu: Option<Menu>,
    /// Windows asked to end the session and has not cancelled it: no
    /// automatic install starts and `quit_if_idle` is refused.
    session_ending: bool,
    /// L8: the answer to the start-at-sign-in question, while it is open.
    autostart_answer: Option<Receiver<bool>>,
    /// U4: what the trigger programs ask for.
    automation: triggers::Automation,
    /// U4: the poll timer runs.
    polling: bool,
    /// H9: programs that start while a profile is active, from the
    /// helper's process events.
    starts: starts::StartWatch,
    /// H9: when `settings.json` was read for `notify_new` last.
    starts_setting_read: Option<Instant>,
    /// H9: `notify_new` as read from `settings.json`.
    notify_new: bool,
    /// M4c: when the active profile was last applied again because a
    /// program of it started (`watch`).
    watch_reapplied: Option<Instant>,
    /// M4c: a program of the watching profile started and the profile was
    /// not applied again yet.
    watch_due: bool,
    /// M4c: re-applications in a row that each came soon after the last
    /// (a program that starts again after the profile stopped it); each
    /// doubles the wait.
    watch_streak: u32,
    /// M4c: the active profile watched at the last poll.
    was_watching: bool,
    /// U4: image paths of processes named like a trigger with a path, by
    /// PID and creation time, so each is opened once (A7). A read that
    /// failed is tried again at the next polls, at most
    /// [`IMAGE_ATTEMPTS`] times (`Err` counts the attempts).
    images: HashMap<(u32, u64), Result<PathBuf, ImageFailure>>,
    /// U4: polls so far; failed image reads count once per poll.
    polls: u64,
    /// U4: the last automatic switch's notification and its way back.
    notice: Option<Notice>,
    /// U4: notifications of this tray on screen, from `NIN_BALLOONSHOW`
    /// and `NIN_BALLOONTIMEOUT`/`NIN_BALLOONHIDE`; never below 0, reset
    /// when Explorer restarts.
    balloons: model::Balloons,
    /// U4: after this long without a notification event the count above
    /// starts afresh.
    balloons_stale: Duration,
    /// The tooltip last drawn; an unchanged icon and tooltip are not sent
    /// to the shell again (the poll redraws every few seconds).
    shown_tip: String,
    /// L9: processes whose path could not be read for a re-check, logged
    /// once each until the next switch.
    unreadable_logged: std::collections::HashSet<String>,
}

/// U4: the way back of the last automatic switch. The menu offers it
/// (`Back to <previous>`) until the next request, the next notification or
/// the end of the session. A click on the notification is honoured only
/// while it is on screen as the only one: `NIN_BALLOONUSERCLICK` does not
/// say which notification was clicked, and an older one may say something
/// else.
struct Notice {
    back_to: String,
    /// `NIN_BALLOONSHOW` arrived and the notification has not left yet.
    shown: bool,
}

/// U4: a failed image path read: attempts so far, and the poll of the
/// last one.
#[derive(Debug, Clone, Copy)]
struct ImageFailure {
    attempts: u8,
    poll: u64,
}

/// U4: how often reading the image path of a process is tried.
const IMAGE_ATTEMPTS: u8 = 3;

/// U4: margin over the time Windows shows a notification before the
/// count of notifications on screen starts afresh, and the time used when
/// that setting cannot be read.
const BALLOON_STALE_MARGIN: Duration = Duration::from_secs(60);
const BALLOON_STALE_FALLBACK: Duration = Duration::from_secs(300);

/// Test only: `log` writes the notification to the log instead of showing
/// it, so tests leave nothing in the notification centre.
const NOTIFY_ENV: &str = "KALTSTELLEN_NOTIFY";

/// L8: answers the start-at-sign-in question without a dialog (`yes`,
/// `no`) or skips it (`skip`); for tests and managed installations.
const AUTOSTART_PROMPT_ENV: &str = "KALTSTELLEN_AUTOSTART_PROMPT";

/// L9: how often errors of the last switch are checked again while one
/// could still resolve itself.
const RECHECK: Duration = Duration::from_secs(30);

/// How long `WM_QUERYENDSESSION` waits for everything to be resumed. Windows
/// treats a window that takes about 5 s as not responding.
const END_SESSION_WAIT: Duration = Duration::from_secs(4);

/// U4, decided detail 3: how often the trigger programs are looked for, in
/// the range of 2 to 5 s. Each poll costs about 3.5 ms of CPU time (the
/// kernel builds the process list); at 5 s that is about 0.07 % of one core,
/// within Q1 however it is read, where 3 s would be 0.12 % (spec L12).
const TRIGGER_POLL: Duration = Duration::from_secs(5);

/// Test only: replaces [`TRIGGER_POLL`] and the debounce of
/// [`triggers::DEBOUNCE`], in milliseconds, so the tests need not wait.
const TRIGGER_POLL_ENV: &str = "KALTSTELLEN_TRIGGER_POLL_MS";
const TRIGGER_DEBOUNCE_ENV: &str = "KALTSTELLEN_TRIGGER_DEBOUNCE_MS";

fn millis_from_env(name: &str) -> Option<Duration> {
    std::env::var(name)
        .ok()
        .and_then(|v| v.trim().parse::<u64>().ok())
        .map(Duration::from_millis)
}

fn trigger_poll() -> Duration {
    millis_from_env(TRIGGER_POLL_ENV).map_or(TRIGGER_POLL, |d| d.max(Duration::from_millis(100)))
}

fn trigger_debounce() -> Duration {
    millis_from_env(TRIGGER_DEBOUNCE_ENV).unwrap_or(triggers::DEBOUNCE)
}

fn icon_name(icon: Icon) -> &'static str {
    match icon {
        Icon::Neutral => "neutral",
        Icon::Active => "active",
        Icon::Error => "error",
    }
}

impl App {
    fn redraw(&mut self) {
        let icon = self.status.icon();
        let tip = self.status.tooltip();
        let tip_unchanged = tip == self.shown_tip;
        if self.shown != Some(icon) {
            // Logged so the state is visible without looking at the screen.
            info!("tray: icon {}", icon_name(icon));
            self.shown = Some(icon);
        } else if tip_unchanged {
            return;
        }
        window::show(icon, &tip);
        self.shown_tip = tip;
    }

    /// The waiting request was replaced by `by` (W3): its clients end with
    /// exit code 3.
    fn supersede(&mut self, superseded: Option<Job>, by: &str) {
        if superseded.is_some() {
            for waiter in self.waiting.drain(..) {
                waiter.send(Body::Superseded { by: by.to_owned() });
            }
        }
    }

    /// A request from the menu (`waiter` is `None`) or the pipe.
    /// L9: runs the re-check timer while an error could still resolve.
    fn arm_recheck(&self) {
        window::set_recheck(self.status.resolvable().then_some(RECHECK));
    }

    /// L9: drops the errors of the last switch whose cause is gone, from
    /// the handle-free snapshot (no process is opened, A7). A failed start
    /// that names its path needs a process of that name running from that
    /// path: only for those processes the image path is read, with
    /// `PROCESS_QUERY_LIMITED_INFORMATION` (A7's fallback). One whose path
    /// cannot be read does not resolve it; that is logged once.
    fn recheck_errors(&mut self) {
        if !self.status.resolvable() {
            window::set_recheck(None);
            return;
        }
        let Ok(snapshot) = Snapshot::take() else {
            return;
        };
        let session = snapshot.own_session();
        let unreadable = std::cell::RefCell::new(Vec::new());
        let running = |exe: &str, path: Option<&str>| {
            let exe = exe.to_lowercase();
            let exe = if exe.ends_with(".exe") {
                exe
            } else {
                format!("{exe}.exe")
            };
            match path {
                None => snapshot
                    .processes()
                    .iter()
                    .any(|p| Some(p.session_id) == session && p.name.to_lowercase() == exe),
                Some(path) => {
                    let (found, errors) = snapshot.program_at(&exe, Path::new(path));
                    unreadable.borrow_mut().extend(errors);
                    !found.is_empty()
                }
            }
        };
        let gone = self.status.resolve(&running);
        for err in unreadable.into_inner() {
            let text = err.to_string();
            // Bounded: a process that keeps restarting has a new PID in
            // its text each time.
            if self.unreadable_logged.len() < 64 && self.unreadable_logged.insert(text.clone()) {
                info!("tray: re-check cannot read a process path, its error stays: {text}");
            }
        }
        for gone in gone {
            info!("tray: error resolved by itself: {}", gone.text);
        }
        self.arm_recheck();
        self.redraw();
    }

    /// WP9: registers what the configuration asks for now (C4 re-read),
    /// releases what it no longer does.
    fn sync_hotkeys(&mut self) {
        let wanted = hotkeys::wanted(&lock(&self.source).config().profiles);
        self.hotkeys.sync(
            &wanted,
            &mut |b| {
                window::register_hotkey(
                    b.id,
                    hotkeys::modifier_flags(&b.hotkey),
                    b.hotkey.virtual_key(),
                )
            },
            &mut window::unregister_hotkey,
        );
        self.status.hotkeys_unavailable = self.hotkeys.unavailable.clone();
    }

    /// N1: after a profile was applied, the snapshot of unassigned programs.
    fn refresh_unassigned(&mut self) {
        self.activate_starts();
        self.status.unassigned = match paths::data_dir() {
            Some(dir) if !self.status.is_neutral() => {
                let config = lock(&self.source).config().clone();
                let list = crate::unassigned::refresh(&dir, &config, &self.status.active);
                info!(
                    "tray: {} unassigned program(s) in \"{}\"",
                    list.len(),
                    self.status.active
                );
                list.len()
            }
            _ => 0,
        };
    }

    /// Reads start at sign-in for the menu.
    /// L8: asks once, on the very first start, whether the tray should
    /// start with Windows. The question runs on a thread of its own, so
    /// recovery, the pipe and the menu go on meanwhile; the answer comes
    /// back through the waker. `KALTSTELLEN_AUTOSTART_PROMPT` = `yes`, `no`
    /// answers without a dialog, `skip` does not ask (tests).
    fn ask_autostart_once(&mut self) {
        let Some(dir) = self.config_dir.clone() else {
            return;
        };
        if settings::load(&dir).0.autostart_asked {
            return;
        }
        if self.status.autostart == Some(true) {
            // Already on: nothing to ask.
            let _ = settings::edit(&dir, |s| s.autostart_asked = true);
            return;
        }
        let preset = std::env::var(AUTOSTART_PROMPT_ENV).ok();
        let (tx, rx) = mpsc::channel();
        self.autostart_answer = Some(rx);
        let waker = self.waker.clone();
        match preset.as_deref().map(str::trim) {
            Some("skip") => {
                self.autostart_answer = None;
            }
            Some("yes") | Some("no") => {
                let _ = tx.send(preset.as_deref().map(str::trim) == Some("yes"));
                waker.wake();
            }
            _ => {
                info!("tray: asking whether to start at sign-in");
                std::thread::spawn(move || {
                    let yes = crate::win32::dialog::ask_yes_no(
                        "kaltstellen",
                        "Start kaltstellen automatically when you sign in to Windows?\n\n\
                         You can change this later with \"Start at sign-in\" in the main window \
                         (\"Open kaltstellen…\" in the tray menu) or with \
                         \"kaltstellen autostart on|off\".",
                    );
                    let _ = tx.send(yes);
                    waker.wake();
                });
            }
        }
    }

    /// The answer to the start-at-sign-in question arrived.
    fn autostart_answered(&mut self) {
        let Some(answer) = self
            .autostart_answer
            .as_ref()
            .and_then(|rx| rx.try_recv().ok())
        else {
            return;
        };
        self.autostart_answer = None;
        info!(
            "tray: start at sign-in question answered {}",
            if answer { "yes" } else { "no" }
        );
        if let Some(dir) = &self.config_dir
            && let Err(err) = settings::edit(dir, |s| s.autostart_asked = true)
        {
            warn!("tray: {err}");
        }
        if answer && self.status.autostart != Some(true) {
            self.toggle_autostart();
        }
    }

    fn refresh_autostart(&mut self) {
        let at = autostart::Location::current();
        self.status.autostart = match std::env::current_exe()
            .map_err(|e| Error::new(Code::AutostartIo, e.to_string()))
            .and_then(|exe| autostart::state(&at, &exe))
        {
            Ok(state) => Some(state == autostart::State::On),
            Err(err) => {
                warn!("tray: {err}");
                None
            }
        };
    }

    fn toggle_autostart(&mut self) {
        let at = autostart::Location::current();
        let result = std::env::current_exe()
            .map_err(|e| Error::new(Code::AutostartIo, e.to_string()))
            .and_then(|exe| match autostart::state(&at, &exe)? {
                autostart::State::On => autostart::disable(&at).map(|_| false),
                _ => autostart::enable(&at, &exe).map(|()| true),
            });
        match result {
            Ok(on) => {
                info!("tray: start at sign-in {}", if on { "on" } else { "off" });
                self.status.autostart = Some(on);
            }
            Err(err) => warn!("tray: {err}"),
        }
    }

    /// UP1: mode and the last known version for the menu.
    fn refresh_update(&mut self) {
        let (mode, _, offer) = update::menu_state(self.config_dir.as_deref());
        self.status.update_mode = mode;
        self.status.update = offer;
    }

    /// UP1: sets the timer for the next automatic check, or for the moment
    /// a version that could not be installed is tried again, whichever
    /// comes first.
    fn schedule_update(&mut self) {
        let check = self.updates.next_delay(self.config_dir.as_deref());
        let retry = self.retry_delay();
        if let Some(delay) = check {
            info!("tray: next update check in {} s", delay.as_secs());
        }
        if let Some(delay) = retry {
            info!("tray: failed update tried again in {} s", delay.as_secs());
        }
        if let Some(delay) = [check, retry].into_iter().flatten().min() {
            window::set_timer(delay);
        }
    }

    /// UP1: how long until a held-back version may be installed
    /// automatically again; `None` when nothing is held back, the mode does
    /// not install by itself, or this copy cannot install it.
    fn retry_delay(&self) -> Option<Duration> {
        if self.status.update_mode != crate::update::Mode::Auto {
            return None;
        }
        let offer = self.status.update.as_ref()?;
        if offer.blocked.is_some() {
            return None;
        }
        let retry_at = offer.retry_at?;
        Some(Duration::from_secs(
            retry_at.saturating_sub(crate::update::record::now()),
        ))
    }

    /// UP1: sets the update mode for the main window or `kaltstellen update
    /// --mode` (pipe `set_update_mode`). A mode in effect that checks
    /// checks at once, as the menu items did before they moved to the
    /// window.
    fn set_update_mode(&mut self, mode: crate::update::Mode) -> Result<(), String> {
        let dir = self
            .config_dir
            .as_ref()
            .ok_or("no configuration directory")?;
        if let Err(err) = settings::edit(dir, |s| s.update = Some(mode.name().to_owned())) {
            warn!("tray: {err}");
            return Err(err.to_string());
        }
        info!("tray: update mode {}", mode.name());
        self.refresh_update();
        // `KALTSTELLEN_UPDATE` may decide another mode; that one counts.
        if self.status.update_mode.checks() {
            // Look now; the timer follows the result.
            self.updates.start_check();
        }
        Ok(())
    }

    /// UP1: `neutral` is active, no switch runs or waits, the tray is not
    /// quitting and the session is not ending. Asked on this window thread,
    /// so the answer holds for the request that is being handled.
    fn idle_in_neutral(&self) -> bool {
        !self.session_ending
            && self.status.is_neutral()
            && self.controller.running().is_none()
            && self.controller.pending().is_none()
            && !self.controller.is_quitting()
    }

    /// UP1: installs a found update by itself in mode `auto`, but only
    /// while `neutral` is active and no switch runs or waits, so nothing is
    /// thawed or disturbed. Otherwise it waits: it is tried again when the
    /// tray returns to `neutral` and after every later check. The install
    /// itself asks again right before the tray quits (`quit_if_idle`).
    /// A version that could not be installed here is left alone for a
    /// while (1, 2, 4, then 7 days); the timer tries it again when the wait
    /// ends (`timer`), or the next return to `neutral` or check does.
    fn maybe_install_update(&mut self) {
        let Some(offer) = self.status.update.clone() else {
            return;
        };
        if let Some(retry_at) = offer
            .retry_at
            .filter(|&at| crate::update::record::now() < at)
        {
            if self.update_held.as_ref() != Some(&(offer.version.clone(), retry_at)) {
                info!(
                    "tray: update {} could not be installed here recently; it is tried again automatically in {} h",
                    offer.version,
                    retry_at
                        .saturating_sub(crate::update::record::now())
                        .div_ceil(3600)
                );
                self.update_held = Some((offer.version.clone(), retry_at));
            }
            return;
        }
        let now = crate::update::may_install_now(
            self.status.update_mode,
            offer.blocked.is_none(),
            self.status.is_neutral(),
            self.idle_in_neutral(),
        );
        if now {
            // An install that runs already is not started or logged again.
            if !self.updates.installing()
                && self.update_started.as_deref() != Some(offer.version.as_str())
            {
                info!("tray: installing update {} automatically", offer.version);
                self.update_started = Some(offer.version.clone());
                self.updates.install_unattended();
            }
        } else if self.status.update_mode == crate::update::Mode::Auto
            && offer.blocked.is_none()
            && self.update_waiting.as_deref() != Some(offer.version.as_str())
        {
            info!(
                "tray: update {} waits until neutral is active and no switch runs",
                offer.version
            );
            self.update_waiting = Some(offer.version.clone());
        }
    }

    /// Carries out the menu command of item `id`.
    fn run(&mut self, id: u16, command: Option<MenuCommand>) {
        match command {
            Some(MenuCommand::Switch(target)) => {
                if self.controller.is_quitting() {
                    return;
                }
                info!("tray: menu asks for \"{target}\"");
                self.submit(Job::switch(&target), None);
            }
            Some(MenuCommand::BackTo(shown)) => {
                info!("tray: menu asks to go back to \"{shown}\"");
                self.go_back(Some(&shown));
            }
            Some(MenuCommand::OpenWindow) => open_window(),
            Some(MenuCommand::InstallUpdate) => {
                if self.updates.installing() {
                    info!("tray: an update is being installed already");
                } else {
                    update::open_install();
                }
            }
            Some(MenuCommand::Quit) => self.quit(),
            None => info!("tray: command {id} has no meaning; ignored"),
        }
    }

    /// Writes the offered last profile to `settings.json` (S5).
    fn save_offer(&self) {
        let Some(dir) = &self.config_dir else {
            return;
        };
        // Only the offer: the update mode (UP1) in the same file stays.
        let offer = self.status.offer.clone();
        if let Err(err) = settings::edit(dir, |s| s.last_profile = offer) {
            warn!("tray: {err}");
        }
    }

    /// The profile that is active once the running and the waiting job
    /// are done (U4 compares triggers against it).
    /// A waiting manual request is not counted: the end of the session
    /// drops it, and an automatic request never replaces it anyway.
    fn will_be_active(&self) -> String {
        self.controller
            .pending()
            .filter(|j| j.is_automatic())
            .or(self.controller.running())
            .map_or_else(|| self.status.active.clone(), |j| j.leads_to().to_owned())
    }

    /// The `status` reply of the pipe (also the answer to `set_automation`
    /// and `set_update_mode`).
    fn tray_status(&self) -> TrayStatus {
        TrayStatus {
            state: self.store.peek().unwrap_or_default(),
            running: self.controller.running().map(ToString::to_string),
            waiting: self.controller.pending().map(ToString::to_string),
            automation: self.automation_status(),
            last_switch: self.status.last_switch(),
            notes: Vec::new(),
        }
    }

    /// U4: for `kaltstellen status`; `None` while no profile has triggers.
    fn automation_status(&self) -> Option<AutomationStatus> {
        if !self.triggers_configured() {
            return None;
        }
        let view = self.automation.view(&self.will_be_active());
        let (trigger, returns_to) = match view.applied {
            Some((_, program, base)) => (Some(program), Some(base)),
            None => (None, None),
        };
        let waiting_for = view.waiting.first().map(|(_, winner)| winner.clone());
        Some(
            AutomationStatus {
                paused: view.paused,
                held: view.held,
                trigger,
                returns_to,
                waiting: view.waiting.into_iter().map(|(p, _)| p).collect(),
                waiting_for,
            }
            .bounded(),
        )
    }

    /// `profile` if the configuration in effect has it (or it is
    /// `neutral`), else `neutral` (U4: a profile to go back to may have
    /// left the file meanwhile).
    fn known_or_neutral(&self, profile: &str) -> String {
        let known = profile.eq_ignore_ascii_case(NEUTRAL)
            || lock(&self.source).config().profile(profile).is_some();
        if known {
            profile.to_owned()
        } else {
            info!("tray: \"{profile}\" is no longer in the configuration; going to neutral");
            NEUTRAL.to_owned()
        }
    }

    /// U4: whether some profile of the configuration in effect has
    /// triggers.
    fn triggers_configured(&self) -> bool {
        triggers::configured(&lock(&self.source).config().profiles)
    }

    /// U4: starts the poll when some profile has triggers and automatic
    /// switching is on, stops it while paused, quitting or while the
    /// session ends. When the triggers are gone from the file, the next
    /// poll ends them first and then stops itself. Updates the menu's view.
    fn arm_poll(&mut self) {
        let configured = self.triggers_configured();
        let off = self.controller.is_quitting() || self.session_ending;
        // H9: the poll also asks the helper for started programs.
        self.refresh_notify_setting(false);
        // The timer also serves the start watch while a profile other than
        // neutral is active (switching `notify_new` on in the window is
        // noticed by the poll); in neutral nothing is watched.
        let starts = !self.status.is_neutral();
        let wanted = !off && ((configured && !self.automation.paused()) || starts);
        if !wanted && self.polling {
            window::set_poll(None);
            self.polling = false;
            info!("tray: trigger poll stopped");
        } else if wanted && !self.polling {
            let interval = trigger_poll();
            // A timer that could not be set is tried again at the next
            // call (menu, end of a switch).
            if window::set_poll(Some(interval)) {
                self.polling = true;
                info!("tray: trigger poll every {} ms", interval.as_millis());
            }
        }
        self.status.automation = configured.then(|| self.automation.view(&self.will_be_active()));
    }

    /// H9: `notify_new` of `settings.json`, read again at most every 20 s
    /// unless `force`. A change made in the main window reaches the poll
    /// the next time it asks.
    fn refresh_notify_setting(&mut self, force: bool) {
        let due = self
            .starts_setting_read
            .is_none_or(|t| force || t.elapsed() >= Duration::from_secs(20));
        if !due {
            return;
        }
        self.starts_setting_read = Some(Instant::now());
        let on = self
            .config_dir
            .as_deref()
            .is_some_and(|dir| settings::load(dir).0.notifying_new());
        if on != self.notify_new {
            info!(
                "tray: announcing new programs is {}",
                if on { "on" } else { "off" }
            );
            self.notify_new = on;
        }
    }

    /// M4c: whether the active profile has `watch = true`.
    fn active_watches(&self) -> bool {
        !self.status.is_neutral()
            && lock(&self.source)
                .config()
                .profile(&self.status.active)
                .is_some_and(|p| p.watch)
    }

    /// H9: a profile was applied: only programs that start from now on
    /// count, and what was noted for the profile before is forgotten.
    fn activate_starts(&mut self) {
        self.watch_due = false;
        self.starts
            .activated(&self.status.active, crate::unassigned::now());
        if !self.status.is_neutral()
            && let Some(dir) = paths::data_dir()
            && let Err(err) = crate::unassigned::clear_started(&dir, &self.status.active)
        {
            warn!("tray: {err}");
        }
    }

    /// H9: asks the helper which programs started, notes those the active
    /// profile has no entry for and announces them. Without the helper (or
    /// its `observe` role) the answer is empty and the next question waits.
    fn poll_starts(&mut self) {
        if self.controller.is_quitting() || self.session_ending {
            return;
        }
        self.refresh_notify_setting(false);
        // Questions to the helper are needed to announce new programs
        // (`notify_new`) or to follow a profile with `watch` (M4c).
        let watching = self.active_watches();
        if watching != self.was_watching {
            // `watch` was switched on (or the profile changed): only starts
            // from now on count, not those since the profile was applied.
            self.was_watching = watching;
            self.watch_due = false;
            info!(
                "tray: watching \"{}\" is {}",
                self.status.active,
                if watching { "on" } else { "off" }
            );
            // Right after the profile was applied the starts since then
            // count (the poll that notices `watch` may come a few
            // seconds later); a profile that was applied long ago starts
            // counting now.
            if watching && self.starts.age(crate::unassigned::now()) > WATCH_NOTICE {
                self.activate_starts();
            }
        }
        self.starts.set_enabled(self.notify_new || watching);
        if !self.starts.enabled() {
            return;
        }
        let active = self.status.active.clone();
        let watched = self.starts.profile().to_owned();
        let now = crate::unassigned::now();
        if watched != active.trim().to_lowercase()
            && !(watched.is_empty() && self.status.is_neutral())
        {
            // Applied by a path that did not announce it (recovery).
            self.activate_starts();
        }
        if let Some(after) = self.starts.due(now) {
            let answer =
                crate::helper::client::ask_after(crate::helper::protocol::Command::Events, after)
                    .map_err(|err| err.to_string())
                    .map(|reply| {
                        reply.and_then(|reply| {
                            Some(starts::Fetched {
                                events: reply.events?,
                                next: reply.next?,
                            })
                        })
                    });
            if let Err(err) = &answer {
                // Once per backoff step is enough.
                info!("tray: process events: {err}");
            }
            self.starts.feed(now, answer);
        }
        if watching {
            self.try_reapply();
        }
        if self.status.is_neutral() || !self.starts.has_pending() {
            self.announce_starts(now, &active);
            return;
        }
        let Some(dir) = paths::data_dir() else {
            return;
        };
        let decided: std::collections::HashSet<String> = lock(&self.source)
            .config()
            .profile(&active)
            .map(|p| p.apps.iter().map(|a| a.exe.to_lowercase()).collect())
            .unwrap_or_default();
        let dismissed = crate::unassigned::load(&dir)
            .0
            .dismissed
            .get(&active.trim().to_lowercase())
            .cloned();
        let ctx = starts::Context {
            decided: &decided,
            dismissed: dismissed.as_ref(),
        };
        // `settled` is every start that stayed running; other features that
        // follow starts (a profile that watches) hook in here.
        let settled = self.starts.settled(now);
        if watching {
            let acting: std::collections::HashSet<String> = lock(&self.source)
                .config()
                .profile(&active)
                .map(|p| {
                    p.apps
                        .iter()
                        .filter(|a| {
                            matches!(
                                a.action,
                                config::AppAction::Stop { .. } | config::AppAction::Suspend { .. }
                            )
                        })
                        .map(|a| a.exe.to_lowercase())
                        .collect()
                })
                .unwrap_or_default();
            self.note_watched(&settled, &acting);
            self.try_reapply();
        }
        let found = if self.notify_new {
            self.starts.offers(&settled, &ctx)
        } else {
            Vec::new()
        };
        if !found.is_empty() {
            match crate::unassigned::note_started(&dir, &active, &found, now) {
                Ok(fresh) => {
                    for (exe, _) in &found {
                        if !fresh.iter().any(|f| f.eq_ignore_ascii_case(exe)) {
                            self.starts.forget(exe);
                        }
                    }
                    info!("tray: started without an entry in \"{active}\": {fresh:?}");
                }
                Err(err) => warn!("tray: {err}"),
            }
        }
        self.announce_starts(now, &active);
    }

    /// M4c: a program that the active profile stops or suspends
    /// (`watch = true`; `acting` holds those entries) started after the profile was applied: the profile is to be applied
    /// again ([`try_reapply`](Self::try_reapply)).
    fn note_watched(
        &mut self,
        settled: &[crate::helper::events::Event],
        acting: &std::collections::HashSet<String>,
    ) {
        if let Some(hit) = settled
            .iter()
            .find(|e| e.path.is_some() && acting.contains(&e.exe.to_lowercase()))
        {
            info!("tray: \"{}\" started while the profile watches", hit.exe);
            self.watch_due = true;
        }
    }

    /// M4c: applies the active profile again when a program of it started,
    /// which handles the program as in a switch. Not while a switch runs
    /// or waits (the second enumeration of W6 catches what starts
    /// meanwhile) and at most once per [`WATCH_GAP`]; it stays due until
    /// then. It is not a manual switch: automatic switching is not held
    /// and the offer of the last session stays.
    fn try_reapply(&mut self) {
        if !self.watch_due {
            return;
        }
        if self.controller.running().is_some()
            || self.controller.pending().is_some()
            || self.controller.is_quitting()
            || self
                .watch_reapplied
                .is_some_and(|t| t.elapsed() < WATCH_GAP * (1 << self.watch_streak))
        {
            return;
        }
        self.watch_due = false;
        // A program that keeps coming back after being stopped must not keep
        // the profile busy: each re-application soon after the last doubles
        // the wait (up to 16 times the gap).
        let soon = self
            .watch_reapplied
            .is_some_and(|t| t.elapsed() < WATCH_GAP * (2 << self.watch_streak));
        self.watch_streak = if soon {
            (self.watch_streak + 1).min(4)
        } else {
            0
        };
        self.watch_reapplied = Some(Instant::now());
        let active = self.status.active.clone();
        info!("tray: applying \"{active}\" again (it watches)");
        if let Decision::Start(job) = self.controller.request(Job::switch(&active)) {
            self.status.switching = Some(job.leads_to().to_owned());
            self.redraw();
        }
    }

    /// H9: the notification for programs noted meanwhile, if one is due.
    fn announce_starts(&mut self, now: u64, active: &str) {
        if !self.notify_new {
            return;
        }
        if let Some((title, text)) = self.starts.announcement(now, active) {
            info!("tray: notification: {title}: {text}");
            let logged_only = std::env::var(NOTIFY_ENV).is_ok_and(|v| v.trim() == "log");
            if !logged_only && !window::balloon(&title, &text) {
                warn!("tray: the notification could not be shown");
            } else if let Some(notice) = &mut self.notice {
                // The new balloon replaces the one of "Back to": a click
                // on it does nothing (the menu item still works).
                notice.shown = false;
            }
        }
    }

    /// U4: one poll of the handle-free snapshot. Processes are opened only
    /// for triggers with a path, and then only those of the trigger's name,
    /// each once (A7).
    fn poll_triggers(&mut self) {
        if self.automation.paused() || self.controller.is_quitting() || self.session_ending {
            self.arm_poll();
            return;
        }
        if !self.triggers_configured() && self.automation.idle() {
            // Only the start watch needs the timer: no snapshot.
            self.arm_poll();
            return;
        }
        let Ok(snapshot) = Snapshot::take() else {
            return;
        };
        let session = snapshot.own_session();
        if !self.images.is_empty() {
            self.images
                .retain(|(pid, created), _| snapshot.find(*pid, *created).is_some());
        }
        self.polls += 1;
        let poll = self.polls;
        let images = &mut self.images;
        let mut image = |info: &ProcessInfo| -> Option<PathBuf> {
            let fresh = ImageFailure {
                attempts: 0,
                poll: 0,
            };
            let entry = images.entry((info.pid, info.created)).or_insert(Err(fresh));
            match entry {
                Ok(path) => return Some(path.clone()),
                // Tried at this poll already (another trigger of the same
                // name), or often enough.
                Err(f) if f.poll == poll || f.attempts >= IMAGE_ATTEMPTS => return None,
                Err(_) => {}
            }
            match Target::open(info, Access::Query)
                .and_then(|t| t.map(|t| t.image_path()).transpose())
            {
                Ok(Some(path)) => {
                    *entry = Ok(path.clone());
                    Some(path)
                }
                // Gone meanwhile: the next snapshot no longer lists it.
                Ok(None) => None,
                Err(err) => {
                    if let Err(failure) = entry {
                        if failure.attempts == 0 {
                            info!("tray: trigger check: {err}");
                        }
                        failure.attempts += 1;
                        failure.poll = poll;
                    }
                    None
                }
            }
        };
        // The file may have changed; an unchanged one is not parsed again
        // (C4). The scan works on the configuration in place.
        let source = Arc::clone(&self.source);
        let seen = {
            let mut source = lock(&source);
            source.refresh();
            self.status.config_error = source.problem().map(str::to_owned);
            triggers::scan(
                &source.config().profiles,
                snapshot.processes(),
                session,
                &mut image,
            )
        };
        let active = self.will_be_active();
        let (action, log) = self.automation.poll(Instant::now(), &seen, &active);
        for line in log {
            info!("tray: {line}");
        }
        if let Some(triggers::Action::Switch {
            profile,
            previous,
            reason,
        }) = action
        {
            let target = self.known_or_neutral(&profile);
            info!("tray: trigger asks for \"{target}\" ({reason})");
            self.submit(Job::automatic(&target, &previous, &reason), None);
        }
        if seen.is_empty() && self.polling && self.status.is_neutral() {
            // No profile has triggers any more: what ran has ended above.
            window::set_poll(None);
            self.polling = false;
            info!("tray: trigger poll stopped, no profile has triggers");
        }
        self.arm_poll();
        self.redraw();
    }

    /// U4: `Automatic switching` of the main window or `kaltstellen
    /// automation on|off` (pipe `set_automation`); remembered in
    /// `settings.json` (decided detail 7), which the tray writes. Setting
    /// the state it has already changes nothing, so a trigger that runs is
    /// not applied again. A file that cannot be written is reported; the
    /// state still changes for this session.
    fn set_automatic(&mut self, paused: bool) -> Result<(), String> {
        let saved = match &self.config_dir {
            Some(dir) => settings::edit(dir, |s| s.triggers_paused = paused).map_err(|err| {
                warn!("tray: {err}");
                err.to_string()
            }),
            None => Err("no configuration directory".to_owned()),
        };
        if paused == self.automation.paused() {
            info!(
                "tray: automatic switching already {}; unchanged",
                if paused { "paused" } else { "on" }
            );
            return saved;
        }
        self.automation.set_paused(paused);
        info!(
            "tray: automatic switching {}",
            if paused { "paused" } else { "on" }
        );
        self.arm_poll();
        self.redraw();
        saved
    }

    /// U4, decided detail 1: an automatic switch ended; tell the user and
    /// offer the way back.
    /// `target` is the profile asked for, `active` the one active now; they
    /// differ when the switch could not run at all.
    fn notify_automatic(
        &mut self,
        target: &str,
        active: &str,
        automatic: &model::Automatic,
        errors: usize,
    ) {
        let switched = active.trim().eq_ignore_ascii_case(target.trim());
        let (title, mut text) = if switched {
            (
                format!("kaltstellen: {target}"),
                format!("{}: switched to {target}", automatic.reason),
            )
        } else {
            (
                format!("kaltstellen: switch to {target} failed"),
                format!("{}: could not switch to {target}", automatic.reason),
            )
        };
        match errors {
            0 => text.push('.'),
            1 => text.push_str(", with 1 error."),
            n => text.push_str(&format!(", with {n} errors.")),
        }
        // A way back only when something changed.
        let back = !active
            .trim()
            .eq_ignore_ascii_case(automatic.previous.trim());
        if back {
            text.push_str(&format!(" Click here: Back to {}", automatic.previous));
        }
        // Set before the notification goes out, in case the shell reports
        // it on screen at once.
        self.set_notice(back.then(|| Notice {
            back_to: automatic.previous.clone(),
            shown: false,
        }));
        info!("tray: notification: {title}: {text}");
        let logged_only = std::env::var(NOTIFY_ENV).is_ok_and(|v| v.trim() == "log");
        if !logged_only && !window::balloon(&title, &text) {
            warn!("tray: the notification could not be shown");
        }
    }

    /// U4: the way back of the last automatic switch, for the menu item
    /// `Back to <previous>` and the notification click.
    fn set_notice(&mut self, notice: Option<Notice>) {
        self.status.back_to = notice.as_ref().map(|n| n.back_to.clone());
        self.notice = notice;
    }

    /// U4, decided detail 1: `Back to <previous>` from the menu or the
    /// notification. A manual switch: it holds automation like any other;
    /// a profile that left the file meanwhile becomes neutral.
    /// `shown`: the target the menu item showed; `None` for the
    /// notification click. A way back that changed meanwhile (the menu was
    /// open while another automatic switch ended) does nothing.
    fn go_back(&mut self, shown: Option<&str>) {
        let current = self.notice.as_ref().map(|n| n.back_to.as_str());
        let Some(previous) = model::back_target(current, shown) else {
            info!("tray: nothing to go back to");
            return;
        };
        if self.controller.is_quitting() {
            return;
        }
        let target = self.known_or_neutral(&previous);
        info!("tray: back to \"{target}\"");
        self.submit(Job::switch(&target), None);
    }

    fn submit(&mut self, job: Job, waiter: Option<Waiter>) {
        let joining = match &job {
            Job::Switch { guests, .. } => guests.list(),
            _ => Vec::new(),
        };
        let automatic = job.is_automatic();
        // A deliberate switch replaces the offer of the last session; an
        // automatic one does not (U4, `[assumed]`).
        if matches!(job, Job::Switch { .. }) && !automatic && self.status.offer.take().is_some() {
            self.save_offer();
        }
        // U4: the last notification's `Back to` means nothing any more.
        self.set_notice(None);
        // U4, conflict rule 1: everything but a trigger is manual here
        // (menu, hotkey, command line, window, `Back to`) and holds
        // automation until the trigger situation changes.
        if !automatic {
            if self.automation.manual() {
                info!(
                    "tray: automatic switching waits after a manual switch until a trigger program starts or ends"
                );
            }
            // The tooltip and menu no longer say "by trigger".
            if self.status.automation.is_some() {
                self.status.automation = Some(self.automation.view(&self.will_be_active()));
            }
        }
        let by = job.leads_to().to_owned();
        match self.controller.request(job) {
            Decision::Ignored => {
                self.automation.refused();
            }
            Decision::Start(job) => {
                self.status.switching = Some(job.leads_to().to_owned());
                if let Some(waiter) = waiter {
                    waiter.send(Body::Accepted { waiting: false });
                    self.running.push(waiter);
                }
            }
            Decision::Queued { superseded } => {
                self.supersede(superseded, &by);
                if let Some(waiter) = waiter {
                    waiter.send(Body::Accepted { waiting: true });
                    self.waiting.push(waiter);
                }
            }
            Decision::Dropped { superseded } => {
                self.supersede(superseded, &by);
                // The running job leads there: its result is this one's too.
                // Its caller is protected by it from its next snapshot on
                // (T16); what the job froze before stays frozen.
                if let Some(Job::Switch { guests, .. }) = self.controller.running() {
                    for guest in &joining {
                        info!(
                            "tray: caller {} joins the running switch and is protected from its next step",
                            guest.pid
                        );
                        guests.add(*guest);
                    }
                }
                if let Some(waiter) = waiter {
                    waiter.send(Body::Accepted { waiting: true });
                    self.running.push(waiter);
                }
            }
            Decision::Refused => {
                if let Some(waiter) = waiter {
                    waiter.send(Body::Rejected {
                        exit_code: exit::UNREACHABLE,
                        lines: vec![format!(
                            "error: {}: the tray is quitting or the session is ending",
                            Code::TrayUnreachable
                        )],
                    });
                }
            }
        }
        self.redraw();
    }

    fn quit(&mut self) {
        info!("tray: quit requested");
        let superseded = self.controller.quit();
        self.supersede(superseded, "quit");
        if self.controller.running() != Some(&Job::Quit) {
            info!("tray: quitting after the running switch");
        }
        self.status.switching = None;
        self.redraw();
    }

    fn profile_names(&self) -> Vec<String> {
        lock(&self.source).profile_names()
    }

    fn finished(&mut self, done: Done) {
        let errors: Vec<String> = done
            .outcome
            .errors
            .iter()
            .map(ToString::to_string)
            .collect();
        info!(
            "tray: {} finished, active profile \"{}\", {} error(s)",
            done.job,
            done.outcome.active,
            errors.len()
        );
        let mut result = Finished::new(
            done.outcome.ran,
            &done.outcome.active,
            &done.outcome.events,
            &done.outcome.errors,
        );
        if result.exit_code == exit::UNKNOWN_PROFILE {
            result.profiles = self.profile_names();
        }
        for waiter in self.running.drain(..) {
            let mut mine = result.clone();
            mine.notes = waiter.notes.clone();
            waiter.send(Body::Done(mine));
        }
        if done.job == Job::Quit {
            // L2 is done: suspended programs resumed, stopped ones stay.
            for error in &errors {
                warn!("tray: on quit: {error}");
            }
            self.exit_code.set(exit::of(true, &done.outcome.errors));
            self.hotkeys.clear(&mut window::unregister_hotkey);
            window::exit();
            return;
        }
        self.unreadable_logged.clear();
        self.status.finished(
            done.outcome.active,
            done.outcome.errors.iter().map(SwitchError::of).collect(),
        );
        self.arm_recheck();
        if done.outcome.ran && matches!(done.job, Job::Switch { .. }) {
            self.refresh_unassigned();
        }
        self.status.config_error = lock(&self.source).problem().map(str::to_owned);
        let next = self.controller.finished(&done.job);
        if let Some(next) = &next {
            self.status.switching = Some(next.leads_to().to_owned());
            self.running = std::mem::take(&mut self.waiting);
        }
        // U4: an automatic switch is announced with its way back, unless
        // another request runs already.
        if let Job::Switch {
            target,
            automatic: Some(automatic),
            ..
        } = &done.job
            && next.is_none()
        {
            let active = self.status.active.clone();
            self.notify_automatic(target, &active, automatic, errors.len());
        }
        self.arm_poll();
        self.sync_hotkeys();
        if self.status.is_neutral() {
            // UP1: an update that waited for neutral may go now.
            self.maybe_install_update();
        }
        // Items that depend on the result (acknowledge, offer) get their IDs.
        // A menu that is open keeps its own commands (`chosen`).
        self.menu = Menu::build(&self.profile_names(), &self.status);
    }

    /// A request from the command line.
    fn incoming(&mut self, incoming: Incoming) {
        let Incoming {
            command,
            reply,
            caller,
        } = incoming;
        if command == Command::QuitIfIdle && !self.idle_in_neutral() {
            // UP1: a profile was switched on while the automatic install
            // downloaded. It gives up; the tray starts it again later.
            info!("tray: the automatic update asked to quit, but the tray is busy; it waits");
            self.update_deferred = true;
            let _ = reply.send(Body::Rejected {
                exit_code: exit::ERRORS,
                lines: vec![format!(
                    "error: {}: a profile is active or a switch runs; the update waits for neutral",
                    Code::UpdateTray
                )],
            });
            return;
        }
        if matches!(command, Command::Quit | Command::QuitIfIdle) {
            // UP1: the updater quits the tray like the menu does (L2) and
            // gets the result once everything is resumed.
            info!("tray: command line asks to quit");
            if !self.controller.is_quitting() {
                self.quit();
            }
            let waiter = Waiter {
                reply,
                notes: Vec::new(),
            };
            if self.controller.running() == Some(&Job::Quit) {
                self.running.push(waiter);
            } else {
                self.waiting.push(waiter);
            }
            return;
        }
        let respond = |body| {
            let _ = reply.send(body);
        };
        if let Command::Status = command {
            // M5: errors whose cause is gone drop out first (L9), so the
            // main window marks only what is still wrong.
            self.recheck_errors();
            respond(Body::Status(self.tray_status()));
            return;
        }
        if self.controller.is_quitting() {
            respond(Body::Rejected {
                exit_code: exit::UNREACHABLE,
                lines: vec![format!(
                    "error: {}: the tray is quitting",
                    Code::TrayUnreachable
                )],
            });
            return;
        }
        // The settings that moved from the menu to the main window (owner,
        // 2026-11-23): the tray takes them over at once and stays the one
        // that writes them while it runs.
        let changed = match &command {
            Command::SetAutomation { paused } => {
                info!("tray: asked over the pipe to set automatic switching");
                // Taken over in any case; a file that could not be written
                // is said in a note, so the caller does not write it again
                // and claim that the tray takes it at its next start.
                let notes = match self.set_automatic(*paused) {
                    Ok(()) => Vec::new(),
                    Err(reason) => vec![format!(
                        "warning: taken over by the tray for this session, but not saved: {reason}"
                    )],
                };
                Some(Ok(notes))
            }
            Command::SetUpdateMode { mode } => {
                info!("tray: asked over the pipe to set the update mode");
                // The tray reads the mode from the file, so a mode that
                // could not be saved is not taken over either: refused.
                Some(
                    crate::update::Mode::parse(mode)
                        .filter(|m| m.name() == mode.trim())
                        .ok_or_else(|| {
                            format!("{}: unknown update mode \"{mode}\"", Code::TrayProtocol)
                        })
                        .and_then(|m| self.set_update_mode(m))
                        .map(|()| Vec::new()),
                )
            }
            _ => None,
        };
        if let Some(changed) = changed {
            respond(match changed {
                Ok(notes) => Body::Status(TrayStatus {
                    notes,
                    ..self.tray_status()
                }),
                Err(reason) => Body::Rejected {
                    exit_code: exit::ERRORS,
                    lines: vec![format!("error: {reason}")],
                },
            });
            return;
        }
        let (job, notes) = match command {
            Command::Switch { profile, force, .. } => {
                let target = profile.trim().to_owned();
                // The same checks and messages as in-process (E2 codes 2, 4).
                let (config, notes) = match config::for_switch(self.config_file.as_deref(), &target)
                {
                    Ok(found) => found,
                    Err(lines) => {
                        respond(Body::Rejected {
                            exit_code: exit::CONFIG_INVALID,
                            lines,
                        });
                        return;
                    }
                };
                if !target.eq_ignore_ascii_case(NEUTRAL) && config.profile(&target).is_none() {
                    let error = Error::new(
                        Code::UnknownProfile,
                        format!("no profile named \"{target}\""),
                    );
                    let mut result = Finished::new(false, NEUTRAL, &[], &[error]);
                    result.notes = notes;
                    result.profiles = config.profiles.iter().map(|p| p.name.clone()).collect();
                    respond(Body::Done(result));
                    return;
                }
                (
                    Job::Switch {
                        target,
                        force,
                        guests: Guests::of(caller),
                        automatic: None,
                    },
                    notes,
                )
            }
            Command::Recover { .. } => (Job::Recover, Vec::new()),
            Command::Status
            | Command::Quit
            | Command::QuitIfIdle
            | Command::SetAutomation { .. }
            | Command::SetUpdateMode { .. } => {
                unreachable!("answered above")
            }
        };
        info!("tray: command line asks for {job}");
        self.submit(job, Some(Waiter { reply, notes }));
    }
}

impl window::Handler for App {
    fn menu(&mut self) -> Vec<model::Item> {
        let names = {
            let mut source = lock(&self.source);
            source.refresh();
            self.status.config_error = source.problem().map(str::to_owned);
            source.profile_names()
        };
        if self.controller.running().is_none()
            && !self.controller.is_quitting()
            && let Some(state) = self.store.peek()
        {
            self.status.active = state.active_profile;
        }
        self.refresh_autostart();
        self.refresh_update();
        self.sync_hotkeys();
        self.arm_poll();
        self.redraw();
        self.menu = Menu::build(&names, &self.status);
        self.shown_menu = Some(self.menu.clone());
        self.menu.items.clone()
    }

    fn chosen(&mut self, id: u16) {
        // The menu that was shown: `finished` may have rebuilt `self.menu`
        // while it was open (a profile added to the file, the offer taken).
        let shown = self.shown_menu.take();
        if id == 0 {
            return;
        }
        let command = shown.and_then(|menu| menu.command(id).cloned());
        self.run(id, command);
    }

    fn command(&mut self, id: u16) {
        let command = self.menu.command(id).cloned();
        self.run(id, command);
    }

    fn wake(&mut self) {
        self.autostart_answered();
        while let Ok(done) = self.done.try_recv() {
            let quit = done.job == Job::Quit;
            self.finished(done);
            if quit {
                return;
            }
        }
        while let Ok(incoming) = self.incoming.try_recv() {
            self.incoming(incoming);
        }
        if self.updates.check_ended() {
            self.refresh_update();
            update::log_result(self.status.update.as_ref());
            // A version whose earlier attempt ended in an error before the
            // tray was touched (offline, checksum) is tried again after a
            // later check, at most once a day. Failures after it was
            // verified are recorded in `update.json` and tried again after
            // their wait (`retry_delay`).
            self.update_started = None;
            self.maybe_install_update();
            self.schedule_update();
        }
        if self.updates.install_ended() {
            // The automatic install ended and this tray still runs: it gave
            // up (busy, or an error), or the new version did not start and
            // `update.json` says so now.
            self.refresh_update();
            if std::mem::take(&mut self.update_deferred) {
                self.update_started = None;
                self.update_waiting = None;
                self.maybe_install_update();
            }
            // A failure recorded just now holds the version back: the timer
            // brings the next try.
            self.schedule_update();
        }
        self.redraw();
    }

    fn recheck(&mut self) {
        self.recheck_errors();
    }

    fn poll(&mut self) {
        self.poll_triggers();
        self.poll_starts();
    }

    fn balloon_clicked(&mut self) {
        // The click does not say which notification was clicked (U4): it
        // is honoured only while the latest one is on screen and no other
        // notification of the tray is. The menu item always works.
        // The clicked notification is dismissed; the shell reports no
        // timeout for it.
        let alone = self.balloons.clicked(Instant::now(), self.balloons_stale);
        let shown = self.notice.as_ref().is_some_and(|n| n.shown);
        if !(alone && shown) {
            info!("tray: notification clicked; nothing to go back to");
            return;
        }
        info!("tray: notification clicked");
        self.go_back(None);
    }

    fn balloon_shown(&mut self) {
        self.balloons.shown(Instant::now(), self.balloons_stale);
        if let Some(notice) = &mut self.notice {
            notice.shown = true;
        }
    }

    fn balloon_gone(&mut self) {
        if self.balloons.gone(Instant::now())
            && let Some(notice) = &mut self.notice
            && std::mem::take(&mut notice.shown)
        {
            info!("tray: notification gone; Back to stays in the menu");
        }
    }

    fn taskbar_created(&mut self) {
        // Explorer restarted: no notification of the old one is on screen.
        self.balloons.reset();
        if let Some(notice) = &mut self.notice {
            notice.shown = false;
        }
    }

    fn timer(&mut self) {
        if self
            .updates
            .next_delay(self.config_dir.as_deref())
            .is_some_and(|delay| delay.is_zero())
        {
            // The check's result decides on an install and sets the timer.
            if self.updates.start_check() {
                return;
            }
            // It could not be started: again a day later, and a held-back
            // version is still tried when its wait ends.
            let retry = self.retry_delay();
            let check = crate::update::record::INTERVAL;
            window::set_timer(retry.map_or(check, |r| r.min(check)));
            return;
        }
        // While a check runs, its end decides with fresh data (`wake`), and
        // the install must not write `update.json` alongside it.
        if !self.updates.checking() {
            // UP1: a version that could not be installed has waited long
            // enough. `update.json` is read again first; the wait may also
            // have ended unseen, when the menu read it meanwhile.
            let due = self.retry_delay().is_some_and(|delay| delay.is_zero());
            let held = self.update_held.clone();
            self.refresh_update();
            let released = held.is_some_and(|(version, _)| {
                self.status
                    .update
                    .as_ref()
                    .is_some_and(|o| o.version == version && o.retry_at.is_none())
            });
            if due || released {
                self.update_started = None;
                self.update_held = None;
                self.maybe_install_update();
            }
        }
        // Otherwise update.json changed meanwhile (a check from the command
        // line): the timer follows it.
        self.schedule_update();
    }

    fn close(&mut self) {
        info!("tray: WM_CLOSE received");
        self.quit();
    }

    fn query_end_session(&mut self) -> bool {
        info!("tray: session is ending; resuming everything");
        self.session_ending = true;
        // The profile to offer next time: the active one, or the offer that
        // was not taken up yet. A profile a trigger applied is not offered,
        // but the one from before the trigger (U4, decided detail 8).
        self.arm_poll();
        // A notification's way back means nothing after this.
        self.set_notice(None);
        // While an automatic switch runs or waits (a return to the profile
        // from before, for example), the profile it leads to counts.
        let automatic_ahead = [self.controller.running(), self.controller.pending()]
            .into_iter()
            .flatten()
            .any(Job::is_automatic);
        let current = if automatic_ahead {
            self.will_be_active()
        } else {
            self.status.active.clone()
        };
        if !current.trim().eq_ignore_ascii_case(NEUTRAL) {
            match self.automation.base_of(&current) {
                Some(base) if base.eq_ignore_ascii_case(NEUTRAL) => {}
                Some(base) => self.status.offer = Some(base.to_owned()),
                None => self.status.offer = Some(current),
            }
        }
        if self.status.offer.is_some() {
            self.save_offer();
        }
        let superseded = self.controller.end_session();
        self.supersede(superseded, "end of session");
        // Answer only once frozen programs run again: Windows asks them next.
        let deadline = Instant::now() + END_SESSION_WAIT;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            match self.done.recv_timeout(left) {
                Ok(done) => {
                    let last = matches!(done.job, Job::EndSession | Job::Quit);
                    self.finished(done);
                    if last {
                        break;
                    }
                }
                Err(_) => {
                    warn!("tray: resuming did not finish in time; letting the session end");
                    break;
                }
            }
        }
        self.redraw();
        true
    }

    fn end_session(&mut self, ending: bool) {
        self.session_ending = ending;
        if !ending {
            // The profile is off now; offer it again (S5).
            match &self.status.offer {
                Some(last) => info!("tray: end of session cancelled; offering \"{last}\" again"),
                None => info!("tray: end of session cancelled"),
            }
            // U4: triggers start afresh; one that still runs applies again
            // (`[assumed]`).
            self.automation.restart();
            self.arm_poll();
            self.redraw();
            return;
        }
        // Stopped records are cleared at sign-out and shutdown, kept on Quit
        // and after a crash (S5).
        let cleared = self.store.lock().and_then(|_lock| {
            let (mut state, _) = self.store.load();
            let count = state.stopped.len();
            state.stopped.clear();
            self.store.save(&state).map(|()| count)
        });
        match cleared {
            Ok(count) => info!("tray: session ends; {count} stopped record(s) cleared"),
            Err(err) => warn!("tray: session ends; {err}"),
        }
    }

    fn hotkey(&mut self, id: i32) {
        let Some(profile) = self.hotkeys.profile(id).map(str::to_owned) else {
            info!("tray: hotkey {id} has no profile; ignored");
            return;
        };
        if self.controller.is_quitting() {
            return;
        }
        // The same queue as a menu click (W3).
        info!("tray: hotkey asks for \"{profile}\"");
        self.submit(Job::switch(&profile), None);
    }

    fn power(&mut self, event: u32) {
        // Standby is left alone (S5): frozen programs stay frozen.
        let name = match event {
            0x4 => "suspend",
            0x7 => "resume after suspend",
            0x12 => "automatic resume",
            0xA => "power status change",
            0x8013 => "power setting change",
            _ => "other",
        };
        info!("tray: power event {name} (0x{event:x})");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn l1_instance_name_is_per_session_and_per_redirected_data_dir() {
        let real = instance_name(Path::new("C:\\any"), false);
        assert_eq!(real, "Local\\kaltstellen-tray");
        let a = instance_name(Path::new("C:\\tmp\\a"), true);
        let a_upper = instance_name(Path::new("C:\\TMP\\A"), true);
        let b = instance_name(Path::new("C:\\tmp\\b"), true);
        assert!(a.starts_with("Local\\kaltstellen-tray-"));
        assert_eq!(a, a_upper);
        assert_ne!(a, b);
    }

    #[test]
    fn l1_pipe_name_carries_user_session_and_data_dir() {
        let real = pipe_name("S-1-5-21-1", 3, Path::new("C:\\any"), false);
        assert_eq!(real, "\\\\.\\pipe\\kaltstellen-S-1-5-21-1-3");
        let test = pipe_name("S-1-5-21-1", 3, Path::new("C:\\tmp\\a"), true);
        let mutex = instance_name(Path::new("C:\\tmp\\a"), true);
        assert!(test.ends_with(mutex.rsplit('-').next().unwrap()));
        assert_ne!(
            pipe_name("S-1-5-21-2", 3, Path::new("C:\\any"), false),
            real
        );
        assert_ne!(
            pipe_name("S-1-5-21-1", 4, Path::new("C:\\any"), false),
            real
        );
    }
}
