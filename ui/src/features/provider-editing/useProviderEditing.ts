import { useCallback, useEffect, useRef, useState } from 'react';
import type { ProviderReaderReference } from '../../platform/providers';
import { nativeProvidersBackend } from '../../platform/providers';
import { useEditorStore } from '../../stores/editor';
import { sameReference } from '../reader-history/model';
import { receipt } from '../reader-history/references';
import { useReaderHistoryStore } from '../reader-history/store';
import { currentCreatedDocument } from './currentCreatedDocument';
import { beginProviderEdit, findProviderEdit, protectedProviderEdit, useProviderEdits } from './store';

export function useProviderEditing(reference: ProviderReaderReference | null, tabId: string | undefined, notify: (message: string) => void, refreshKey = 0) {
  const [displayKey, setDisplayKey] = useState<string | null>(null);
  const [opening, setOpening] = useState(false);
  const [writable, setWritable] = useState<{ sourceId: string; path: string; revision: string; refreshKey: number; allowed: boolean } | null>(null);
  const action = useRef(0);
  const mounted = useRef(true);
  const current = useRef({ reference, tabId, refreshKey });
  current.current = { reference, tabId, refreshKey };
  useEffect(() => {
    mounted.current = true;
    return () => { mounted.current = false; action.current++; };
  }, []);
  useEffect(() => {
    let active = true;
    setWritable(null);
    if (reference?.sourceId.startsWith('provider:')) {
      const { sourceId, path, revision } = reference;
      void nativeProvidersBackend.state().then((snapshot) => {
        const source = snapshot.sources.find((item) => item.readerSourceId === sourceId);
        if (active) setWritable({ sourceId, path, revision, refreshKey, allowed: Boolean(source?.enabled && source.health === 'ready' && source.capabilities.guardedWrite) });
      }).catch(() => { if (active) setWritable({ sourceId, path, revision, refreshKey, allowed: false }); });
    }
    return () => { active = false; };
  }, [reference?.sourceId, reference?.path, reference?.revision, refreshKey]);
  const canEdit = writable?.sourceId === reference?.sourceId && writable?.path === reference?.path
    && writable?.revision === reference?.revision && writable?.refreshKey === refreshKey && writable?.allowed === true;
  const begin = useCallback(async (target: ProviderReaderReference, fetchCurrent: boolean) => {
    if (!tabId || !target.sourceId.startsWith('provider:') || opening || !canEdit) return;
    const epoch = ++action.current;
    const selection = useReaderHistoryStore.getState().selection;
    const active = () => mounted.current && epoch === action.current && current.current.tabId === tabId && current.current.refreshKey === refreshKey
      && current.current.reference === target && useReaderHistoryStore.getState().selection === selection;
    const editorTabId = `reader:${tabId}`;
    const protectedDraft = () => Object.values(useProviderEdits.getState().edits).find(value =>
      value.tabId === editorTabId && value.reference.sourceId === target.sourceId && value.reference.path === target.path
      && protectedProviderEdit(value));
    setOpening(true);
    try {
      const retained = protectedDraft();
      if (retained) {
        if (findProviderEdit(editorTabId, target)?.key === retained.key) setDisplayKey(retained.key);
        else notify('Your existing cloud draft was kept. Open it from Cloud drafts before editing another revision.');
        return;
      }
      const latest = fetchCurrent ? await currentCreatedDocument(target, nativeProvidersBackend, active) : target;
      if (!latest || !active()) return;
      const key = await beginProviderEdit(editorTabId, latest, () => active() && !protectedDraft());
      if (!key || !active() || protectedDraft()) return;
      const store = useReaderHistoryStore.getState();
      const tab = store.data.tabs.find(value => value.id === store.data.activeTabId);
      if (!tab || tab.id !== tabId || !sameReference(tab.entries[tab.index]!.reference, receipt(target))) return;
      if (!sameReference(receipt(target), receipt(latest))) store.navigate(tab.id, tab.entries[tab.index]!.reference, selection, {
        reference: receipt(latest), title: latest.title ?? latest.path, heading: null, position: null,
      });
      const selected = useReaderHistoryStore.getState();
      const currentTab = selected.data.tabs.find(value => value.id === selected.data.activeTabId);
      if (currentTab?.id === tabId && sameReference(currentTab.entries[currentTab.index]!.reference, receipt(latest))) setDisplayKey(key);
    } catch (cause) { if (active()) notify(cause instanceof Error ? cause.message : 'Provider editing is unavailable.'); }
    finally { if (epoch === action.current && mounted.current) setOpening(false); }
  }, [tabId, notify, opening, canEdit, refreshKey]);
  const edit = useCallback((target: ProviderReaderReference) => begin(target, true), [begin]);
  // Only Create's current-head proof may bypass a second identical body fetch.
  const editValidated = useCallback((target: ProviderReaderReference) => begin(target, false), [begin]);
  const entries = useProviderEdits((state) => state.edits);
  const buffers = useEditorStore(state => state.entries);
  const draftKept = Boolean(reference && Object.values(entries).some(edit => edit.tabId === `reader:${tabId}`
    && edit.reference.sourceId === reference.sourceId && edit.reference.path === reference.path
    && buffers[`${edit.tabId} ${edit.key}`]?.dirty));
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
    const next = { ...value.reference, sourceId: value.projection.readerSourceId, path: value.projection.path, revision: value.projection.revision };
    if (!sameReference(receipt(value.reference), receipt(next))) store.navigate(tab.id, tab.entries[tab.index]!.reference, store.selection, {
      reference: receipt(next), title: next.title ?? next.path, heading: null, position: null,
    });
    const selected = useReaderHistoryStore.getState();
    const selectedTab = selected.data.tabs.find(item => item.id === selected.data.activeTabId);
    if (selectedTab?.id === tab.id && sameReference(selectedTab.entries[selectedTab.index]!.reference, receipt(next)))
      useProviderEdits.getState().update(key, { reference: next });
    // Retain the mounted editor and its view/cursor after verified publication.

  }, []);
  return { edit, editValidated, opening, canEdit, activeKey, openDraft, published, draftKept, read: () => { if (displayKey) published(displayKey, useReaderHistoryStore.getState().selection); setDisplayKey(null); } };
}
