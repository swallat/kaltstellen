//! The helper's pipe protocol: versioned JSON lines, size-limited, unknown
//! fields refused, one request per connection line (docs/design-optional-helper.md,
//! "Trust between tray and helper").
//!
//! M4a knows `hello` and `status`. Later milestones add verbs here; an
//! older helper answers an unknown verb with an error, never with a guess.

use std::io::{BufRead, Read, Write};

use serde::{Deserialize, Serialize};

use crate::error::{Code, Error, Result};

/// Protocol version of the pipe messages.
pub const VERSION: u32 = 1;
/// Longest request line accepted, in bytes.
pub const MAX_REQUEST: usize = 4096;

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    /// Protocol version of the sender.
    pub v: u32,
    pub cmd: Command,
    /// With `events`: the first event number wanted (0 for the oldest).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after: Option<u64>,
    /// With `network`: the window in seconds, 1 to 600 (default 10).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seconds: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Command {
    /// Version check and who is answering.
    Hello,
    /// What the helper is doing: roles and counters.
    Status,
    /// M4b: process start and stop events of the caller's own Windows
    /// session, from `after` on; needs the role *observe*.
    Events,
    /// M4b: bytes sent and received per process of the caller's own
    /// Windows session over the last `seconds`; needs the role *observe*.
    Network,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Roles {
    pub observe: bool,
    pub act: bool,
    pub services: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Reply {
    pub v: u32,
    pub ok: bool,
    /// Version of the helper executable.
    pub version: Option<String>,
    /// Process id of the service, so the client can check it against the
    /// service control manager.
    pub pid: Option<u32>,
    /// Roles switched on in the policy file (nothing is implemented behind
    /// them before M4b).
    pub roles: Option<Roles>,
    /// Roles the running helper actually implements.
    pub active: Option<Roles>,
    pub error: Option<String>,
    /// `events`: the events, oldest first.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub events: Option<Vec<super::events::Event>>,
    /// `events`: ask again with `after` set to this.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next: Option<u64>,
    /// `events`: how many events are missing since the helper started.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lost: Option<u64>,
    /// `network`: bytes per process (those with none are left out).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub traffic: Option<Vec<super::events::Usage>>,
    /// `status`: whether the network counters run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub network: Option<bool>,
}

impl Reply {
    fn failure(text: impl Into<String>) -> Reply {
        Reply {
            v: VERSION,
            ok: false,
            version: None,
            pid: None,
            roles: None,
            active: None,
            error: Some(text.into()),
            events: None,
            next: None,
            lost: None,
            traffic: None,
            network: None,
        }
    }
}

/// What the helper knows when it answers.
#[derive(Debug, Clone)]
pub struct Context {
    pub version: String,
    pub pid: u32,
    pub roles: Roles,
    /// The ring of events when the role *observe* runs.
    pub events: Option<std::sync::Arc<super::events::Ring>>,
    /// The Windows session of a process id, `None` if it cannot be told.
    pub session_of: fn(u32) -> Option<u32>,
}

/// Answers one request line. Never panics on bad input. `session` is the
/// Windows session of the client, as far as it could be told.
pub fn answer(line: &str, context: &Context, session: Option<u32>) -> Reply {
    let request: Request = match serde_json::from_str(line) {
        Ok(request) => request,
        Err(e) => return Reply::failure(format!("malformed request: {e}")),
    };
    if request.v != VERSION {
        return Reply::failure(format!(
            "protocol version {} is not supported (this helper speaks {VERSION})",
            request.v
        ));
    }
    if request.after.is_some() && request.cmd != Command::Events {
        return Reply::failure("`after` belongs to `events` only");
    }
    if request.seconds.is_some() && request.cmd != Command::Network {
        return Reply::failure("`seconds` belongs to `network` only");
    }
    let mut reply = Reply {
        v: VERSION,
        ok: true,
        version: Some(context.version.clone()),
        pid: Some(context.pid),
        roles: Some(context.roles.clone()),
        active: Some(Roles {
            observe: context.events.as_ref().is_some_and(|ring| ring.running()),
            act: false,
            services: false,
        }),
        error: None,
        events: None,
        next: None,
        lost: None,
        traffic: None,
        network: context
            .events
            .as_ref()
            .map(|ring| ring.running() && ring.traffic.running()),
    };
    match request.cmd {
        Command::Hello | Command::Status => reply,
        Command::Events => {
            let Some(ring) = context.events.as_ref().filter(|ring| ring.running()) else {
                return Reply::failure("the role observe is not running");
            };
            let Some(session) = session else {
                return Reply::failure("the session of the caller is not known");
            };
            let batch = ring.read(request.after.unwrap_or(0), Some(session));
            reply.events = Some(batch.events);
            reply.next = Some(batch.next);
            reply.lost = Some(batch.lost);
            reply
        }
        Command::Network => {
            let Some(ring) = context
                .events
                .as_ref()
                .filter(|ring| ring.running() && ring.traffic.running())
            else {
                return Reply::failure("the network counters are not running");
            };
            let Some(session) = session else {
                return Reply::failure("the session of the caller is not known");
            };
            let seconds = request.seconds.unwrap_or(10);
            if !(1..=600).contains(&seconds) {
                return Reply::failure("`seconds` is 1 to 600");
            }
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_secs());
            reply.traffic = Some(ring.traffic.usage(u64::from(seconds), now, |pid| {
                (context.session_of)(pid) == Some(session)
            }));
            reply
        }
    }
}

/// Longest reply line a client accepts, in bytes.
pub const MAX_REPLY: usize = 256 * 1024;

/// Reads one line of at most `limit` bytes ([`MAX_REQUEST`] for requests,
/// [`MAX_REPLY`] for replies). `Ok(None)` at the end of the stream before
/// any byte.
pub fn read_line(reader: &mut impl BufRead, limit: usize) -> Result<Option<String>> {
    let mut bytes = Vec::new();
    let read = reader
        .take(limit as u64 + 1)
        .read_until(b'\n', &mut bytes)
        .map_err(|e| {
            Error::new(
                Code::HelperProtocol,
                format!("cannot read the message: {e}"),
            )
        })?;
    if read == 0 {
        return Ok(None);
    }
    if bytes.len() > limit {
        return Err(Error::new(
            Code::HelperProtocol,
            format!("message longer than {limit} bytes"),
        ));
    }
    let text = String::from_utf8(bytes)
        .map_err(|_| Error::new(Code::HelperProtocol, "message is not UTF-8"))?;
    Ok(Some(text.trim_end_matches(['\r', '\n']).to_owned()))
}

/// Serves one connection: one request line, one reply line.
pub fn serve(
    conn: &mut (impl BufRead + Write),
    context: &Context,
    session: Option<u32>,
) -> Result<()> {
    let reply = match read_line(conn, MAX_REQUEST) {
        Ok(Some(line)) => answer(&line, context, session),
        Ok(None) => return Ok(()),
        Err(e) => Reply::failure(e.reason),
    };
    write_line(conn, &reply)
}

/// Writes `value` as one JSON line and flushes.
pub fn write_line(out: &mut impl Write, value: &impl Serialize) -> Result<()> {
    let mut text = serde_json::to_string(value)
        .map_err(|e| Error::new(Code::HelperProtocol, format!("cannot encode: {e}")))?;
    text.push('\n');
    out.write_all(text.as_bytes())
        .and_then(|()| out.flush())
        .map_err(|e| Error::new(Code::HelperProtocol, format!("cannot send: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn context() -> Context {
        Context {
            version: "1.2.3".into(),
            pid: 4242,
            roles: Roles {
                observe: true,
                act: false,
                services: false,
            },
            events: None,
            session_of: |_| Some(1),
        }
    }

    #[test]
    fn hello_names_the_version_and_the_roles() {
        let reply = answer(r#"{"v":1,"cmd":"hello"}"#, &context(), Some(1));
        assert!(reply.ok, "{reply:?}");
        assert_eq!(reply.version.as_deref(), Some("1.2.3"));
        assert_eq!(reply.pid, Some(4242));
        assert!(reply.roles.unwrap().observe);
        // No event session runs in this context.
        assert_eq!(
            reply.active,
            Some(Roles {
                observe: false,
                act: false,
                services: false
            })
        );
    }

    #[test]
    fn unknown_verbs_fields_and_versions_are_refused() {
        for line in [
            r#"{"v":1,"cmd":"terminate","pid":1}"#,
            r#"{"v":1,"cmd":"hello","after":3}"#,
            r#"{"v":1,"cmd":"events"}"#,
            r#"{"v":1,"cmd":"hello","extra":true}"#,
            r#"{"v":2,"cmd":"hello"}"#,
            r#"{"cmd":"hello"}"#,
            "not json",
            "",
            r#"{"v":1}"#,
        ] {
            let reply = answer(line, &context(), Some(1));
            assert!(!reply.ok, "{line:?} was answered: {reply:?}");
            assert!(reply.error.is_some());
        }
    }

    #[test]
    fn a_connection_gets_one_reply_line() {
        // One buffer to read the request from, one to collect the reply.
        struct Duplex {
            input: Cursor<Vec<u8>>,
            output: Vec<u8>,
        }
        impl Read for Duplex {
            fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
                self.input.read(buf)
            }
        }
        impl BufRead for Duplex {
            fn fill_buf(&mut self) -> std::io::Result<&[u8]> {
                self.input.fill_buf()
            }
            fn consume(&mut self, n: usize) {
                self.input.consume(n);
            }
        }
        impl Write for Duplex {
            fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
                self.output.write(buf)
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let mut duplex = Duplex {
            input: Cursor::new(b"{\"v\":1,\"cmd\":\"status\"}\n".to_vec()),
            output: Vec::new(),
        };
        serve(&mut duplex, &context(), Some(1)).unwrap();
        let text = String::from_utf8(duplex.output).unwrap();
        assert_eq!(text.matches('\n').count(), 1);
        let reply: Reply = serde_json::from_str(text.trim()).unwrap();
        assert!(reply.ok);
    }

    #[test]
    fn an_oversized_message_is_not_buffered_without_end() {
        let big = vec![b'x'; MAX_REQUEST * 4];
        let err = read_line(&mut Cursor::new(big), MAX_REQUEST).unwrap_err();
        assert_eq!(err.code, Code::HelperProtocol);
        assert_eq!(
            read_line(&mut Cursor::new(Vec::new()), MAX_REQUEST).unwrap(),
            None
        );
    }

    #[test]
    fn events_are_given_to_the_session_of_the_caller_only() {
        use crate::helper::events::Ring;
        use crate::win32::etw::{Kind, ProcessEvent};
        let ring = std::sync::Arc::new(Ring::new());
        ring.set_running(true);
        // Now, as a FILETIME: older events are not given out.
        let now = (std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs()
            + 11_644_473_600)
            * 10_000_000;
        for (pid, session) in [(10, 1), (11, 2)] {
            ring.push(
                &ProcessEvent {
                    kind: Kind::Start,
                    pid,
                    parent_pid: 4,
                    session: Some(session),
                    created: now,
                    time: now,
                    image: r"\Device\X\a.exe".to_owned(),
                },
                Vec::new,
            );
        }
        let mut with = context();
        with.events = Some(ring);
        let reply = answer(r#"{"v":1,"cmd":"events","after":0}"#, &with, Some(2));
        assert!(reply.ok, "{reply:?}");
        let events = reply.events.unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].pid, 11);
        assert_eq!(reply.next, Some(2));
        // Network counters need the traffic to run, and only the caller's session counts.
        assert!(!answer(r#"{"v":1,"cmd":"network"}"#, &with, Some(1)).ok);
        with.events.as_ref().unwrap().traffic.set_running(true);
        with.events
            .as_ref()
            .unwrap()
            .traffic
            .add(&crate::win32::etw::NetEvent {
                pid: 11,
                bytes: 5,
                received: false,
            });
        let own = answer(r#"{"v":1,"cmd":"network","seconds":600}"#, &with, Some(1));
        assert!(own.ok, "{own:?}");
        assert_eq!(
            own.traffic.unwrap().len(),
            1,
            "session_of stub says session 1"
        );
        assert!(!answer(r#"{"v":1,"cmd":"network","seconds":0}"#, &with, Some(1)).ok);
        assert!(!answer(r#"{"v":1,"cmd":"events","seconds":5}"#, &with, Some(1)).ok);
        // Without a known session of the caller nothing is given out.
        assert!(!answer(r#"{"v":1,"cmd":"events"}"#, &with, None).ok);
        // The status says the role runs.
        assert!(
            answer(r#"{"v":1,"cmd":"status"}"#, &with, Some(1))
                .active
                .unwrap()
                .observe
        );
    }
}
