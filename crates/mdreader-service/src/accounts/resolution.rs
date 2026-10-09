//! Explicit account-scoped pending actions use the same serial native actor.
use super::*;
impl Coordinator {
    pub fn list_pending(&self, scope: SelectedScope) -> Result<Uuid> {
        scope.validate()?;
        self.begin_action(actor::Action::ListPending(scope))
    }
    pub fn resolve_pending(
        &self,
        scope: SelectedScope,
        request: crate::sync::incoming::ResolutionRequest,
    ) -> Result<Uuid> {
        scope.validate()?;
        request.validate()?;
        self.begin_action(actor::Action::ResolvePending(scope, request))
    }
}
