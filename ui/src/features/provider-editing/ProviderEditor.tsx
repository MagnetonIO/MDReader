import { useState } from 'react';
import { useReaderHistoryStore } from '../reader-history/store';
import { SourcePane } from '../../shell/SourcePane';
import { useEditorStore } from '../../stores/editor';
import { inspectProviderEdit, providerTabSaving, saveProviderEdit, useProviderEdits } from './store';
import './provider-editing.css';
import { EditorToolbar } from '../editor-controls/EditorToolbar';
import { useDraftCopy } from '../editor-controls/useDraftCopy';
import { documentFilename } from '../document-transfer/filename';

export function ProviderEditor({ editKey, onRead, onPublished }: {
  editKey: string;
  onRead(): void;
  onPublished(selection: number): void;
}) {
  const edit = useProviderEdits((state) => state.edits[editKey]);
  const entry = useEditorStore((state) => edit ? state.entry(edit.tabId, editKey) : undefined);
  const [busy, setBusy] = useState(false);
  const copy = useDraftCopy(editKey);
  if (!edit || !entry) return <p role="alert">The provider editor is unavailable. No save was attempted.</p>;
  const ready = edit.phase === 'ready';
  const inFlight = busy || providerTabSaving(edit.tabId);
  const published = edit.phase === 'published' && edit.projection !== null;
  const saved = published && !entry.dirty;
  const attention = !ready && !published && !inFlight;
  async function save(prepared = false) {
    if (busy) return;
    const selection = useReaderHistoryStore.getState().selection;
    setBusy(true);
    try { if (await saveProviderEdit(editKey, prepared) === 'saved') onPublished(selection); }
    finally { setBusy(false); }
  }
  async function inspect(repair = false) {
    if (busy) return;
    setBusy(true);
    try { await inspectProviderEdit(editKey, repair); }
    finally { setBusy(false); }
  }
  return <section className="provider-editor" aria-label="Guarded cloud editor">
    <EditorToolbar title={edit.reference.title || 'Selected cloud document'} dirty={entry.dirty}
      saving={inFlight} pending={attention}
      status={inFlight ? 'Saving…' : saved ? 'Saved' : attention ? 'Save needs attention' : entry.dirty ? 'Unsaved changes' : 'No unsaved changes'}
      saveDisabled={inFlight || !ready || !entry.dirty} saveLabel={inFlight ? 'Saving…' : 'Save changes'}
      readLabel="Read original revision" onRead={onRead} onSave={() => void save()} copying={copy.busy} onSaveCopy={() => void copy.save(entry.buffer, documentFilename(edit.reference))} />
    {copy.notice ? <p role="status">{copy.notice}</p> : null}
    {copy.error ? <p role="alert">{copy.error}</p> : null}
    {!inFlight && (attention || edit.error) ? <section className="provider-editor__save-state" aria-label="Save needs attention">
      <p role="status">{edit.phase === 'conflict'
        ? 'The cloud document changed. Your draft is kept; save a copy before opening the current cloud version.'
        : edit.phase === 'prepared' ? 'Your draft is ready to save.'
        : 'The save could not be confirmed. Your draft is kept.'}</p>
      {!ready ? <>
        <button type="button" onClick={() => void inspect()}>Check save status</button>
        <button type="button" onClick={() => void inspect(true)}>Check cloud copy</button>
      </> : null}
      {edit.phase === 'prepared' ? <button type="button" onClick={() => void save(true)}>Continue save</button> : null}
      <details><summary>Save details</summary>
        <p>State: {edit.phase.replaceAll('_', ' ')}. Operation {edit.ticket.editId}</p>
        {edit.error ? <p>{edit.error}</p> : null}
      </details>
    </section> : null}
    {!inFlight && saved ? <div className="provider-editor__saved">
      <button type="button" onClick={() => onPublished(useReaderHistoryStore.getState().selection)}>Read saved revision</button>
    </div> : null}
    <div className="provider-editor__source" inert={inFlight || !ready}>
      <SourcePane value={entry.buffer} onChange={(value) => useEditorStore.getState().edit(edit.tabId, editKey, value)}
        onSave={() => void save()} onToggleMode={onRead} />
    </div>
  </section>;
}
