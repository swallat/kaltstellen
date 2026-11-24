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
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Command {
    /// Version check and who is answering.
    Hello,
    /// What the helper is doing: roles and counters.
    Status,
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
        }
    }
}

/// What the helper knows when it answers.
#[derive(Debug, Clone)]
pub struct Context {
    pub version: String,
    pub pid: u32,
    pub roles: Roles,
}

/// Answers one request line. Never panics on bad input.
pub fn answer(line: &str, context: &Context) -> Reply {
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
    match request.cmd {
        Command::Hello | Command::Status => Reply {
            v: VERSION,
            ok: true,
            version: Some(context.version.clone()),
            pid: Some(context.pid),
            roles: Some(context.roles.clone()),
            // M4a implements no role yet.
            active: Some(Roles {
                observe: false,
                act: false,
                services: false,
            }),
            error: None,
        },
    }
}

/// Reads one line of at most [`MAX_REQUEST`] bytes. `Ok(None)` at the end of
/// the stream before any byte.
pub fn read_line(reader: &mut impl BufRead) -> Result<Option<String>> {
    let mut bytes = Vec::new();
    let read = reader
        .take(MAX_REQUEST as u64 + 1)
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
    if bytes.len() > MAX_REQUEST {
        return Err(Error::new(
            Code::HelperProtocol,
            format!("message longer than {MAX_REQUEST} bytes"),
        ));
    }
    let text = String::from_utf8(bytes)
        .map_err(|_| Error::new(Code::HelperProtocol, "message is not UTF-8"))?;
    Ok(Some(text.trim_end_matches(['\r', '\n']).to_owned()))
}

/// Serves one connection: one request line, one reply line.
pub fn serve(conn: &mut (impl BufRead + Write), context: &Context) -> Result<()> {
    let reply = match read_line(conn) {
        Ok(Some(line)) => answer(&line, context),
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
        }
    }

    #[test]
    fn hello_names_the_version_and_the_roles() {
        let reply = answer(r#"{"v":1,"cmd":"hello"}"#, &context());
        assert!(reply.ok, "{reply:?}");
        assert_eq!(reply.version.as_deref(), Some("1.2.3"));
        assert_eq!(reply.pid, Some(4242));
        assert!(reply.roles.unwrap().observe);
        // Nothing is implemented behind the roles in M4a.
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
            r#"{"v":1,"cmd":"hello","extra":true}"#,
            r#"{"v":2,"cmd":"hello"}"#,
            r#"{"cmd":"hello"}"#,
            "not json",
            "",
            r#"{"v":1}"#,
        ] {
            let reply = answer(line, &context());
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
        serve(&mut duplex, &context()).unwrap();
        let text = String::from_utf8(duplex.output).unwrap();
        assert_eq!(text.matches('\n').count(), 1);
        let reply: Reply = serde_json::from_str(text.trim()).unwrap();
        assert!(reply.ok);
    }

    #[test]
    fn an_oversized_message_is_not_buffered_without_end() {
        let big = vec![b'x'; MAX_REQUEST * 4];
        let err = read_line(&mut Cursor::new(big)).unwrap_err();
        assert_eq!(err.code, Code::HelperProtocol);
        assert_eq!(read_line(&mut Cursor::new(Vec::new())).unwrap(), None);
    }
}
