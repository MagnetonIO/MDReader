import { useEffect, useRef, useState } from 'react';
import { CatalogSidebar } from '../catalog/CatalogSidebar';
import type { CanonicalReaderReference } from '../reader/contracts';
import './profile-import.css';

/** Uses existing granted, cached copies only. The native binder validates the
 * selected exact receipt again before recording any historical association. */
export function ExistingDocumentPicker({ title, onChoose, onClose }: {
  title: string;
  onChoose(reference: CanonicalReaderReference): Promise<void>;
  onClose(): void;
}) {
  const dialog = useRef<HTMLDialogElement>(null);
  const active = useRef(true);
  const running = useRef(false);
  const [selected, setSelected] = useState<CanonicalReaderReference | null>(null);
  const [busy, setBusy] = useState(false);
  const [failure, setFailure] = useState<string | null>(null);
  useEffect(() => {
    active.current = true;
    dialog.current?.showModal();
    return () => { active.current = false; };
  }, []);
  async function choose() {
    if (!selected || running.current) return;
    running.current = true;
    setBusy(true);
    setFailure(null);
    try {
      await onChoose(selected);
      if (active.current) onClose();
    } catch (cause) {
      if (active.current) setFailure(cause instanceof Error ? cause.message : 'This document could not be matched. The historical entry was kept.');
    } finally {
      running.current = false;
      if (active.current) setBusy(false);
    }
  }
  return <dialog ref={dialog} className="profile-history-picker" aria-labelledby="profile-history-picker-title"
    onCancel={(event) => { event.preventDefault(); if (!running.current) onClose(); }}>
    <header><h2 id="profile-history-picker-title">Match a historical document</h2>
      <button type="button" className="settings-link-button" disabled={busy} onClick={onClose}>Cancel</button></header>
    <p>Choose the saved copy that belongs to <strong>{title}</strong>. This does not connect an account or add folder access.</p>
    <div className="profile-history-picker__library" inert={busy}>
      <CatalogSidebar exactCopiesOnly onOpen={(reference) => { setSelected(reference); setFailure(null); }} />
    </div>
    {selected ? <section aria-label="Selected historical match">
      <h3>{selected.title || 'Selected document'}</h3>
      <p>This exact version will be associated with the historical entry. Its old reading position stays archived and is not applied to this copy.</p>
      <button type="button" className="settings-link-button" disabled={busy} onClick={() => void choose()}>
        {busy ? 'Matching…' : 'Use this copy'}
      </button>
    </section> : <p role="status">Browse your existing sources and choose a saved document.</p>}
    {failure ? <p role="alert">{failure}</p> : null}
  </dialog>;
}
