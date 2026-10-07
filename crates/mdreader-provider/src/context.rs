use crate::*;
use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::Instant,
};
#[derive(Clone, Default)]
pub struct Cancellation {
    inner: Arc<CancelInner>,
}
#[derive(Default)]
struct CancelInner {
    flag: AtomicBool,
    notify: tokio::sync::Notify,
}
impl Cancellation {
    pub fn cancel(&self) {
        self.inner.flag.store(true, Ordering::Release);
        self.inner.notify.notify_waiters();
    }
    pub fn is_cancelled(&self) -> bool {
        self.inner.flag.load(Ordering::Acquire)
    }
    pub async fn cancelled(&self) {
        loop {
            let notified = self.inner.notify.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if self.is_cancelled() {
                return;
            }
            notified.await;
        }
    }
}
pub struct IoContext {
    pub operation_id: uuid::Uuid,
    deadline: Instant,
    page_rows: usize,
    byte_limit: usize,
    cancellation: Cancellation,
}
impl IoContext {
    pub fn new(
        operation_id: uuid::Uuid,
        deadline: Instant,
        page_rows: usize,
        byte_limit: usize,
        cancellation: Cancellation,
    ) -> Result<Self> {
        if operation_id.is_nil()
            || page_rows == 0
            || page_rows > MAX_PAGE_ROWS
            || byte_limit == 0
            || byte_limit > 8 * 1024 * 1024
        {
            return Err(ProviderError::Invalid);
        }
        Ok(Self {
            operation_id,
            deadline,
            page_rows,
            byte_limit,
            cancellation,
        })
    }
    pub fn check(&self) -> Result<()> {
        if self.cancellation.is_cancelled() {
            return Err(ProviderError::Cancelled);
        }
        if Instant::now() >= self.deadline {
            return Err(ProviderError::Timeout);
        }
        Ok(())
    }
    pub fn deadline(&self) -> Instant {
        self.deadline
    }
    pub fn page_rows(&self) -> usize {
        self.page_rows
    }
    pub fn byte_limit(&self) -> usize {
        self.byte_limit
    }
    pub fn cancellation(&self) -> &Cancellation {
        &self.cancellation
    }
}
