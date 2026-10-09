import { useEffect, useRef, useState } from 'react';
import { ImportFailure, importRequest, isWaitingImport, type ImportReply, type ImportRequest } from './backend';
import { ImportCoverageSummary } from './ImportCoverageSummary';
import { ImportDocuments } from './ImportDocuments';
import { OriginalLibraryHandoff, ReturnLibraryHandoff, type OriginalLibraryWait } from './OriginalLibraryHandoff';
import './profile-import.css';

function initialWaitReason(reply: ImportReply): OriginalLibraryWait {
  return reply.state === 'background_service_unsupported' ? 'background_unsupported'
    : ['service_handoff_pending', 'rollback_handoff_pending'].includes(reply.state) ? 'handoff_unconfirmed' : 'original_open';
}

/** Appears only after an explicit import/continue action. No startup discovery. */
export function ProfileImport({ initial, onClose }: { initial: ImportReply; onClose: () => void }) {
  const [reply, setReply] = useState(initial);
  const [selected, setSelected] = useState<Set<string>>(() => new Set());
  const [busy, setBusy] = useState<string | null>(null);
  const [failure, setFailure] = useState<string | null>(null);
  const [uncertain, setUncertain] = useState(initial.state === 'service_handoff_pending' || initial.state === 'rollback_handoff_pending');
  const [waitReason, setWaitReason] = useState<OriginalLibraryWait>(() => initialWaitReason(initial));
  const [rollbackPending, setRollbackPending] = useState(initial.state === 'rollback_handoff_pending');
  const [confirmRollback, setConfirmRollback] = useState(false);
  const running = useRef(false);
  const mounted = useRef(true);
  const latestOperation = useRef(reply.operationId);
  const title = useRef<HTMLHeadingElement>(null);
  const keepSeparate = useRef<HTMLButtonElement>(null);
  latestOperation.current = reply.operationId;
  const operationId = reply.operationId;
  const waitingForOriginal = isWaitingImport(reply) && !rollbackPending;
  const prepared = reply.state === 'prepared';
  // Native only offers activated when it retains the original launch identity.
  const rollbackAvailable = reply.state === 'activated' && !rollbackPending;
  const opening = rollbackAvailable || reply.state === 'launch_requested';
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
      setUncertain(next.state === 'service_handoff_pending' || next.state === 'rollback_handoff_pending');
      setRollbackPending(next.state === 'rollback_handoff_pending');
      setWaitReason(initialWaitReason(next));
      // Export/preparation replies need not repeat the verified metadata list.
      setReply((current) => isWaitingImport(next) ? next : ({ ...next, documents: next.documents.length ? next.documents : current.documents,
        origin: next.origin ?? current.origin, coverage: next.coverage ?? current.coverage }));
    } catch (cause) {
      if (!mounted.current) return;
      if (request.operation === 'rollback') setRollbackPending(true);
      if (cause instanceof ImportFailure && cause.kind === 'background_service_unsupported') {
        setWaitReason('background_unsupported');
        setUncertain(false);
        setFailure(request.operation === 'close'
          ? rollbackPending ? 'The return still has a pending service handoff and cannot be dismissed yet. Both libraries were kept. Use Retry return to original library to check the same operation.'
            : 'This import still has a pending service handoff and cannot be dismissed yet. Your originals and selection were kept. Retry import checks the same handoff.'
          : request.operation === 'rollback' || rollbackPending
            ? 'A background service cannot safely finish this return with its current version. Both libraries were kept.'
            : cause.message);
      } else if (cause instanceof ImportFailure && cause.kind === 'service_handoff_unconfirmed') {
        setWaitReason('handoff_unconfirmed');
        setUncertain(true);
        setFailure(request.operation === 'close'
          ? rollbackPending ? 'The return still has a pending service handoff and cannot be dismissed yet. Both libraries were kept. Use Retry return to original library to check the same operation.'
            : 'This import still has a pending service handoff and cannot be dismissed yet. Your originals and selection were kept. Retry import checks the same handoff.'
          : request.operation === 'rollback' || rollbackPending
            ? 'The return could not be confirmed. Both libraries were kept. Retry return to original library checks the same operation.'
            : cause.message);
      } else {
        if (!(cause instanceof ImportFailure) || !['close_source_app_required', 'too_large', 'invalid_profile', 'invalid_destination', 'offline_export_required'].includes(cause.kind)) setUncertain(true);
        if (waitingForOriginal && cause instanceof ImportFailure && cause.kind === 'close_source_app_required') setWaitReason('original_open');
        else if (waitingForOriginal && (!(cause instanceof ImportFailure) || !['too_large', 'invalid_profile', 'invalid_destination', 'offline_export_required', 'profile_changed'].includes(cause.kind))) setWaitReason('handoff_unconfirmed');
        setFailure(cause instanceof ImportFailure && cause.kind === 'close_source_app_required' && request.operation === 'rollback'
          ? 'Close the separate imported app normally, keeping any drafts, then choose Retry return to original library.'
          : waitingForOriginal && cause instanceof ImportFailure && cause.kind === 'close_source_app_required'
            ? 'The original app is still in use. Close its windows normally, keeping any drafts, then choose Retry import here.'
          : waitingForOriginal && (!(cause instanceof ImportFailure) || !['invalid_profile', 'invalid_destination', 'offline_export_required', 'too_large', 'profile_changed'].includes(cause.kind))
            ? 'The selected library check could not be confirmed. Your originals were kept. You can check this same selection again.'
            : cause instanceof Error ? cause.message : 'The import could not be confirmed. Your originals were kept.');
      }
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
    <h3 ref={title} tabIndex={-1} id="profile-import-title">{returning || rollbackPending ? 'Return to your original library' : prepared || opening ? 'Your separate library' : 'Import reader data'}</h3>
    <p>Your original library stays unchanged. A separate library keeps this import independent; folder and cloud access must be connected again explicitly.</p>
    {rollbackPending ? <ReturnLibraryHandoff /> : null}
    {waitingForOriginal ? <OriginalLibraryHandoff reason={waitReason} /> : null}
    {preview ? <>
      <p>{reply.origin === 'legacy_catdb'
        ? 'Select the reading copies and insights you want in the new library. Metadata-only records do not include original Markdown; reconnect a folder to match original-document references. Older references appear in Imported history, where you can match them to saved copies. Pins and reading positions from existing device copies are kept. Older tab positions stay archived; folder and account connections are not reopened.'
        : 'Select the documents to bring into the new library. Their reading positions and pins are kept. Exporting originals also preserves records that are not shown as documents here.'}</p>
      <ImportDocuments documents={reply.documents} selected={selected} onSelection={setSelected}
        disabled={busy !== null || uncertain} />
    </> : null}
    {reply.coverage ? <ImportCoverageSummary origin={reply.origin} coverage={reply.coverage} prepared={prepared || opening || returning} /> : null}
    {reply.state === 'exported' ? <p role="status">The original records were exported. Keep that copy with your original library.</p> : null}
    {prepared ? <p role="status">The separate library is prepared with {reply.imported} document{reply.imported === 1 ? '' : 's'}. Open it when you are ready. This window and its drafts stay available.</p> : null}
    {opening ? <p role="status">Opening the separate library was requested. Keep this window and your originals until you have checked the new library.</p> : null}
    {returning ? <p role="status">The original library was selected again{reply.state === 'rollback_launch_requested' ? ', and opening it was requested' : ''}. The separate library and the backed-up changes were kept. Check the original library before closing this window.</p> : null}
    {confirmRollback && rollbackAvailable ? <fieldset className="profile-import__confirmation">
      <legend>Return to the original library?</legend>
      <p>Close the separate imported app first. MDReader will back up all changes in that library before selecting and opening the original. The separate library and its copies are kept.</p>
      <div className="settings-account-actions">
        <button type="button" className="settings-link-button" disabled={busy !== null || uncertain || !operationId}
          onClick={() => { if (operationId) void perform({ operation: 'rollback', operationId }, 'Backing up changes and requesting the original library…'); }}>Back up changes and open original library</button>
        <button ref={keepSeparate} type="button" className="settings-link-button" disabled={busy !== null} onClick={() => { setConfirmRollback(false); title.current?.focus(); }}>Keep separate library</button>
      </div>
    </fieldset> : null}
    <div className="settings-account-actions">
      {rollbackPending && operationId ? <button type="button" className="settings-link-button" disabled={busy !== null}
        onClick={() => { void perform({ operation: 'rollback', operationId }, 'Checking the return to your original library…'); }}>Retry return to original library</button> : null}
      {waitingForOriginal && operationId ? <button type="button" className="settings-link-button" disabled={busy !== null}
        onClick={() => { void perform({ operation: 'retry', operationId }, 'Checking the selected original library…'); }}>Retry import</button> : null}
      {preview && operationId ? <>
        <button type="button" className="settings-link-button" disabled={busy !== null || uncertain}
          onClick={() => { void perform({ operation: 'export', operationId }, 'Exporting originals…'); }}>Export originals…</button>
        <button type="button" className="settings-link-button" disabled={busy !== null || uncertain || !selected.size}
          onClick={() => { void perform({ operation: 'prepare', operationId, documents: reply.documents.filter((item) => selected.has(item.documentId)).map(({ documentId, revision }) => ({ documentId, revision })) }, 'Preparing library…'); }}>Create separate library…</button>
      </> : null}
      {prepared && operationId ? <button type="button" className="settings-link-button" disabled={busy !== null || uncertain}
        onClick={() => { void perform({ operation: 'activate', operationId }, 'Requesting open…'); }}>Open separate library</button> : null}
      {rollbackAvailable && operationId && !confirmRollback ? <button type="button" className="settings-link-button" disabled={busy !== null || uncertain}
        onClick={() => setConfirmRollback(true)}>Return to original library…</button> : null}
      <button type="button" className="settings-link-button" disabled={busy !== null}
        onClick={() => { if (operationId) void perform({ operation: 'close', operationId }, 'Closing import…'); else onClose(); }}>Done</button>
    </div>
    {busy ? <p role="status">{busy}</p> : null}
    {failure ? <p role="alert">{failure}</p> : null}
    {uncertain && !rollbackPending && waitReason !== 'handoff_unconfirmed' ? <p>{waitingForOriginal
      ? 'The check could not be confirmed. Retry import checks the same selection; it does not choose another library.'
      : 'Choose Done, then Continue an import to inspect a retained copy before taking another action.'}</p> : null}
  </section>;
}
