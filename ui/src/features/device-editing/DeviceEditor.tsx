import { useState } from 'react';
import { SourcePane } from '../../shell/SourcePane';
import { useEditorStore } from '../../stores/editor';
import { useReaderHistoryStore } from '../reader-history/store';
import { saveDeviceEdit, useDeviceEdits } from './store';
import './device-editing.css';

export function DeviceEditor({ editKey, onRead, onPublished }: {
  editKey: string; onRead(): void; onPublished(selection: number): void;
}) {
  const edit = useDeviceEdits((state) => state.edits[editKey]);
  const entry = useEditorStore((state) => edit ? state.entry(edit.tabId, editKey) : undefined);
  const [busy, setBusy] = useState(false);
  if (!edit || !entry) return <p role="alert">This Markdown draft is unavailable. No save was attempted.</p>;
  async function save() {
    if (busy || edit?.saving) return;
    const selection = useReaderHistoryStore.getState().selection;
    setBusy(true);
    try { if (await saveDeviceEdit(editKey) === 'saved') onPublished(selection); }
    finally { setBusy(false); }
  }
  function download() {
    if (!edit || !entry) return;
    const url = URL.createObjectURL(new Blob([entry.buffer], { type: 'text/markdown;charset=utf-8' }));
    const anchor = document.createElement('a');
    anchor.href = url; anchor.download = edit.reference.title || 'markdown-draft.md'; anchor.click();
    window.setTimeout(() => URL.revokeObjectURL(url), 0);
  }
  return <section className="device-editor" aria-label="Markdown editor">
    <header className="device-editor__bar">
      <strong>{edit.reference.title || 'Markdown copy'}</strong>
      <span>{entry.dirty ? 'Unsaved changes' : 'Saved'}</span>
      <button type="button" onClick={onRead}>Read</button>
      <button type="button" disabled={busy || edit.saving || !entry.dirty && !edit.pending} onClick={() => void save()}>
        {busy || edit.saving ? 'Saving…' : edit.pending ? 'Check saved edit' : 'Save changes'}
      </button>
      <button type="button" onClick={download}>Download draft</button>
    </header>
    <p>Edits change this library’s Markdown copy. Original files and import archives stay unchanged.</p>
    {edit.error ? <p role="alert">{edit.error}</p> : null}
    {edit.pending && !busy && !edit.saving ? <p role="status">The previous save is unconfirmed. Check saved edit retries that exact edit; any newer typing is kept.</p> : null}
    <div className="device-editor__source">
      <SourcePane value={entry.buffer} onChange={(value) => useEditorStore.getState().edit(edit.tabId, editKey, value)}
        onSave={() => void save()} onToggleMode={onRead} />
    </div>
  </section>;
}
