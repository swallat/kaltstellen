//! H2: what a program costs, from two samples of the process list taken a
//! moment apart. Safe code only; the samples come from
//! [`crate::process::Snapshot`] and [`crate::win32::net`].
//!
//! CPU is kernel plus user time over the interval, divided by interval and
//! number of logical processors: an average share of the whole CPU, not a
//! peak. I/O is the sum of the process I/O counters, which count files,
//! pipes and sockets together: it is labelled `I/O`, never `Network`
//! (spec R10). A process that is in only one of the two samples is left out
//! of the deltas, so the figures can undercount.

use std::collections::{HashMap, HashSet};
use std::time::Duration;

use crate::process::ProcessInfo;
use crate::win32::net::Connections;

/// 500 MiB of private memory.
pub const HEAVY_MEMORY: u64 = 500 * 1024 * 1024;
/// 5 % of the whole CPU.
pub const HEAVY_CPU: f64 = 0.05;
/// 1 MiB per second of I/O.
pub const HEAVY_IO: u64 = 1024 * 1024;

/// One reading of the system.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Sample {
    pub processes: Vec<ProcessInfo>,
    pub connections: HashMap<u32, Connections>,
    /// PID and creation time of the processes kaltstellen holds frozen.
    pub frozen: HashSet<(u32, u64)>,
    /// Logical processors.
    pub cpus: u32,
}

/// The figures of one program.
#[derive(Debug, Clone, PartialEq)]
pub struct Load {
    /// Held frozen by kaltstellen: shown as `frozen`, not 0 %.
    pub frozen: bool,
    pub private_bytes: u64,
    /// Share of the whole CPU, 0.0 to 1.0.
    pub cpu: Option<f64>,
    /// Bytes per second.
    pub io_per_s: Option<u64>,
    pub tcp: u32,
    pub udp: u32,
    /// Why the program is `Heavy`, e.g. `1.3 GB memory`.
    pub heavy: Option<String>,
}

/// H2 with the helper: bytes per second sent and received by the processes
/// of `exe`, from the per-process byte counts of the last `window` seconds.
pub fn network(
    processes: &[ProcessInfo],
    usage: &[crate::helper::events::Usage],
    exe: &str,
    window: u32,
) -> (u64, u64) {
    let name = exe.to_lowercase();
    let pids: HashSet<u32> = processes
        .iter()
        .filter(|p| p.name.to_lowercase() == name)
        .map(|p| p.pid)
        .collect();
    let (up, down) = usage
        .iter()
        .filter(|u| pids.contains(&u.pid))
        .fold((0u64, 0u64), |(up, down), u| {
            (up.saturating_add(u.up), down.saturating_add(u.down))
        });
    let seconds = u64::from(window.max(1));
    (up / seconds, down / seconds)
}

/// The figures of the program `exe` (all its processes) between `before`
/// and `after`, which lie `elapsed` apart.
pub fn measure(before: &Sample, after: &Sample, elapsed: Duration, exe: &str) -> Load {
    let name = exe.to_lowercase();
    let mine: Vec<&ProcessInfo> = after
        .processes
        .iter()
        .filter(|p| p.name.to_lowercase() == name)
        .collect();
    let earlier: HashMap<(u32, u64), &ProcessInfo> = before
        .processes
        .iter()
        .map(|p| ((p.pid, p.created), p))
        .collect();
    let frozen = mine
        .iter()
        .any(|p| after.frozen.contains(&(p.pid, p.created)));
    let private_bytes = mine.iter().map(|p| p.private_bytes).sum();
    let (mut cpu_time, mut io_bytes) = (0u64, 0u64);
    for p in &mine {
        if let Some(old) = earlier.get(&(p.pid, p.created)) {
            cpu_time += p.cpu_time.saturating_sub(old.cpu_time);
            io_bytes += p.io_bytes.saturating_sub(old.io_bytes);
        }
    }
    let seconds = elapsed.as_secs_f64();
    let (cpu, io_per_s) = if frozen || seconds <= 0.0 {
        (None, None)
    } else {
        let capacity = seconds * 1e7 * f64::from(after.cpus.max(1));
        (
            Some((cpu_time as f64 / capacity).clamp(0.0, 1.0)),
            Some((io_bytes as f64 / seconds) as u64),
        )
    };
    let (mut tcp, mut udp) = (0, 0);
    for p in &mine {
        if let Some(c) = after.connections.get(&p.pid) {
            tcp += c.tcp;
            udp += c.udp;
        }
    }
    Load {
        frozen,
        private_bytes,
        cpu,
        io_per_s,
        tcp,
        udp,
        heavy: heavy(private_bytes, cpu, io_per_s),
    }
}

/// `Heavy` (spec H2): names every reason that applies.
pub fn heavy(private_bytes: u64, cpu: Option<f64>, io_per_s: Option<u64>) -> Option<String> {
    let mut reasons = Vec::new();
    if private_bytes >= HEAVY_MEMORY {
        reasons.push(format!(
            "{:.1} GB memory",
            private_bytes as f64 / (1024.0 * 1024.0 * 1024.0)
        ));
    }
    if let Some(cpu) = cpu.filter(|c| *c >= HEAVY_CPU) {
        reasons.push(format!("{:.0} % CPU", cpu * 100.0));
    }
    if let Some(io) = io_per_s.filter(|i| *i >= HEAVY_IO) {
        reasons.push(format!("{:.1} MB/s I/O", io as f64 / (1024.0 * 1024.0)));
    }
    (!reasons.is_empty()).then(|| reasons.join(", "))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(pid: u32, name: &str, cpu: u64, io: u64, bytes: u64) -> ProcessInfo {
        ProcessInfo {
            pid,
            name: name.to_owned(),
            created: 7,
            cpu_time: cpu,
            io_bytes: io,
            private_bytes: bytes,
            ..ProcessInfo::default()
        }
    }

    fn sample(processes: Vec<ProcessInfo>) -> Sample {
        Sample {
            processes,
            cpus: 4,
            ..Sample::default()
        }
    }

    #[test]
    fn h2_cpu_is_a_share_of_the_whole_cpu() {
        // Two processes use 0.2 s of CPU each in 1 s on 4 processors: 10 %.
        let before = sample(vec![p(1, "a.exe", 0, 0, 0), p(2, "A.exe", 0, 0, 0)]);
        let after = sample(vec![
            p(1, "a.exe", 2_000_000, 1000, 10),
            p(2, "A.exe", 2_000_000, 3000, 20),
        ]);
        let load = measure(&before, &after, Duration::from_secs(1), "a.exe");
        assert!((load.cpu.unwrap() - 0.1).abs() < 1e-9);
        assert_eq!(load.io_per_s, Some(4000));
        assert_eq!(load.private_bytes, 30);
        assert!(load.heavy.as_deref().unwrap().contains("10 % CPU"));
    }

    #[test]
    fn h2_a_process_in_one_sample_only_is_left_out_of_the_delta() {
        let before = sample(vec![p(1, "a.exe", 100, 0, 0)]);
        // pid 2 started in between: its whole time is not a delta.
        let after = sample(vec![
            p(1, "a.exe", 100, 0, 0),
            p(2, "a.exe", 9_999_999, 0, 0),
        ]);
        let load = measure(&before, &after, Duration::from_secs(1), "a.exe");
        assert_eq!(load.cpu, Some(0.0));
    }

    #[test]
    fn h2_a_frozen_program_shows_frozen_and_no_figures() {
        let before = sample(vec![p(1, "a.exe", 0, 0, 0)]);
        let mut after = sample(vec![p(1, "a.exe", 0, 0, 5)]);
        after.frozen.insert((1, 7));
        let load = measure(&before, &after, Duration::from_secs(1), "a.exe");
        assert!(load.frozen);
        assert_eq!((load.cpu, load.io_per_s), (None, None));
    }

    #[test]
    fn h2_connections_are_summed_over_the_processes() {
        let before = sample(vec![p(1, "a.exe", 0, 0, 0), p(2, "a.exe", 0, 0, 0)]);
        let mut after = before.clone();
        after.connections.insert(1, Connections { tcp: 2, udp: 1 });
        after.connections.insert(2, Connections { tcp: 1, udp: 0 });
        after.connections.insert(9, Connections { tcp: 5, udp: 5 });
        let load = measure(&before, &after, Duration::from_secs(1), "a.exe");
        assert_eq!((load.tcp, load.udp), (3, 1));
    }

    #[test]
    fn h2_network_bytes_are_summed_per_program_and_made_per_second() {
        use crate::helper::events::Usage;
        let processes = vec![
            p(1, "a.exe", 0, 0, 0),
            p(2, "A.exe", 0, 0, 0),
            p(3, "b.exe", 0, 0, 0),
        ];
        let usage = vec![
            Usage {
                pid: 1,
                up: 100,
                down: 1000,
            },
            Usage {
                pid: 2,
                up: 300,
                down: 3000,
            },
            Usage {
                pid: 3,
                up: 9,
                down: 9,
            },
            Usage {
                pid: 99,
                up: 7,
                down: 7,
            },
        ];
        assert_eq!(network(&processes, &usage, "a.exe", 2), (200, 2000));
        assert_eq!(network(&processes, &usage, "none.exe", 2), (0, 0));
    }

    #[test]
    fn h2_heavy_names_every_reason() {
        assert_eq!(heavy(1, Some(0.01), Some(10)), None);
        let reason = heavy(HEAVY_MEMORY, Some(0.5), Some(2 * HEAVY_IO)).unwrap();
        assert_eq!(reason, "0.5 GB memory, 50 % CPU, 2.0 MB/s I/O");
    }
}
