import type { Outcome } from '../../../bindings';
import type { SelectedScope } from '../backend';

export type PendingList = Extract<Outcome, { kind: 'pending_list' }>;
export type PendingEntry = PendingList['entries'][number];
export type PendingResolution = Extract<Outcome, { kind: 'pending_resolved' }>;
export type ResolutionChoice = PendingEntry['choices'][number];
export type Choice = { scope: SelectedScope; entry: PendingEntry; choice: ResolutionChoice };

export function pendingExplanation(reason: string, kind: string): string {
  if (reason === 'ready_to_apply') return 'This change is ready. Continue sync to apply it.';
  if (kind === 'user_state' && reason === 'local_state_changed') return 'This device and another device have different pins or reading positions.';
  if (kind === 'deletion') return 'A deletion needs review. Your local copy is kept.';
  if (kind === 'user_state') return 'A saved pin or reading position could not be applied safely.';
  switch (reason) {
    case 'head_conflict': return 'Another device has a different version of this document.';
    case 'local_state_changed': return 'This document changed on this device before the incoming copy could be applied.';
    case 'baseline_unknown': return 'The saved version could not be verified. Your local copy is kept.';
    case 'reauthorization_required': return 'Sign in again to review this change.';
    case 'target_unavailable': return 'The document is unavailable. Its incoming change is kept for later.';
    case 'position_stale': return 'The incoming reading position belongs to a different version.';
    case 'position_unsupported': return 'The incoming reading position cannot be applied safely.';
    case 'tombstoned': return 'This document has a deletion that needs review.';
    case 'not_contiguous': return 'An earlier change must finish before this one can be applied.';
    default: return 'This change needs attention before sync can continue.';
  }
}
