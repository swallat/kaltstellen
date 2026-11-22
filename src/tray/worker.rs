//! Runs switches on a worker thread so the menu stays responsive, feeding
//! it through the W3 [`Queue`]. The window thread owns the [`Controller`];
//! the worker reports each finished job through a callback.

use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};

use log::{info, warn};

use super::model::{Decision, Queue};
use super::source::Source;
use crate::config::NEUTRAL;
use crate::engine::{self, Options};
use crate::error::Error;
use crate::state::Store;

/// Work for the worker thread.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Job {
    /// S3 at start: resume what a crashed instance left frozen; `neutral`.
    Recover,
    Switch(String),
    /// L2: resume everything suspended, keep stopped records.
    Quit,
}

/// What a job left behind.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outcome {
    pub active: String,
    pub errors: Vec<Error>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Done {
    pub job: Job,
    pub outcome: Outcome,
}

/// Carries out jobs; [`EngineRunner`] for real, a fake in tests.
pub trait Runner: Send + 'static {
    fn run(&mut self, job: &Job) -> Outcome;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Quit {
    No,
    /// Asked for while a switch runs; starts when it ends.
    Requested,
    Running,
}

pub struct Controller {
    queue: Queue,
    quit: Quit,
    jobs: Option<Sender<Job>>,
    worker: Option<JoinHandle<()>>,
}

impl Controller {
    /// Starts the worker thread. `notify` runs on the worker after each job.
    pub fn spawn(mut runner: impl Runner, notify: impl Fn(Done) + Send + 'static) -> Self {
        let (jobs, inbox) = mpsc::channel::<Job>();
        let worker = thread::Builder::new()
            .name("switch".to_owned())
            .spawn(move || {
                for job in inbox {
                    let outcome = runner.run(&job);
                    notify(Done { job, outcome });
                }
            })
            .expect("cannot start the switch thread");
        Self {
            queue: Queue::default(),
            quit: Quit::No,
            jobs: Some(jobs),
            worker: Some(worker),
        }
    }

    fn send(&self, job: Job) {
        info!("tray: running {job:?}");
        if let Some(jobs) = &self.jobs
            && jobs.send(job).is_err()
        {
            warn!("tray: the switch thread is gone");
        }
    }

    /// S3: recovery counts as a switch to `neutral` for the queue, so a
    /// `Neutral` click during it is dropped and any other click waits.
    pub fn recover(&mut self) {
        if let Decision::Start(_) = self.queue.request(NEUTRAL) {
            self.send(Job::Recover);
        }
    }

    /// A menu click on a profile.
    pub fn request(&mut self, target: &str) -> Decision {
        if self.quit != Quit::No {
            info!("tray: quitting, request for \"{target}\" ignored");
            return Decision::Dropped { superseded: None };
        }
        let decision = self.queue.request(target);
        match &decision {
            Decision::Start(target) => self.send(Job::Switch(target.clone())),
            Decision::Queued { superseded } => {
                info!("tray: \"{}\" waits for the running switch", target.trim());
                if let Some(old) = superseded {
                    info!("tray: request for \"{old}\" superseded");
                }
            }
            Decision::Dropped { superseded } => {
                info!(
                    "tray: \"{}\" dropped, the running switch leads there",
                    target.trim()
                );
                if let Some(old) = superseded {
                    info!("tray: request for \"{old}\" superseded");
                }
            }
        }
        decision
    }

    /// The window thread got `done`. Starts what comes next and returns it.
    pub fn finished(&mut self, done: &Job) -> Option<Job> {
        if *done == Job::Quit {
            return None;
        }
        if self.quit == Quit::Requested {
            self.queue.finish();
            self.quit = Quit::Running;
            self.send(Job::Quit);
            return Some(Job::Quit);
        }
        let next = self.queue.finish().map(Job::Switch);
        if let Some(job) = &next {
            self.send(job.clone());
        }
        next
    }

    /// L2. Returns true if the quit job started now; otherwise it starts
    /// when the running switch ends. A waiting request is dropped.
    pub fn quit(&mut self) -> bool {
        if self.quit != Quit::No {
            return false;
        }
        if let Some(old) = self.queue.clear_pending() {
            info!("tray: quitting, request for \"{old}\" dropped");
        }
        if self.queue.running().is_some() {
            self.quit = Quit::Requested;
            false
        } else {
            self.quit = Quit::Running;
            self.send(Job::Quit);
            true
        }
    }

    /// Target of the running switch, if any.
    pub fn running(&self) -> Option<&str> {
        match self.quit {
            Quit::Running => None,
            _ => self.queue.running(),
        }
    }

    pub fn is_quitting(&self) -> bool {
        self.quit != Quit::No
    }
}

impl Drop for Controller {
    /// Lets the worker finish its job and end.
    fn drop(&mut self) {
        self.jobs.take();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

/// The real runner: the switch engine on the state file. Each job takes the
/// state lock (L1) and reads the state afresh, so a command-line switch
/// between two tray switches is not lost.
pub struct EngineRunner {
    store: Store,
    source: Arc<Mutex<Source>>,
}

impl EngineRunner {
    pub fn new(store: Store, source: Arc<Mutex<Source>>) -> Self {
        Self { store, source }
    }
}

impl Runner for EngineRunner {
    fn run(&mut self, job: &Job) -> Outcome {
        let store = self.store.clone();
        let _lock = match store.lock() {
            Ok(lock) => lock,
            Err(err) => {
                warn!("{err}");
                let active = store
                    .peek()
                    .map_or_else(|| NEUTRAL.to_owned(), |s| s.active_profile);
                return Outcome {
                    active,
                    errors: vec![err],
                };
            }
        };
        let (mut state, problem) = store.load();
        let mut errors: Vec<Error> = problem.into_iter().collect();
        let mut persist = |s: &crate::state::State| store.save(s);
        let result = match job {
            Job::Recover | Job::Quit => engine::recover(&mut state, &mut persist),
            Job::Switch(target) => {
                // C4: the switch uses the configuration as it is now and
                // keeps it to the end, whatever happens to the file meanwhile.
                let config = {
                    let mut source = self.source.lock().unwrap_or_else(|e| e.into_inner());
                    source.refresh();
                    source.config().clone()
                };
                engine::switch(
                    &config,
                    target,
                    &mut state,
                    Options::default(),
                    &mut persist,
                )
            }
        };
        match result {
            Ok(report) => errors.extend(report.errors),
            Err(err) => {
                warn!("{err}");
                errors.push(err);
            }
        }
        Outcome {
            active: state.active_profile,
            errors,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc::{Receiver, channel};
    use std::time::Duration;

    /// Blocks every job until the test releases it.
    struct Gate {
        started: Sender<Job>,
        release: Receiver<()>,
    }

    impl Runner for Gate {
        fn run(&mut self, job: &Job) -> Outcome {
            self.started.send(job.clone()).unwrap();
            self.release.recv().unwrap();
            let active = match job {
                Job::Switch(t) => t.clone(),
                _ => NEUTRAL.to_owned(),
            };
            Outcome {
                active,
                errors: vec![],
            }
        }
    }

    /// `release` is dropped first, so a job still blocked ends and the
    /// controller's drop can join the worker.
    struct Rig {
        release: Sender<()>,
        controller: Controller,
        started: Receiver<Job>,
        done: Receiver<Done>,
    }

    fn rig() -> Rig {
        let (started_tx, started) = channel();
        let (release, release_rx) = channel();
        let (done_tx, done) = channel();
        let controller = Controller::spawn(
            Gate {
                started: started_tx,
                release: release_rx,
            },
            move |d| done_tx.send(d).unwrap(),
        );
        Rig {
            controller,
            started,
            release,
            done,
        }
    }

    const WAIT: Duration = Duration::from_secs(5);

    impl Rig {
        fn started(&self) -> Job {
            self.started.recv_timeout(WAIT).expect("a job started")
        }

        /// Lets the running job end and hands its result to the controller.
        fn complete(&mut self) -> (Done, Option<Job>) {
            self.release.send(()).unwrap();
            let done = self.done.recv_timeout(WAIT).expect("job done");
            let next = self.controller.finished(&done.job);
            (done, next)
        }

        fn nothing_started(&self) -> bool {
            self.started
                .recv_timeout(Duration::from_millis(100))
                .is_err()
        }
    }

    #[test]
    fn w3_requests_during_a_switch_run_only_the_latest_afterwards() {
        let mut r = rig();
        r.controller.recover();
        assert_eq!(r.started(), Job::Recover);
        // Neutral is where recovery leads: dropped.
        assert_eq!(
            r.controller.request("neutral"),
            Decision::Dropped { superseded: None }
        );
        r.controller.request("a");
        r.controller.request("b");
        assert!(r.nothing_started(), "the worker runs one job at a time");
        let (done, next) = r.complete();
        assert_eq!(done.job, Job::Recover);
        assert_eq!(next, Some(Job::Switch("b".into())));
        assert_eq!(r.started(), Job::Switch("b".into()));
        assert_eq!(r.controller.running(), Some("b"));
        let (done, next) = r.complete();
        assert_eq!(done.outcome.active, "b");
        assert_eq!(next, None);
        assert_eq!(r.controller.running(), None);
        assert!(r.nothing_started(), "\"a\" was superseded and never ran");
    }

    #[test]
    fn l2_quit_waits_for_the_running_switch_and_drops_the_waiting_one() {
        let mut r = rig();
        r.controller.request("a");
        assert_eq!(r.started(), Job::Switch("a".into()));
        r.controller.request("b");
        assert!(!r.controller.quit(), "quit waits for the running switch");
        assert_eq!(
            r.controller.request("c"),
            Decision::Dropped { superseded: None },
            "no new switches while quitting"
        );
        let (_, next) = r.complete();
        assert_eq!(next, Some(Job::Quit));
        assert_eq!(r.started(), Job::Quit);
        let (done, next) = r.complete();
        assert_eq!(done.job, Job::Quit);
        assert_eq!(next, None);
        assert!(r.nothing_started());
    }

    #[test]
    fn l2_quit_when_idle_starts_at_once() {
        let mut r = rig();
        assert!(r.controller.quit());
        assert_eq!(r.started(), Job::Quit);
        assert!(!r.controller.quit(), "a second quit does nothing");
        r.complete();
    }
}
