//! What the *observe* role keeps (M4b): the last process start and stop
//! events, in memory only, bounded by count and age. Nothing is written to
//! disk; a restart starts empty. No command lines, no window titles, no
//! network data (the counters come later).

use std::collections::VecDeque;
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

use crate::win32::etw::{Kind, ProcessEvent};

/// Most events kept.
pub const CAPACITY: usize = 4096;
/// Longest time an event is kept, in seconds (24 h, design note).
pub const MAX_AGE_S: u64 = 24 * 60 * 60;
/// Most events one answer carries (keeps a reply line small).
pub const BATCH: usize = 200;
/// Seconds between 1601 and 1970, for `FILETIME` values.
const UNIX_EPOCH_S: u64 = 11_644_473_600;

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Event {
    /// Position in the order the helper saw the events; grows without gaps.
    pub seq: u64,
    /// `start` or `stop`.
    pub kind: String,
    pub pid: u32,
    /// Parent process (0 when not reported, always 0 for `stop`).
    pub parent: u32,
    pub session: Option<u32>,
    /// Creation time of the process, seconds since 1970 (0 if unknown).
    pub created: u64,
    /// When it happened, seconds since 1970.
    pub time: u64,
    /// File name of the program, e.g. `editor.exe`.
    pub exe: String,
    /// Full path in the usual drive form; `null` where the kernel gave none
    /// that maps to a drive.
    pub path: Option<String>,
}

/// `FILETIME` (100 ns since 1601) as seconds since 1970; 0 before 1970.
pub fn unix_seconds(filetime: u64) -> u64 {
    (filetime / 10_000_000).saturating_sub(UNIX_EPOCH_S)
}

/// `\Device\HarddiskVolume3\Tools\a.exe` as `C:\Tools\a.exe`, given the
/// drive letters and their device names; `None` if no drive matches.
pub fn win32_path(nt: &str, devices: &[(String, String)]) -> Option<String> {
    for (drive, device) in devices {
        if nt.len() > device.len()
            && nt.as_bytes()[device.len()] == b'\\'
            && nt[..device.len()].eq_ignore_ascii_case(device)
        {
            return Some(format!("{drive}{}", &nt[device.len()..]));
        }
    }
    None
}

#[derive(Debug, Default)]
struct Inner {
    events: VecDeque<Event>,
    next: u64,
    /// Events pushed out by the capacity or the age limit.
    dropped: u64,
    devices: Vec<(String, String)>,
    /// `(pid, created)` -> session of the processes seen starting, so a stop
    /// that does not name its session is only given to the right user.
    sessions: VecDeque<((u32, u64), u32)>,
}

/// The ring of events, shared by the event thread and the pipe threads.
#[derive(Debug, Default)]
pub struct Ring {
    inner: Mutex<Inner>,
    /// Events the kernel reported lost (set from the session).
    lost: std::sync::atomic::AtomicU64,
}

/// What a reader gets.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Batch {
    pub events: Vec<Event>,
    /// Ask again `after` this number.
    pub next: u64,
    /// Events missing before `next` (dropped from the ring or lost by the
    /// kernel) since the helper started.
    pub lost: u64,
}

impl Ring {
    pub fn new() -> Ring {
        Ring::default()
    }

    pub fn set_lost(&self, lost: u32) {
        self.lost
            .store(u64::from(lost), std::sync::atomic::Ordering::Relaxed);
    }

    /// Adds an event. `devices` is consulted for the path; the map is read
    /// again when no drive matches (a drive may have been added).
    pub fn push(&self, event: &ProcessEvent, devices: impl Fn() -> Vec<(String, String)>) {
        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let mut path = win32_path(&event.image, &inner.devices);
        if path.is_none() && event.kind == Kind::Start && !event.image.is_empty() {
            inner.devices = devices();
            path = win32_path(&event.image, &inner.devices);
        }
        let created = unix_seconds(event.created);
        let session = match (event.kind, event.session) {
            (Kind::Start, Some(session)) => {
                inner
                    .sessions
                    .push_back(((event.pid, event.created), session));
                if inner.sessions.len() > CAPACITY * 4 {
                    inner.sessions.pop_front();
                }
                Some(session)
            }
            (Kind::Stop, Some(session)) => Some(session),
            (Kind::Stop, None) => inner
                .sessions
                .iter()
                .rev()
                .find(|(key, _)| *key == (event.pid, event.created))
                .map(|(_, session)| *session),
            (Kind::Start, None) => None,
        };
        let exe = match &path {
            Some(path) => path.rsplit('\\').next().unwrap_or(path).to_owned(),
            None => event.image.rsplit('\\').next().unwrap_or("").to_owned(),
        };
        let seq = inner.next;
        inner.next += 1;
        let time = unix_seconds(event.time);
        inner.events.push_back(Event {
            seq,
            kind: match event.kind {
                Kind::Start => "start",
                Kind::Stop => "stop",
            }
            .to_owned(),
            pid: event.pid,
            parent: event.parent_pid,
            session,
            created,
            time,
            exe,
            path,
        });
        // Capacity and age.
        let oldest_kept = time.saturating_sub(MAX_AGE_S);
        while inner.events.len() > CAPACITY
            || inner
                .events
                .front()
                .is_some_and(|front| front.time < oldest_kept)
        {
            inner.events.pop_front();
            inner.dropped += 1;
        }
    }

    /// Up to [`BATCH`] events with `seq >= after`. With `session`, only
    /// those of that Windows session (the reader's own); events whose
    /// session is not known are never given out.
    pub fn read(&self, after: u64, session: Option<u32>) -> Batch {
        let inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let mut events = Vec::new();
        let mut next = after;
        let mut complete = true;
        for event in inner.events.iter().filter(|e| e.seq >= after) {
            if events.len() >= BATCH {
                complete = false;
                break;
            }
            next = event.seq + 1;
            if session.is_some_and(|wanted| event.session == Some(wanted)) {
                events.push(event.clone());
            }
        }
        if complete {
            // Everything in the ring was looked at: the reader is up to date
            // (and a reader that fell behind the ring skips to its start).
            next = next.max(inner.next);
        }
        Batch {
            events,
            next,
            lost: inner.dropped + self.lost.load(std::sync::atomic::Ordering::Relaxed),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn devices() -> Vec<(String, String)> {
        vec![
            ("C:".to_owned(), r"\Device\HarddiskVolume3".to_owned()),
            ("D:".to_owned(), r"\Device\HarddiskVolume12".to_owned()),
        ]
    }

    fn start(pid: u32, image: &str, session: u32, time: u64) -> ProcessEvent {
        ProcessEvent {
            kind: Kind::Start,
            pid,
            parent_pid: 4,
            session: Some(session),
            created: (time + UNIX_EPOCH_S) * 10_000_000,
            time: (time + UNIX_EPOCH_S) * 10_000_000,
            image: image.to_owned(),
        }
    }

    #[test]
    fn nt_paths_become_drive_paths_without_mixing_up_prefixes() {
        let map = devices();
        assert_eq!(
            win32_path(r"\Device\HarddiskVolume3\Tools\a.exe", &map).as_deref(),
            Some(r"C:\Tools\a.exe")
        );
        // Volume3 must not match Volume30, and the case does not matter.
        assert_eq!(win32_path(r"\Device\HarddiskVolume30\a.exe", &map), None);
        assert_eq!(
            win32_path(r"\DEVICE\harddiskvolume12\b.exe", &map).as_deref(),
            Some(r"D:\b.exe")
        );
        assert_eq!(win32_path(r"\Device\Mup\server\a.exe", &map), None);
    }

    #[test]
    fn filetime_counts_from_1970() {
        assert_eq!(unix_seconds(0), 0);
        assert_eq!(
            unix_seconds((UNIX_EPOCH_S + 1_700_000_000) * 10_000_000),
            1_700_000_000
        );
    }

    #[test]
    fn a_reader_gets_only_events_of_its_own_session_and_resumes_after_next() {
        let ring = Ring::new();
        let now = 1_700_000_000;
        ring.push(
            &start(10, r"\Device\HarddiskVolume3\a.exe", 1, now),
            devices,
        );
        ring.push(
            &start(11, r"\Device\HarddiskVolume3\b.exe", 2, now),
            devices,
        );
        ring.push(
            &start(12, r"\Device\HarddiskVolume3\c.exe", 1, now + 1),
            devices,
        );
        let first = ring.read(0, Some(1));
        let names: Vec<_> = first.events.iter().map(|e| e.exe.as_str()).collect();
        assert_eq!(names, ["a.exe", "c.exe"]);
        assert_eq!(first.next, 3);
        assert_eq!(first.events[0].path.as_deref(), Some(r"C:\a.exe"));
        // Up to date: nothing more, and the next number does not move back.
        let again = ring.read(first.next, Some(1));
        assert!(again.events.is_empty());
        assert_eq!(again.next, 3);
        // Without a known session nothing is given out.
        assert!(ring.read(0, None).events.is_empty());
    }

    #[test]
    fn a_stop_without_a_session_takes_it_from_the_start() {
        let ring = Ring::new();
        let begin = start(10, r"\Device\HarddiskVolume3\a.exe", 5, 1_700_000_000);
        ring.push(&begin, devices);
        let mut end = begin.clone();
        end.kind = Kind::Stop;
        end.session = None;
        end.image = String::new();
        ring.push(&end, devices);
        let mut stranger = end.clone();
        stranger.pid = 99;
        ring.push(&stranger, devices);
        let batch = ring.read(0, Some(5));
        assert_eq!(batch.events.len(), 2);
        assert_eq!(batch.events[1].kind, "stop");
    }

    #[test]
    fn the_ring_is_bounded_by_count_and_by_age() {
        let ring = Ring::new();
        let now = 1_700_000_000;
        for i in 0..(CAPACITY as u32 + 10) {
            ring.push(&start(i, r"\Device\HarddiskVolume3\a.exe", 1, now), devices);
        }
        let batch = ring.read(0, Some(1));
        assert_eq!(batch.lost, 10);
        assert_eq!(batch.events.first().unwrap().seq, 10);
        // A much later event pushes out everything older than a day.
        ring.push(
            &start(1, r"\Device\HarddiskVolume3\a.exe", 1, now + MAX_AGE_S + 5),
            devices,
        );
        let all: usize = ring.read(0, Some(1)).events.len();
        assert_eq!(all, 1);
    }
}
