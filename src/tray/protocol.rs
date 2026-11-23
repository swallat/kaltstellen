//! Messages between the command line and the tray over the named pipe
//! (spec L1, documented in `docs/pipe-protocol.md`).
//!
//! One JSON object per line, UTF-8, `\n`-terminated. The client sends one
//! request; the tray answers with zero or more `accepted` replies and ends
//! with exactly one final reply, then closes the connection. Every message
//! carries `version`; a side that does not speak that version answers or
//! fails with `KS505`.

use std::fmt::Write as _;

use serde::{Deserialize, Serialize};

use crate::engine::Event;
use crate::error::{Code, Error};
use crate::exit;
use crate::state::State;

pub const VERSION: u32 = 1;
/// Longest line either side accepts.
pub const MAX_LINE: usize = 64 * 1024;

fn yes() -> bool {
    true
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Request {
    pub version: u32,
    #[serde(flatten)]
    pub command: Command,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "command", rename_all = "snake_case")]
pub enum Command {
    /// `kaltstellen switch PROFILE [--force] [--no-wait]`. `force` confirms
    /// warn-list programs for this request only (P6).
    Switch {
        profile: String,
        #[serde(default)]
        force: bool,
        #[serde(default = "yes")]
        wait: bool,
    },
    /// `kaltstellen recover [--no-wait]`.
    Recover {
        #[serde(default = "yes")]
        wait: bool,
    },
    /// `kaltstellen status`.
    Status,
    /// UP1: quit as from the menu (L2: everything suspended is resumed);
    /// the reply `done` comes when that finished, then the tray ends.
    Quit,
    /// UP1 automatic install: `quit`, but only if `neutral` is active and no
    /// switch runs or waits at the moment the tray reads it. Otherwise the
    /// reply is `rejected` and the tray keeps running. A tray that does not
    /// know this command rejects it as malformed, so it never quits.
    QuitIfIdle,
    /// U4, from the main window and `kaltstellen automation on|off`:
    /// pauses or resumes automatic switching at once; the tray writes
    /// `triggers_paused` to `settings.json`. The final reply is `status`.
    /// A tray that does not know it rejects it as malformed (`KS505`); the
    /// caller then writes `settings.json` itself and the tray takes it
    /// over at its next start.
    SetAutomation { paused: bool },
    /// UP1, from the main window and `kaltstellen update --mode`: sets the
    /// update mode (`auto`, `manual`, `off`) as the tray's menu once did:
    /// the tray writes `update` to `settings.json` and, if the mode in
    /// effect checks, checks at once. The final reply is `status`; an
    /// unknown mode is `rejected` (exit code 1). An older tray rejects the
    /// command as malformed; the caller then writes the file itself.
    SetUpdateMode { mode: String },
}

impl Command {
    /// Whether the client waits for the final reply after `accepted`.
    pub fn waits(&self) -> bool {
        match self {
            Command::Switch { wait, .. } | Command::Recover { wait } => *wait,
            Command::Status
            | Command::Quit
            | Command::QuitIfIdle
            | Command::SetAutomation { .. }
            | Command::SetUpdateMode { .. } => true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Reply {
    pub version: u32,
    #[serde(flatten)]
    pub body: Body,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "reply", rename_all = "snake_case")]
pub enum Body {
    /// Not final. The request runs now (`waiting: false`) or waits for the
    /// running switch (`waiting: true`).
    Accepted { waiting: bool },
    /// Final: the job ran (or could not run, then `active_profile` is
    /// absent).
    Done(Finished),
    /// Final: a later request replaced this one while it waited (W3, exit
    /// code 3).
    Superseded { by: String },
    /// Final: the request was refused before it was queued; the lines say
    /// why (for example an invalid configuration, exit code 2).
    Rejected { exit_code: u8, lines: Vec<String> },
    /// Final: answer to `status`.
    Status(TrayStatus),
}

impl Body {
    /// Whether this reply ends the conversation for a client that waits.
    pub fn is_final(&self) -> bool {
        !matches!(self, Body::Accepted { .. })
    }
}

/// The result of a switch or recovery, printed by the command line exactly
/// like an in-process run.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Finished {
    pub exit_code: u8,
    /// Absent when nothing could be attempted (for example an unknown
    /// profile).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_profile: Option<String>,
    /// Lines for stderr before the events (configuration warnings).
    #[serde(default)]
    pub notes: Vec<String>,
    #[serde(default)]
    pub events: Vec<EventLine>,
    /// Errors as `KSnnn [step] program: reason`.
    #[serde(default)]
    pub errors: Vec<String>,
    /// For exit code 4: the profiles that exist.
    #[serde(default)]
    pub profiles: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EventLine {
    pub step: String,
    pub program: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pid: Option<u32>,
}

impl From<&Event> for EventLine {
    fn from(event: &Event) -> Self {
        Self {
            step: event.step.to_string(),
            program: event.program.clone(),
            pid: event.pid,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrayStatus {
    pub state: State,
    /// The job that runs right now, as text (`switch to "focus"`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub running: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub waiting: Option<String>,
    /// U4: present while some profile has triggers. Added within version
    /// 1 with a default, so an older client or tray ignores it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub automation: Option<AutomationStatus>,
    /// M5: the errors of the last switch that are not resolved yet (L9),
    /// for the markers of the main window. Absent when there are none, and
    /// from a tray that does not know the field.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_switch: Option<LastSwitch>,
    /// Lines for the caller of `set_automation` (for example that the
    /// setting was taken over but could not be saved). Added within
    /// version 1; absent when empty.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<String>,
}

/// U4: what automatic switching does, for `kaltstellen status`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AutomationStatus {
    /// `Automatic switching` is off (main window, `kaltstellen automation`).
    #[serde(default)]
    pub paused: bool,
    /// A manual switch holds automation until a trigger program starts or
    /// ends.
    #[serde(default)]
    pub held: bool,
    /// The active profile was applied by this trigger program.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trigger: Option<String>,
    /// The profile it returns to when the trigger ends.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub returns_to: Option<String>,
    /// Profiles whose trigger runs but waits for a higher one.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub waiting: Vec<String>,
    /// The profile they wait for: the winning trigger's, which is active.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub waiting_for: Option<String>,
}

/// U4: the names of waiting profiles a `status` reply carries at most, in
/// bytes, so the reply stays below [`MAX_LINE`] with the other fields.
pub const WAITING_BUDGET: usize = 8 * 1024;

impl AutomationStatus {
    /// Keeps the waiting profiles within [`WAITING_BUDGET`]; the rest are
    /// left out (a configuration with that many triggers running at once
    /// is not expected).
    pub fn bounded(mut self) -> Self {
        let mut used = 0;
        self.waiting.retain(|name| {
            used += name.len() + 3;
            used <= WAITING_BUDGET
        });
        self
    }

    /// The lines `kaltstellen status` prints after `tray: …`.
    pub fn lines(&self, active: &str) -> Vec<String> {
        let mut lines = Vec::new();
        if self.paused {
            lines.push("tray: automatic switching paused".to_owned());
        } else if self.held {
            lines.push(
                "tray: automatic switching held by a manual switch until a trigger program \
                 starts or ends"
                    .to_owned(),
            );
        } else {
            lines.push("tray: automatic switching on".to_owned());
        }
        if let (Some(trigger), Some(back)) = (&self.trigger, &self.returns_to) {
            lines.push(format!(
                "tray: \"{active}\" applied by trigger {trigger}; back to \"{back}\" when it ends"
            ));
        }
        // Listed only together with the winner they wait for.
        for (waiting, winner) in self
            .waiting
            .iter()
            .filter_map(|w| self.waiting_for.as_ref().map(|f| (w, f)))
        {
            lines.push(format!(
                "tray: trigger for \"{waiting}\" waits while \"{winner}\" is active"
            ));
        }
        lines
    }
}

/// M5: the open errors of the last switch and the profile it made active.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LastSwitch {
    pub profile: String,
    pub errors: Vec<ErrorLine>,
}

/// One error of the last switch that names a program.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ErrorLine {
    /// `KS110`.
    pub code: String,
    /// The executable name.
    pub program: String,
    /// The full path, where the step knew it (a start).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// As the menu shows it: `KS110 [start] tool.exe: reason`.
    pub text: String,
}

impl Finished {
    /// From what an engine run returned. `ran` is false when nothing could
    /// be attempted.
    pub fn new(ran: bool, active: &str, events: &[Event], errors: &[Error]) -> Self {
        Self {
            exit_code: exit::of(ran, errors),
            active_profile: ran.then(|| active.to_owned()),
            notes: Vec::new(),
            events: events.iter().map(EventLine::from).collect(),
            errors: errors.iter().map(ToString::to_string).collect(),
            profiles: Vec::new(),
        }
    }

    /// The text the command line prints: (stdout, stderr).
    pub fn render(&self) -> (String, String) {
        let mut out = String::new();
        let mut err = String::new();
        for note in &self.notes {
            let _ = writeln!(err, "{note}");
        }
        for event in &self.events {
            let _ = match event.pid {
                Some(pid) => writeln!(out, "{} {} (pid {pid})", event.step, event.program),
                None => writeln!(out, "{} {}", event.step, event.program),
            };
        }
        for error in &self.errors {
            let _ = writeln!(err, "error: {error}");
        }
        if self.exit_code == exit::UNKNOWN_PROFILE {
            let names: Vec<&str> = std::iter::once("neutral")
                .chain(self.profiles.iter().map(String::as_str))
                .collect();
            let _ = writeln!(err, "profiles: {}", names.join(", "));
        }
        if let Some(active) = &self.active_profile {
            let _ = writeln!(out, "active profile: {active}");
        }
        (out, err)
    }
}

/// One message as a line.
pub fn encode<T: Serialize>(message: &T) -> String {
    let mut line = serde_json::to_string(message).expect("messages always serialise");
    line.push('\n');
    line
}

fn protocol_error(reason: impl Into<String>) -> Error {
    Error::new(Code::TrayProtocol, reason)
}

/// Reads the version first, so a message of another version is reported as
/// such rather than as malformed.
fn check_version(line: &str) -> Result<(), Error> {
    let value: serde_json::Value = serde_json::from_str(line)
        .map_err(|e| protocol_error(format!("not a JSON message: {e}")))?;
    match value.get("version").and_then(serde_json::Value::as_u64) {
        Some(v) if v == u64::from(VERSION) => Ok(()),
        Some(v) => Err(protocol_error(format!(
            "protocol version {v} is not supported (this kaltstellen speaks {VERSION})"
        ))),
        None => Err(protocol_error("message without version")),
    }
}

pub fn decode_request(line: &str) -> Result<Request, Error> {
    check_version(line)?;
    serde_json::from_str(line).map_err(|e| protocol_error(format!("malformed request: {e}")))
}

pub fn decode_reply(line: &str) -> Result<Reply, Error> {
    check_version(line)?;
    serde_json::from_str(line).map_err(|e| protocol_error(format!("malformed reply: {e}")))
}

pub fn request(command: Command) -> Request {
    Request {
        version: VERSION,
        command,
    }
}

pub fn reply(body: Body) -> Reply {
    Reply {
        version: VERSION,
        body,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::Step;

    #[test]
    fn u4_status_lines_name_the_winner_waited_for() {
        let status = AutomationStatus {
            trigger: Some("sim.exe".into()),
            returns_to: Some("neutral".into()),
            waiting: vec!["low".into()],
            waiting_for: Some("high".into()),
            ..AutomationStatus::default()
        };
        assert_eq!(
            status.lines("high"),
            [
                "tray: automatic switching on",
                "tray: \"high\" applied by trigger sim.exe; back to \"neutral\" when it ends",
                "tray: trigger for \"low\" waits while \"high\" is active",
            ]
        );
        let held = AutomationStatus {
            held: true,
            waiting: vec!["low".into()],
            ..AutomationStatus::default()
        };
        assert_eq!(
            held.lines("x"),
            [
                "tray: automatic switching held by a manual switch until a trigger program starts or ends"
            ]
        );
    }

    #[test]
    fn requests_have_a_stable_wire_form() {
        let line = encode(&request(Command::Switch {
            profile: "focus".into(),
            force: true,
            wait: false,
        }));
        assert_eq!(
            line,
            "{\"version\":1,\"command\":\"switch\",\"profile\":\"focus\",\"force\":true,\"wait\":false}\n"
        );
        assert_eq!(
            encode(&request(Command::Quit)),
            "{\"version\":1,\"command\":\"quit\"}\n"
        );
        assert_eq!(
            encode(&request(Command::QuitIfIdle)),
            "{\"version\":1,\"command\":\"quit_if_idle\"}\n"
        );
        assert_eq!(
            encode(&request(Command::Status)),
            "{\"version\":1,\"command\":\"status\"}\n"
        );
        assert_eq!(
            encode(&request(Command::SetAutomation { paused: true })),
            "{\"version\":1,\"command\":\"set_automation\",\"paused\":true}\n"
        );
        assert_eq!(
            encode(&request(Command::SetUpdateMode {
                mode: "manual".into()
            })),
            "{\"version\":1,\"command\":\"set_update_mode\",\"mode\":\"manual\"}\n"
        );
        assert!(Command::SetAutomation { paused: false }.waits());
        // `paused` is required: a request without it is malformed.
        assert_eq!(
            decode_request("{\"version\":1,\"command\":\"set_automation\"}")
                .unwrap_err()
                .code,
            Code::TrayProtocol
        );
        // Defaults: a minimal request waits and does not force.
        let parsed =
            decode_request("{\"version\":1,\"command\":\"switch\",\"profile\":\"x\"}").unwrap();
        assert_eq!(
            parsed.command,
            Command::Switch {
                profile: "x".into(),
                force: false,
                wait: true
            }
        );
        assert!(parsed.command.waits());
        assert_eq!(
            decode_request("{\"version\":1,\"command\":\"recover\"}")
                .unwrap()
                .command,
            Command::Recover { wait: true }
        );
    }

    #[test]
    fn every_reply_round_trips() {
        let bodies = vec![
            Body::Accepted { waiting: true },
            Body::Done(Finished {
                exit_code: 1,
                active_profile: Some("focus".into()),
                notes: vec!["n".into()],
                events: vec![EventLine {
                    step: "suspend".into(),
                    program: "a.exe".into(),
                    pid: Some(7),
                }],
                errors: vec!["KS105 boom".into()],
                profiles: vec![],
            }),
            Body::Superseded { by: "b".into() },
            Body::Rejected {
                exit_code: 2,
                lines: vec!["x".into()],
            },
            Body::Status(TrayStatus {
                state: State::default(),
                running: Some("switch to \"a\"".into()),
                waiting: None,
                automation: Some(AutomationStatus {
                    held: true,
                    ..AutomationStatus::default()
                }),
                last_switch: Some(LastSwitch {
                    profile: "a".into(),
                    errors: vec![ErrorLine {
                        code: "KS110".into(),
                        program: "t.exe".into(),
                        path: Some("C:\\t\\t.exe".into()),
                        text: "KS110 [start] t.exe: missing".into(),
                    }],
                }),
                notes: vec!["warning: not saved".into()],
            }),
        ];
        for body in bodies {
            let line = encode(&reply(body.clone()));
            assert!(line.ends_with('\n') && !line[..line.len() - 1].contains('\n'));
            assert_eq!(decode_reply(line.trim_end()).unwrap().body, body);
        }
        let done = encode(&reply(Body::Superseded { by: "b".into() }));
        assert_eq!(
            done,
            "{\"version\":1,\"reply\":\"superseded\",\"by\":\"b\"}\n"
        );
    }

    #[test]
    fn m5_a_status_without_last_switch_still_decodes() {
        // A tray from before M5 sends no `last_switch`; no error is shown.
        let line = encode(&reply(Body::Status(TrayStatus {
            state: State::default(),
            running: None,
            waiting: None,
            automation: None,
            last_switch: None,
            notes: Vec::new(),
        })));
        assert!(!line.contains("last_switch"), "{line}");
        let Body::Status(status) = decode_reply(line.trim_end()).unwrap().body else {
            panic!("not a status");
        };
        assert_eq!(status.last_switch, None);
    }

    #[test]
    fn other_versions_and_garbage_are_ks505() {
        for line in [
            "{\"version\":2,\"command\":\"status\"}",
            "{\"command\":\"status\"}",
            "not json",
            "{\"version\":1,\"command\":\"explode\"}",
            "{\"version\":1,\"command\":\"switch\"}",
        ] {
            let err = decode_request(line).unwrap_err();
            assert_eq!(err.code, Code::TrayProtocol, "{line}");
        }
        let err = decode_request("{\"version\":2,\"command\":\"status\"}").unwrap_err();
        assert!(err.reason.contains("version 2"), "{}", err.reason);
    }

    #[test]
    fn finished_renders_like_the_in_process_command_line() {
        let events = vec![
            Event {
                step: Step::Suspend,
                program: "a.exe".into(),
                pid: Some(12),
            },
            Event {
                step: Step::Stop,
                program: "b.exe".into(),
                pid: None,
            },
        ];
        let errors = vec![Error::new(Code::StartFailed, "missing").program("c.exe")];
        let (out, err) = Finished::new(true, "focus", &events, &errors).render();
        assert_eq!(
            out,
            "suspend a.exe (pid 12)\nstop b.exe\nactive profile: focus\n"
        );
        assert_eq!(err, "error: KS110 c.exe: missing\n");

        let unknown = Error::new(Code::UnknownProfile, "no profile named \"x\"");
        let mut finished = Finished::new(false, "neutral", &[], &[unknown]);
        finished.profiles = vec!["a".into(), "b".into()];
        assert_eq!(finished.exit_code, exit::UNKNOWN_PROFILE);
        let (out, err) = finished.render();
        assert_eq!(out, "");
        assert_eq!(
            err,
            "error: KS401: no profile named \"x\"\nprofiles: neutral, a, b\n"
        );
    }
}
