/**
 * The non-blocking conflict banner — the visible face of "never silently
 * rewrite files". It appears (not a modal; the user can keep reading/editing
 * behind it) when either entry path fires:
 *   - the watcher reports the open document changed on disk while it was dirty
 *     (`source: 'external'`), or
 *   - a save hit the on-disk hash guard (`source: 'save'`).
 *
 * Two phases. `choose` offers the safe options — Reload (adopt disk, discard the
 * local edits) or Keep mine (hold the buffer). Keep-mine does NOT overwrite; it
 * advances to `confirm`, the EXPLICIT second confirmation, where Overwrite
 * force-writes and Cancel backs out. So an on-disk change is never clobbered
 * without a deliberate, separate confirmation.
 */

import type { ConflictSource } from '../stores/editor.js';

export interface ConflictBannerProps {
  source: ConflictSource;
  phase: 'choose' | 'confirm';
  onReload: () => void;
  onKeepMine: () => void;
  onOverwrite: () => void;
  onCancel: () => void;
}

export function ConflictBanner({
  source,
  phase,
  onReload,
  onKeepMine,
  onOverwrite,
  onCancel,
}: ConflictBannerProps) {
  if (phase === 'confirm') {
    return (
      <div className="conflict-banner conflict-banner--confirm" role="alert">
        <span className="conflict-banner__msg">
          Overwrite the on-disk changes? This replaces the version on disk with your edits.
        </span>
        <div className="conflict-banner__actions">
          <button type="button" className="conflict-banner__btn conflict-banner__btn--danger" onClick={onOverwrite}>
            Overwrite
          </button>
          <button type="button" className="conflict-banner__btn" onClick={onCancel}>
            Cancel
          </button>
        </div>
      </div>
    );
  }

  const message =
    source === 'save'
      ? 'File changed on disk since you opened it — your save was held.'
      : 'This file changed on disk while you were editing.';

  return (
    <div className="conflict-banner" role="alert">
      <span className="conflict-banner__msg">{message}</span>
      <div className="conflict-banner__actions">
        <button type="button" className="conflict-banner__btn" onClick={onReload}>
          Reload
        </button>
        <button type="button" className="conflict-banner__btn conflict-banner__btn--primary" onClick={onKeepMine}>
          Keep mine
        </button>
      </div>
    </div>
  );
}
