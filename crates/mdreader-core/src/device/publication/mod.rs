//! Explicit incoming reservations on the existing device owner.
//! Reserved incoming bytes never participate in ordinary local-import recovery.
//! This namespace is available without image decoding so local quotas remain shared.
//! Older clients unaware of this namespace cannot enforce its reservations: do not
//! downgrade an active incoming profile without resolving/exporting its intents.
//! Immutable replacements and retirement preserve archived originals; caller owns
//! remote-head conflicts, authentication and receipt/cursor transactions.
use super::{Error, Result};
use rusqlite::{OptionalExtension, Transaction};

mod schema;
pub(crate) use schema::reserved_usage;

#[cfg(feature = "device-assets")]
mod types;
#[cfg(feature = "device-assets")]
pub use types::*;
#[cfg(feature = "device-assets")]
mod input;
#[cfg(feature = "device-assets")]
mod reservation;
#[cfg(feature = "device-assets")]
pub use reservation::{reserve_new, resume};
#[cfg(feature = "device-assets")]
mod files;
#[cfg(feature = "device-assets")]
mod publish;
#[cfg(feature = "device-assets")]
pub use publish::{publish_new, revalidate_publication, with_publication, PublicationGuard};
#[cfg(feature = "device-assets")]
mod current;
#[cfg(feature = "device-assets")]
pub use current::{
    prepare_current, revalidate_current, with_transition, CurrentGuard, CurrentPlan,
    VerifiedCurrent,
};
#[cfg(feature = "device-assets")]
mod state_change;
#[cfg(feature = "device-assets")]
pub use state_change::{
    publish_state, revalidate_state, PositionUpdate, StateReceipt, StateUpdate,
};
#[cfg(feature = "device-assets")]
mod retire;
#[cfg(feature = "device-assets")]
pub use retire::{retire_current, revalidate_retirement, RetirementReceipt};
#[cfg(feature = "device-assets")]
mod replacement;
#[cfg(feature = "device-assets")]
pub use replacement::{
    publish_replacement, revalidate_replacement, with_replacement, ReplacementGuard,
    ReplacementReceipt,
};
#[cfg(feature = "device-assets")]
mod archive;
#[cfg(feature = "device-assets")]
pub use archive::{prepare_archive, ArchivePlan, ArchivedAsset, ArchivedDocument};
#[cfg(feature = "device-assets")]
mod archive_fence;
#[cfg(feature = "device-assets")]
pub use archive_fence::revalidate_current_archive;
#[cfg(feature = "device-assets")]
mod drop_intent;
#[cfg(feature = "device-assets")]
pub use drop_intent::{finish_drop, prepare_drop, DropPlan, RemovedIncoming};
#[cfg(all(test, feature = "device-assets", unix))]
mod tests;

#[cfg(feature = "device-assets")]
mod preserved_copy;
#[cfg(feature = "device-assets")]
pub use preserved_copy::{publish_preserved_copy, revalidate_preserved_copy};
