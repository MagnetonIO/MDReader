import { create } from 'zustand';
import { useEditorStore } from '../../stores/editor';
import { editorAdmission } from '../editor-preservation/admission';
import { deviceReaderBackend, type DeviceReaderBackend, type DeviceReference } from '../device-reader/backend';
import { notifyDeviceReadChanged } from '../device-reader/changed';

interface PendingSave { operationId: string; reference: DeviceReference; markdown: string }
export interface DeviceEdit {
  key: string; tabId: string; origin: DeviceReference; reference: DeviceReference;
  backend: DeviceReaderBackend; pending: PendingSave | null; saving: boolean; error: string | null;
}
interface DeviceEdits {
  edits: Record<string, DeviceEdit>;
  update(key: string, values: Partial<DeviceEdit>): void;
}
export const useDeviceEdits = create<DeviceEdits>((set) => ({
  edits: {},
  update(key, values) {
    set((state) => state.edits[key] ? { edits: { ...state.edits, [key]: { ...state.edits[key]!, ...values } } } : {});
  },
}));
export const sameDevice = (left: DeviceReference, right: DeviceReference) => left.documentId === right.documentId
  && left.sourceId === right.sourceId && left.revision === right.revision;
export function findDeviceEdit(tabId: string, reference: DeviceReference): DeviceEdit | undefined {
  return Object.values(useDeviceEdits.getState().edits).find((edit) => edit.tabId === tabId
    && (sameDevice(edit.origin, reference) || sameDevice(edit.reference, reference)));
}
export function retainedDeviceEntry(tabId: string, key: string): boolean {
  return useDeviceEdits.getState().edits[key]?.tabId === tabId;
}
const activeTasks = new Set<string>();
export function deviceTabSaving(tabId: string): boolean {
  return Object.values(useDeviceEdits.getState().edits).some((edit) => edit.tabId === tabId && (activeTasks.has(edit.key) || edit.pending !== null));
}
export async function beginDeviceEdit(tabId: string, reference: DeviceReference, current: () => boolean,
  backend: DeviceReaderBackend = deviceReaderBackend): Promise<string | null> {
  const existing = findDeviceEdit(tabId, reference);
  if (existing) return existing.key;
  if (!backend.save || editorAdmission.denied()) throw new Error('Editing this copy is currently unavailable.');
  const retired = new Set(Object.values(useDeviceEdits.getState().edits).filter((edit) => !edit.pending
    && !activeTasks.has(edit.key) && useEditorStore.getState().entry(edit.tabId, edit.key)?.dirty === false).map((edit) => edit.key));
  useEditorStore.getState().retain((_tabId, path) => !retired.has(path));
  useDeviceEdits.setState((state) => ({ edits: Object.fromEntries(Object.entries(state.edits)
    .filter(([key, edit]) => !retired.has(key) || useEditorStore.getState().entry(edit.tabId, key))) }));
  if (Object.keys(useDeviceEdits.getState().edits).length >= 32) throw new Error('Close a Markdown editor before opening another. Your drafts were kept.');
  const body = await backend.read(reference);
  if (!current() || editorAdmission.denied()) return null;
  const key = `device:mdreader-edit:${crypto.randomUUID()}`;
  useEditorStore.getState().seed(tabId, key, body.markdown, reference.revision, 'source');
  if (!useEditorStore.getState().entry(tabId, key)) throw new Error('Editing is currently paused.');
  useDeviceEdits.setState((state) => ({ edits: { ...state.edits, [key]: {
    key, tabId, origin: reference, reference, backend, pending: null, saving: false, error: null,
  } } }));
  return key;
}
export function forgetDeviceTab(tabId: string): void {
  useDeviceEdits.setState((state) => ({ edits: Object.fromEntries(Object.entries(state.edits)
    .filter(([key, edit]) => edit.tabId !== tabId || useEditorStore.getState().entry(tabId, key))) }));
}

/** A lost reply retains the exact native operation and bytes. An explicit retry
 * replays them; subsequent typing never becomes a second unconfirmed write. */
export async function saveDeviceEdit(key: string): Promise<'saved' | 'error' | 'noop'> {
  const edit = useDeviceEdits.getState().edits[key];
  if (!edit?.backend.save || activeTasks.has(key)) return 'error';
  const entry = useEditorStore.getState().entry(edit.tabId, key);
  if (!entry) return 'error';
  if (!entry.dirty && !edit.pending) return 'noop';
  if (!edit.pending && new TextEncoder().encode(entry.buffer).length > 1024 * 1024) {
    useDeviceEdits.getState().update(key, { error: 'This Markdown copy exceeds the 1 MiB save limit. Shorten the draft or download it.' });
    return 'error';
  }
  const lease = useEditorStore.getState().beginSave(edit.tabId, key);
  if (!lease) { useDeviceEdits.getState().update(key, { error: 'Editing is paused or this document is already saving.' }); return 'error'; }
  const pending = edit.pending ?? { operationId: crypto.randomUUID(), reference: edit.reference, markdown: lease.entry.buffer };
  activeTasks.add(key);
  useDeviceEdits.getState().update(key, { pending, saving: true, error: null });
  try {
    const next = await edit.backend.save(pending.reference, pending.operationId, pending.markdown);
    // The replay may settle older bytes than the current captured editor lease.
    // Adopt that native revision without declaring newer edits clean.
    const settled = lease.saved(next.revision, pending.markdown);
    useDeviceEdits.getState().update(key, { reference: next, pending: null, error: null });
    notifyDeviceReadChanged({ documentId: next.documentId, sourceId: next.sourceId, revision: next.revision });
    return settled && useEditorStore.getState().entry(edit.tabId, key)?.dirty === false ? 'saved' : 'error';
  } catch (cause) {
    useDeviceEdits.getState().update(key, { error: cause instanceof Error ? cause.message : 'The save could not be confirmed. Your draft was kept.' });
    return 'error';
  } finally { activeTasks.delete(key); useDeviceEdits.getState().update(key, { saving: false }); lease.finish(); }
}
