import { create } from 'zustand';
import type { ProviderProjection, ProviderReaderReference } from '../../platform/providers';
import { useEditorStore } from '../../stores/editor';
import { editorAdmission } from '../editor-preservation/admission';
import { providerEditBackend, type ProviderEditTicket, type SaveState } from './backend';

export type EditPhase = 'ready' | 'preparing' | 'dispatched' | 'uncertain' | SaveState;
export interface ProviderEdit {
  key: string; tabId: string; reference: ProviderReaderReference; ticket: ProviderEditTicket;
  phase: EditPhase; pendingBody: string | null; projection: ProviderProjection | null; error: string | null;
}
interface ProviderEdits {
  edits: Record<string, ProviderEdit>;
  update(key: string, values: Partial<ProviderEdit>): void;
}
export const useProviderEdits = create<ProviderEdits>((set) => ({
  edits: {},
  update(key, values) {
    set((state) => state.edits[key] ? { edits: { ...state.edits, [key]: { ...state.edits[key]!, ...values } } } : {});
  },
}));
export function findProviderEdit(tabId: string, reference: ProviderReaderReference): ProviderEdit | undefined {
  return Object.values(useProviderEdits.getState().edits).find((edit) => edit.tabId === tabId
    && edit.reference.sourceId === reference.sourceId && edit.reference.path === reference.path && edit.reference.revision === reference.revision);
}
/** Completed clean publication is not a draft; newer text and unresolved saves remain protected. */
export function protectedProviderEdit(edit: ProviderEdit): boolean {
  return useEditorStore.getState().entry(edit.tabId, edit.key)?.dirty === true
    || !['ready', 'published'].includes(edit.phase) || edit.phase === 'published' && edit.projection === null;
}
export function retainedProviderEntry(tabId: string, key: string): boolean {
  return useProviderEdits.getState().edits[key]?.tabId === tabId;
}
function message(cause: unknown): string { return cause instanceof Error ? cause.message : 'The save could not be confirmed. Your buffer was kept.'; }
const activeTasks = new Set<string>();
export function providerTabSaving(tabId: string): boolean {
  return Object.values(useProviderEdits.getState().edits).some((edit) => edit.tabId === tabId && activeTasks.has(edit.key));
}
function patch(key: string, values: Partial<ProviderEdit>) { useProviderEdits.getState().update(key, values); }

export async function beginProviderEdit(tabId: string, reference: ProviderReaderReference, current: () => boolean): Promise<string | null> {
  const existing = findProviderEdit(tabId, reference);
  if (existing) return existing.key;
  if (editorAdmission.denied()) throw new Error('Editing is currently paused.');
  // Completed, clean editors may be replaced after an explicit new Edit.
  const retired = new Set(Object.values(useProviderEdits.getState().edits).filter((edit) =>
    edit.phase === 'published' && !activeTasks.has(edit.key)
    && useEditorStore.getState().entry(edit.tabId, edit.key)?.dirty === false).map((edit) => edit.key));
  useEditorStore.getState().retain((_tabId, path) => !retired.has(path));
  useProviderEdits.setState((state) => ({ edits: Object.fromEntries(Object.entries(state.edits)
    .filter(([key, edit]) => !retired.has(key) || useEditorStore.getState().entry(edit.tabId, key))) }));
  if (Object.keys(useProviderEdits.getState().edits).length >= 32) throw new Error('Close a provider editor before opening another. Your existing buffers have been kept.');
  const ticket = await providerEditBackend.begin(reference);
  if (!current() || editorAdmission.denied()) return null;
  const key = `cloud:mdreader-provider:${ticket.editId}`;
  useEditorStore.getState().seed(tabId, key, ticket.markdown, ticket.editId, 'source');
  if (!useEditorStore.getState().entry(tabId, key)) throw new Error('Editing is currently paused.');
  useProviderEdits.setState((state) => ({ edits: { ...state.edits, [key]: {
    key, tabId, reference, ticket, phase: 'ready', pendingBody: null, projection: null, error: null,
  } } }));
  return key;
}
export function forgetProviderTab(tabId: string): void {
  // The normal close/quit guard decides whether dirty data may be discarded.
  // Never remove association while the editor owner refused buffer removal.
  useProviderEdits.setState((state) => ({ edits: Object.fromEntries(Object.entries(state.edits)
    .filter(([key, edit]) => edit.tabId !== tabId || useEditorStore.getState().entry(tabId, key))) }));
}

/** Real native write tasks retain an editor save lease through their finally,
 * including lost replies. Unknown outcomes never trigger an automatic PUT. */
export async function saveProviderEdit(key: string, dispatchPrepared = false): Promise<'saved' | 'error' | 'noop'> {
  const edit = useProviderEdits.getState().edits[key];
  if (!edit || activeTasks.has(key)) return 'error';
  const entry = useEditorStore.getState().entry(edit.tabId, key);
  if (!entry?.dirty) return 'noop';
  if (dispatchPrepared ? edit.phase !== 'prepared' : edit.phase !== 'ready') {
    patch(key, { error: 'Inspect or reconcile the retained save before attempting another write.' }); return 'error';
  }
  if (!dispatchPrepared && Date.now() >= edit.ticket.expiresAtMs) {
    patch(key, { error: 'This edit ticket expired. Save a copy of your buffer, then explicitly open a fresh provider revision.' }); return 'error';
  }
  const lease = useEditorStore.getState().beginSave(edit.tabId, key);
  if (!lease) { patch(key, { error: 'Editing is paused or this document is already saving.' }); return 'error'; }
  const snapshot = dispatchPrepared ? edit.pendingBody : lease.entry.buffer;
  if (snapshot === null) { lease.finish(); patch(key, { error: 'The prepared body is unavailable. Your buffer was kept.' }); return 'error'; }
  activeTasks.add(key);
  patch(key, { phase: dispatchPrepared ? 'dispatched' : 'preparing', pendingBody: snapshot, error: null });
  try {
    if (!dispatchPrepared) await providerEditBackend.prepare(edit.ticket.writeSourceId, edit.ticket.editId, snapshot);
    patch(key, { phase: 'dispatched' });
    const projection = await providerEditBackend.dispatch(edit.ticket.writeSourceId, edit.ticket.editId, edit.reference);
    // A prepared retry always settles the originally prepared bytes, not edits
    // made after the first attempt. Preserve such a newer buffer as dirty.
    const settled = snapshot === lease.entry.buffer && lease.saved(projection.revision);
    patch(key, { phase: 'published', projection, error: null });
    return settled && useEditorStore.getState().entry(edit.tabId, key)?.dirty === false ? 'saved' : 'error';
  } catch (cause) {
    patch(key, { phase: 'uncertain', error: message(cause) }); return 'error';
  } finally { activeTasks.delete(key); lease.finish(); }
}
export async function inspectProviderEdit(key: string, repair = false): Promise<void> {
  const edit = useProviderEdits.getState().edits[key];
  if (!edit || activeTasks.has(key)) return;
  const lease = useEditorStore.getState().beginSave(edit.tabId, key);
  if (!lease) { patch(key, { error: 'Editing is paused or this document is already saving.' }); return; }
  activeTasks.add(key);
  patch(key, { error: null });
  try {
    if (repair) await providerEditBackend.repair(edit.ticket.writeSourceId);
    const value = await providerEditBackend.inspect(edit.ticket.writeSourceId, edit.ticket.editId, edit.reference);
    if (value.state === 'published' && value.projection && edit.pendingBody !== null) {
      if (lease.entry.buffer === edit.pendingBody) lease.saved(value.projection.revision);
    }
    patch(key, { phase: value.state === 'absent' ? 'ready' : value.state, projection: value.projection,
      error: value.state === 'published' && !value.projection ? 'The remote save is recorded, but its exact local revision is unavailable. Your buffer was kept.' : null });
  } catch (cause) { patch(key, { error: message(cause) }); }
  finally { activeTasks.delete(key); lease.finish(); }
}
