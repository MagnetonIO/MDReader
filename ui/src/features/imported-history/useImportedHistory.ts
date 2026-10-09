import { useCallback, useEffect, useRef, useState } from 'react';
import type { CanonicalReaderReference } from '../reader/contracts';
import { importedHistoryBackend, ImportedHistoryFailure, type ImportedHistoryBackend, type ImportedHistoryEntry, type ImportedHistoryPage } from './backend';

export function useImportedHistory(backend: ImportedHistoryBackend = importedHistoryBackend) {
  const [snapshot, setSnapshot] = useState<ImportedHistoryPage | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [stale, setStale] = useState(false);
  const [matching, setMatching] = useState<ImportedHistoryEntry | null>(null);
  const epoch = useRef(0);
  const running = useRef<number | null>(null);
  const refresh = useCallback(async () => {
    if (running.current !== null) return;
    const request = ++epoch.current;
    running.current = request;
    setBusy(true); setError(null); setMatching(null);
    try {
      const page = await backend.list(null);
      if (epoch.current !== request) return;
      setSnapshot(page); setStale(false);
    } catch (cause) {
      if (epoch.current === request) setError(cause instanceof Error ? cause.message : 'Imported history is unavailable.');
    } finally { if (running.current === request) running.current = null; if (epoch.current === request) setBusy(false); }
  }, [backend]);
  useEffect(() => {
    setSnapshot(null); setStale(false); setError(null);
    void refresh();
    return () => { ++epoch.current; running.current = null; };
  }, [refresh]);
  async function more() {
    if (!snapshot?.nextAfterEntryId || running.current !== null || stale) return;
    const request = ++epoch.current;
    running.current = request;
    setBusy(true); setError(null);
    try {
      const page = await backend.list(snapshot.nextAfterEntryId);
      if (epoch.current !== request) return;
      if (page.historyDigest !== snapshot.historyDigest || snapshot.entries.length + page.entries.length > 1000
        || page.entries.some((entry) => snapshot.entries.some((prior) => prior.entryId === entry.entryId))) {
        throw new ImportedHistoryFailure('stale');
      }
      setSnapshot({ ...page, entries: [...snapshot.entries, ...page.entries] });
    } catch (cause) {
      if (epoch.current === request) { setStale(true); setError(cause instanceof Error ? cause.message : 'Refresh imported history before continuing.'); }
    } finally { if (running.current === request) running.current = null; if (epoch.current === request) setBusy(false); }
  }
  async function bind(reference: CanonicalReaderReference) {
    if (!snapshot || !matching || running.current !== null || stale) throw new ImportedHistoryFailure('stale');
    const request = ++epoch.current;
    running.current = request;
    setBusy(true); setError(null);
    try {
      const page = await backend.bind(matching, snapshot.historyDigest, reference);
      if (epoch.current !== request) throw new ImportedHistoryFailure('stale');
      setMatching(null);
      // A bind reply may contain only the matched row, not the remaining
      // history. Re-read the first page rather than claiming it is complete.
      try {
        const current = await backend.list(null);
        if (epoch.current === request) { setSnapshot(current); setStale(false); }
      } catch {
        if (epoch.current === request) {
          const bound = page.entries.find((entry) => entry.entryId === matching.entryId)!;
          setSnapshot({ ...snapshot, entries: snapshot.entries.map((entry) => entry.entryId === bound.entryId ? bound : entry) });
          setStale(true); setError('The match was saved. Refresh history before opening it or making another match.');
        }
      }
    } catch (cause) {
      if (epoch.current === request) { setStale(true); setError(cause instanceof Error ? cause.message : 'The match could not be confirmed. Refresh history before trying again.'); }
      throw cause;
    } finally { if (running.current === request) running.current = null; if (epoch.current === request) setBusy(false); }
  }
  return { snapshot, busy, error, stale, matching, refresh, more, bind,
    match(entry: ImportedHistoryEntry) { if (!busy && !stale) setMatching(entry); },
    closeMatch() { if (running.current === null) setMatching(null); },
  };
}
