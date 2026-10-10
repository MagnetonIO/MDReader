import { useEffect, useRef } from 'react';
import { nativeProvidersBackend, type NativeProvidersBackend } from '../../platform/providers';
import { isTauri } from '../../shell/persistence';
import { useEditorStore } from '../../stores/editor';
import type { CanonicalReaderReference } from '../reader/contracts';
import { sameReference } from '../reader-history/model';
import { receipt } from '../reader-history/references';
import { useReaderHistoryStore } from '../reader-history/store';
import { protectedProviderEdit, useProviderEdits } from './store';

type Backend = Pick<NativeProvidersBackend, 'readCurrentDocument'>;
/** Reopening is a local admitted-head read, never an exact-read fallback or provider fetch. */
export function useCurrentProviderReopen(reference: CanonicalReaderReference | null, tabId: string | undefined,
  notify: (message: string) => void, backend: Backend = nativeProvidersBackend, enabled = isTauri()) {
  const selection = useReaderHistoryStore(state => state.selection);
  const intent = useReaderHistoryStore(state => state.selectionIntent);
  const selected = useRef({ reference, tabId, backend, enabled }); selected.current = { reference, tabId, backend, enabled };
  const notification = useRef(notify); notification.current = notify;
  useEffect(() => {
    if (!enabled || intent !== 'reopen' || !reference || reference.documentId || !tabId || !reference.sourceId.startsWith('provider:') || !backend.readCurrentDocument) return;
    const expected = receipt(reference);
    let active = true;
    const protectedTab = () => useEditorStore.getState().tabIsDirty(`reader:${tabId}`)
      || Object.values(useProviderEdits.getState().edits).some(edit => edit.tabId === `reader:${tabId}` && protectedProviderEdit(edit));
    const current = () => {
      const state = useReaderHistoryStore.getState();
      const tab = state.data.tabs.find(value => value.id === state.data.activeTabId);
      return active && selected.current.reference === reference && selected.current.tabId === tabId
        && selected.current.backend === backend && selected.current.enabled && state.selection === selection
        && tab?.id === tabId && sameReference(tab.entries[tab.index]!.reference, expected) && !protectedTab();
    };
    if (protectedTab()) return;
    void backend.readCurrentDocument(reference).then(body => {
      if (!current()) return;
      if (body.operation !== 'read_document' || body.sourceKind !== undefined && body.sourceKind !== 'provider'
        || body.sourceId !== reference.sourceId || body.path !== reference.path || typeof body.markdown !== 'string'
        || !Number.isSafeInteger(body.lineStart) || body.lineStart < 1 || !Number.isSafeInteger(body.lineEnd) || body.lineEnd < body.lineStart)
        throw new Error('The current cached document could not be verified.');
      const latest = receipt({ ...reference, revision: body.revision });
      if (sameReference(expected, latest)) return;
      // Keep the old entry and its revision-bound position. The new head gets no old placement.
      const store = useReaderHistoryStore.getState();
      const tab = store.data.tabs.find(value => value.id === tabId)!;
      const entry = tab.entries[tab.index]!;
      store.navigate(tabId, expected, selection, { reference: latest, title: entry.title, heading: null, position: null });
    }).catch(() => {
      if (current()) notification.current('The current cached copy could not be opened. Refresh this connection from Sources.');
    });
    return () => { active = false; };
  }, [reference, tabId, selection, intent, backend, enabled]);
}
