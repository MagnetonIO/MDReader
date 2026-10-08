import { useCallback, useEffect, useRef, useState } from 'react';
import { readDoc } from '../../platform/fs';
import { useEditorStore } from '../../stores/editor';
import { useTabsStore, selectActiveTab } from '../../stores/tabs';
import { useSourcesStore } from '../../stores/sources';
import { isLocalDocument } from '../../stores/types';
import { editorAdmission } from '../editor-preservation/admission';
import { useReaderHistoryStore } from '../reader-history/store';
import type { CanonicalReaderReference } from '../reader/contracts';
import { resolveLocalEdit } from './edit-local-backend';

export interface CatalogEditResume {
  kind: 'catalog-edit';
  reference: CanonicalReaderReference;
  selection: number;
}
interface Composition {
  beginIntent(): number;
  isIntentCurrent(intent: number): boolean;
  switchWorkspace(root: string, sourceId: string, resume: CatalogEditResume, intent: number): Promise<boolean>;
  openLocal(path: string, title: string): void;
  notify(message: string): void;
}
/** Explicit catalog-to-editor handoff; canonical receipts never reach the
 * ordinary filesystem editor until native membership and actual bytes agree. */
export function useCatalogEditing(composition: Composition) {
  const latest = useRef(composition);
  latest.current = composition;
  const [opening, setOpening] = useState<CanonicalReaderReference | null>(null);
  const issued = useRef<object | null>(null);
  const mounted = useRef(true);
  useEffect(() => {
    mounted.current = true;
    return () => { mounted.current = false; issued.current = null; };
  }, []);
  const edit = useCallback(async (reference: CanonicalReaderReference, existingIntent?: number, expectedSelection?: number) => {
    const config = latest.current;
    const intent = existingIntent ?? config.beginIntent();
    const selection = expectedSelection ?? useReaderHistoryStore.getState().selection;
    const selected = () => mounted.current && latest.current.isIntentCurrent(intent)
      && useReaderHistoryStore.getState().selection === selection;
    if (!selected() || editorAdmission.denied()) return;
    const operation = {};
    issued.current = operation; setOpening(reference);
    const current = () => issued.current === operation && selected();
    try {
      const first = await resolveLocalEdit(reference);
      if (!current()) return;
      if (useSourcesStore.getState().root !== first.root) {
        const activated = await config.switchWorkspace(first.root, first.librarySourceId,
          { kind: 'catalog-edit', reference, selection }, intent);
        if (!activated || !current()) return;
      }
      // Re-resolve after activation or a guard continuation. A previous result
      // is neither a folder grant nor proof that this exact file is current.
      const exact = await resolveLocalEdit(reference);
      if (!current()) return;
      if (exact.path !== first.path || exact.root !== first.root || exact.librarySourceId !== first.librarySourceId
        || useSourcesStore.getState().root !== exact.root) throw new Error('The selected source changed before editing.');
      const disk = await readDoc(exact.path);
      if (!current() || editorAdmission.denied()) return;
      if (disk.hash !== reference.revision) throw new Error('This file changed since you opened it. Reload the source before editing.');
      config.openLocal(exact.path, reference.title || reference.path);
      if (!latest.current.isIntentCurrent(intent)) return;
      const tab = selectActiveTab(useTabsStore.getState());
      if (!tab || !isLocalDocument(tab.docRef) || tab.docRef.path !== exact.path) return;
      const editor = useEditorStore.getState();
      // Focusing an existing dirty editor must retain its buffer and guard.
      if (editor.entry(tab.id, exact.path)?.dirty) editor.setMode(tab.id, exact.path, 'source');
      else editor.seed(tab.id, exact.path, disk.content, disk.hash, 'source');
    } catch (cause) {
      if (current()) config.notify(cause instanceof Error ? cause.message : 'The selected document is unavailable for editing.');
    } finally {
      if (issued.current === operation) { issued.current = null; setOpening(null); }
    }
  }, []);
  return { edit, opening };
}
