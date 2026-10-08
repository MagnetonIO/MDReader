//! Bounded identities for one already-created private snapshot member.
use crate::{Error, RelativeLocator, Result, RootBinding};
use serde::{Deserialize, Serialize};
use std::path::Path;
use uuid::Uuid;
pub type FileStamp = (u64, u64, u64, i64, i64, i64, i64);
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BackupSource {
    pub root: RootBinding,
    pub relative: RelativeLocator,
    pub stamp: FileStamp,
}
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BackupDestination {
    pub root: RootBinding,
    pub member: String,
    pub stamp: FileStamp,
}
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BackupRequest {
    pub id: Uuid,
    pub source: BackupSource,
    pub destination: BackupDestination,
    pub max_bytes: u64,
    pub max_steps: usize,
    pub duration_ms: u32,
}
impl BackupRequest {
    pub fn validate(&self) -> Result<()> {
        self.source.root.validate()?;
        self.destination.root.validate()?;
        let source = self.source.relative.restore()?;
        let origin = self.source.root.locator.restore()?;
        let target = self.destination.root.locator.restore()?;
        let name = &self.destination.member;
        if self.id.get_version_num() != 4
            || source.components().count() > 64
            || !member(name)
            || self.destination.stamp.2 != 0
            || self.destination.stamp.1 == 0
            || self.source.stamp.1 == 0
            || target.starts_with(&origin)
            || origin.starts_with(&target)
            || self.max_bytes == 0
            || self.max_bytes > 16 * 1024 * 1024 * 1024
            || self.max_steps == 0
            || self.max_steps > 1_000_000
            || self.duration_ms == 0
            || self.duration_ms > 30_000
        {
            return Err(Error::Invalid);
        }
        Ok(())
    }
}
fn member(name: &str) -> bool {
    name.len() == 14
        && name.ends_with(".snapshot")
        && name.as_bytes()[..5].iter().all(u8::is_ascii_digit)
        && Path::new(name).components().count() == 1
}
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BackupReceipt {
    pub request: BackupRequest,
    pub target_stamp: FileStamp,
    pub bytes: u64,
    pub sha256: String,
}
impl std::fmt::Debug for BackupReceipt {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("BackupReceipt(<private>)")
    }
}
impl BackupReceipt {
    pub fn validate(&self) -> Result<()> {
        self.request.validate()?;
        if self.bytes == 0
            || self.bytes > self.request.max_bytes
            || self.target_stamp.2 != self.bytes
            || (self.target_stamp.0, self.target_stamp.1)
                != (
                    self.request.destination.stamp.0,
                    self.request.destination.stamp.1,
                )
            || !crate::model::valid_hash(&self.sha256)
        {
            return Err(Error::Integrity);
        }
        Ok(())
    }
}
