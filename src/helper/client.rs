//! The tray's and the command line's side of the helper pipe.

use std::io::BufReader;
use std::time::Duration;

use super::protocol::{self, Command, Reply, Request};
use super::{PIPE_NAME, freshness};
use crate::error::{Code, Error, Result};
use crate::win32::pipe::{self, Peer};

/// How long a conversation may take before it is cut off.
const DEADLINE: Duration = Duration::from_secs(3);

/// Sends `command` to the helper. `Ok(None)` if no helper pipe exists (not
/// installed or not running); an error if something answers that is not the
/// helper (wrong account, wrong protocol).
pub fn ask(command: Command) -> Result<Option<Reply>> {
    ask_after(command, None)
}

/// [`ask`] with the `after` of an `events` request.
pub fn ask_after(command: Command, after: Option<u64>) -> Result<Option<Reply>> {
    ask_full(command, after, None)
}

/// [`ask`] for `network` with the window in seconds.
pub fn ask_network(seconds: Option<u32>) -> Result<Option<Reply>> {
    ask_full(Command::Network, None, seconds)
}

fn ask_full(command: Command, after: Option<u64>, seconds: Option<u32>) -> Result<Option<Reply>> {
    let Some(mut conn) = pipe::connect_peer(PIPE_NAME, Duration::from_secs(1), Peer::Helper)?
    else {
        return Ok(None);
    };
    // The pipe name can be taken by any local user before the service
    // runs: only the service's own process counts as the helper.
    let service_pid = crate::win32::service::query(super::SERVICE_NAME)?.and_then(|info| info.pid);
    if service_pid.is_none() || pipe::server_pid(&conn) != service_pid {
        return Err(Error::new(
            Code::HelperUnreachable,
            "the pipe is not served by the helper service; not used",
        ));
    }
    let request = Request {
        v: protocol::VERSION,
        cmd: command,
        after,
        seconds,
    };
    let reply = pipe::with_deadline(DEADLINE, || -> Result<Option<String>> {
        protocol::write_line(&mut conn, &request)?;
        protocol::read_line(&mut BufReader::new(&conn), protocol::MAX_REPLY)
    })?
    .ok_or_else(|| Error::new(Code::HelperProtocol, "the helper closed without an answer"))?;
    let reply: Reply = serde_json::from_str(&reply)
        .map_err(|e| Error::new(Code::HelperProtocol, format!("unreadable answer: {e}")))?;
    if reply.v != protocol::VERSION {
        return Err(Error::new(
            Code::HelperProtocol,
            format!("the helper speaks protocol {}", reply.v),
        ));
    }
    if !reply.ok {
        return Err(Error::new(
            Code::HelperProtocol,
            reply.error.unwrap_or_else(|| "refused".to_owned()),
        ));
    }
    Ok(Some(reply))
}

/// A line telling how the running helper's version relates to this one,
/// or `None` when it matches or cannot be compared.
pub fn version_note(helper_version: &str) -> Option<String> {
    match freshness(helper_version, super::version()) {
        super::Freshness::Older => Some(format!(
            "the helper is older ({helper_version}) than this kaltstellen ({}); run `kaltstellen helper install` again to update it",
            super::version()
        )),
        super::Freshness::Newer => Some(format!(
            "the helper ({helper_version}) is newer than this kaltstellen ({})",
            super::version()
        )),
        _ => None,
    }
}
