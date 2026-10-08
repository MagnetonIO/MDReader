import { useEffect, useId, useRef } from 'react';
import './account-sync.css';
import type { DeletionSelection } from './useSelectedSync';

/** Confirmation holds the exact receipt and account scope shown to the user. */
export function DeleteSelectedDocument({ selection, disabled, onCancel, onConfirm }: {
  selection: DeletionSelection; disabled: boolean; onCancel(): void; onConfirm(): void;
}) {
  const dialog = useRef<HTMLDialogElement>(null);
  const cancel = useRef<HTMLButtonElement>(null);
  const title = useId();
  const description = useId();
  useEffect(() => {
    const target = dialog.current;
    const trigger = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    if (target?.showModal) target.showModal();
    else target?.setAttribute('open', '');
    cancel.current?.focus();
    return () => { target?.close?.(); trigger?.focus(); };
  }, []);
  return <dialog ref={dialog} className="settings-confirm settings-delete-document" aria-labelledby={title} aria-describedby={description}
    onCancel={(event) => { event.preventDefault(); onCancel(); }}
    onKeyDown={(event) => {
      event.stopPropagation();
      if (event.key === 'Escape') { event.preventDefault(); onCancel(); }
      if (event.key === 'Tab') {
        const buttons = [...(dialog.current?.querySelectorAll<HTMLButtonElement>('button:not([disabled])') ?? [])];
        const first = buttons[0]; const last = buttons.at(-1);
        if (event.shiftKey && document.activeElement === first) { event.preventDefault(); last?.focus(); }
        else if (!event.shiftKey && document.activeElement === last) { event.preventDefault(); first?.focus(); }
      }
    }}>
    <h2 id={title}>Delete this synced document?</h2>
    <p id={description}>Delete “{selection.reference.title || 'Untitled document'}” from the selected encrypted library and its synced devices. Its local copy stays until the authenticated deletion is applied. This is separate from forgetting a local source.</p>
    <p>A newer revision or changed account is refused. Offline or conflicting requests keep the original copy and a retained operation for reconciliation. Unsaved editor buffers are not discarded.</p>
    <details><summary>Exact selected revision</summary><p>{selection.reference.revision}</p></details>
    <div className="settings-confirm__actions">
      <button ref={cancel} type="button" onClick={onCancel}>Cancel</button>
      <button type="button" className="settings-button--danger" disabled={disabled} onClick={onConfirm}>Queue deletion</button>
    </div>
  </dialog>;
}
