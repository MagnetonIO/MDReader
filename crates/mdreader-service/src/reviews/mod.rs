//! Explicit read-only archival reviews. No installed-profile discovery,
//! source grants, inferred legacy paths, review transitions or remote fetches.
mod model;
mod reader;
mod schema;
use mdreader_core::{ReaderStore, SourceRoot};
use mdreader_legacy_export::{
    binding::{
        verify_binding, CurrentDocumentReference, CurrentHashComparison, CurrentObservation,
        SelectedBinding,
    },
    reviews::{ArchiveIdentity, ReviewArchive},
    Limits,
};
pub use model::*;
use std::{
    path::Path,
    sync::{Arc, Mutex},
};
use uuid::Uuid;

const MAX_REPLY: usize = 4 * 1024 * 1024;
#[derive(Clone)]
struct Selection {
    handle: String,
    identity: ArchiveIdentity,
    root: SourceRoot,
}
/// One volatile selection per service lifetime. Restart requires explicit
/// reselection; durable profile identity prevents cross-owner comparisons.
pub struct Coordinator {
    owner: Arc<Mutex<ReaderStore>>,
    profile: SourceRoot,
    scope: Uuid,
    selected: Mutex<Option<Selection>>,
    active: Mutex<()>,
}
impl Coordinator {
    pub fn new(owner: Arc<Mutex<ReaderStore>>, directory: &Path) -> Result<Self> {
        if !cfg!(any(unix, windows)) {
            return Err(ReviewError::Unsupported);
        }
        let profile =
            SourceRoot::capture(directory.to_path_buf()).map_err(|_| ReviewError::Unavailable)?;
        let scope = owner
            .lock()
            .map_err(|_| ReviewError::Storage)?
            .with_library_connection(schema::initialize)?;
        profile.verify().map_err(|_| ReviewError::Conflict)?;
        Ok(Self {
            owner,
            profile,
            scope,
            selected: Mutex::new(None),
            active: Mutex::new(()),
        })
    }
    fn check_profile(&self) -> Result<()> {
        self.profile.verify().map_err(|_| ReviewError::Conflict)?;
        let current = self
            .owner
            .lock()
            .map_err(|_| ReviewError::Storage)?
            .with_library_connection(|connection| schema::read(connection))?;
        if current != self.scope {
            return Err(ReviewError::Conflict);
        }
        self.profile.verify().map_err(|_| ReviewError::Conflict)
    }
    fn selection(&self, handle: &str) -> Result<Selection> {
        if Uuid::parse_str(handle).is_err() {
            return Err(ReviewError::Invalid);
        }
        let selected = self.selected.lock().map_err(|_| ReviewError::Storage)?;
        selected
            .as_ref()
            .filter(|selection| selection.handle == handle)
            .cloned()
            .ok_or(ReviewError::Unavailable)
    }
    fn check_selection(&self, selection: &Selection) -> Result<()> {
        self.check_profile()?;
        selection.root.verify().map_err(|_| ReviewError::Conflict)?;
        if self.selection(&selection.handle)?.root != selection.root {
            return Err(ReviewError::Conflict);
        }
        Ok(())
    }
    pub fn execute(&self, request: ReviewRequest) -> Result<ReviewReply> {
        self.execute_with_gate(request, || {})
    }
    fn execute_with_gate(
        &self,
        request: ReviewRequest,
        gate: impl FnOnce(),
    ) -> Result<ReviewReply> {
        let _active = self.active.try_lock().map_err(|error| match error {
            std::sync::TryLockError::WouldBlock => ReviewError::Busy,
            std::sync::TryLockError::Poisoned(_) => ReviewError::Storage,
        })?;
        self.check_profile()?;
        let reply = match request {
            ReviewRequest::Select { stage, identity } => {
                if identity.export_id.get_version_num() != 4
                    || identity.manifest_sha256.len() != 64
                    || !identity
                        .manifest_sha256
                        .bytes()
                        .all(|b| b.is_ascii_hexdigit())
                {
                    return Err(ReviewError::Invalid);
                }
                let root = SourceRoot::capture(stage).map_err(|_| ReviewError::Unavailable)?;
                // Full archive hashing happens outside the owner lock.
                let archive = ReviewArchive::open(&root.path, &identity, &Limits::default())
                    .map_err(model::archive_error)?;
                archive.list(0, 1).map_err(model::archive_error)?;
                self.check_profile()?;
                root.verify().map_err(|_| ReviewError::Conflict)?;
                let selection = Selection {
                    handle: Uuid::new_v4().to_string(),
                    identity: identity.clone(),
                    root,
                };
                let handle = selection.handle.clone();
                *self.selected.lock().map_err(|_| ReviewError::Storage)? = Some(selection);
                ReviewReply::Selected {
                    handle,
                    identity,
                    owner_scope: self.scope.to_string(),
                }
            }
            ReviewRequest::Close { handle } => {
                self.selection(&handle)?;
                let mut selected = self.selected.lock().map_err(|_| ReviewError::Storage)?;
                if selected.as_ref().is_none_or(|value| value.handle != handle) {
                    return Err(ReviewError::Conflict);
                }
                *selected = None;
                ReviewReply::Closed
            }
            request => {
                let handle = match &request {
                    ReviewRequest::List { handle, .. }
                    | ReviewRequest::Get { handle, .. }
                    | ReviewRequest::Compare { handle, .. } => handle,
                    _ => return Err(ReviewError::Invalid),
                };
                let selection = self.selection(handle)?;
                self.check_selection(&selection)?;
                let archive = ReviewArchive::open(
                    &selection.root.path,
                    &selection.identity,
                    &Limits::default(),
                )
                .map_err(model::archive_error)?;
                let result = match request {
                    ReviewRequest::List { offset, limit, .. } => {
                        if !(1..=100).contains(&limit) {
                            return Err(ReviewError::Invalid);
                        }
                        let page = archive
                            .list(offset, limit as usize)
                            .map_err(model::archive_error)?;
                        ReviewReply::Listed {
                            page: ReviewPage {
                                records: page
                                    .records
                                    .into_iter()
                                    .map(ReviewRecord::try_from)
                                    .collect::<Result<_>>()?,
                                next_offset: page.next_offset,
                            },
                        }
                    }
                    ReviewRequest::Get { record_id, .. } => ReviewReply::Record {
                        record: archive
                            .get(&record_id)
                            .map_err(model::archive_error)?
                            .ok_or(ReviewError::Unavailable)?
                            .try_into()?,
                    },
                    ReviewRequest::Compare {
                        record_id,
                        source_id,
                        path,
                        revision,
                        ..
                    } => {
                        let current = CurrentDocumentReference::new(
                            self.scope,
                            source_id.clone(),
                            path.clone(),
                            revision.clone(),
                        )
                        .map_err(reader::binding_error)?;
                        let binding =
                            SelectedBinding::new(selection.identity.clone(), record_id, current)
                                .map_err(reader::binding_error)?;
                        let selected = reader::SelectedReader {
                            coordinator: self,
                            selection: &selection,
                            gate: std::cell::RefCell::new(Some(gate)),
                            markdown: std::cell::RefCell::new(None),
                        };
                        let verified = verify_binding(&archive, &binding, &selected)
                            .map_err(reader::binding_error)?;
                        let md = selected.markdown.borrow();
                        let md = md.as_deref().ok_or(ReviewError::Unavailable)?;
                        let mdreader_legacy_export::reviews::Assertion::DocumentSpan(span) =
                            &verified.archival_review.assertion
                        else {
                            return Err(ReviewError::Unsupported);
                        };
                        let mut passage = String::new();
                        for (index, line) in md
                            .lines()
                            .skip((span.start_line - 1) as usize)
                            .take((span.end_line - span.start_line + 1) as usize)
                            .enumerate()
                        {
                            if index > 0 {
                                passage.push('\n');
                            }
                            passage.push_str(line);
                        }
                        let comparison = |value| {
                            match value {
                                CurrentHashComparison::Matches => "matches",
                                CurrentHashComparison::Differs => "differs",
                                CurrentHashComparison::Unknown => "unknown",
                            }
                            .to_string()
                        };
                        ReviewReply::Compared {
                            comparison: ReviewComparison {
                                review: verified.archival_review.try_into()?,
                                current: CurrentReference {
                                    source_id,
                                    path,
                                    revision,
                                },
                                observation: match verified.observation {
                                    CurrentObservation::LocalCurrentRead => "local_current_read",
                                    CurrentObservation::ProviderCachedRevision => {
                                        "provider_cached_revision"
                                    }
                                }
                                .into(),
                                current_span_hash: verified.current_span_hash,
                                current_passage: passage,
                                recorded_current: comparison(verified.recorded_current),
                                reviewed_current: comparison(verified.reviewed_current),
                            },
                        }
                    }
                    _ => return Err(ReviewError::Invalid),
                };
                self.check_selection(&selection)?;
                result
            }
        };
        self.check_profile()?;
        if serde_json::to_vec(&reply)
            .map_err(|_| ReviewError::Storage)?
            .len()
            > MAX_REPLY
        {
            return Err(ReviewError::TooLarge);
        }
        Ok(reply)
    }
}
#[cfg(all(test, unix))]
mod tests;
