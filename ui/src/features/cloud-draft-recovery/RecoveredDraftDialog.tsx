import { useEffect, useId, useRef } from 'react';
import { useDraftCopy } from '../editor-controls/useDraftCopy';
import type { RecoveredDraft, RecoveredDraftBody } from './backend';
import './cloud-draft-recovery.css';

export function RecoveredDraftDialog({ draft, body, error, onClose }: {
  draft: RecoveredDraft; body: RecoveredDraftBody | null; error: string | null; onClose(): void;
}) {
  const dialog = useRef<HTMLDialogElement>(null), close = useRef<HTMLButtonElement>(null);
  const title = useId(), description = useId(), text = useId();
  const copy = useDraftCopy(`${draft.draftId}:${draft.revision}`);
  useEffect(() => {
    const node = dialog.current;
    const trigger = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    if (node?.showModal) node.showModal(); else node?.setAttribute('open', '');
    close.current?.focus();
    return () => { node?.close?.(); if (trigger?.isConnected && !trigger.closest('[inert]')) trigger.focus({ preventScroll: true }); };
  }, []);
  return <dialog ref={dialog} className="recovered-cloud-draft" aria-labelledby={title} aria-describedby={description}
    onCancel={event => { event.preventDefault(); onClose(); }} onKeyDown={event => {
      event.stopPropagation();
      if (event.key === 'Escape') { event.preventDefault(); onClose(); }
    }}>
    <header><h2 id={title}>{draft.title}</h2><button ref={close} type="button" onClick={onClose}>Close</button></header>
    <p>{draft.sourceName} · Read only</p>
    <p id={description}>This is the text prepared for a cloud save. Later unsaved changes may not be included.</p>
    {error ? <p role="alert">{error}</p> : !body ? <p role="status">Loading retained draft…</p> : <>
      <label htmlFor={text}>Retained Markdown</label>
      <textarea id={text} readOnly value={body.markdown} spellCheck={false} />
      <button type="button" disabled={copy.busy} onClick={() => void copy.save(body.markdown, body.draft.title)}>Save a copy…</button>
    </>}
    {copy.notice ? <p role="status">{copy.notice.replaceAll('editor draft', 'retained draft')}</p> : null}
    {copy.error ? <p role="alert">{copy.error.replaceAll('editor draft', 'retained draft')}</p> : null}
  </dialog>;
}
