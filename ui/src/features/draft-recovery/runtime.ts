import { useEffect } from 'react';
import { useEditorStore, type EditorEntry } from '../../stores/editor';
import { useTabsStore } from '../../stores/tabs';
import { useSourcesStore } from '../../stores/sources';
import { useProviderEdits } from '../provider-editing/store';
import { useDeviceEdits } from '../device-editing/store';
import { editorAdmission } from '../editor-preservation/admission';
import { isTauri } from '../../shell/persistence';
import { draftBackend, type DraftSnapshot } from './backend';
import { DraftJournal } from './journal';

export const draftJournal = new DraftJournal(draftBackend);
export function snapshotFor(entry: EditorEntry): DraftSnapshot | null {
  const provider = useProviderEdits.getState().edits[entry.path];
  const device = useDeviceEdits.getState().edits[entry.path];
  if (provider) return { key: { kind: 'provider', source_id: provider.reference.sourceId, path: provider.reference.path },
    base_revision: provider.projection?.revision ?? provider.reference.revision, base_hash: null,
    title: provider.reference.title ?? provider.reference.path.split('/').pop() ?? 'Untitled.md', markdown: entry.buffer, dirty: entry.dirty };
  if (device) return { key: { kind: 'device', document_id: device.reference.documentId },
    base_revision: device.reference.revision, base_hash: null, title: device.reference.title ?? 'Untitled.md', markdown: entry.buffer, dirty: entry.dirty };
  // Renderer operation keys must never be treated as local filesystem paths.
  if (entry.path.startsWith('cloud:') || entry.path.startsWith('device:')) return null;
  const tab = useTabsStore.getState().tabs.find((tab) => tab.id === entry.tabId);
  const root = tab?.originRoot ?? useSourcesStore.getState().root;
  if (!root || !tab) return null;
  return { key: { kind: 'local', source_id: root, path: entry.path }, base_revision: entry.baseHash,
    base_hash: /^[a-f0-9]{64}$/.test(entry.baseHash) ? entry.baseHash : null, title: entry.path.split(/[/\\]/).pop() ?? 'Untitled.md', markdown: entry.buffer, dirty: entry.dirty };
}
function update() {
  draftJournal.update(Object.values(useEditorStore.getState().entries).flatMap((entry) => {
    const snapshot = snapshotFor(entry); return snapshot ? [snapshot] : [];
  }));
}
export async function flushEditorDrafts(required?: EditorEntry): Promise<void> {
  if (!isTauri()) return;
  if (editorAdmission.denied()) throw new Error('Editing is paused. Your draft is kept in the current editor.');
  const snapshot = required ? snapshotFor(required) : undefined;
  if (required?.dirty && !snapshot) throw new Error('This document has no available draft recovery location.');
  update(); await draftJournal.flush(snapshot ?? undefined);
}
/** Chosen editors only. No scanning, indexing, source admission or automatic saves. */
export function useDraftRecovery(ready: boolean): void {
  useEffect(() => {
    if (!ready || !isTauri()) return;
    let timer: ReturnType<typeof setTimeout> | undefined;
    const changed = () => {
      clearTimeout(timer);
      update();
      if (editorAdmission.denied()) return;
      timer = setTimeout(() => { void flushEditorDrafts().catch(() => {}); }, 1000);
    };
    const off = [useEditorStore.subscribe(changed), useProviderEdits.subscribe(changed), useDeviceEdits.subscribe(changed),
      useTabsStore.subscribe(changed), editorAdmission.subscribe(changed)];
    changed();
    return () => { clearTimeout(timer); for (const unsubscribe of off) unsubscribe(); };
  }, [ready]);
}

/** Hold editor mutation while draining its user-approved discard/save cleanup.
 * The caller must recheck its close intent after this await before removing UI. */
export async function retireEditorDrafts(entries: EditorEntry[]): Promise<void> {
  if (!isTauri()) return;
  const hold = editorAdmission.hold();
  if (!hold) throw new Error('Editing is paused. Your tabs and drafts were kept.');
  try {
    await hold.drain();
    const snapshots = entries.flatMap((entry) => { const snapshot = snapshotFor(entry); return snapshot ? [snapshot] : []; });
    await draftJournal.retire(snapshots);
  } finally { hold.release(); }
}

export function resumeEditorDrafts(entries: EditorEntry[]): void {
  draftJournal.resume(entries.flatMap((entry) => { const snapshot = snapshotFor(entry); return snapshot ? [snapshot] : []; }));
}
