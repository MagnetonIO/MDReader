import { useEffect, useRef, useState } from 'react';
import type { DeviceReference } from './backend';
import { deviceReadingStateBackend, type DeviceReadingState, type DeviceReadingStateBackend } from './reading-state-backend';
import './device-reading.css';

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
        onClick={() => void change({ pinned: !state?.pinned })}>{state?.pinned ? 'Unpin document' : 'Pin document'}</button>
      <button type="button" disabled={busy || !state || !validLine}
        onClick={() => void change({ line })}>Save reading position</button>
      <button type="button" disabled={busy || !state} onClick={() => state && onResume(state.line)}>Resume saved position</button>
      <button type="button" disabled={busy} onClick={() => void change()}>Refresh reading status</button>
    </div>
    {state ? <p>{state.pinned ? 'Pinned · ' : ''}Saved position: line {state.line}. Current position: line {line}.</p>
      : !error ? <p role="status">Reading saved document state…</p> : null}
    {notice ? <p role="status">{notice} Sync it explicitly from Account &amp; sync.</p> : null}
    {error ? <p role="alert">{error}</p> : null}
  </section>;
}
function failure(cause: unknown) {
  return cause instanceof Error ? cause.message : 'The native owner could not save this document state. Your reading copy is unchanged.';
}
