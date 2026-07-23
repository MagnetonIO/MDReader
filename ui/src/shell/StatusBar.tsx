/** The bottom status bar: document word count and the workspace watcher's live
 * state. The backend indicator is a placeholder ("Local") until the engine lands
 * in P3. */

import type { WatcherStatus } from '../stores/sources.js';

export interface StatusBarProps {
  words: number | null;
  watcher: WatcherStatus;
  fileCount: number;
}

const WATCHER_LABEL: Record<WatcherStatus, string> = {
  idle: 'Not watching',
  watching: 'Watching',
  error: 'Watch error',
};

export function StatusBar({ words, watcher, fileCount }: StatusBarProps) {
  return (
    <footer className="statusbar" aria-label="Status">
      <span>{words === null ? '—' : `${words.toLocaleString()} words`}</span>
      <span className="statusbar__spacer" />
      <span>{fileCount > 0 ? `${fileCount.toLocaleString()} files` : ''}</span>
      <span title="Filesystem watcher">
        <span className={`statusbar__dot statusbar__dot--${watcher === 'watching' ? 'watching' : watcher}`} />
        {WATCHER_LABEL[watcher]}
      </span>
      <span title="Backend (local files this phase)">Local</span>
    </footer>
  );
}
