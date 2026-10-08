//! Actual dialog/file jobs stay retained after a cancelled account await.
use mdreader_service::accounts::AccountError;
use std::{
    future::Future,
    sync::{Arc, Mutex},
    time::Instant,
};
use tokio::{
    sync::{oneshot, watch},
    task::JoinHandle,
};
type Result<T> = std::result::Result<T, AccountError>;

pub(super) struct Cancel(watch::Sender<bool>);
impl Cancel {
    fn new() -> Arc<Self> {
        Arc::new(Self(watch::channel(false).0))
    }
    pub(super) fn cancel(&self) {
        self.0.send_replace(true);
    }
    pub(super) fn check(&self) -> Result<()> {
        if *self.0.borrow() {
            Err(AccountError::Cancelled)
        } else {
            Ok(())
        }
    }
    pub(super) async fn wait(&self) {
        let mut receiver = self.0.subscribe();
        while !*receiver.borrow_and_update() {
            if receiver.changed().await.is_err() {
                return;
            }
        }
    }
}
struct CancelOnDrop(Arc<Cancel>);
impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        self.0.cancel();
    }
}
struct Job {
    cancel: Arc<Cancel>,
    join: JoinHandle<()>,
}
#[derive(Default)]
pub(super) struct Owner {
    job: Mutex<Option<Job>>,
}
impl Owner {
    pub(super) async fn run<T: Send + 'static, F: Future<Output = Result<T>> + Send + 'static>(
        &self,
        operation: impl FnOnce(Arc<Cancel>) -> F + Send + 'static,
    ) -> Result<T> {
        let cancel = Cancel::new();
        let (send, receive) = oneshot::channel();
        {
            let mut current = self.job.lock().map_err(|_| AccountError::Interrupted)?;
            if current.as_ref().is_some_and(|job| !job.join.is_finished()) {
                return Err(AccountError::Busy);
            }
            let task_cancel = cancel.clone();
            // The actual JoinHandle remains in this owner if the waiting RPC or
            // actor future is cancelled. A later freeze joins this exact task.
            *current = Some(Job {
                cancel: cancel.clone(),
                join: tokio::spawn(async move {
                    let _ = send.send(operation(task_cancel).await);
                }),
            });
        }
        let _guard = CancelOnDrop(cancel);
        receive.await.map_err(|_| AccountError::Interrupted)?
    }
    pub(super) async fn settle(&self, deadline: Instant) -> Result<()> {
        loop {
            let completed = {
                let mut current = self.job.lock().map_err(|_| AccountError::Interrupted)?;
                match current.as_ref() {
                    None => return Ok(()),
                    Some(job) if job.join.is_finished() => current.take(),
                    Some(job) => {
                        job.cancel.cancel();
                        None
                    }
                }
            };
            if let Some(job) = completed {
                return job.join.await.map_err(|_| AccountError::Interrupted);
            }
            if Instant::now() >= deadline {
                return Err(AccountError::Interrupted);
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    }
}
