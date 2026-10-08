use super::*;
use crate::sync::incoming::{IncomingCoordinator, SelectedIncoming};
use std::collections::VecDeque;

pub(super) struct Active {
    pub authority: NativeAuthority,
    pub incoming: Arc<IncomingCoordinator>,
    pub selected: SelectedIncoming,
}
pub(super) struct View {
    pub epoch: u64,
    pub running: Option<Uuid>,
    pub jobs: VecDeque<Job>,
    pub active: Option<Arc<Active>>,
    pub retained: bool,
    pub custody: Option<CustodyView>,
    pub frozen: Option<freeze::Tag>,
    pub projections: usize,
}
impl View {
    pub fn start(&mut self, id: Uuid) {
        if self.jobs.len() == 8 {
            self.jobs.pop_front();
        }
        self.jobs.push_back(Job {
            id,
            state: JobState::Running {
                phase: Phase::Queued,
            },
        });
        self.running = Some(id);
    }
    fn job(&mut self, id: Uuid) -> Result<&mut Job> {
        self.jobs
            .iter_mut()
            .find(|j| j.id == id)
            .ok_or(AccountError::Cancelled)
    }
}
pub(super) struct Shared {
    pub view: Mutex<View>,
    pub controller: Option<Arc<SessionController>>,
    pub changes: tokio::sync::watch::Sender<u64>,
    pub settled: tokio::sync::Notify,
    recovery_available: bool,
}
impl Shared {
    pub fn new(
        controller: Option<Arc<SessionController>>,
        retained: bool,
        recovery_available: bool,
    ) -> Self {
        Self {
            view: Mutex::new(View {
                epoch: 0,
                running: None,
                jobs: VecDeque::new(),
                active: None,
                retained,
                custody: None,
                frozen: None,
                projections: 0,
            }),
            controller,
            changes: tokio::sync::watch::channel(0).0,
            settled: tokio::sync::Notify::new(),
            recovery_available,
        }
    }
    pub fn status(&self) -> Result<AccountStatus> {
        let state = self.view.lock().map_err(|_| AccountError::Interrupted)?;
        Ok(AccountStatus {
            configured: self.controller.is_some(),
            recovery_available: self.recovery_available,
            retained_library: state.retained,
            connected: state
                .active
                .as_ref()
                .is_some_and(|a| a.authority.scope().is_ok()),
            custody: state.custody.clone(),
            latest_job: state.jobs.back().cloned(),
        })
    }
    pub fn invalidate(&self, requested: Option<Uuid>) -> Result<()> {
        let mut state = self.view.lock().map_err(|_| AccountError::Interrupted)?;
        if state.frozen.is_some() {
            return Err(AccountError::Busy);
        }
        if requested.is_some() && state.running != requested {
            return Err(AccountError::Conflict);
        }
        state.epoch = state.epoch.checked_add(1).ok_or(AccountError::Limit)?;
        if let Some(id) = state.running {
            state.job(id)?.state = JobState::Cancelled;
        }
        // Wake the actor even if a poisoned platform/selection lock fails below.
        self.changes.send_replace(state.epoch);
        // Short synchronous lease change only. No OS/network/SQLite operation here.
        if let Some(controller) = &self.controller {
            controller.logout()?;
        }
        if let Some(active) = state.active.take() {
            active
                .incoming
                .clear_selection()
                .map_err(|_| AccountError::Interrupted)?;
        }
        // running stays occupied until the serial actor observes cancellation.
        Ok(())
    }
    pub fn check(&self, command: &actor::Command) -> Result<()> {
        let state = self.view.lock().map_err(|_| AccountError::Interrupted)?;
        if state.epoch != command.epoch || state.running != Some(command.id) {
            return Err(AccountError::Cancelled);
        }
        Ok(())
    }
    pub fn phase(&self, command: &actor::Command, phase: Phase) -> Result<()> {
        let mut state = self.view.lock().map_err(|_| AccountError::Interrupted)?;
        if state.epoch != command.epoch || state.running != Some(command.id) {
            return Err(AccountError::Cancelled);
        }
        state.job(command.id)?.state = JobState::Running { phase };
        Ok(())
    }
    pub fn finish(&self, command: &actor::Command, result: Result<Outcome>) {
        if let Ok(mut state) = self.view.lock() {
            if state.running != Some(command.id) {
                return;
            }
            if state.epoch == command.epoch {
                if let Ok(job) = state.job(command.id) {
                    job.state = match result {
                        Ok(outcome) => JobState::Completed { outcome },
                        Err(error) => JobState::Failed { error },
                    };
                }
            }
            state.running = None;
            self.settled.notify_one();
        }
    }
}
