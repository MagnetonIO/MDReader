/** The slim toolbar above the reading column: a breadcrumb of the document's
 * path, plus view affordances. Source/split toggles are present but disabled —
 * honest affordances for capabilities that land in later phases. */

import { ListIcon, SplitIcon } from './icons.js';
import type { DocRef } from '../stores/types.js';

export interface ReaderToolbarProps {
  docRef: DocRef | null;
  root: string | null;
  outlineVisible: boolean;
  onToggleOutline: () => void;
  hasOutline: boolean;
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

      <button type="button" className="icon-btn" disabled title="Reading mode (edit arrives later)">
        Read
      </button>
      <button
        type="button"
        className="icon-btn"
        disabled
        title="Split view arrives in a later phase"
        aria-label="Split view (unavailable)"
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
