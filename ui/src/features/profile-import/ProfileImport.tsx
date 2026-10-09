import { useEffect, useRef, useState } from 'react';
import { ImportFailure, importRequest, type ImportReply, type ImportRequest } from './backend';
import { ImportCoverageSummary } from './ImportCoverageSummary';
import { ImportDocuments } from './ImportDocuments';
import './profile-import.css';

/** Appears only after an explicit import/continue action. No startup discovery. */
export function ProfileImport({ initial, onClose }: { initial: ImportReply; onClose: () => void }) {
  const [reply, setReply] = useState(initial);
  const [selected, setSelected] = useState<Set<string>>(() => new Set());
  const [busy, setBusy] = useState<string | null>(null);
  const [failure, setFailure] = useState<string | null>(null);
  const [uncertain, setUncertain] = useState(false);
  const [confirmRollback, setConfirmRollback] = useState(false);
  const running = useRef(false);
  const mounted = useRef(true);
  const latestOperation = useRef(reply.operationId);
  const title = useRef<HTMLHeadingElement>(null);
  const keepSeparate = useRef<HTMLButtonElement>(null);
  latestOperation.current = reply.operationId;
  const operationId = reply.operationId;
  const prepared = reply.state === 'prepared';
  const opening = reply.state === 'activated' || reply.state === 'launch_requested';
  const returning = reply.state === 'rollback_launch_requested' || reply.state === 'rolled_back';
  const preview = reply.state === 'preview' || reply.state === 'exported';
  useEffect(() => { title.current?.focus(); }, [reply.state]);
  useEffect(() => { if (confirmRollback) keepSeparate.current?.focus(); }, [confirmRollback]);
  useEffect(() => {
    mounted.current = true;
    return () => {
      mounted.current = false;
      // StrictMode immediately reattaches the same owner. Actual dismissal
      // releases only native volatile selection; prepared copies are retained.
      queueMicrotask(() => {
        const id = latestOperation.current;
        if (!mounted.current && id) void importRequest({ operation: 'close', operationId: id }).catch(() => {});
      });
    };
  }, []);

  async function perform(request: ImportRequest, label: string) {
    if (running.current) return;
    running.current = true;
    setBusy(label);
    setFailure(null);
    try {
      const next = await importRequest(request);
      if (!mounted.current) return;
      if (next.state === 'closed') { latestOperation.current = null; onClose(); return; }
      if (next.state === 'cancelled') return;
      // Export/preparation replies need not repeat the verified metadata list.
      setReply((current) => ({ ...next, documents: next.documents.length ? next.documents : current.documents,
        origin: next.origin ?? current.origin, coverage: next.coverage ?? current.coverage }));
    } catch (cause) {
      if (!mounted.current) return;
      if (!(cause instanceof ImportFailure) || !['close_source_app_required', 'too_large', 'invalid_profile', 'invalid_destination', 'offline_export_required'].includes(cause.kind)) setUncertain(true);
      setFailure(cause instanceof Error ? cause.message : 'The import could not be confirmed. Your originals were kept.');
    } finally {
      running.current = false;
      if (mounted.current) setBusy(null);
      else if (latestOperation.current) {
        // If dismissal raced an OS dialog, its earlier close could be Busy.
        // Retry only releasing this exact volatile selection after it settles.
        void importRequest({ operation: 'close', operationId: latestOperation.current }).catch(() => {});
      }
    }
  }

  return <section className="profile-import" aria-labelledby="profile-import-title" aria-busy={busy !== null}>
    <h3 ref={title} tabIndex={-1} id="profile-import-title">{returning ? 'Return to your original library' : prepared || opening ? 'Your separate library' : 'Import reader data'}</h3>
    <p>Your original library stays unchanged. A separate library keeps this import independent; folder and cloud access must be connected again explicitly.</p>
    {preview ? <>
      <p>{reply.origin === 'legacy_catdb'
        ? 'Select the documents and insights you want to read in the new library. Older references appear in Imported history, where you can match them to saved copies. Old reading positions stay archived; folder and account connections are not reopened.'
        : 'Select the documents to bring into the new library. Their reading positions and pins are kept. Exporting originals also preserves records that are not shown as documents here.'}</p>
      <ImportDocuments documents={reply.documents} selected={selected} onSelection={setSelected}
        disabled={busy !== null || uncertain} />
    </> : null}
    {reply.coverage ? <ImportCoverageSummary origin={reply.origin} coverage={reply.coverage} prepared={prepared || opening || returning} /> : null}
    {reply.state === 'exported' ? <p role="status">The original records were exported. Keep that copy with your original library.</p> : null}
    {prepared ? <p role="status">The separate library is prepared with {reply.imported} document{reply.imported === 1 ? '' : 's'}. Open it when you are ready. This window and its drafts stay available.</p> : null}
    {opening ? <p role="status">Opening the separate library was requested. Keep this window and your originals until you have checked the new library.</p> : null}
    {returning ? <p role="status">The original library was selected again{reply.state === 'rollback_launch_requested' ? ', and opening it was requested' : ''}. The separate library and the backed-up changes were kept. Check the original library before closing this window.</p> : null}
    {confirmRollback && reply.state === 'activated' ? <fieldset className="profile-import__confirmation">
      <legend>Return to the original library?</legend>
      <p>Close the separate imported app first. MDReader will back up all changes in that library before selecting and opening the original. The separate library and its copies are kept.</p>
      <div className="settings-account-actions">
        <button type="button" className="settings-link-button" disabled={busy !== null || uncertain || !operationId}
          onClick={() => { if (operationId) void perform({ operation: 'rollback', operationId }, 'Backing up changes and requesting the original library…'); }}>Back up changes and open original library</button>
        <button ref={keepSeparate} type="button" className="settings-link-button" disabled={busy !== null} onClick={() => { setConfirmRollback(false); title.current?.focus(); }}>Keep separate library</button>
      </div>
    </fieldset> : null}
    <div className="settings-account-actions">
      {preview && operationId ? <>
        <button type="button" className="settings-link-button" disabled={busy !== null || uncertain}
          onClick={() => { void perform({ operation: 'export', operationId }, 'Exporting originals…'); }}>Export originals…</button>
        <button type="button" className="settings-link-button" disabled={busy !== null || uncertain || !selected.size}
          onClick={() => { void perform({ operation: 'prepare', operationId, documents: reply.documents.filter((item) => selected.has(item.documentId)).map(({ documentId, revision }) => ({ documentId, revision })) }, 'Preparing library…'); }}>Create separate library…</button>
      </> : null}
      {prepared && operationId ? <button type="button" className="settings-link-button" disabled={busy !== null || uncertain}
        onClick={() => { void perform({ operation: 'activate', operationId }, 'Requesting open…'); }}>Open separate library</button> : null}
      {reply.state === 'activated' && operationId && !confirmRollback ? <button type="button" className="settings-link-button" disabled={busy !== null || uncertain}
        onClick={() => setConfirmRollback(true)}>Return to original library…</button> : null}
      <button type="button" className="settings-link-button" disabled={busy !== null}
        onClick={() => { if (operationId) void perform({ operation: 'close', operationId }, 'Closing import…'); else onClose(); }}>Done</button>
    </div>
    {busy ? <p role="status">{busy}</p> : null}
    {failure ? <p role="alert">{failure}</p> : null}
    {uncertain ? <p>Choose Done, then Continue an import to inspect a retained copy before taking another action.</p> : null}
  </section>;
}
