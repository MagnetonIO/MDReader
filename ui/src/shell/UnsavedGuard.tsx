/**
 * The unsaved-changes guard — a small modal shown before an action that would
 * discard an unsaved buffer (closing a dirty tab, or quitting with dirty tabs).
 * Three deliberate choices: Save (write, then proceed), Discard (proceed, losing
 * edits), Cancel (stay). Nothing destructive happens without the user picking it,
 * so an in-progress edit is never lost to a stray ⌘W or quit.
 */

import { useEffect } from 'react';

export interface UnsavedGuardProps {
  /** How many documents have unsaved changes (1 for a tab close, N for quit). */
  count: number;
  /** The single document's name when `count === 1` (a more specific message). */
  name?: string;
  onSave: () => void;
  onDiscard: () => void;
  onCancel: () => void;
}

export function UnsavedGuard({ count, name, onSave, onDiscard, onCancel }: UnsavedGuardProps) {
  // Escape cancels (never discards) — the safe default.
  useEffect(() => {
    function onKey(event: KeyboardEvent) {
      if (event.key === 'Escape') {
        event.preventDefault();
        event.stopPropagation();
        onCancel();
      }
    }
    window.addEventListener('keydown', onKey, true);
    return () => window.removeEventListener('keydown', onKey, true);
  }, [onCancel]);

  const message =
    count > 1
      ? `${count} documents have unsaved changes.`
      : name
        ? `“${name}” has unsaved changes.`
        : 'This document has unsaved changes.';

  return (
    <div className="modal-scrim" onClick={onCancel}>
      <div
        className="unsaved-guard"
        role="alertdialog"
        aria-modal="true"
        aria-label="Unsaved changes"
        onClick={(event) => event.stopPropagation()}
      >
        <h2 className="unsaved-guard__title">Unsaved changes</h2>
        <p className="unsaved-guard__msg">{message}</p>
        <div className="unsaved-guard__actions">
          <button type="button" className="unsaved-guard__btn" onClick={onCancel}>
            Cancel
          </button>
          <button
            type="button"
            className="unsaved-guard__btn unsaved-guard__btn--danger"
            onClick={onDiscard}
          >
            Discard
          </button>
          <button
            type="button"
            className="unsaved-guard__btn unsaved-guard__btn--primary"
            onClick={onSave}
          >
            Save
          </button>
        </div>
      </div>
    </div>
  );
}
