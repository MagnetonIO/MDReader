import { useCallback, useEffect, useRef, useState } from 'react';
import { useEditorStore } from '../../stores/editor';
import { isDeviceReference, type DeviceReference } from '../device-reader/backend';
import type { CanonicalReaderReference } from '../reader/contracts';
import { sameReference } from '../reader-history/model';
import { receipt } from '../reader-history/references';
import { useReaderHistoryStore } from '../reader-history/store';
import { beginDeviceEdit, sameDevice, useDeviceEdits } from './store';

export function useDeviceEditing(reference: CanonicalReaderReference | null, tabId: string | undefined, notify: (message: string) => void) {
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
  const edit = useCallback(async (target: DeviceReference) => {
    if (!tabId || opening) return;
    const epoch = ++action.current;
    const selection = useReaderHistoryStore.getState().selection;
    const active = () => mounted.current && epoch === action.current && current.current.tabId === tabId
      && current.current.reference === target && useReaderHistoryStore.getState().selection === selection;
    setOpening(true);
    try {
      const key = await beginDeviceEdit(`reader:${tabId}`, target, active);
      if (key && active()) setDisplayKey(key);
    } catch (cause) { if (active()) notify(cause instanceof Error ? cause.message : 'Markdown editing is unavailable.'); }
    finally { if (epoch === action.current && mounted.current) setOpening(false); }
  }, [tabId, notify, opening]);
  const edits = useDeviceEdits((state) => state.edits);
  const buffers = useEditorStore(state => state.entries);
  const draftKept = Boolean(reference && isDeviceReference(reference) && Object.values(edits).some(edit => edit.tabId === `reader:${tabId}`
    && (sameDevice(edit.origin, reference) || sameDevice(edit.reference, reference)) && buffers[`${edit.tabId} ${edit.key}`]?.dirty));
  const displayed = displayKey ? edits[displayKey] : null;
  const activeKey = displayed && reference && isDeviceReference(reference) && displayed.tabId === `reader:${tabId}`
    && (sameDevice(displayed.origin, reference) || sameDevice(displayed.reference, reference)) ? displayKey : null;
  const openDraft = useCallback((key: string) => {
    const value = useDeviceEdits.getState().edits[key];
    const store = useReaderHistoryStore.getState();
    const id = value?.tabId.slice(7);
    const tab = store.data.tabs.find((item) => item.id === id);
    if (!value || !tab) { notify('This draft’s tab is unavailable. Your buffer was kept.'); return; }
    store.select(tab.id);
    const selected = useReaderHistoryStore.getState();
    const entry = tab.entries[tab.index]!;
    if (!sameReference(entry.reference, receipt(value.reference))) selected.navigate(tab.id, entry.reference, selected.selection, {
      reference: receipt(value.reference), title: value.reference.title ?? 'Markdown copy', heading: null, position: null,
    });
    setDisplayKey(key);
  }, [notify]);
  const published = useCallback((key: string, expectedSelection: number, requireClean = true) => {
    const value = useDeviceEdits.getState().edits[key];
    const origin = current.current;
    const store = useReaderHistoryStore.getState();
    if (store.selection !== expectedSelection || !value || value.pending || !origin.reference
      || !isDeviceReference(origin.reference) || value.tabId !== `reader:${origin.tabId}`
      || !(sameDevice(value.origin, origin.reference) || sameDevice(value.reference, origin.reference))
      || requireClean && useEditorStore.getState().entry(value.tabId, key)?.dirty !== false) return;
    const tab = store.data.tabs.find((item) => item.id === store.data.activeTabId);
    if (!tab || tab.id !== origin.tabId || !sameReference(tab.entries[tab.index]!.reference, receipt(origin.reference))) return;
    if (!sameDevice(value.reference, origin.reference)) store.navigate(tab.id, tab.entries[tab.index]!.reference, store.selection, {
      reference: receipt(value.reference), title: value.reference.title ?? 'Markdown copy', heading: null, position: null,
    });
    // Keep the selected editor view and mounted buffer after receipt adoption.
  }, []);
  const read = useCallback(() => {
    if (displayKey) published(displayKey, useReaderHistoryStore.getState().selection, false);
    // An unresolved save has no new verified receipt to navigate to; the draft
    // remains reachable while the original revision is displayed.
    setDisplayKey(null);
  }, [displayKey, published]);
  return { edit, opening, activeKey, openDraft, published, read, draftKept };
}
