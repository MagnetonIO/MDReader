import { useRef, type RefObject } from 'react';
import type { EditorMode } from '../../stores/editor';
import type { SourcePaneCommands } from '../../shell/SourcePane';
import './editor-toolbar.css';

export function EditorToolbar({ title, dirty, saving, pending = false, saveDisabled,
  saveLabel, readLabel = 'Done', status, copying = false, leaving = false, mode, narrow = false, commandsRef,
  onSetMode, onRead, onSave, onSaveCopy }: {
  title: string; dirty: boolean; saving: boolean; pending?: boolean; saveDisabled: boolean;
  saveLabel: string; readLabel?: string; status?: string; copying?: boolean; leaving?: boolean; mode: EditorMode; narrow?: boolean;
  commandsRef?: RefObject<SourcePaneCommands | null>; onSetMode(mode: EditorMode): void;
  onRead(): void; onSave(): void; onSaveCopy(): void;
}) {
  const menu = useRef<HTMLDetailsElement>(null);
  const closeMenu = () => { if (menu.current) menu.current.open = false; };
  return <header className="editor-toolbar">
    <div className="editor-toolbar__document">
      <strong title={title}>{title}</strong>
      <span className="editor-toolbar__status" role="status">
        {status ?? (saving ? 'Saving…' : pending ? 'Save unconfirmed' : dirty ? 'Unsaved changes' : 'Saved')}
      </span>
    </div>
    <div className="editor-toolbar__actions" role="group" aria-label="Markdown editing actions">
      <div className="editor-toolbar__views" role="group" aria-label="Editor view">
        {(['source', 'preview', 'split'] as const).filter(view => view !== 'split' || !narrow).map(view => <button key={view} type="button" aria-pressed={mode === view} onClick={() => onSetMode(view)}>{view === 'source' ? 'Edit' : view === 'preview' ? 'Preview' : 'Split'}</button>)}
      </div>
      <button type="button" className="editor-toolbar__save" disabled={saveDisabled} onClick={onSave}>
        <svg viewBox="0 0 24 24" aria-hidden="true"><path d="m4 3 13 0 3 3v15H4V3Zm3 0v6h10V3M8 21v-7h8v7" /></svg>{saveLabel}
      </button>
      <button type="button" className="editor-toolbar__done" disabled={leaving} title="Return to reading; your draft is kept" onClick={onRead}>{leaving ? 'Keeping draft…' : readLabel}</button>
      <details ref={menu} className="editor-toolbar__menu" onKeyDown={event => {
        if (event.key === 'Escape') { event.preventDefault(); closeMenu(); menu.current?.querySelector('summary')?.focus(); }
      }}>
        <summary aria-label="More editor actions" title="More editor actions">⋯</summary>
        <div className="editor-toolbar__popover">
          {commandsRef && mode !== 'preview' ? <div role="group" aria-label="Format Markdown">
            {(['bold', 'italic', 'link', 'bullet-list', 'ordered-list'] as const).map(command => <button key={command} type="button" disabled={saving || pending} onClick={() => { commandsRef.current?.format(command); closeMenu(); commandsRef.current?.focus(); }}>{({ bold: 'Bold', italic: 'Italic', link: 'Link', 'bullet-list': 'Bullet list', 'ordered-list': 'Numbered list' })[command]}</button>)}
          </div> : null}
          <button type="button" disabled={copying} onClick={() => { closeMenu(); onSaveCopy(); }}>{copying ? 'Saving copy…' : 'Save a copy…'}</button>
        </div>
      </details>
    </div>
  </header>;
}
