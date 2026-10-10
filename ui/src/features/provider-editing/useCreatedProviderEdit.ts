import { useCallback, useEffect, useRef } from 'react';
import type { ProviderReaderReference } from '../../platform/providers';
import { useReaderHistoryStore } from '../reader-history/store';
import { receipt } from '../reader-history/references';
import { sameReference } from '../reader-history/model';

/** A confirmed Create click carries edit intent only into its exact resulting selection. */
export function useCreatedProviderEdit(reference: ProviderReaderReference | null, canEdit: boolean,
  edit: (target: ProviderReaderReference) => Promise<void>, open: (target: ProviderReaderReference) => void, refresh: () => void) {
  const pending = useRef<{ reference: ProviderReaderReference; selection: number } | null>(null);
  const selection = useReaderHistoryStore(state => state.selection);
  if (pending.current && pending.current.selection !== selection) pending.current = null;
  const created = useCallback((target: ProviderReaderReference) => {
    open(target);
    const state = useReaderHistoryStore.getState();
    const tab = state.data.tabs.find(item => item.id === state.data.activeTabId);
    const exact = tab?.entries[tab.index]?.reference;
    pending.current = exact && sameReference(exact, receipt(target)) ? { reference: target, selection: state.selection } : null;
    refresh();
  }, [open, refresh]);
  useEffect(() => {
    const intent = pending.current;
    if (!intent || !reference || !canEdit || intent.selection !== selection || !sameReference(receipt(intent.reference), receipt(reference))) return;
    pending.current = null;
    void edit(reference);
  }, [reference, canEdit, edit, selection]);
  return created;
}
