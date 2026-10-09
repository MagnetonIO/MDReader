import { useState } from 'react';
import { useReaderHistoryStore } from '../reader-history/store';
import { SourcePane } from '../../shell/SourcePane';
import { useEditorStore } from '../../stores/editor';
import { inspectProviderEdit, saveProviderEdit, useProviderEdits } from './store';
import './provider-editing.css';
import { EditorToolbar } from '../editor-controls/EditorToolbar';
import { documentFilename } from '../document-transfer/filename';

export function ProviderEditor({ editKey, onRead, onPublished }: {
  editKey: string;
  onRead(): void;
  onPublished(selection: number): void;
}) {
  const edit = useProviderEdits((state) => state.edits[editKey]);
  const entry = useEditorStore((state) => edit ? state.entry(edit.tabId, editKey) : undefined);
  const [busy, setBusy] = useState(false);
  if (!edit || !entry) return <p role="alert">The provider editor is unavailable. No save was attempted.</p>;
  const ready = edit.phase === 'ready';
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
  function download() {
    if (!entry) return;
    const url = URL.createObjectURL(new Blob([entry.buffer], { type: 'text/markdown;charset=utf-8' }));
    const anchor = document.createElement('a');
    anchor.href = url; anchor.download = documentFilename(edit.reference); anchor.click();
    window.setTimeout(() => URL.revokeObjectURL(url), 0);
  }
  return <section className="provider-editor" aria-label="Guarded cloud editor">
    <EditorToolbar title={edit.reference.title || 'Selected cloud document'} dirty={entry.dirty}
      saving={busy} pending={!ready && edit.phase !== 'published'}
      status={busy ? 'Working…' : !ready ? `Save ${edit.phase.replaceAll('_', ' ')}` : entry.dirty ? 'Unsaved changes' : 'No unsaved changes'}
      saveDisabled={busy || !ready || !entry.dirty} saveLabel={busy ? 'Saving…' : 'Save to provider'}
      readLabel="Read original revision" onRead={onRead} onSave={() => void save()} onDownload={download} />
    <p>Edits apply only to this selected provider document. A changed remote revision is refused; MDReader never overwrites it automatically.</p>
    {edit.error ? <p role="alert">{edit.error}</p> : null}
    {busy ? <p role="status">Waiting for the native provider operation…</p> : null}
    {!ready ? <section className="provider-editor__save-state" aria-label="Provider save state">
      <p>Save state: {edit.phase.replaceAll('_', ' ')}. Operation {edit.ticket.editId}</p>
      <button type="button" disabled={busy} onClick={() => void inspect()}>Inspect retained save</button>
      <button type="button" disabled={busy} onClick={() => void inspect(true)}>Reconcile with provider</button>
      {edit.phase === 'prepared' ? <button type="button" disabled={busy} onClick={() => void save(true)}>Send prepared save</button> : null}
      {edit.phase === 'published' && edit.projection && !entry.dirty ? <button type="button" onClick={() => onPublished(useReaderHistoryStore.getState().selection)}>Read saved revision</button> : null}
      {edit.phase === 'conflict' ? <p>Your buffer is kept. Download it, then explicitly fetch the current provider document to compare and reopen editing.</p> : null}
    </section> : null}
    <div className="provider-editor__source" inert={busy || !ready}>
      <SourcePane value={entry.buffer} onChange={(value) => useEditorStore.getState().edit(edit.tabId, editKey, value)}
        onSave={() => void save()} onToggleMode={onRead} />
    </div>
  </section>;
}
