//! Native-only control packets. These are not renderer commands or backup proof.
use mdreader_core::SourceRootIdentity;
use serde::{Deserialize, Serialize};
use std::{fmt, path::PathBuf};
use uuid::Uuid;

pub(crate) const MAX_DURATION_MS: u32 = 30_000;
pub(crate) const MAX_CONTROL_BYTES: u64 = 8192;
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Error {
    Invalid,
    Bounds,
    Busy,
    Unsupported,
    Unavailable,
    StaleOwner,
    UnknownOperation,
    NotHeld,
    Deadline,
    Quarantined,
    Uncertain,
}
pub type Result<T> = std::result::Result<T, Error>;
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SelectedRoot {
    pub directory: PathBuf,
    pub identity: SourceRootIdentity,
}
impl fmt::Debug for SelectedRoot {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SelectedRoot(<private>)")
    }
}
impl SelectedRoot {
    pub(crate) fn validate(&self) -> Result<()> {
        if !self.directory.is_absolute()
            || self.directory.as_os_str().len() > 4096
            || self.directory.components().count() > 64
        {
            return Err(Error::Invalid);
        }
        Ok(())
    }
}
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Scope {
    pub operation: Uuid,
    pub owner_epoch: Uuid,
    pub boot: Uuid,
    pub root: SelectedRoot,
}
impl Scope {
    pub(crate) fn validate(&self) -> Result<()> {
        self.root.validate()?;
        if [self.operation, self.owner_epoch, self.boot]
            .iter()
            .any(|id| id.get_version_num() != 4)
        {
            return Err(Error::Invalid);
        }
        Ok(())
    }
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
pub enum ControlRequest {
    Inspect {
        root: SelectedRoot,
    },
    Begin {
        scope: Scope,
        duration_ms: u32,
    },
    Reconcile {
        scope: Scope,
    },
    Revalidate {
        scope: Scope,
        lease: Uuid,
    },
    Release {
        scope: Scope,
        lease: Uuid,
    },
    /// Native offline handoff only. Requires the exact held/drained capability;
    /// a reply acknowledges intent, never certifies that the owner has closed.
    Shutdown {
        scope: Scope,
        lease: Uuid,
    },
    Backup {
        scope: Scope,
        lease: Uuid,
        request: Box<mdreader_profile::database_backup::BackupRequest>,
    },
    ReconcileBackup {
        scope: Scope,
        lease: Uuid,
        request: Box<mdreader_profile::database_backup::BackupRequest>,
    },
}
impl fmt::Debug for ControlRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("PreservationControl(<private>)")
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    Acquiring,
    BackingUp,
    Held,
    Released,
    Failed,
    Quarantined,
}
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Status {
    pub phase: Phase,
    pub remaining_ms: u32,
    pub lease: Option<Uuid>,
    pub failure: Option<Error>,
}
impl fmt::Debug for Status {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PreservationStatus")
            .field("phase", &self.phase)
            .field("remaining_ms", &self.remaining_ms)
            .field("failure", &self.failure)
            .finish_non_exhaustive()
    }
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
pub enum ControlReply {
    Backup { status: BackupStatus },
    Hello { boot: Uuid },
    ShutdownRequested { boot: Uuid },
    Status { status: Status },
    Failure { error: Error },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
pub enum BackupStatus {
    Running,
    Completed {
        receipt: Box<mdreader_profile::database_backup::BackupReceipt>,
    },
    Failed {
        error: Error,
    },
}
