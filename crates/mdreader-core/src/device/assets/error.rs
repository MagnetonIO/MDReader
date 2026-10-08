//! Fixed domain diagnostics: never expose SQL, filesystem paths or decoder errors.
#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum Error {
    #[error("The selected asset or document revision is unavailable.")]
    Unavailable,
    #[error("Device asset metadata could not be updated.")]
    Metadata,
    #[error("Unsupported device asset schema.")]
    UnsupportedSchema,
    #[error("Unsupported device document schema.")]
    UnsupportedDocumentSchema,
    #[error("Invalid relative asset target.")]
    InvalidTarget,
    #[error("Two image reads are already active.")]
    ReadsBusy,
    #[error("An image is being validated. Try again when it completes.")]
    ValidationBusy,
    #[error("Select a valid PNG or JPEG image.")]
    InvalidImage,
    #[error("Only PNG and JPEG images are supported.")]
    UnsupportedImage,
    #[error("Animated PNG assets are unsupported.")]
    AnimatedPng,
    #[error("Image exceeds 4096 pixels per side or 4 million pixels.")]
    ImageBounds,
    #[error("Image could not be fully decoded within the asset limits.")]
    DecodeFailed,
    #[error("Remove the published asset before discarding its bytes.")]
    PublishedAsset,
    #[error("Device asset inventory exceeds its supported bound.")]
    InventoryBounds,
    #[error("Device asset inventory exceeds its supported quota.")]
    InventoryQuota,
    #[error("Asset selection or manifest changed.")]
    SelectionChanged,
    #[error("Asset quota reached or a selection for this target is already pending. Remove or discard unused assets first.")]
    Quota,
    #[error("Asset chunk exceeds 64 KiB.")]
    ChunkBounds,
    #[error("Invalid asset chunk.")]
    InvalidChunk,
    #[error("Invalid asset chunk offset.")]
    InvalidOffset,
    #[error("Asset chunk replay conflicts.")]
    ReplayConflict,
    #[error("Asset chunk recovery requires the original offset.")]
    RecoveryOffset,
    #[error("Asset upload is incomplete.")]
    IncompleteUpload,
    #[error("Asset manifest changed. Discard this pending selection and try again.")]
    ManifestChanged,
    #[error("Asset directory privacy could not be verified.")]
    Privacy,
}
impl From<rusqlite::Error> for Error {
    fn from(_: rusqlite::Error) -> Self {
        Self::Metadata
    }
}
