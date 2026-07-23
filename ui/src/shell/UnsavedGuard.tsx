/**
 * The unsaved-changes guard — a small modal shown before an action that would
 * discard an unsaved buffer (closing a dirty tab, or quitting with dirty tabs).
 * Three deliberate choices: Save (write, then proceed), Discard (proceed, losing
 * edits), Cancel (stay). Nothing destructive happens without the user picking it,
 * so an in-progress edit is never lost to a stray ⌘W or quit.
 */

import { useEffect } from 'react';

export interface UnsavedGuardProps {
  /** The FULL list of document paths with unsaved changes in the affected scope —
   *  a tab can hold several dirty docs (some not currently displayed), so Discard
   *  must show EVERY doc it would lose, not just the one on screen. */
  docs: string[];
  onSave: () => void;
  onDiscard: () => void;
  onCancel: () => void;
}

/** The trailing path segment, for a readable label (full path shown on hover). */
function basename(path: string): string {
  const parts = path.replace(/[\\/]+$/, '').split(/[\\/]+/);
  return parts[parts.length - 1] || path;
}

export function UnsavedGuard({ docs, onSave, onDiscard, onCancel }: UnsavedGuardProps) {
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
    docs.length === 1
      ? `“${basename(docs[0]!)}” has unsaved changes.`
      : `${docs.length} documents have unsaved changes:`;

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
        {docs.length > 1 ? (
          <ul className="unsaved-guard__docs">
            {docs.map((path) => (
              <li key={path} className="unsaved-guard__doc" title={path}>
                {basename(path)}
              </li>
            ))}
          </ul>
        ) : null}
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
