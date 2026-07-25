/** The bottom status bar: document word count, the workspace watcher's live
 * state, and the engine index's build progress (spinner + counts while
 * importing, then Indexed/Failed).
 *
 * Every phase the backend can be in has a label of its own here, and none of
 * them is a benign-looking zero. In particular a model that could not be loaded
 * is neither "Indexed" nor "Index failed": it reads "Embedding model
 * unavailable", carries the reason the backend actually observed, and offers the
 * retry that re-attempts the load. */

import type { ImportState, ListingFailure } from '../platform/fs.js';
import type { WatcherStatus } from '../stores/sources.js';

export interface StatusBarProps {
  words: number | null;
  watcher: WatcherStatus;
  fileCount: number;
  /** Why the listing failed, or `null`. When set, the file-count segment says
   *  the folder is unreadable rather than falling silent — a blank count is
   *  exactly how a permission denial used to pass for an empty workspace. */
  listingError?: ListingFailure | null;
  /** Live index-build progress, or `null` before any import. */
  importState: ImportState | null;
  /** Whether the file list currently comes from the engine index. */
  fromIndex: boolean;
  /** Re-run the import. Shown as a retry affordance while the index is in the
   *  `error` phase, and — with its own label — while the embedding model is
   *  unavailable, where the retry's job is to re-attempt the model load. */
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
  if (phase === 'reembedding') {
    // Its own label: this pass re-embeds every file, not just what changed, so
    // "Indexing N files…" would understate how long it takes.
    const total = importState?.filesTotal ?? 0;
    return {
      label: total > 0 ? `Re-embedding ${total.toLocaleString()} files…` : 'Re-embedding…',
      busy: true,
      title:
        'The embedding model changed, so every file is being embedded again — a one-time pass. Search keeps using the previous index until it finishes.',
    };
  }
  if (phase === 'model-unavailable') {
    // Its own label, because it is its own condition. "Index failed" would blame
    // the index, and "Indexed"/"Local files" would say nothing happened at all —
    // when in fact a nameable thing did, and the user can act on it. The reason
    // the backend observed travels in the tooltip verbatim; nothing is invented.
    const reason = importState?.error;
    return {
      label: 'Embedding model unavailable',
      busy: false,
      title: `${
        reason ?? 'The embedding model could not be loaded'
      }. The workspace is open and you can read and text-search it; indexing and semantic ranking resume once the model loads.`,
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

/** The file-count segment. A failed listing names itself; only a listing that
 *  actually succeeded is allowed to report a count (or, at zero, stay quiet). */
function fileCountSegment(
  fileCount: number,
  listingError: ListingFailure | null | undefined,
): { text: string; title: string | undefined } {
  if (listingError?.kind === 'unreadable') {
    return { text: 'Folder unreadable', title: `${listingError.reason} — ${listingError.remedy}` };
  }
  if (listingError) {
    return { text: 'File list unavailable', title: listingError.message };
  }
  return { text: fileCount > 0 ? `${fileCount.toLocaleString()} files` : '', title: undefined };
}

export function StatusBar({
  words,
  watcher,
  fileCount,
  listingError,
  importState,
  fromIndex,
  onRetryIndex,
}: StatusBarProps) {
  const files = fileCountSegment(fileCount, listingError);
  const index = indexIndicator(importState, fromIndex);
  const phase = importState?.phase;
  // Two different retries, labelled as such: one re-runs an index build that
  // failed, the other re-attempts a model load that never completed.
  const retryLabel =
    phase === 'error' ? 'Retry' : phase === 'model-unavailable' ? 'Retry model load' : null;
  return (
    <footer className="statusbar" aria-label="Status">
      <span>{words === null ? '—' : `${words.toLocaleString()} words`}</span>
      <span className="statusbar__spacer" />
      <span className={files.title ? 'statusbar__alert' : undefined} title={files.title}>
        {files.text}
      </span>
      <span title="Filesystem watcher">
        <span className={`statusbar__dot statusbar__dot--${watcher === 'watching' ? 'watching' : watcher}`} />
        {WATCHER_LABEL[watcher]}
      </span>
      <span title={index.title} aria-live="polite">
        {index.busy ? <span className="statusbar__spinner" aria-hidden="true" /> : null}
        {index.label}
        {retryLabel && onRetryIndex ? (
          <button type="button" className="statusbar__retry" onClick={onRetryIndex}>
            {retryLabel}
          </button>
        ) : null}
      </span>
    </footer>
  );
}
