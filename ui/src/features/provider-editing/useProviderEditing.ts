import { useCallback, useEffect, useRef, useState } from 'react';
import type { ProviderReaderReference } from '../../platform/providers';
import { useEditorStore } from '../../stores/editor';
import { sameReference } from '../reader-history/model';
import { receipt } from '../reader-history/references';
import { useReaderHistoryStore } from '../reader-history/store';
import { beginProviderEdit, useProviderEdits } from './store';

export function useProviderEditing(reference: ProviderReaderReference | null, tabId: string | undefined, notify: (message: string) => void) {
  const [displayKey, setDisplayKey] = useState<string | null>(null);
  const [opening, setOpening] = useState(false);
  const action = useRef(0);
  const mounted = useRef(true);
  const current = useRef({ reference, tabId });
  current.current = { reference, tabId };
  useEffect(() => {
    mounted.current = true;
    return () => { mounted.current = false; action.current++; };
  }, []);
  const edit = useCallback(async (target: ProviderReaderReference) => {
    if (!tabId || !target.sourceId.startsWith('provider:') || opening) return;
    const epoch = ++action.current;
    const selection = useReaderHistoryStore.getState().selection;
    const active = () => mounted.current && epoch === action.current && current.current.tabId === tabId
      && current.current.reference === target && useReaderHistoryStore.getState().selection === selection;
    setOpening(true);
    try {
      const key = await beginProviderEdit(`reader:${tabId}`, target, active);
      if (key && active()) setDisplayKey(key);
    } catch (cause) { if (active()) notify(cause instanceof Error ? cause.message : 'Provider editing is unavailable.'); }
    finally { if (epoch === action.current && mounted.current) setOpening(false); }
  }, [tabId, notify, opening]);
  const entries = useProviderEdits((state) => state.edits);
  const displayed = displayKey ? entries[displayKey] : null;
  const activeKey = displayed && reference && displayed.tabId === `reader:${tabId}`
    && sameReference(receipt(displayed.reference), receipt(reference)) ? displayKey : null;
  const openDraft = useCallback((target: ProviderReaderReference, editorTabId: string) => {
    const edits = Object.values(useProviderEdits.getState().edits);
    const existing = edits.find((value) => value.tabId === editorTabId && sameReference(receipt(value.reference), receipt(target)));
    const store = useReaderHistoryStore.getState();
    const id = editorTabId.slice(7);
    const tab = store.data.tabs.find((value) => value.id === id);
    if (!existing || !tab) { notify('The retained editor tab is unavailable. Your buffer was kept.'); return; }
    store.select(id);
    const selected = useReaderHistoryStore.getState();
    if (!sameReference(tab.entries[tab.index]!.reference, receipt(target))) {
      selected.navigate(id, tab.entries[tab.index]!.reference, selected.selection, {
        reference: receipt(target), title: target.title ?? target.path, heading: null, position: null,
      });
    }
    setDisplayKey(existing.key);
  }, [notify]);
  const published = useCallback((key: string, expectedSelection: number) => {
    const value = useProviderEdits.getState().edits[key];
    const origin = current.current;
    const store = useReaderHistoryStore.getState();
    if (store.selection !== expectedSelection || !value?.projection || !origin.reference || value.tabId !== `reader:${origin.tabId}`
      || !sameReference(receipt(value.reference), receipt(origin.reference))
      || useEditorStore.getState().entry(value.tabId, key)?.dirty !== false) return;
    const tab = store.data.tabs.find((item) => item.id === store.data.activeTabId);
    if (!tab || tab.id !== origin.tabId || !sameReference(tab.entries[tab.index]!.reference, receipt(value.reference))) return;
    store.navigate(tab.id, tab.entries[tab.index]!.reference, store.selection, {
      reference: { kind: 'provider', sourceId: value.projection.readerSourceId, path: value.projection.path, revision: value.projection.revision },
      title: value.reference.title ?? value.reference.path, heading: null, position: null,
    });
    setDisplayKey(null);
  }, []);
  return { edit, opening, activeKey, openDraft, published, read: () => setDisplayKey(null) };
}
