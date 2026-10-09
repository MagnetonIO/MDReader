import './editor-toolbar.css';

export function EditorToolbar({ title, dirty, saving, pending = false, saveDisabled,
  saveLabel, readLabel = 'Read', status, onRead, onSave, onDownload }: {
  title: string; dirty: boolean; saving: boolean; pending?: boolean; saveDisabled: boolean;
  saveLabel: string; readLabel?: string; status?: string; onRead(): void; onSave(): void; onDownload(): void;
}) {
  return <header className="editor-toolbar">
    <div className="editor-toolbar__document">
      <strong title={title}>{title}</strong>
      <span className="editor-toolbar__status" role="status">
        {status ?? (saving ? 'Saving…' : pending ? 'Save unconfirmed' : dirty ? 'Unsaved changes' : 'Saved')}
      </span>
    </div>
    <div className="editor-toolbar__actions" role="group" aria-label="Markdown editing actions">
      <button type="button" aria-label={readLabel} title="Return to reading; your draft is kept" onClick={onRead}>
        <svg viewBox="0 0 24 24" aria-hidden="true"><path d="M12 5v15m0-15C8 2 4 3 2 4v15c4-2 7-1 10 1 3-2 6-3 10-1V4c-2-1-6-2-10 1Z" /></svg>Read
      </button>
      <button type="button" onClick={onDownload}>
        <svg viewBox="0 0 24 24" aria-hidden="true"><path d="M12 3v12m-4-4 4 4 4-4M5 16v4h14v-4" /></svg>Download draft
      </button>
      <button type="button" className="editor-toolbar__save" disabled={saveDisabled} onClick={onSave}>
        <svg viewBox="0 0 24 24" aria-hidden="true"><path d="m4 3 13 0 3 3v15H4V3Zm3 0v6h10V3M8 21v-7h8v7" /></svg>{saveLabel}
      </button>
    </div>
  </header>;
}
