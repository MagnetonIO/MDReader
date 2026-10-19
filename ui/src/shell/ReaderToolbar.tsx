/** Saved document location, Edit entry, and outline affordance. */

import { ListIcon, CodeIcon } from './icons.js';
import type { DocRef } from '../stores/types.js';
import type { EditorMode } from '../stores/editor.js';

export interface ReaderToolbarProps {
  docRef: DocRef | null;
  root: string | null;
  outlineVisible: boolean;
  onToggleOutline: () => void;
  hasOutline: boolean;
  /** The active tab's current view mode. */
  mode: EditorMode;
  /** Switch view mode (seeds the editor buffer on first source/split entry). */
  onSetMode: (mode: EditorMode) => void;
  /** Whether a document is open (the toggles are inert otherwise). */
  canEdit: boolean;
  /** The active source forbids writes (e.g. Claude plans): no Edit entry. */
  readOnly?: boolean;
}

function crumbs(path: string, root: string | null): string[] {
  let rel = path;
  if (root) {
    const normalized = root.replace(/[\\/]+$/, '');
    if (rel.startsWith(normalized)) rel = rel.slice(normalized.length);
  }
  return rel.split(/[\\/]+/).filter((segment) => segment.length > 0);
}

export function ReaderToolbar({
  docRef,
  root,
  outlineVisible,
  onToggleOutline,
  hasOutline,
  onSetMode,
  canEdit,
  readOnly = false,
}: ReaderToolbarProps) {
  const parts = docRef ? crumbs(docRef.path, root) : [];
  const leaf = parts.length > 0 ? parts[parts.length - 1] : '';
  const trail = parts.slice(0, -1);

  return (
    <div className="reader-toolbar">
      <nav className="breadcrumb" aria-label="Document location">
        {trail.map((crumb, index) => (
          <span key={`${crumb}-${index}`} style={{ display: 'contents' }}>
            <span className="breadcrumb__crumb">{crumb}</span>
            <span className="breadcrumb__sep">/</span>
          </span>
        ))}
        {leaf ? <span className="breadcrumb__leaf">{leaf}</span> : <span>No document open</span>}
      </nav>

      {readOnly ? null : <button type="button" className="icon-btn" onClick={() => onSetMode('source')}
        disabled={!canEdit} title="Edit Markdown (⌘E)" aria-label="Edit">
        <CodeIcon width="15" height="15" /> Edit
      </button>}
      <button
        type="button"
        className={`icon-btn${outlineVisible ? ' icon-btn--active' : ''}`}
        aria-pressed={outlineVisible}
        onClick={onToggleOutline}
        disabled={!hasOutline}
        title="Toggle outline"
        aria-label="Toggle outline"
      >
        <ListIcon width="15" height="15" />
      </button>
    </div>
  );
}
