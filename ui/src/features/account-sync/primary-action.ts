import type { AccountAction, AccountStatus } from './backend.js';

/** Explicit actions only. Pending custody never falls through to ordinary resume. */
export function primaryAccountAction(status: AccountStatus): { label: string; action: AccountAction } | null {
  const custody = status.custody;
  const library = status.activeLibraryId;
  if (custody?.state === 'pair_candidate') {
    if (custody.materialStaged) return { label: 'Check connection', action: { action: 'reconcile_enrollment' } };
    if (!library || custody.enrollmentExpired) return null;
    return custody.enrollmentUncertain
      ? { label: 'Show device request', action: { action: 'show_pair_request' } }
      : { label: 'Continue connection', action: { action: 'join_device', remoteLibrary: library } };
  }
  if (custody?.state === 'recovery_candidate') {
    if (!library) return null;
    if (custody.materialStaged && custody.enrollmentExpired) return { label: 'Retry recovery', action: { action: 'renew_enrollment' } };
    if (custody.materialStaged && custody.enrollmentUncertain) return { label: 'Check recovery', action: { action: 'reconcile_enrollment' } };
    return status.recoveryAvailable ? { label: 'Continue recovery', action: { action: 'recover', remoteLibrary: library } } : null;
  }
  if (status.existingLibraryId) return status.existingLibrarySameSelection
    ? { label: 'Check setup', action: { action: 'reconcile_enrollment' } }
    : { label: 'Connect this device', action: { action: 'join_device', remoteLibrary: status.existingLibraryId } };
  if (custody?.state === 'bootstrap_pending') {
    return { label: 'Retry setup', action: { action: 'renew_enrollment' } };
  }
  return { label: 'Sign in / reconnect', action: { action: 'sign_in' } };
}
