import { useRef, useState } from 'react';
import { useReaderHistoryStore } from '../reader-history/store';
import { SourcePane, type SourcePaneCommands } from '../../shell/SourcePane';
import { useEditorStore } from '../../stores/editor';
import { inspectProviderEdit, providerTabSaving, saveProviderEdit, useProviderEdits } from './store';
import './provider-editing.css';
import { EditorToolbar } from '../editor-controls/EditorToolbar';
import { useDraftCopy } from '../editor-controls/useDraftCopy';
import { documentFilename } from '../document-transfer/filename';

import { useEditorView } from '../editor-controls/useEditorView';
import { DraftPreview } from '../editor-controls/DraftPreview';
import { useEditorExit } from '../editor-controls/useEditorExit';
import { EditorExitNotice } from '../editor-controls/EditorExitNotice';

export function ProviderEditor({ editKey, onRead, onPublished }: {
  editKey: string;
  onRead(): void;
  onPublished(selection: number): void;
}) {
  const edit = useProviderEdits((state) => state.edits[editKey]);
  const entry = useEditorStore((state) => edit ? state.entry(edit.tabId, editKey) : undefined);
  const [busy, setBusy] = useState(false);
  const copy = useDraftCopy(editKey);
  const commandsRef = useRef<SourcePaneCommands | null>(null);
  const { view, narrow } = useEditorView(entry?.mode ?? 'source');
  const exit = useEditorExit(edit?.tabId ?? null, editKey, onRead);
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
    const selection = useReaderHistoryStore.getState().selection;
    setBusy(true);
    try {
      await inspectProviderEdit(editKey, repair);
      if (useProviderEdits.getState().edits[editKey]?.phase === 'published') onPublished(selection);
    }
    finally { setBusy(false); }
  }
  return <section className="provider-editor" aria-label="Guarded cloud editor">
    <EditorToolbar mode={view} narrow={narrow} commandsRef={commandsRef} onSetMode={mode => useEditorStore.getState().setMode(edit.tabId, editKey, mode)} title={edit.reference.title || 'Selected cloud document'} dirty={entry.dirty}
      saving={inFlight} pending={attention}
      status={inFlight ? 'Saving…' : saved ? 'Saved' : attention ? 'Save needs attention' : entry.dirty ? 'Unsaved changes' : 'No unsaved changes'}
      saveDisabled={inFlight || (!ready && !published) || !entry.dirty} saveLabel={inFlight ? 'Saving…' : 'Save changes'}
      leaving={exit.busy} onRead={() => void exit.done()} onSave={() => void save()} copying={copy.busy} onSaveCopy={() => void copy.save(entry.buffer, documentFilename(edit.reference))} />
    <EditorExitNotice error={exit.error} retry={() => void exit.done()} copying={copy.busy} saveCopy={() => void copy.save(entry.buffer, documentFilename(edit.reference))} />
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
    <div className={`editor-views editor-views--${view}`}>
    <div className="editor-views__source" hidden={view === 'preview'} inert={inFlight || !ready && !published}>
      <SourcePane documentKey={`${edit.tabId} ${editKey}`} commandsRef={commandsRef} visible={view !== 'preview'} value={entry.buffer} onChange={(value) => useEditorStore.getState().edit(edit.tabId, editKey, value)}
        onSave={() => void save()} onToggleMode={() => void exit.done()} />
    </div>
    {view === 'preview' || view === 'split' ? <DraftPreview identity={editKey} markdown={entry.buffer} /> : null}
    </div>
  </section>;
}
