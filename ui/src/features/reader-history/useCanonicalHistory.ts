import { useCallback, useMemo, useRef } from 'react';
import type { CanonicalReaderReference } from '../reader/contracts';
import type { ReaderPosition, ReaderReference } from './model';
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
  const explicit = useRef<{ selection: number; reference: ReaderReference } | null>(null);
  const presented = useRef<{ selection: number; reference: ReaderReference | undefined; heading: string | null } | null>(null);
  const data = useReaderHistoryStore((s) => s.data);
  const selection = useReaderHistoryStore((s) => s.selection);
  const tab = data.tabs.find((item) => item.id === data.activeTabId);
  const entry = tab?.entries[tab.index];
  const exact = entry?.reference;
  const title = entry?.title;
  const heading = entry?.heading;
  const id = tab?.id;
  // Position updates must not manufacture a new receipt and reload the body.
  if (explicit.current && explicit.current.selection !== selection) explicit.current = null;
  if (presented.current?.selection !== selection || presented.current.reference !== exact) {
    presented.current = { selection, reference: exact, heading: entry?.position ? null : heading ?? null };
  }
  const requestedHeading = explicit.current?.selection === selection && exact
    && sameReference(explicit.current.reference, exact) ? heading ?? null : presented.current.heading;
  const reference = useMemo(() => exact
    ? presentation({ reference: exact, title: title ?? '', heading: requestedHeading }) : null,
  [exact, title, requestedHeading, selection]);
  const recordIntent = useCallback((before: number, reference: ReaderReference, heading: string | null) => {
    const state = useReaderHistoryStore.getState();
    const tab = state.data.tabs.find((value) => value.id === state.data.activeTabId);
    const entry = tab?.entries[tab.index];
    // A rejected/full-history operation must never leave intent for a later selection.
    if (state.selection !== before && entry && sameReference(entry.reference, reference)) {
      explicit.current = heading && entry.heading === heading ? { selection: state.selection, reference } : null;
    }
  }, []);
  const setReference = useCallback((next: CanonicalReaderReference | null, intent: 'reopen' | 'exact' = 'reopen') => {
    const store = useReaderHistoryStore.getState();
    if (!next) { store.select(null); return; }
    const exact = receipt(next);
    const before = store.selection;
    const prior = store.data.recent.find((entry) => sameReference(entry.reference, exact));
    store.open(exact, next.title ?? next.path,
      requestedPosition(next) ?? prior?.position ?? null, next.heading ?? prior?.heading ?? null, intent);
    recordIntent(before, exact, next.heading ?? null);
  }, [recordIntent]);
  const navigate = useCallback((next: CanonicalReaderReference) => {
    if (!id || !exact) return;
    const state = useReaderHistoryStore.getState();
    const before = state.selection;
    const target = receipt(next);
    state.navigate(id, exact, selection, {
      reference: target, title: next.title ?? next.path,
      heading: next.heading ?? null, position: requestedPosition(next),
    });
    recordIntent(before, target, next.heading ?? null);
  }, [id, exact, selection, recordIntent]);
  const savePosition = useCallback((position: ReaderPosition) => {
    if (id && exact) useReaderHistoryStore.getState().position(id, exact, selection, position);
  }, [id, exact, selection]);
  return { tab, entry, reference, setReference, navigate, savePosition, back, forward };
}
