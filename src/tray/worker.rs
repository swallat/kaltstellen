//! Runs switches on a worker thread so the menu stays responsive, feeding
//! it through the W3 [`Queue`]. The window thread owns the [`Controller`];
//! the worker reports each finished job through a callback.

use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};

use log::{info, warn};

pub use super::model::Job;
use super::model::{Decision, Queue};
use super::source::Source;
use crate::config::NEUTRAL;
use crate::engine::{self, Event, Options};
use crate::error::Error;
use crate::state::Store;

/// What a job left behind.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outcome {
    /// False when nothing could be attempted (unknown profile, lock held).
    pub ran: bool,
    pub active: String,
    pub events: Vec<Event>,
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

pub struct Controller {
    queue: Queue,
    quitting: bool,
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
            quitting: false,
            jobs: Some(jobs),
            worker: Some(worker),
        }
    }

    fn send(&self, job: Job) {
        info!("tray: running {job}");
        if let Some(jobs) = &self.jobs
            && jobs.send(job).is_err()
        {
            warn!("tray: the switch thread is gone");
        }
    }

    /// A request from the menu or the pipe. While quitting nothing new is
    /// accepted; the caller checks [`Controller::is_quitting`] first.
    pub fn request(&mut self, job: Job) -> Decision {
        if self.quitting {
            info!("tray: quitting, {job} ignored");
            return Decision::Refused;
        }
        let label = job.to_string();
        let decision = self.queue.request(job);
        match &decision {
            Decision::Start(job) => self.send(job.clone()),
            Decision::Queued { superseded } => {
                info!("tray: {label} waits for the running job");
                if let Some(old) = superseded {
                    info!("tray: {old} superseded");
                }
            }
            Decision::Dropped { superseded } => {
                info!("tray: {label} dropped, the running job leads there");
                if let Some(old) = superseded {
                    info!("tray: {old} superseded");
                }
            }
            Decision::Refused => info!("tray: {label} refused, the tray quits or the session ends"),
            Decision::Ignored => {
                info!("tray: {label} ignored, a manual request waits (manual beats automatic)")
            }
        }
        decision
    }

    /// Runs `job` now or right after the running one; a waiting request is
    /// dropped and returned.
    fn jump(&mut self, job: Job) -> Option<Job> {
        match self.queue.jump(job) {
            Ok(job) => {
                self.send(job);
                None
            }
            Err(superseded) => {
                if let Some(old) = &superseded {
                    info!("tray: {old} superseded");
                }
                superseded
            }
        }
    }

    /// L2. Returns the waiting request it dropped, if any.
    pub fn quit(&mut self) -> Option<Job> {
        if self.quitting {
            return None;
        }
        self.quitting = true;
        self.jump(Job::Quit)
    }

    /// S5: resume everything as soon as possible. Returns the waiting
    /// request it dropped, if any.
    pub fn end_session(&mut self) -> Option<Job> {
        if self.quitting
            || matches!(self.queue.running(), Some(Job::EndSession))
            || matches!(self.queue.pending(), Some(Job::EndSession))
        {
            return None;
        }
        self.jump(Job::EndSession)
    }

    /// The window thread got the result of `done`. Starts what comes next
    /// and returns it.
    pub fn finished(&mut self, done: &Job) -> Option<Job> {
        if *done == Job::Quit {
            return None;
        }
        let next = self.queue.finish();
        if let Some(job) = &next {
            self.send(job.clone());
        }
        next
    }

    pub fn running(&self) -> Option<&Job> {
        self.queue.running()
    }

    pub fn pending(&self) -> Option<&Job> {
        self.queue.pending()
    }

    pub fn is_quitting(&self) -> bool {
        self.quitting
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
/// state lock (L1) and reads the state afresh.
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
                    ran: false,
                    active,
                    events: Vec::new(),
                    errors: vec![err],
                };
            }
        };
        let (mut state, problem) = store.load();
        let mut errors: Vec<Error> = problem.into_iter().collect();
        let mut persist = |s: &crate::state::State| store.save(s);
        let result = match job {
            Job::Recover | Job::Quit | Job::EndSession => {
                // H5: an activation that ends here has no B.
                if let Some(data) = crate::paths::data_dir() {
                    crate::usage::drop_pending(&data);
                }
                engine::recover(&mut state, &mut persist)
            }
            Job::Switch {
                target,
                force,
                guests,
                ..
            } => {
                // C4: the switch uses the configuration as it is now and
                // keeps it to the end, whatever happens to the file meanwhile.
                let config = {
                    let mut source = self.source.lock().unwrap_or_else(|e| e.into_inner());
                    source.refresh();
                    source.config().clone()
                };
                let options = Options { force: *force };
                let data = crate::paths::data_dir();
                let learning = data.clone().filter(|_| crate::usage::enabled());
                if learning.is_none()
                    && let Some(data) = &data
                {
                    // Off: nothing is recorded and no old A is kept.
                    crate::usage::drop_pending(data);
                }
                let stopped = crate::usage::stopped_names(&state);
                if let Some(data) = &learning
                    && crate::usage::known_target(&config, target)
                {
                    crate::usage::before(data, true, &config, &state);
                }
                let result = engine::switch_guarded(
                    &config,
                    target,
                    &mut state,
                    options,
                    &mut persist,
                    &|| false,
                    &|| guests.list(),
                );
                if let (Some(data), Ok(report)) = (&learning, &result)
                    && report
                        .errors
                        .iter()
                        .all(|e| e.code != crate::error::Code::Interrupted)
                {
                    crate::usage::after(data, true, &state, &stopped);
                }
                result
            }
        };
        let mut events = Vec::new();
        let ran = match result {
            Ok(report) => {
                events = report.events;
                errors.extend(report.errors);
                true
            }
            Err(err) => {
                warn!("{err}");
                errors.push(err);
                false
            }
        };
        Outcome {
            ran,
            active: state.active_profile,
            events,
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
            Outcome {
                ran: true,
                active: job.leads_to().to_owned(),
                events: vec![],
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
            release,
            controller,
            started,
            done,
        }
    }

    const WAIT: Duration = Duration::from_secs(5);

    fn sw(target: &str) -> Job {
        Job::switch(target)
    }

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
        r.controller.request(Job::Recover);
        assert_eq!(r.started(), Job::Recover);
        // Neutral is where recovery leads: dropped.
        assert_eq!(
            r.controller.request(sw("neutral")),
            Decision::Dropped { superseded: None }
        );
        r.controller.request(sw("a"));
        r.controller.request(sw("b"));
        assert!(r.nothing_started(), "the worker runs one job at a time");
        let (done, next) = r.complete();
        assert_eq!(done.job, Job::Recover);
        assert_eq!(next, Some(sw("b")));
        assert_eq!(r.started(), sw("b"));
        assert_eq!(r.controller.running(), Some(&sw("b")));
        let (done, next) = r.complete();
        assert_eq!(done.outcome.active, "b");
        assert_eq!(next, None);
        assert_eq!(r.controller.running(), None);
        assert!(r.nothing_started(), "\"a\" was superseded and never ran");
    }

    #[test]
    fn l2_quit_waits_for_the_running_switch_and_drops_the_waiting_one() {
        let mut r = rig();
        r.controller.request(sw("a"));
        assert_eq!(r.started(), sw("a"));
        r.controller.request(sw("b"));
        assert_eq!(r.controller.quit(), Some(sw("b")));
        assert!(r.controller.is_quitting());
        assert_eq!(
            r.controller.request(sw("c")),
            Decision::Refused,
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
        assert_eq!(r.controller.quit(), None);
        assert_eq!(r.started(), Job::Quit);
        assert_eq!(r.controller.quit(), None, "a second quit does nothing");
        r.complete();
    }

    #[test]
    fn s5_end_session_runs_next_and_once() {
        let mut r = rig();
        r.controller.request(sw("a"));
        r.started();
        r.controller.request(sw("b"));
        assert_eq!(r.controller.end_session(), Some(sw("b")));
        assert_eq!(r.controller.end_session(), None, "asked once");
        let (_, next) = r.complete();
        assert_eq!(next, Some(Job::EndSession));
        assert_eq!(r.started(), Job::EndSession);
        let (done, next) = r.complete();
        assert_eq!(done.outcome.active, "neutral");
        assert_eq!(next, None);
        // The tray keeps running: new requests work again.
        assert!(matches!(r.controller.request(sw("c")), Decision::Start(_)));
        r.started();
        r.complete();
    }
}
