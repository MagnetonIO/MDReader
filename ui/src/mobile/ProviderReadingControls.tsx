import { PinIcon } from '../shell/icons';
import { ActionIcon } from '../features/document-transfer/ActionIcon';
import type { useProviderReadingState } from './useProviderReadingState';
import '../features/device-reader/device-reading.css';
export function ProviderReadingControls({ reading, line, onResume }: {
  reading: ReturnType<typeof useProviderReadingState>; line: number; onResume(line: number): void;
}) {
  const state = reading.state;
  const validLine = Number.isSafeInteger(line) && line >= 1 && !!state && line <= state.lineEnd;
  return <section className="device-reading-controls" aria-label="Pin and reading position" aria-busy={reading.busy}>
    <div className="device-reading-controls__actions">
      <button type="button" disabled={reading.busy || !state} aria-pressed={state?.pinned ?? false} aria-label={state?.pinned ? 'Unpin document' : 'Pin document'}
        onClick={() => void reading.change({ pinned: !state?.pinned })}><PinIcon />{state?.pinned ? 'Pinned' : 'Pin'}</button>
      <button type="button" disabled={reading.busy || !validLine} aria-label="Save reading position" title={`Save your place at line ${line}`} onClick={() => void reading.change({ line })}><ActionIcon kind="bookmark" />Save place</button>
      {state && state.line !== line ? <button type="button" disabled={reading.busy} aria-label="Resume saved position" onClick={() => onResume(state.line)}><ActionIcon kind="resume" />Resume</button> : null}
    </div>
    {reading.notice ? <p role="status">{reading.notice}</p> : null}
    {reading.error ? <p role="alert">{reading.error} <button type="button" disabled={reading.busy} onClick={() => void reading.change()}>Refresh saved place</button></p> : null}
  </section>;
}
