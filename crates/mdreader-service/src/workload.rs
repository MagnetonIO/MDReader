//! Persisted admission policy, not an OS CPU quota. Foreground queries bypass
//! background cooldown; pausing cannot cancel an already running ONNX call.
use mdreader_core::{ReaderStore, Result};
use serde::{Deserialize, Serialize};
use std::time::Instant;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EmbeddingPolicy {
    #[default]
    Balanced,
    Paused,
    FullSpeed,
}
impl EmbeddingPolicy {
    fn value(self) -> &'static str {
        match self {
            Self::Balanced => "balanced",
            Self::Paused => "paused",
            Self::FullSpeed => "full_speed",
        }
    }
    pub(crate) fn batch_limit(self) -> usize {
        if self == Self::Balanced {
            8
        } else {
            32
        }
    }
}
pub fn initialize(store: &mut ReaderStore) -> Result<()> {
    store.with_library_connection(|db|db.execute_batch("CREATE TABLE IF NOT EXISTS reader_workload_v1(id INTEGER PRIMARY KEY CHECK(id=1), embedding_policy TEXT NOT NULL CHECK(embedding_policy IN('balanced','paused','full_speed'))); INSERT OR IGNORE INTO reader_workload_v1 VALUES(1,'balanced');"))?;
    Ok(())
}
pub fn get(store: &mut ReaderStore) -> Result<EmbeddingPolicy> {
    let value: String = store.with_library_connection(|db| {
        db.query_row(
            "SELECT embedding_policy FROM reader_workload_v1 WHERE id=1",
            [],
            |r| r.get(0),
        )
    })?;
    match value.as_str() {
        "balanced" => Ok(EmbeddingPolicy::Balanced),
        "paused" => Ok(EmbeddingPolicy::Paused),
        "full_speed" => Ok(EmbeddingPolicy::FullSpeed),
        _ => Err(mdreader_core::ReaderError::Invalid(
            "Invalid embedding policy".into(),
        )),
    }
}
pub fn set(store: &mut ReaderStore, policy: EmbeddingPolicy) -> Result<()> {
    initialize(store)?;
    store.with_library_connection(|db| {
        db.execute(
            "UPDATE reader_workload_v1 SET embedding_policy=?1 WHERE id=1",
            [policy.value()],
        )
    })?;
    Ok(())
}
pub(crate) struct Budget {
    pub policy: EmbeddingPolicy,
    ready_at: Instant,
}
impl Default for Budget {
    fn default() -> Self {
        Self {
            policy: EmbeddingPolicy::Balanced,
            ready_at: Instant::now(),
        }
    }
}
impl Budget {
    pub fn configure(&mut self, policy: EmbeddingPolicy) {
        self.policy = policy;
        if policy == EmbeddingPolicy::FullSpeed {
            self.ready_at = Instant::now();
        }
    }
    pub fn admits(&self, now: Instant) -> bool {
        match self.policy {
            EmbeddingPolicy::Paused => false,
            EmbeddingPolicy::FullSpeed => true,
            EmbeddingPolicy::Balanced => now >= self.ready_at,
        }
    }
    pub fn wait(&self, now: Instant) -> Option<std::time::Duration> {
        match self.policy {
            EmbeddingPolicy::Paused => Some(std::time::Duration::from_secs(1)),
            EmbeddingPolicy::Balanced if now < self.ready_at => {
                Some((self.ready_at - now).min(std::time::Duration::from_secs(1)))
            }
            _ => None,
        }
    }
    pub fn completed(&mut self, started: Instant, completed: Instant) {
        // 1 part inference + 3 parts cooldown targets 25% inference wall duty.
        // Actual CPU depends on the runtime/host; short spikes remain possible.
        self.ready_at = completed
            + completed
                .saturating_duration_since(started)
                .saturating_mul(3);
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    #[test]
    fn balanced_admission_pause_and_persisted_explicit_override() {
        let start = Instant::now();
        let mut budget = Budget::default();
        budget.completed(start, start + Duration::from_millis(20));
        assert!(!budget.admits(start + Duration::from_millis(79)));
        assert!(budget.admits(start + Duration::from_millis(80)));
        budget.configure(EmbeddingPolicy::Paused);
        assert!(!budget.admits(start + Duration::from_secs(60)));
        assert_eq!(budget.wait(start), Some(Duration::from_secs(1)));
        budget.configure(EmbeddingPolicy::FullSpeed);
        assert!(budget.admits(start));
        let data = tempfile::tempdir().unwrap();
        let mut store = ReaderStore::open(data.path()).unwrap();
        initialize(&mut store).unwrap();
        assert_eq!(get(&mut store).unwrap(), EmbeddingPolicy::Balanced);
        set(&mut store, EmbeddingPolicy::Paused).unwrap();
        drop(store);
        let mut store = ReaderStore::open(data.path()).unwrap();
        initialize(&mut store).unwrap();
        assert_eq!(get(&mut store).unwrap(), EmbeddingPolicy::Paused);
    }
}
