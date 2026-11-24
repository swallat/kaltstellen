//! What the *observe* role keeps (M4b): the last process start and stop
//! events, in memory only, bounded by count and age. Nothing is written to
//! disk; a restart starts empty. No command lines, no window titles, no
//! network data (the counters come later).

use std::collections::{HashMap, VecDeque};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

use crate::win32::etw::{Kind, NetEvent, ProcessEvent};

/// Most events kept.
pub const CAPACITY: usize = 4096;
/// Longest time an event is kept, in seconds (24 h, design note).
pub const MAX_AGE_S: u64 = 24 * 60 * 60;
/// Most events one answer carries (keeps a reply line small).
pub const BATCH: usize = 100;
/// Longest file name and full path kept, in characters. A reply stays far
/// below the reader's limit with [`BATCH`] events of this size; a longer
/// path is not given out (`null`).
pub const MAX_EXE: usize = 255;
pub const MAX_PATH: usize = 520;
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

/// Seconds since 1970 now.
fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
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
    devices: Vec<(String, String)>,
    /// `(pid, created)` -> session of the processes seen starting, so a stop
    /// that does not name its session is only given to the right user.
    sessions: VecDeque<((u32, u64), u32)>,
}

/// How long per-second byte counters are kept, in seconds.
pub const TRAFFIC_WINDOW_S: u64 = 600;
/// Most processes with counters at once.
const MAX_TRAFFIC_PIDS: usize = 4096;

/// Bytes of one process over a time window.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Usage {
    pub pid: u32,
    /// Bytes sent.
    pub up: u64,
    /// Bytes received.
    pub down: u64,
}

#[derive(Debug, Clone, Copy)]
struct Bucket {
    second: u64,
    up: u64,
    down: u64,
}

/// Per-process byte counters per second for the last [`TRAFFIC_WINDOW_S`]
/// seconds: no addresses, no ports, nothing on disk. Counters are not a
/// record of traffic; the kernel may lose events under load.
#[derive(Debug, Default)]
pub struct Traffic {
    counters: Mutex<HashMap<u32, VecDeque<Bucket>>>,
    running: std::sync::atomic::AtomicBool,
}

impl Traffic {
    pub fn running(&self) -> bool {
        self.running.load(std::sync::atomic::Ordering::Relaxed)
    }

    pub fn set_running(&self, running: bool) {
        self.running
            .store(running, std::sync::atomic::Ordering::Relaxed);
    }

    pub fn add(&self, event: &NetEvent) {
        self.add_at(event, now());
    }

    /// [`add`](Traffic::add) with the current time (seconds since 1970) given.
    pub fn add_at(&self, event: &NetEvent, now: u64) {
        let mut counters = self.counters.lock().unwrap_or_else(|e| e.into_inner());
        let oldest = now.saturating_sub(TRAFFIC_WINDOW_S);
        if counters.len() >= MAX_TRAFFIC_PIDS && !counters.contains_key(&event.pid) {
            counters.retain(|_, buckets| buckets.back().is_some_and(|b| b.second > oldest));
            if counters.len() >= MAX_TRAFFIC_PIDS {
                return;
            }
        }
        let buckets = counters.entry(event.pid).or_default();
        while buckets.front().is_some_and(|b| b.second <= oldest) {
            buckets.pop_front();
        }
        if buckets.back().is_none_or(|b| b.second != now) {
            buckets.push_back(Bucket {
                second: now,
                up: 0,
                down: 0,
            });
        }
        if let Some(last) = buckets.back_mut() {
            let bytes = u64::from(event.bytes);
            if event.received {
                last.down += bytes;
            } else {
                last.up += bytes;
            }
        }
    }

    /// Bytes per process over the last `window_s` seconds (at most
    /// [`TRAFFIC_WINDOW_S`]) for the processes `wanted` accepts, those with
    /// no traffic left out, ordered by process id.
    pub fn usage(&self, window_s: u64, now: u64, wanted: impl Fn(u32) -> bool) -> Vec<Usage> {
        let counters = self.counters.lock().unwrap_or_else(|e| e.into_inner());
        let from = now.saturating_sub(window_s.min(TRAFFIC_WINDOW_S));
        let mut all: Vec<Usage> = counters
            .iter()
            .filter(|(pid, _)| wanted(**pid))
            .filter_map(|(pid, buckets)| {
                let (up, down) = buckets
                    .iter()
                    .filter(|b| b.second > from)
                    .fold((0, 0), |(up, down), b| (up + b.up, down + b.down));
                (up + down > 0).then_some(Usage {
                    pid: *pid,
                    up,
                    down,
                })
            })
            .collect();
        all.sort_by_key(|u| u.pid);
        all
    }
}

/// The ring of events, shared by the event thread and the pipe threads.
#[derive(Debug, Default)]
pub struct Ring {
    /// Network counters of the same session.
    pub traffic: Traffic,
    inner: Mutex<Inner>,
    /// Events the kernel reported lost (set from the session).
    lost: std::sync::atomic::AtomicU64,
    /// Whether the event session still delivers.
    running: std::sync::atomic::AtomicBool,
}

/// What a reader gets.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Batch {
    pub events: Vec<Event>,
    /// Ask again `after` this number.
    pub next: u64,
    /// Events or buffers the kernel reported lost since the helper started.
    /// Events that fell out of the ring (count or age) are not counted: a
    /// reader sees those as a jump of `next` past its `after`.
    pub lost: u64,
}

impl Ring {
    pub fn new() -> Ring {
        Ring::default()
    }

    /// Whether the event session delivers (false once it has ended).
    pub fn running(&self) -> bool {
        self.running.load(std::sync::atomic::Ordering::Relaxed)
    }

    pub fn set_running(&self, running: bool) {
        self.running
            .store(running, std::sync::atomic::Ordering::Relaxed);
    }

    pub fn set_lost(&self, lost: u32) {
        self.lost
            .store(u64::from(lost), std::sync::atomic::Ordering::Relaxed);
    }

    /// Adds an event. `devices` is consulted for the path; the map is read
    /// again when no drive matches (a drive may have been added).
    pub fn push(&self, event: &ProcessEvent, devices: impl Fn() -> Vec<(String, String)>) {
        self.push_at(event, devices, now());
    }

    /// [`push`](Ring::push) with the current time (seconds since 1970) given.
    pub fn push_at(
        &self,
        event: &ProcessEvent,
        devices: impl Fn() -> Vec<(String, String)>,
        now: u64,
    ) {
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
            (Kind::Stop, None) if event.created == 0 => None,
            (Kind::Stop, None) => inner
                .sessions
                .iter()
                .rev()
                .find(|(key, _)| *key == (event.pid, event.created))
                .map(|(_, session)| *session),
            (Kind::Start, None) => None,
        };
        // A path too long for the reply is not given out; the name is cut.
        let path = path.filter(|p| p.chars().count() <= MAX_PATH);
        let exe: String = match &path {
            Some(path) => path.rsplit('\\').next().unwrap_or(path),
            None => event.image.rsplit('\\').next().unwrap_or(""),
        }
        .chars()
        .take(MAX_EXE)
        .collect();
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
        // Capacity and age (by the clock now, not by the newest event).
        let oldest_kept = now.saturating_sub(MAX_AGE_S);
        while inner.events.len() > CAPACITY
            || inner
                .events
                .front()
                .is_some_and(|front| front.time < oldest_kept)
        {
            inner.events.pop_front();
        }
    }

    /// Up to [`BATCH`] events with `seq >= after`. With `session`, only
    /// those of that Windows session (the reader's own); events whose
    /// session is not known are never given out.
    pub fn read(&self, after: u64, session: Option<u32>) -> Batch {
        self.read_at(after, session, now())
    }

    /// [`read`](Ring::read) with the current time (seconds since 1970) given.
    pub fn read_at(&self, after: u64, session: Option<u32>, now: u64) -> Batch {
        let inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        // A number from the future means the helper restarted since the
        // reader last asked: its events are gone, start from the beginning.
        let after = if after > inner.next { 0 } else { after };
        let oldest_kept = now.saturating_sub(MAX_AGE_S);
        let mut events = Vec::new();
        let mut next = after;
        let mut complete = true;
        for event in inner
            .events
            .iter()
            .filter(|e| e.seq >= after && e.time >= oldest_kept)
        {
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
            lost: self.lost.load(std::sync::atomic::Ordering::Relaxed),
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

    const NOW: u64 = 1_700_000_100;
    const T0: u64 = 1_700_000_000;

    fn push(ring: &Ring, event: &ProcessEvent) {
        ring.push_at(event, devices, NOW);
    }

    #[test]
    fn a_reader_gets_only_events_of_its_own_session_and_resumes_after_next() {
        let ring = Ring::new();
        push(&ring, &start(10, r"\Device\HarddiskVolume3\a.exe", 1, T0));
        push(&ring, &start(11, r"\Device\HarddiskVolume3\b.exe", 2, T0));
        push(
            &ring,
            &start(12, r"\Device\HarddiskVolume3\c.exe", 1, T0 + 1),
        );
        let first = ring.read_at(0, Some(1), NOW);
        let names: Vec<_> = first.events.iter().map(|e| e.exe.as_str()).collect();
        assert_eq!(names, ["a.exe", "c.exe"]);
        assert_eq!(first.next, 3);
        assert_eq!(first.events[0].path.as_deref(), Some(r"C:\a.exe"));
        // Up to date: nothing more, and the next number does not move back.
        let again = ring.read_at(first.next, Some(1), NOW);
        assert!(again.events.is_empty());
        assert_eq!(again.next, 3);
        // Without a known session nothing is given out.
        assert!(ring.read_at(0, None, NOW).events.is_empty());
    }

    #[test]
    fn a_number_from_before_a_restart_starts_again_from_the_beginning() {
        let ring = Ring::new();
        push(&ring, &start(10, r"\Device\HarddiskVolume3\a.exe", 1, T0));
        let batch = ring.read_at(5000, Some(1), NOW);
        assert_eq!(batch.events.len(), 1);
        assert_eq!(batch.next, 1, "next below `after` tells of the restart");
        assert_eq!(ring.read_at(u64::MAX, Some(1), NOW).events.len(), 1);
    }

    #[test]
    fn a_stop_without_a_session_takes_it_from_the_start() {
        let ring = Ring::new();
        let begin = start(10, r"\Device\HarddiskVolume3\a.exe", 5, T0);
        push(&ring, &begin);
        let mut end = begin.clone();
        end.kind = Kind::Stop;
        end.session = None;
        end.image = String::new();
        push(&ring, &end);
        let mut stranger = end.clone();
        stranger.pid = 99;
        push(&ring, &stranger);
        // Without a creation time the pid alone proves nothing.
        let mut unknown = end.clone();
        unknown.created = 0;
        push(&ring, &unknown);
        let batch = ring.read_at(0, Some(5), NOW);
        assert_eq!(batch.events.len(), 2);
        assert_eq!(batch.events[1].kind, "stop");
    }

    #[test]
    fn the_ring_is_bounded_by_count_and_by_age() {
        let ring = Ring::new();
        for i in 0..(CAPACITY as u32 + 10) {
            push(&ring, &start(i, r"\Device\HarddiskVolume3\a.exe", 1, T0));
        }
        let batch = ring.read_at(0, Some(1), NOW);
        assert_eq!(batch.events.first().unwrap().seq, 10);
        // A day later the old events are not given out, even without a
        // new event, and the next push removes them.
        let later = NOW + MAX_AGE_S;
        assert!(ring.read_at(0, Some(1), later).events.is_empty());
        ring.push_at(
            &start(1, r"\Device\HarddiskVolume3\a.exe", 1, later),
            devices,
            later,
        );
        assert_eq!(ring.read_at(0, Some(1), later).events.len(), 1);
    }

    #[test]
    fn traffic_is_summed_over_the_window_and_old_seconds_fall_out() {
        let traffic = Traffic::default();
        let sent = |pid, bytes| NetEvent {
            pid,
            bytes,
            received: false,
        };
        let got = |pid, bytes| NetEvent {
            pid,
            bytes,
            received: true,
        };
        traffic.add_at(&sent(7, 100), NOW);
        traffic.add_at(&got(7, 40), NOW);
        traffic.add_at(&got(7, 10), NOW + 5);
        traffic.add_at(&sent(8, 1), NOW + 5);
        let all = |_: u32| true;
        let usage = traffic.usage(10, NOW + 5, all);
        assert_eq!(
            usage,
            [
                Usage {
                    pid: 7,
                    up: 100,
                    down: 50
                },
                Usage {
                    pid: 8,
                    up: 1,
                    down: 0
                }
            ]
        );
        // A shorter window sees only the later seconds.
        let recent = traffic.usage(3, NOW + 5, all);
        assert_eq!(recent.len(), 2);
        assert_eq!(recent[0].up + recent[0].down, 10);
        // Only the wanted processes, and nothing once the window has passed.
        assert_eq!(traffic.usage(10, NOW + 5, |pid| pid == 8).len(), 1);
        assert!(
            traffic
                .usage(10, NOW + TRAFFIC_WINDOW_S + 100, all)
                .is_empty()
        );
        // Old seconds are dropped when new bytes arrive.
        traffic.add_at(&sent(7, 1), NOW + TRAFFIC_WINDOW_S + 100);
        let counters = traffic.counters.lock().unwrap();
        assert_eq!(counters[&7].len(), 1);
    }

    #[test]
    fn long_names_and_paths_stay_small() {
        let ring = Ring::new();
        let long = format!(r"\Device\HarddiskVolume3\{}\a.exe", "x".repeat(600));
        push(&ring, &start(10, &long, 1, T0));
        let name = "n".repeat(400);
        push(
            &ring,
            &start(11, &format!(r"\Device\HarddiskVolume3\{name}.exe"), 1, T0),
        );
        let events = ring.read_at(0, Some(1), NOW).events;
        assert_eq!(
            events[0].path, None,
            "a path of 600 characters is not given out"
        );
        assert_eq!(events[0].exe, "a.exe");
        assert!(events[1].exe.chars().count() <= MAX_EXE);
        // The worst reply stays far below the reader's limit.
        let worst = serde_json::to_string(&events[1]).unwrap().len() * BATCH;
        assert!(worst < crate::helper::protocol::MAX_REPLY, "{worst}");
    }
}
