use super::*;
use mdreader_legacy_export::binding::{
    CurrentDocument, CurrentDocumentReader, CurrentDocumentReference, CurrentObservation,
    CurrentReadError,
};
use std::cell::RefCell;

/// Uses only the selected live owner. Neither the archived locator nor a cache
/// path participates in a read. Provider reads remain exact admitted snapshots.
pub(super) struct SelectedReader<'a, F: FnOnce()> {
    pub coordinator: &'a Coordinator,
    pub selection: &'a Selection,
    pub gate: RefCell<Option<F>>,
    pub markdown: RefCell<Option<String>>,
}
fn core_error(error: mdreader_core::ReaderError) -> CurrentReadError {
    use mdreader_core::ReaderError;
    match error {
        ReaderError::Conflict => CurrentReadError::Conflict,
        ReaderError::Invalid(_) => CurrentReadError::Bounds,
        _ => CurrentReadError::Unavailable,
    }
}
impl<F: FnOnce()> CurrentDocumentReader for SelectedReader<'_, F> {
    fn owner_scope(&self) -> Uuid {
        self.coordinator.scope
    }
    fn read_exact(
        &self,
        reference: &CurrentDocumentReference,
    ) -> std::result::Result<CurrentDocument, CurrentReadError> {
        if reference.owner_scope() != self.coordinator.scope {
            return Err(CurrentReadError::Conflict);
        }
        self.coordinator
            .check_selection(self.selection)
            .map_err(|_| CurrentReadError::Conflict)?;
        let document = self
            .coordinator
            .owner
            .lock()
            .map_err(|_| CurrentReadError::Unavailable)?
            .read_document(
                reference.source_id(),
                reference.path(),
                Some(reference.revision()),
            )
            .map_err(core_error)?;
        self.coordinator
            .check_selection(self.selection)
            .map_err(|_| CurrentReadError::Conflict)?;
        let observation = if reference.source_id().starts_with("provider:") {
            CurrentObservation::ProviderCachedRevision
        } else {
            CurrentObservation::LocalCurrentRead
        };
        let current =
            CurrentDocument::new(reference.clone(), document.markdown.clone(), observation)?;
        *self.markdown.borrow_mut() = Some(document.markdown);
        Ok(current)
    }
    fn revalidate_exact(
        &self,
        reference: &CurrentDocumentReference,
    ) -> std::result::Result<(), CurrentReadError> {
        if let Some(gate) = self.gate.borrow_mut().take() {
            gate();
        }
        self.read_exact(reference).map(|_| ())
    }
}
pub(super) fn binding_error(error: mdreader_legacy_export::binding::BindingError) -> ReviewError {
    use mdreader_legacy_export::binding::{BindingError, CurrentReadError};
    match error {
        BindingError::Current(CurrentReadError::Conflict)
        | BindingError::OwnerMismatch
        | BindingError::ReferenceMismatch
        | BindingError::ArchiveMismatch => ReviewError::Conflict,
        BindingError::Current(CurrentReadError::Unavailable) | BindingError::ArchiveUnavailable => {
            ReviewError::Unavailable
        }
        BindingError::Current(CurrentReadError::Bounds) => ReviewError::TooLarge,
        BindingError::InvalidSelection => ReviewError::Invalid,
        _ => ReviewError::Unsupported,
    }
}
