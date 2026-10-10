import { useEffect, useId, useRef } from 'react';
import type { ProviderSource } from '../../platform/providers';
import { cloudDestination, cloudMarkdownFilename } from './cloud-create-model';
import './cloud-create.css';

export function CloudCreateDialog({ source, filename, busy, pending, retry, error, onName, onClose, onCreate, onInspect }: {
  source: ProviderSource; filename: string; busy: boolean; pending: boolean; retry: boolean; error: string | null;
  onName(name: string): void; onClose(): void; onCreate(): void; onInspect(): void;
}) {
  const dialog = useRef<HTMLDialogElement>(null), input = useRef<HTMLInputElement>(null);
  const title = useId(), destination = useId();
  useEffect(() => {
    const node = dialog.current;
    const trigger = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    if (node?.showModal) node.showModal(); else node?.setAttribute('open', '');
    input.current?.focus();
    return () => { node?.close?.(); if (trigger?.isConnected && !trigger.closest('[inert]')) trigger.focus({ preventScroll: true }); };
  }, []);
  return <dialog ref={dialog} className="cloud-create-dialog" aria-labelledby={title} aria-describedby={destination}
    onCancel={event => { event.preventDefault(); onClose(); }}
    onKeyDown={event => {
      event.stopPropagation();
      if (event.key === 'Escape') { event.preventDefault(); onClose(); }
    }}>
    <form onSubmit={event => { event.preventDefault(); if (!busy && !pending && cloudMarkdownFilename(filename)) onCreate(); }}>
      <h2 id={title}>New Markdown document</h2>
      <p id={destination}>{source.name} · {cloudDestination(source)}</p>
      <label>Filename<input ref={input} value={filename} disabled={busy || pending} maxLength={240}
        onChange={event => onName(event.target.value)} placeholder="Untitled.md" spellCheck={false} /></label>
      {!pending ? <p className="cloud-create-dialog__hint">A .md extension is added if needed.</p> : null}
      {busy ? <p role="status">Waiting for confirmation…</p> : pending ? <p role="status">Creation is not confirmed yet. Check status before creating another document.</p> : null}
      {error ? <p role="alert">{error}</p> : null}
      <div className="cloud-create-dialog__actions">
        <button type="button" onClick={onClose}>{pending || busy ? 'Close' : 'Cancel'}</button>
        {pending ? <button type="button" disabled={busy} onClick={onInspect}>Check status</button>
          : <button type="submit" disabled={busy || !cloudMarkdownFilename(filename)}>{retry ? 'Retry create' : 'Create document'}</button>}
      </div>
    </form>
  </dialog>;
}
