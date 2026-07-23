/** The bottom status bar: document word count, the workspace watcher's live
 * state, and the engine index's build progress (spinner + counts while
 * importing, then Indexed/Failed). */

import type { ImportState } from '../platform/fs.js';
import type { WatcherStatus } from '../stores/sources.js';

export interface StatusBarProps {
  words: number | null;
  watcher: WatcherStatus;
  fileCount: number;
  /** Live index-build progress, or `null` before any import. */
  importState: ImportState | null;
  /** Whether the file list currently comes from the engine index. */
  fromIndex: boolean;
  /** Re-run the import (e.g. after a model-load timeout). Shown as a Retry
   *  affordance while the index is in the `error` phase. */
  onRetryIndex?: () => void;
}

const WATCHER_LABEL: Record<WatcherStatus, string> = {
  idle: 'Not watching',
  watching: 'Watching',
  error: 'Watch error',
};

/** The index segment's text and whether it should show a spinner. */
function indexIndicator(
  importState: ImportState | null,
  fromIndex: boolean,
): { label: string; busy: boolean; title: string } {
  const phase = importState?.phase ?? 'idle';
  if (phase === 'loading') {
    return {
      label: 'Loading model…',
      busy: true,
      title: 'Preparing the local embedding model (first run downloads it once)',
    };
  }
  if (phase === 'importing') {
    const total = importState?.filesTotal ?? 0;
    return {
      label: total > 0 ? `Indexing ${total.toLocaleString()} files…` : 'Indexing…',
      busy: true,
      title: 'Building the search index',
    };
  }
  if (phase === 'error') {
    return {
      label: 'Index failed',
      busy: false,
      title: importState?.error ?? 'The index build failed',
    };
  }
  if (phase === 'done') {
    const files = importState?.filesDone ?? 0;
    const chunks = importState?.chunks ?? 0;
    return {
      label: `Indexed ${files.toLocaleString()} files`,
      busy: false,
      title: `${chunks.toLocaleString()} chunks embedded`,
    };
  }
  return {
    label: fromIndex ? 'Indexed' : 'Local files',
    busy: false,
    title: fromIndex ? 'Serving from the index' : 'Reading local files',
  };
}

export function StatusBar({
  words,
  watcher,
  fileCount,
  importState,
  fromIndex,
  onRetryIndex,
}: StatusBarProps) {
  const index = indexIndicator(importState, fromIndex);
  const failed = importState?.phase === 'error';
  return (
    <footer className="statusbar" aria-label="Status">
      <span>{words === null ? '—' : `${words.toLocaleString()} words`}</span>
      <span className="statusbar__spacer" />
      <span>{fileCount > 0 ? `${fileCount.toLocaleString()} files` : ''}</span>
      <span title="Filesystem watcher">
        <span className={`statusbar__dot statusbar__dot--${watcher === 'watching' ? 'watching' : watcher}`} />
        {WATCHER_LABEL[watcher]}
      </span>
      <span title={index.title} aria-live="polite">
        {index.busy ? <span className="statusbar__spinner" aria-hidden="true" /> : null}
        {index.label}
        {failed && onRetryIndex ? (
          <button type="button" className="statusbar__retry" onClick={onRetryIndex}>
            Retry
          </button>
        ) : null}
      </span>
    </footer>
  );
}
