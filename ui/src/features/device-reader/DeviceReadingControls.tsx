import { useEffect, useRef, useState } from 'react';
import type { DeviceReference } from './backend';
import { deviceReadingStateBackend, type DeviceReadingState, type DeviceReadingStateBackend } from './reading-state-backend';
import './device-reading.css';
import { PinIcon } from '../../shell/icons';

/** Pin and source-line state are saved by the same native document owner.
 * Scrolling alone does not issue a write or an upload. */
export function DeviceReadingControls({ reference, line, onResume, backend = deviceReadingStateBackend }: {
  reference: DeviceReference;
  line: number;
  onResume(line: number): void;
  backend?: DeviceReadingStateBackend;
}) {
  const [loaded, setLoaded] = useState<{ reference: DeviceReference; value: DeviceReadingState } | null>(null);
  const state = loaded?.reference === reference ? loaded.value : null;
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const current = useRef(reference);
  current.current = reference;
  const work = useRef<object | null>(null);
  useEffect(() => {
    const operation = {};
    work.current = operation;
    setLoaded(null); setError(null); setNotice(null); setBusy(true);
    void backend.read(reference).then((value) => {
      if (work.current === operation && current.current === reference) setLoaded({ reference, value });
    }).catch((cause: unknown) => {
      if (work.current === operation && current.current === reference) setError(failure(cause));
    }).finally(() => {
      if (work.current === operation && current.current === reference) { work.current = null; setBusy(false); }
    });
    return () => { if (work.current === operation) work.current = null; };
  }, [backend, reference]);

  async function change(patch?: { pinned?: boolean; line?: number }) {
    if (work.current || patch && !state) return;
    const operation = {};
    work.current = operation; setBusy(true); setError(null); setNotice(null);
    try {
      const next = patch && state
        ? await backend.update(reference, state.generation, patch)
        : await backend.read(reference);
      if (work.current !== operation || current.current !== reference) return;
      setLoaded({ reference, value: next });
      if (patch) setNotice(patch.line !== undefined ? `Reading position saved at line ${next.line}.` : next.pinned ? 'Pinned on this device.' : 'Unpinned on this device.');
    } catch (cause) {
      if (work.current === operation && current.current === reference) setError(failure(cause));
    } finally {
      if (work.current === operation && current.current === reference) { work.current = null; setBusy(false); }
    }
  }
  const validLine = Number.isSafeInteger(line) && line >= 1 && (!state || line <= state.lineEnd);
  return <section className="device-reading-controls" aria-label="Pin and reading position" aria-busy={busy}>
    <div className="device-reading-controls__actions">
      <button type="button" disabled={busy || !state} aria-pressed={state?.pinned ?? false}
        aria-label={state?.pinned ? 'Unpin document' : 'Pin document'}
        onClick={() => void change({ pinned: !state?.pinned })}><PinIcon />{state?.pinned ? 'Pinned' : 'Pin'}</button>
      <button type="button" disabled={busy || !state || !validLine} aria-label="Save reading position"
        title={`Save your place at line ${line}`} onClick={() => void change({ line })}>Save place</button>
      <button type="button" disabled={busy || !state} aria-label="Resume saved position"
        title={state ? `Return to line ${state.line}` : undefined} onClick={() => state && onResume(state.line)}>Resume</button>
      <button type="button" disabled={busy} aria-label="Refresh reading status" onClick={() => void change()}>Refresh</button>
    </div>
    {state ? <p className="device-reading-controls__position">Line {line} <span>· Saved at {state.line}</span></p>
      : !error ? <p role="status">Loading your saved place…</p> : null}
    {notice ? <p role="status">{notice}</p> : null}
    {error ? <p role="alert">{error}</p> : null}
  </section>;
}
function failure(cause: unknown) {
  return cause instanceof Error ? cause.message : 'The native owner could not save this document state. Your reading copy is unchanged.';
}
