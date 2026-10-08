import { useCallback, useMemo } from 'react';
import type { CanonicalReaderReference } from '../reader/contracts';
import type { ReaderPosition } from './model';
import { useReaderHistoryStore } from './store';
import { presentation, receipt } from './references';
import { sameReference } from './model';
const back = () => useReaderHistoryStore.getState().step(-1);
const forward = () => useReaderHistoryStore.getState().step(1);
function requestedPosition(reference: CanonicalReaderReference): ReaderPosition | null {
  return reference.line && reference.line > 1
    ? { revision: reference.revision, line: reference.line, block: 0, offset: 0 } : null;
}
export function useCanonicalHistory() {
  const data = useReaderHistoryStore((s) => s.data);
  const selection = useReaderHistoryStore((s) => s.selection);
  const tab = data.tabs.find((item) => item.id === data.activeTabId);
  const entry = tab?.entries[tab.index];
  const exact = entry?.reference;
  const title = entry?.title;
  const heading = entry?.heading;
  const id = tab?.id;
  // Position updates must not manufacture a new receipt and reload the body.
  const reference = useMemo(() => exact
    ? presentation({ reference: exact, title: title ?? '', heading: heading ?? null }) : null,
  [exact, title, heading]);
  const setReference = useCallback((next: CanonicalReaderReference | null) => {
    const store = useReaderHistoryStore.getState();
    if (!next) { store.select(null); return; }
    const exact = receipt(next);
    const prior = store.data.recent.find((entry) => sameReference(entry.reference, exact));
    store.open(exact, next.title ?? next.path,
      requestedPosition(next) ?? prior?.position ?? null, next.heading ?? prior?.heading ?? null);
  }, []);
  const navigate = useCallback((next: CanonicalReaderReference) => {
    if (!id || !exact) return;
    useReaderHistoryStore.getState().navigate(id, exact, selection, {
      reference: receipt(next), title: next.title ?? next.path,
      heading: next.heading ?? null, position: requestedPosition(next),
    });
  }, [id, exact, selection]);
  const savePosition = useCallback((position: ReaderPosition) => {
    if (id && exact) useReaderHistoryStore.getState().position(id, exact, selection, position);
  }, [id, exact, selection]);
  return { tab, entry, reference, setReference, navigate, savePosition, back, forward };
}
