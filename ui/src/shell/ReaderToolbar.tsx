/** The slim toolbar above the reading column: a breadcrumb of the document's
 * path, plus view affordances. The Read / Source / Split toggles switch the pane
 * between the rendered reader, the CodeMirror source editor, and a side-by-side
 * split (light editing — a toggleable source pane, not an editor product). */

import { ListIcon, SplitIcon, CodeIcon } from './icons.js';
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
  onArchivedReviews?: () => void;
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
  mode,
  onSetMode,
  canEdit,
  onArchivedReviews,
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

      {onArchivedReviews ? <details className="reader-import-tools"><summary>Import tools</summary>
        <button type="button" className="icon-btn" onClick={onArchivedReviews}>Archived reviews</button>
      </details> : null}

      <button
        type="button"
        className={`icon-btn${mode === 'read' ? ' icon-btn--active' : ''}`}
        aria-pressed={mode === 'read'}
        onClick={() => onSetMode('read')}
        disabled={!canEdit}
        title="Reading view"
      >
        Read
      </button>
      <button
        type="button"
        className={`icon-btn${mode === 'source' ? ' icon-btn--active' : ''}`}
        aria-pressed={mode === 'source'}
        onClick={() => onSetMode('source')}
        disabled={!canEdit}
        title="Source view (⌘E)"
        aria-label="Source view"
      >
        <CodeIcon width="15" height="15" />
      </button>
      <button
        type="button"
        className={`icon-btn${mode === 'split' ? ' icon-btn--active' : ''}`}
        aria-pressed={mode === 'split'}
        onClick={() => onSetMode('split')}
        disabled={!canEdit}
        title="Split view (source + preview)"
        aria-label="Split view"
      >
        <SplitIcon width="15" height="15" />
      </button>
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
