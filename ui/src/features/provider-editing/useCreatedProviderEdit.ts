import { useCallback, useEffect, useRef, useState } from 'react';
import { nativeProvidersBackend, type NativeProvidersBackend, type ProviderReaderReference } from '../../platform/providers';
import { useReaderHistoryStore } from '../reader-history/store';
import { receipt } from '../reader-history/references';
import { sameReference } from '../reader-history/model';
import { currentCreatedDocument } from './currentCreatedDocument';
import { protectedProviderEdit, useProviderEdits } from './store';

type Intent = { reference: ProviderReaderReference; selection: number; ready: boolean };
/** A confirmed Create click carries edit intent only into its exact resulting selection. */
export function useCreatedProviderEdit(reference: ProviderReaderReference | null, canEdit: boolean,
  edit: (target: ProviderReaderReference) => Promise<void>, open: (target: ProviderReaderReference) => void, refresh: () => void,
  notify: (message: string) => void, backend: Pick<NativeProvidersBackend, 'state' | 'documents' | 'fetchCurrent'> = nativeProvidersBackend) {
  const pending = useRef<Intent | null>(null);
  const mounted = useRef(true);
  const [readyEpoch, changed] = useState(0);
  const currentBackend = useRef(backend);
  currentBackend.current = backend;
  const selection = useReaderHistoryStore(state => state.selection);
  if (pending.current && pending.current.selection !== selection) pending.current = null;
  useEffect(() => { mounted.current = true; return () => { mounted.current = false; pending.current = null; }; }, []);
  const created = useCallback((target: ProviderReaderReference) => {
    open(target);
    const selected = () => {
      const state = useReaderHistoryStore.getState();
      const tab = state.data.tabs.find(item => item.id === state.data.activeTabId);
      return { tab, exact: tab?.entries[tab.index]?.reference, selection: state.selection };
    };
    const state = selected();
    const intent: Intent | null = state.exact && sameReference(state.exact, receipt(target))
      ? { reference: target, selection: state.selection, ready: false } : null;
    pending.current = intent;
    refresh();
    if (!intent) return;
    const retainedDraft = () => Object.values(useProviderEdits.getState().edits).some(edit =>
      edit.tabId === `reader:${state.tab?.id}` && edit.reference.sourceId === target.sourceId && edit.reference.path === target.path
      && protectedProviderEdit(edit));
    if (retainedDraft()) { pending.current = null; notify('The existing draft was kept. Open it from its editor tab.'); return; }
    const active = () => {
      const current = selected();
      return mounted.current && currentBackend.current === backend && pending.current === intent && current.selection === intent.selection
        && Boolean(current.exact && sameReference(current.exact, receipt(intent.reference)));
    };
    void currentCreatedDocument(target, backend, active).then(current => {
      if (!current || !active()) return;
      // Only the untouched creation selection is advanced; existing dirty edits are never rewritten.
      if (retainedDraft()) { pending.current = null; notify('The existing draft was kept. Open it from its editor tab.'); return; }
      const prior = selected();
      if (!prior.tab || !prior.exact) return;
      if (!sameReference(prior.exact, receipt(current))) {
        useReaderHistoryStore.getState().navigate(prior.tab.id, prior.exact, prior.selection, {
          reference: receipt(current), title: current.title ?? current.path, heading: null, position: null,
        });
      }
      const next = selected();
      pending.current = next.exact && sameReference(next.exact, receipt(current))
        ? { reference: current, selection: next.selection, ready: true } : null;
      changed(value => value + 1);
    }).catch(cause => {
      if (!active()) return;
      pending.current = null;
      notify(cause instanceof Error ? cause.message : 'The new document’s current copy is unavailable.');
    });
  }, [open, refresh, notify, backend]);
  useEffect(() => {
    const intent = pending.current;
    if (!intent?.ready || !reference || !canEdit || intent.selection !== selection || !sameReference(receipt(intent.reference), receipt(reference))) return;
    pending.current = null;
    void edit(reference);
  }, [reference, canEdit, edit, selection, readyEpoch]);
  return created;
}
