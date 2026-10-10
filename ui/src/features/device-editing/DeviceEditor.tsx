import { useState } from 'react';
import { SourcePane } from '../../shell/SourcePane';
import { useEditorStore } from '../../stores/editor';
import { useReaderHistoryStore } from '../reader-history/store';
import { saveDeviceEdit, useDeviceEdits } from './store';
import './device-editing.css';
import { EditorToolbar } from '../editor-controls/EditorToolbar';
import { useDraftCopy } from '../editor-controls/useDraftCopy';
import { documentFilename } from '../document-transfer/filename';

export function DeviceEditor({ editKey, onRead, onPublished }: {
  editKey: string; onRead(): void; onPublished(selection: number): void;
}) {
  const edit = useDeviceEdits((state) => state.edits[editKey]);
  const entry = useEditorStore((state) => edit ? state.entry(edit.tabId, editKey) : undefined);
  const [busy, setBusy] = useState(false);
  const copy = useDraftCopy(editKey);
  if (!edit || !entry) return <p role="alert">This Markdown draft is unavailable. No save was attempted.</p>;
  async function save() {
    if (busy || edit?.saving) return;
    const selection = useReaderHistoryStore.getState().selection;
    setBusy(true);
    try { if (await saveDeviceEdit(editKey) === 'saved') onPublished(selection); }
    finally { setBusy(false); }
  }
  return <section className="device-editor" aria-label="Markdown editor">
    <EditorToolbar title={edit.reference.title || 'Markdown copy'} dirty={entry.dirty}
      saving={busy || edit.saving} pending={Boolean(edit.pending)}
      saveDisabled={busy || edit.saving || !entry.dirty && !edit.pending}
      saveLabel={busy || edit.saving ? 'Saving…' : edit.pending ? 'Check saved edit' : 'Save changes'}
      onRead={onRead} onSave={() => void save()} copying={copy.busy} onSaveCopy={() => void copy.save(entry.buffer, documentFilename(edit.reference))} />
    <p>Edits change this library’s Markdown copy. Original files and import archives stay unchanged.</p>
    {copy.notice ? <p role="status">{copy.notice}</p> : null}
    {copy.error ? <p role="alert">{copy.error}</p> : null}
    {edit.error ? <p role="alert">{edit.error}</p> : null}
    {edit.pending && !busy && !edit.saving ? <p role="status">The previous save is unconfirmed. Check saved edit retries that exact edit; any newer typing is kept.</p> : null}
    <div className="device-editor__source">
      <SourcePane value={entry.buffer} onChange={(value) => useEditorStore.getState().edit(edit.tabId, editKey, value)}
        onSave={() => void save()} onToggleMode={onRead} />
    </div>
  </section>;
}
