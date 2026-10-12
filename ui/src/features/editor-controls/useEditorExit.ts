import { useEffect, useMemo, useRef, useState } from 'react';
import { flushEditorDrafts } from '../draft-recovery/runtime';
import { useEditorStore } from '../../stores/editor';

export async function keepDraftForReading(tabId: string, path: string, isCurrent: () => boolean, onRead: () => void) {
  while (isCurrent()) {
    const before = useEditorStore.getState().entry(tabId, path);
    if (!before) return;
    if (before.dirty) await flushEditorDrafts(before);
    if (!isCurrent()) return;
    const after = useEditorStore.getState().entry(tabId, path);
    if (!after) return;
    if (after.buffer !== before.buffer || after.baseHash !== before.baseHash) continue;
    onRead();
    return;
  }
}

/** Leaving a view never publishes a document or cancels its pending save. */
export function useEditorExit(tabId: string | null, path: string | null, onRead: () => void) {
  const identity = useMemo(() => ({}), [tabId, path]);
  const current = useRef<object | null>(identity); current.current = identity;
  const flight = useRef<object | null>(null);
  const [result, setResult] = useState<{ identity: object; busy: boolean; error: string | null } | null>(null);
  useEffect(() => { current.current = identity; return () => { if (current.current === identity) current.current = null; }; }, [identity]);
  async function done() {
    if (!tabId || !path || flight.current === identity) return;
    flight.current = identity;
    setResult({ identity, busy: true, error: null });
    try {
      // A provider save may settle during the checkpoint. Preserve the newest
      // buffer/revision before leaving, without waiting for or resending it.
      await keepDraftForReading(tabId, path, () => current.current === identity, onRead);
    } catch {
      if (current.current === identity) setResult({ identity, busy: false,
        error: 'Your draft could not be kept. Retry or save a copy before leaving.' });
    } finally {
      if (flight.current === identity) flight.current = null;
      setResult(value => value?.identity === identity ? { ...value, busy: false } : value);
    }
  }
  return { done, busy: result?.identity === identity && result.busy,
    error: result?.identity === identity ? result.error : null };
}
