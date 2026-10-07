//! One model-owner thread. ONNX/download calls cannot be cancelled: a deadline
//! failure permanently closes admission until the service process restarts.
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};

pub(super) const MAX_BATCH_TEXTS: usize = 32;
pub(super) const MAX_BATCH_BYTES: usize = 2 * 1024 * 1024 + 64 * 1024;
pub(super) const QUERY_DEADLINE: Duration = Duration::from_millis(750);
pub(super) const BATCH_DEADLINE: Duration = Duration::from_secs(30);
const LOAD_DEADLINE: Duration = Duration::from_secs(120);

pub(super) trait Engine: Send {
    fn embed(&mut self, texts: &[String]) -> mdreader_core::Result<Vec<Vec<f32>>>;
}
#[cfg(feature = "local-embed")]
impl Engine for mdreader_core::BgeEmbedder {
    fn embed(&mut self, texts: &[String]) -> mdreader_core::Result<Vec<Vec<f32>>> {
        self.embed(texts)
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ModelState {
    Dormant,
    Loading,
    Ready,
    Busy,
    Unavailable,
    RestartRequired,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ModelError {
    Busy,
    Unavailable,
    TimedOut,
}
type ModelResult = std::result::Result<Vec<Vec<f32>>, ModelError>;
struct State {
    phase: ModelState,
    deadline: Option<Instant>,
    serial: u64,
    budget: crate::workload::Budget,
}
impl State {
    fn refresh(&mut self) {
        if self
            .deadline
            .is_some_and(|deadline| Instant::now() >= deadline)
        {
            self.phase = ModelState::RestartRequired;
            self.deadline = None;
        }
    }
}
enum Command {
    Load {
        serial: u64,
    },
    Embed {
        serial: u64,
        background: bool,
        texts: Vec<String>,
        reply: mpsc::SyncSender<(Instant, ModelResult)>,
    },
}
pub(super) struct Ticket {
    reply: mpsc::Receiver<(Instant, ModelResult)>,
    deadline: Instant,
}
#[derive(Clone)]
pub(super) struct ModelActor {
    commands: mpsc::SyncSender<Command>,
    state: Arc<Mutex<State>>,
    load_deadline: Duration,
}
impl ModelActor {
    pub(super) fn new(cache: std::path::PathBuf, disabled: bool) -> Self {
        Self::with_factory(LOAD_DEADLINE, move || {
            if disabled {
                return Err(mdreader_core::ReaderError::ModelUnavailable);
            }
            #[cfg(feature = "local-embed")]
            {
                Ok(Box::new(mdreader_core::BgeEmbedder::open(&cache)?) as Box<dyn Engine>)
            }
            #[cfg(not(feature = "local-embed"))]
            {
                let _ = cache;
                Err(mdreader_core::ReaderError::ModelUnavailable)
            }
        })
    }
    pub(super) fn with_factory<F>(load_deadline: Duration, factory: F) -> Self
    where
        F: FnOnce() -> mdreader_core::Result<Box<dyn Engine>> + Send + 'static,
    {
        let state = Arc::new(Mutex::new(State {
            phase: ModelState::Dormant,
            deadline: None,
            serial: 0,
            budget: crate::workload::Budget::default(),
        }));
        let owner_state = state.clone();
        // The state gate allows one command total, including the executing one.
        let (commands, receiver) = mpsc::sync_channel(1);
        std::thread::spawn(move || {
            let mut factory = Some(factory);
            let mut engine: Option<Box<dyn Engine>> = None;
            while let Ok(command) = receiver.recv() {
                let started = Instant::now();
                let background = matches!(
                    &command,
                    Command::Embed {
                        background: true,
                        ..
                    }
                );
                let serial = match &command {
                    Command::Load { serial } | Command::Embed { serial, .. } => *serial,
                };
                let (texts, reply) = match command {
                    Command::Load { .. } => (None, None),
                    Command::Embed { texts, reply, .. } => (Some(texts), Some(reply)),
                };
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    if let Some(texts) = texts {
                        engine
                            .as_mut()
                            .ok_or(ModelError::Unavailable)
                            .and_then(|model| {
                                model.embed(&texts).map_err(|_| ModelError::Unavailable)
                            })
                    } else {
                        engine = Some(
                            factory.take().ok_or(ModelError::Unavailable)?()
                                .map_err(|_| ModelError::Unavailable)?,
                        );
                        Ok(Vec::new())
                    }
                }))
                .unwrap_or(Err(ModelError::Unavailable));
                let completed = Instant::now();
                if let Ok(mut state) = owner_state.lock() {
                    state.refresh();
                    if state.serial == serial && state.phase != ModelState::RestartRequired {
                        if background {
                            state.budget.completed(started, completed);
                        }
                        state.phase = if result.is_ok() {
                            ModelState::Ready
                        } else {
                            ModelState::Unavailable
                        };
                        state.deadline = None;
                    }
                }
                if let Some(reply) = reply {
                    // Buffered reply never waits for an abandoned caller, and
                    // completion time distinguishes a late result from a late poll.
                    let _ = reply.try_send((completed, result));
                }
            }
        });
        Self {
            commands,
            state,
            load_deadline,
        }
    }
    pub(super) fn ensure_loaded(&self) -> ModelState {
        let Ok(mut state) = self.state.lock() else {
            return ModelState::Unavailable;
        };
        state.refresh();
        if state.phase == ModelState::Dormant {
            state.serial += 1;
            state.phase = ModelState::Loading;
            state.deadline = Some(Instant::now() + self.load_deadline);
            if self
                .commands
                .try_send(Command::Load {
                    serial: state.serial,
                })
                .is_err()
            {
                state.phase = ModelState::Unavailable;
                state.deadline = None;
            }
        }
        state.phase
    }
    pub(super) fn start(
        &self,
        texts: Vec<String>,
        duration: Duration,
    ) -> std::result::Result<Ticket, ModelError> {
        self.start_classified(texts, duration, false)
    }
    pub(super) fn set_background_policy(&self, policy: crate::workload::EmbeddingPolicy) {
        if let Ok(mut state) = self.state.lock() {
            state.budget.configure(policy);
        }
    }
    pub(super) fn background_ready(&self) -> bool {
        self.state
            .lock()
            .is_ok_and(|state| state.budget.admits(Instant::now()))
    }
    pub(super) fn background_wait(&self) -> Option<Duration> {
        self.state
            .lock()
            .ok()
            .and_then(|state| state.budget.wait(Instant::now()))
    }
    pub(super) fn background_batch_limit(&self) -> usize {
        self.state
            .lock()
            .map_or(8, |state| state.budget.policy.batch_limit())
    }
    pub(super) fn start_background(
        &self,
        texts: Vec<String>,
        duration: Duration,
    ) -> std::result::Result<Ticket, ModelError> {
        self.start_classified(texts, duration, true)
    }
    fn start_classified(
        &self,
        texts: Vec<String>,
        duration: Duration,
        background: bool,
    ) -> std::result::Result<Ticket, ModelError> {
        if texts.is_empty()
            || texts.len() > MAX_BATCH_TEXTS
            || texts.iter().map(String::len).sum::<usize>() > MAX_BATCH_BYTES
        {
            return Err(ModelError::Unavailable);
        }
        let mut state = self.state.lock().map_err(|_| ModelError::Unavailable)?;
        state.refresh();
        if background
            && (!state.budget.admits(Instant::now())
                || texts.len() > state.budget.policy.batch_limit())
        {
            return Err(ModelError::Busy);
        }
        match state.phase {
            ModelState::Ready => {}
            ModelState::Busy | ModelState::Loading | ModelState::Dormant => {
                return Err(ModelError::Busy)
            }
            ModelState::RestartRequired => return Err(ModelError::TimedOut),
            ModelState::Unavailable => return Err(ModelError::Unavailable),
        }
        state.serial += 1;
        let deadline = Instant::now() + duration;
        state.phase = ModelState::Busy;
        state.deadline = Some(deadline);
        let (reply, receiver) = mpsc::sync_channel(1);
        if self
            .commands
            .try_send(Command::Embed {
                serial: state.serial,
                background,
                texts,
                reply,
            })
            .is_err()
        {
            state.phase = ModelState::Unavailable;
            state.deadline = None;
            return Err(ModelError::Unavailable);
        }
        Ok(Ticket {
            reply: receiver,
            deadline,
        })
    }
    pub(super) fn poll(&self, ticket: &mut Ticket) -> Option<ModelResult> {
        // First inspect completion. A completed operation does not time out just
        // because the scheduler was servicing another source at the deadline.
        match ticket.reply.try_recv() {
            Ok((completed, result)) if completed <= ticket.deadline => Some(result),
            Ok(_) => {
                self.restart_required();
                Some(Err(ModelError::TimedOut))
            }
            Err(mpsc::TryRecvError::Disconnected) => Some(Err(ModelError::Unavailable)),
            Err(mpsc::TryRecvError::Empty) if Instant::now() >= ticket.deadline => {
                if let Ok(mut state) = self.state.lock() {
                    state.refresh();
                }
                Some(Err(ModelError::TimedOut))
            }
            Err(mpsc::TryRecvError::Empty) => None,
        }
    }
    fn restart_required(&self) {
        if let Ok(mut state) = self.state.lock() {
            state.phase = ModelState::RestartRequired;
            state.deadline = None;
        }
    }
    pub(super) fn query(
        &self,
        text: String,
        duration: Duration,
    ) -> std::result::Result<Vec<f32>, ModelError> {
        let ticket = self.start(vec![text], duration)?;
        match ticket.reply.recv_timeout(duration) {
            Ok((completed, Ok(mut vectors)))
                if completed <= ticket.deadline && vectors.len() == 1 =>
            {
                Ok(vectors.remove(0))
            }
            Ok((completed, _)) if completed > ticket.deadline => {
                self.restart_required();
                Err(ModelError::TimedOut)
            }
            Ok(_) | Err(mpsc::RecvTimeoutError::Disconnected) => Err(ModelError::Unavailable),
            Err(mpsc::RecvTimeoutError::Timeout) => {
                if let Ok(mut state) = self.state.lock() {
                    state.refresh();
                }
                Err(ModelError::TimedOut)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    #[test]
    fn pause_blocks_background_admission_but_foreground_queries_remain_available() {
        let calls = Arc::new(AtomicUsize::new(0));
        let actor = ModelActor::with_factory(Duration::from_secs(1), move || {
            Ok(Box::new(Fake { calls, wait: None }))
        });
        ready(&actor);
        actor.set_background_policy(crate::workload::EmbeddingPolicy::Paused);
        assert!(matches!(
            actor.start_background(vec!["passage".into()], Duration::from_secs(1)),
            Err(ModelError::Busy)
        ));
        assert_eq!(
            actor
                .query("foreground".into(), Duration::from_secs(1))
                .unwrap()
                .len(),
            384
        );
        actor.set_background_policy(crate::workload::EmbeddingPolicy::Balanced);
        // Deliberately hold the background budget in the future, without sleeps.
        actor
            .state
            .lock()
            .unwrap()
            .budget
            .completed(Instant::now(), Instant::now() + Duration::from_secs(1));
        assert!(!actor.background_ready());
        assert!(actor
            .query("foreground during cooldown".into(), Duration::from_secs(1))
            .is_ok());
        actor.set_background_policy(crate::workload::EmbeddingPolicy::FullSpeed);
        assert!(actor
            .start_background(vec!["passage".into()], Duration::from_secs(1))
            .is_ok());
    }

    struct Fake {
        calls: Arc<AtomicUsize>,
        wait: Option<std::sync::mpsc::Receiver<()>>,
    }
    impl Engine for Fake {
        fn embed(&mut self, texts: &[String]) -> mdreader_core::Result<Vec<Vec<f32>>> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            if let Some(wait) = self.wait.take() {
                wait.recv().unwrap();
            }
            Ok(texts.iter().map(|_| vec![1.0; 384]).collect())
        }
    }
    fn ready(actor: &ModelActor) {
        let until = Instant::now() + Duration::from_secs(1);
        while actor.ensure_loaded() != ModelState::Ready {
            assert!(Instant::now() < until);
            std::thread::yield_now();
        }
    }
    #[test]
    fn a_stuck_initializer_does_not_block_or_spawn_a_second_model() {
        let (release, blocked) = std::sync::mpsc::channel();
        let opens = Arc::new(AtomicUsize::new(0));
        let count = opens.clone();
        let actor = ModelActor::with_factory(Duration::from_millis(20), move || {
            count.fetch_add(1, Ordering::SeqCst);
            blocked.recv().unwrap();
            Ok(Box::new(Fake {
                calls: Arc::new(AtomicUsize::new(0)),
                wait: None,
            }))
        });
        assert_eq!(actor.ensure_loaded(), ModelState::Loading);
        let start = Instant::now();
        assert!(matches!(
            actor.start(vec!["query".into()], Duration::from_millis(5)),
            Err(ModelError::Busy)
        ));
        assert!(start.elapsed() < Duration::from_millis(10));
        std::thread::sleep(Duration::from_millis(30));
        assert_eq!(actor.ensure_loaded(), ModelState::RestartRequired);
        release.send(()).unwrap();
        for _ in 0..100 {
            assert_eq!(actor.ensure_loaded(), ModelState::RestartRequired);
        }
        assert_eq!(opens.load(Ordering::SeqCst), 1);
    }
    #[test]
    fn timed_out_inference_stays_gated_even_after_it_eventually_returns() {
        let (release, blocked) = std::sync::mpsc::channel();
        let calls = Arc::new(AtomicUsize::new(0));
        let observed = calls.clone();
        let actor = ModelActor::with_factory(Duration::from_secs(1), move || {
            Ok(Box::new(Fake {
                calls: observed,
                wait: Some(blocked),
            }))
        });
        ready(&actor);
        let mut ticket = actor
            .start(vec!["query".into()], Duration::from_millis(20))
            .unwrap();
        assert!(matches!(
            actor.start(vec!["second".into()], Duration::from_secs(1)),
            Err(ModelError::Busy)
        ));
        std::thread::sleep(Duration::from_millis(30));
        assert!(matches!(
            actor.poll(&mut ticket),
            Some(Err(ModelError::TimedOut))
        ));
        release.send(()).unwrap();
        std::thread::sleep(Duration::from_millis(10));
        assert_eq!(actor.ensure_loaded(), ModelState::RestartRequired);
        assert!(matches!(
            actor.start(vec!["third".into()], Duration::from_secs(1)),
            Err(ModelError::TimedOut)
        ));
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }
    #[test]
    fn query_deadline_bounds_wait_and_rejects_busy_work_immediately() {
        let (release, blocked) = std::sync::mpsc::channel();
        let calls = Arc::new(AtomicUsize::new(0));
        let actor = ModelActor::with_factory(Duration::from_secs(1), move || {
            Ok(Box::new(Fake {
                calls,
                wait: Some(blocked),
            }))
        });
        ready(&actor);
        let begin = Instant::now();
        assert!(matches!(
            actor.query("hung".into(), Duration::from_millis(20)),
            Err(ModelError::TimedOut)
        ));
        assert!(begin.elapsed() < Duration::from_millis(150));
        assert_eq!(actor.ensure_loaded(), ModelState::RestartRequired);
        let begin = Instant::now();
        assert!(matches!(
            actor.query("again".into(), Duration::from_secs(1)),
            Err(ModelError::TimedOut)
        ));
        assert!(begin.elapsed() < Duration::from_millis(20));
        release.send(()).unwrap();
    }
    #[test]
    fn successful_inference_releases_admission_and_enforces_batch_bounds() {
        let calls = Arc::new(AtomicUsize::new(0));
        let actor = ModelActor::with_factory(Duration::from_secs(1), move || {
            Ok(Box::new(Fake { calls, wait: None }))
        });
        ready(&actor);
        assert!(matches!(
            actor.start(
                vec!["x".into(); MAX_BATCH_TEXTS + 1],
                Duration::from_secs(1)
            ),
            Err(ModelError::Unavailable)
        ));
        let vectors = actor
            .query("hello".into(), Duration::from_millis(100))
            .unwrap();
        assert_eq!(vectors.len(), 384);
        assert_eq!(actor.ensure_loaded(), ModelState::Ready);
        assert!(actor
            .query("again".into(), Duration::from_millis(100))
            .is_ok());
    }
}
