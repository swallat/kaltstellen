//! M2, N1: program information for the main window, read in the
//! background. A signature check hashes the whole file (measured 0.4 s for
//! 238 MiB), so it never runs on the window thread: the window thread only
//! looks up the [`Cache`] and hands what is missing to one reader thread,
//! which reads the version resources of a batch first (about 1 to 7 ms
//! each), then the signatures one after the other, and tells the window
//! when there is more (`program_info` event, at most every
//! [`NOTIFY_EVERY`] while it works).

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use log::info;

use crate::program_info::{self, Cache, Stamp, View};
use crate::win32::fileinfo::VersionResource;

/// How often the reader tells the window about new signatures while it
/// works; once more when the queue is empty.
pub const NOTIFY_EVERY: Duration = Duration::from_millis(300);

type Job = (PathBuf, Stamp);

/// The cache of the window and the way to its reader thread.
pub struct Reader {
    cache: Arc<Mutex<Cache>>,
    jobs: std::cell::RefCell<Option<Sender<Vec<Job>>>>,
    notify: Arc<dyn Fn() + Send + Sync>,
}

fn lock(cache: &Mutex<Cache>) -> MutexGuard<'_, Cache> {
    cache.lock().unwrap_or_else(PoisonError::into_inner)
}

impl Reader {
    /// `notify` is called on the reader thread when more is known.
    pub fn new(notify: impl Fn() + Send + Sync + 'static) -> Self {
        Self {
            cache: Arc::default(),
            jobs: std::cell::RefCell::default(),
            notify: Arc::new(notify),
        }
    }

    /// What is known about each file (`None`: no file, or not on a fixed
    /// local disk); hands the rest to the reader. Never waits for it.
    pub fn views(&self, files: &[Option<PathBuf>]) -> Vec<Option<View>> {
        let mut wanted = Vec::new();
        let views = {
            let mut cache = lock(&self.cache);
            files
                .iter()
                .map(|file| {
                    let file = file.as_ref()?;
                    let stamp = program_info::file_stamp(file)?;
                    let (view, hand_over) = cache.lookup(file, stamp);
                    if hand_over {
                        wanted.push((file.clone(), stamp));
                    }
                    Some(view)
                })
                .collect()
        };
        if !wanted.is_empty() {
            self.hand_over(wanted);
        }
        views
    }

    fn hand_over(&self, jobs: Vec<Job>) {
        let mut sender = self.jobs.borrow_mut();
        if sender.is_none() {
            let (tx, rx) = channel();
            let cache = Arc::clone(&self.cache);
            let notify = Arc::clone(&self.notify);
            std::thread::spawn(move || work(&cache, &rx, notify.as_ref()));
            *sender = Some(tx);
        }
        if let Some(Err(err)) = sender.as_ref().map(|s| s.send(jobs)) {
            // The reader ended (it panicked): nothing more will come.
            let mut cache = lock(&self.cache);
            for (path, _) in err.0 {
                cache.unqueue(&path);
            }
        }
    }
}

/// The reader thread: ends when the window drops its [`Reader`].
fn work(cache: &Mutex<Cache>, jobs: &Receiver<Vec<Job>>, notify: &(dyn Fn() + Send + Sync)) {
    let mut queue: VecDeque<Job> = VecDeque::new();
    loop {
        // Wait only when there is nothing to do.
        let arrived: Vec<Vec<Job>> = if queue.is_empty() {
            match jobs.recv() {
                Ok(batch) => std::iter::once(batch).chain(jobs.try_iter()).collect(),
                Err(_) => return,
            }
        } else {
            jobs.try_iter().collect()
        };
        if !arrived.is_empty() {
            // Version resources first, so descriptions show at once.
            let started = Instant::now();
            let batch: Vec<Job> = arrived.into_iter().flatten().collect();
            for (path, stamp) in &batch {
                if lock(cache).missing(path, *stamp).is_some_and(|m| m.0) {
                    let version =
                        VersionResource::read(path).and_then(|v| program_info::version(&v));
                    lock(cache).set_version(path, *stamp, version);
                }
            }
            info!(
                "gui: version resources of {} files in {} ms",
                batch.len(),
                started.elapsed().as_millis()
            );
            queue.extend(batch);
            notify();
        }
        let started = Instant::now();
        let mut checked = 0;
        while let Some((path, stamp)) = queue.pop_front() {
            if lock(cache).missing(&path, stamp).is_some_and(|m| m.1) {
                let signature = program_info::signature_of(&path, stamp.len);
                lock(cache).set_signature(&path, stamp, signature);
                checked += 1;
            }
            if started.elapsed() >= NOTIFY_EVERY {
                break;
            }
        }
        if checked > 0 {
            info!(
                "gui: {checked} signatures in {} ms, {} waiting",
                started.elapsed().as_millis(),
                queue.len()
            );
            notify();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn m2_reader_answers_at_once_and_fills_in_the_background() {
        let windir = std::env::var_os("SystemRoot").expect("SystemRoot");
        let notepad = std::path::Path::new(&windir)
            .join("System32")
            .join("notepad.exe");
        let dir = tempfile::tempdir().unwrap();
        let junk = dir.path().join("junk.exe");
        std::fs::write(&junk, b"MZ not really a program").unwrap();
        let calls = Arc::new(AtomicUsize::new(0));
        let reader = {
            let calls = Arc::clone(&calls);
            Reader::new(move || {
                calls.fetch_add(1, Ordering::SeqCst);
            })
        };
        let files = vec![
            Some(notepad.clone()),
            Some(junk.clone()),
            None,
            Some(PathBuf::from(r"\server\share\a.exe")),
            Some(PathBuf::from(r"C:\no\such\program.exe")),
        ];
        let first = reader.views(&files);
        assert!(first[0].as_ref().unwrap().pending);
        assert!(first[1].as_ref().unwrap().pending);
        // No file, a share (never asked), a missing file.
        assert_eq!((&first[2], &first[3], &first[4]), (&None, &None, &None));
        let deadline = Instant::now() + Duration::from_secs(20);
        let done = loop {
            let views = reader.views(&files);
            if views.iter().flatten().all(|v| !v.pending) || Instant::now() > deadline {
                break views;
            }
            std::thread::sleep(Duration::from_millis(20));
        };
        let notepad_view = done[0].as_ref().unwrap();
        assert_eq!(notepad_view.signature, "valid", "{notepad_view:?}");
        assert!(notepad_view.description.is_some());
        let junk_view = done[1].as_ref().unwrap();
        assert_eq!(junk_view.signature, "unsigned");
        assert_eq!(junk_view.description, None);
        assert!(calls.load(Ordering::SeqCst) >= 1);
        // A changed file is read again.
        std::fs::write(&junk, b"MZ changed, and longer than before").unwrap();
        assert!(reader.views(&files)[1].as_ref().unwrap().pending);
    }
}
