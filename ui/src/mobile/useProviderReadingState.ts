import { useEffect, useRef, useState } from 'react';
import type { ProviderReaderReference } from '../platform/providers';
import { ProviderDocumentActionError, type ProviderDocumentActionsBackend, type ProviderReadingState } from './provider-document-actions';
/** Scroll observation is transient. Only explicit Pin/Save place performs a native generation CAS. */
export function useProviderReadingState(reference: ProviderReaderReference, backend: ProviderDocumentActionsBackend, resumeSavedPlace = true) {
  const [state, setState] = useState<ProviderReadingState | null>(null);
  const [initializing, setInitializing] = useState(true);
  const [initialLine, setInitialLine] = useState(reference.line ?? 1);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [unavailable, setUnavailable] = useState(false);
  const epoch = useRef(0);
  const operation = useRef(false);
  useEffect(() => {
    let current = true; const selected = ++epoch.current;
    operation.current = true; setBusy(true); setInitializing(true); setInitialLine(reference.line ?? 1); setState(null); setError(null); setNotice(null); setUnavailable(false);
    void backend.read(reference).then(value => {
      if (!current || selected !== epoch.current) return;
      setState(value);
      if (resumeSavedPlace && !reference.heading && (reference.line ?? 1) === 1) setInitialLine(value.line);
    }).catch(cause => { if (current && selected === epoch.current) failure(cause); })
      .finally(() => { if (current && selected === epoch.current) { operation.current = false; setBusy(false); setInitializing(false); } });
    return () => { current = false; epoch.current++; operation.current = false; };
  }, [reference, backend, resumeSavedPlace]);
  function failure(cause: unknown) {
    setError(cause instanceof Error ? cause.message : 'Saved reading state could not be confirmed.');
    // Never reuse an old generation after any unknown outcome. An explicit refresh is required.
    setState(null);
    if (cause instanceof ProviderDocumentActionError && cause.kind === 'stale') setUnavailable(true);
  }
  function retire(cause: unknown) {
    // A transfer may discover revocation while an earlier state CAS is still in
    // flight. Its late result must never resurrect the retired visible body.
    epoch.current++; operation.current = false; setBusy(false); setInitializing(false); setNotice(null);
    failure(cause);
  }
  async function change(patch?: { pinned?: boolean; line?: number }) {
    if (operation.current || patch && !state) return;
    const selected = epoch.current; operation.current = true; setBusy(true); setError(null); setNotice(null);
    try {
      const value = patch && state ? await backend.update(reference, state.generation, patch) : await backend.read(reference);
      if (selected !== epoch.current) return;
      setState(value); setUnavailable(false);
      if (patch) setNotice(patch.line !== undefined ? `Reading position saved at line ${value.line}.` : value.pinned ? 'Pinned on this device.' : 'Unpinned on this device.');
    } catch (cause) { if (selected === epoch.current) failure(cause); }
    finally { if (selected === epoch.current) { operation.current = false; setBusy(false); } }
  }
  return { state, initializing, initialLine, busy, error, notice, unavailable, change, retire };
}
