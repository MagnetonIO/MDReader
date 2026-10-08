import type { ProviderReaderReference } from '../../platform/providers';
import { useEditorStore } from '../../stores/editor';
import { useProviderEdits } from './store';

/** Navigation can leave a draft; the same exact receipt remains explicitly
 * reachable without granting a folder or silently replacing its base version. */
export function ProviderEditsTray({ onOpen }: { onOpen(reference: ProviderReaderReference, tabId: string): void }) {
  const edits = useProviderEdits((state) => state.edits);
  const entries = useEditorStore((state) => state.entries);
  const retained = Object.values(edits).filter((edit) => {
    const entry = entries[`${edit.tabId} ${edit.key}`];
    return entry && (entry.dirty || !['ready', 'published'].includes(edit.phase));
  });
  if (!retained.length) return null;
  return <nav className="provider-edit-drafts" aria-label="Retained cloud edits">
    <h3>Cloud drafts</h3>
    {retained.map((edit) => <button type="button" key={edit.key} onClick={() => onOpen(edit.reference, edit.tabId)}>
      {edit.reference.title || 'Cloud document'} · {edit.phase === 'ready' ? 'unsaved' : edit.phase}
    </button>)}
  </nav>;
}
