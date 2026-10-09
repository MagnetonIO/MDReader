import { useCallback, useEffect, useRef, useState } from 'react';
import { saveDraftCopy } from '../document-transfer/draft-copy';

/** A copy never completes an editor save or clears dirty/uncertain user state. */
export function useDraftCopy(editKey: string) {
  const [busy, setBusy] = useState(false);
  const [notice, setNotice] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const inFlight = useRef(false);
  const epoch = useRef(0);
  const mounted = useRef(true);
  useEffect(() => {
    mounted.current = true;
    return () => { mounted.current = false; };
  }, []);
  useEffect(() => {
    epoch.current++; setNotice(null); setError(null);
    return () => { epoch.current++; };
  }, [editKey]);
  const save = useCallback(async (markdown: string, filename: string) => {
    if (inFlight.current) return;
    inFlight.current = true;
    const current = epoch.current;
    setBusy(true); setError(null); setNotice(null);
    try {
      const result = await saveDraftCopy(markdown, filename);
      if (current !== epoch.current) return;
      setNotice(result === 'cancelled' ? 'Copy not saved. Your editor draft is kept.'
        : result === 'downloaded' ? 'Copy download started. Your editor draft is unchanged.'
        : 'Copy saved. Your editor draft is unchanged.');
    } catch (cause) {
      if (current === epoch.current) setError(cause instanceof Error ? cause.message : 'The copy could not be confirmed. Your editor draft is kept.');
    } finally {
      inFlight.current = false;
      if (mounted.current) setBusy(false);
    }
  }, []);
  return { busy, notice, error, save };
}
