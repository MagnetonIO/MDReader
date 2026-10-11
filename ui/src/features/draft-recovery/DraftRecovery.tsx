import { useState, useSyncExternalStore } from 'react';
import { useEditorStore } from '../../stores/editor';
import { editorAdmission } from '../editor-preservation/admission';
import { saveDraftCopy } from '../document-transfer/draft-copy';
import { draftKeyId, sameBase } from './backend';
import { draftJournal, snapshotFor } from './runtime';
import './draft-recovery.css';

/** Offered only beside a freshly opened, authorized editor. A changed baseline
 * allows an explicit copy; it can never acquire the original's save authority. */
export function DraftRecovery({ tabId, path }: { tabId: string | null; path: string | null }) {
  const entry = useEditorStore((state) => tabId && path ? state.entry(tabId, path) : undefined);
  const state = useSyncExternalStore(draftJournal.subscribe, draftJournal.getState);
  const snapshot = entry ? snapshotFor(entry) : null;
  const draft = snapshot ? state.drafts.get(draftKeyId(snapshot.key)) : undefined;
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  if (!entry || !snapshot || !draft) return state.error && entry ? <div className="draft-recovery" role="status">{state.error}</div> : null;
  const canRestore = !entry.dirty && sameBase(draft, snapshot);
  const action = async (kind: 'restore' | 'copy' | 'discard') => {
    if (busy || editorAdmission.denied()) return;
    setBusy(true); setError(null);
    try {
      if (kind === 'discard') await draftJournal.discard(draft);
      else {
        const markdown = await draftJournal.read(draft);
        if (kind === 'copy') await saveDraftCopy(markdown, draft.title.endsWith('.md') ? draft.title : `${draft.title}.md`);
        else {
          const live = useEditorStore.getState().entry(entry.tabId, entry.path);
          const current = live ? snapshotFor(live) : null;
          if (!live || live !== entry || live.dirty || !current || !sameBase(draft, current) || editorAdmission.denied())
            throw new Error('The document changed. Save the recovered draft as a copy.');
          useEditorStore.getState().edit(live.tabId, live.path, markdown);
          if (useEditorStore.getState().entry(live.tabId, live.path)?.buffer !== markdown) throw new Error('Editing is paused. The recovered draft is kept.');
          draftJournal.accept(draft);
        }
      }
    } catch (cause) { setError(cause instanceof Error ? cause.message : 'The recovered draft is kept. Try again.'); }
    finally { setBusy(false); }
  };
  return <aside className="draft-recovery" role="status">
    <span>{canRestore ? 'An unsaved draft is available.' : 'A recovered draft is available. Save a copy to keep both versions.'}</span>
    {canRestore ? <button type="button" disabled={busy} onClick={() => void action('restore')}>Restore draft</button> : null}
    <button type="button" disabled={busy} onClick={() => void action('copy')}>Save a copy…</button>
    <button type="button" disabled={busy} onClick={() => void action('discard')}>Discard draft</button>
    {error ? <span role="alert">{error}</span> : null}
  </aside>;
}
