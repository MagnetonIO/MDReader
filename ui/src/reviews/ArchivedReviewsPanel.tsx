import { useEffect, useLayoutEffect, useRef, useState } from 'react';
import { archivedReviewsBackend, type ArchivedReviewsBackend, type ArchiveSelection, type ArchivedReviewRecord, type ArchivedReviewComparison, type ReviewCurrent } from './adapter';
import './archived-reviews.css';
export type ReviewTarget = { kind: 'local'; path: string; generation: number } | { kind: 'exact'; reference: ReviewCurrent };
export interface ArchivedReviewsPanelProps {
  current: ReviewTarget | null;
  backend?: ArchivedReviewsBackend;
  onClose(): void;
}
export function ArchivedReviewsPanel({ current, backend = archivedReviewsBackend, onClose }: ArchivedReviewsPanelProps) {
  const dialog = useRef<HTMLDialogElement>(null);
  const epoch = useRef(0);
  const selectedRef = useRef<ArchiveSelection | null>(null);
  const [exportId, setExportId] = useState('');
  const [manifestHash, setManifestHash] = useState('');
  const [selected, setSelected] = useState<ArchiveSelection | null>(null);
  const [records, setRecords] = useState<ArchivedReviewRecord[]>([]);
  const [next, setNext] = useState<number | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [comparison, setComparison] = useState<ArchivedReviewComparison | null>(null);
  const identity = JSON.stringify(current);
  useLayoutEffect(() => { epoch.current++; setComparison(null); setBusy(false); setError(null); }, [identity]);
  useEffect(() => {
    const element = dialog.current;
    if (element?.showModal) element.showModal(); else element?.setAttribute('open', '');
    return () => { epoch.current++; const handle = selectedRef.current?.handle; if (handle) void backend.close(handle).catch(() => {}); };
  }, [backend]);
  const validIdentity = /^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/iu.test(exportId) && /^[0-9a-f]{64}$/iu.test(manifestHash);
  async function select() {
    const request = ++epoch.current; setBusy(true); setError(null); setComparison(null);
    try {
      const archive = await backend.select(exportId, manifestHash);
      if (!archive) return;
      if (epoch.current !== request) { void backend.close(archive.handle).catch(() => {}); return; }
      selectedRef.current = archive; setSelected(archive); setRecords([]); setNext(null);
      const page = await backend.list(archive.handle, 0);
      if (epoch.current !== request) return;
      setRecords(page.records); setNext(page.nextOffset);
    } catch (cause) { if (epoch.current === request) setError(cause instanceof Error ? cause.message : 'Archive unavailable.'); }
    finally { if (epoch.current === request) setBusy(false); }
  }
  async function page(offset: number) {
    if (!selected) return;
    const request = ++epoch.current; setBusy(true); setError(null); setComparison(null);
    try { const result = await backend.list(selected.handle, offset); if (epoch.current === request) { setRecords(result.records); setNext(result.nextOffset); } }
    catch (cause) { if (epoch.current === request) setError(cause instanceof Error ? cause.message : 'Archive unavailable.'); }
    finally { if (epoch.current === request) setBusy(false); }
  }
  async function compare(record: ArchivedReviewRecord) {
    if (!current || !selected) return;
    const request = ++epoch.current; setBusy(true); setError(null); setComparison(null);
    try {
      const reference = current.kind === 'local' ? await backend.current(current.path, current.generation) : current.reference;
      if (epoch.current !== request) return;
      const result = await backend.compare(selected.handle, record.recordId, reference, current.kind === 'local' ? current.generation : null);
      if (epoch.current !== request) return;
      if (result.review.recordId !== record.recordId || result.current.sourceId !== reference.sourceId || result.current.path !== reference.path || result.current.revision !== reference.revision) throw new Error('The comparison returned a different selected reference.');
      setComparison(result);
    } catch (cause) { if (epoch.current === request) setError(cause instanceof Error ? cause.message : 'Current comparison unavailable.'); }
    finally { if (epoch.current === request) setBusy(false); }
  }
  function close() { epoch.current++; onClose(); }
  return <dialog className="archived-reviews" ref={dialog} aria-label="Archived document reviews" onCancel={(event) => { event.preventDefault(); close(); }}>
    <header><h2>Archived document reviews</h2><button type="button" autoFocus onClick={close}>Close archived reviews</button></header>
    <p>Read only. Archived rulings and unknown records remain preserved. Graph membership and review chronology are unavailable. No approval is migrated.</p>
    <p>Choose a verified review stage produced by the offline exporter. Original archive locators grant no access and are never opened.</p>
    <label>Export UUID<input value={exportId} onChange={(event) => setExportId(event.target.value)} maxLength={36} /></label>
    <label>Manifest SHA-256<input value={manifestHash} onChange={(event) => setManifestHash(event.target.value)} maxLength={64} /></label>
    <button type="button" disabled={busy || !validIdentity} onClick={() => void select()}>Choose review stage…</button>
    {error ? <p role="alert">{error}</p> : null}
    {busy ? <p role="status">Reading selected review evidence…</p> : null}
    {!selected ? <p>No archive selected.</p> : <>
      <p>Selected export {selected.exportId}. Comparison uses only the explicitly open document, never a match inferred from an archived path.</p>
      {!current ? <p>Open a selected local or cached provider document to compare its exact revision.</p> : null}
      {!busy && records.length === 0 ? <p>This page contains no archival reviews.</p> : null}
      <ol>{records.map((record) => <li key={record.recordId}>
        <strong>{record.state} archival ruling</strong><p>Reviewer: {record.reviewer ?? 'unavailable'}</p>
        <p>Record {record.rowId} · artifact {record.artifactIndex} · row SHA-256 {record.rowSha256}</p>
        {record.unavailableReason ? <p>Interpretation unavailable: {record.unavailableReason}. Original row retained.</p> : <>
          <p>{record.schema} · {record.extractor} · rule {record.rule} · {record.qualified}</p>
          <p>Recorded inclusive lines {record.startLine}–{record.endLine}</p>
          {record.reading ? <pre>{record.reading}</pre> : null}
          <p>Recorded citation: {record.recordedContentHash ?? 'unknown'}</p>
          <p>Reviewed citation: {record.reviewedContentHash ?? 'unknown'}</p>
          <p>Recorded versus reviewed: {record.recordedHashComparison}</p>
          <button type="button" disabled={busy || !current} onClick={() => void compare(record)}>Compare with open document</button>
        </>}
      </li>)}</ol>
      <button type="button" disabled={busy} onClick={() => void page(0)}>First page</button>
      {next !== null ? <button type="button" disabled={busy} onClick={() => void page(next)}>Next page</button> : null}
    </>}
    {comparison ? <section aria-label="Current revision comparison">
      <h3>Selected current revision</h3>
      <p>Recorded text versus current: {comparison.recordedCurrent}</p>
      <p>Reviewed text versus current: {comparison.reviewedCurrent}</p>
      <p>{comparison.observation === 'provider_cached_revision' ? 'Exact admitted cached provider revision; remote head freshness is unknown.' : 'Exact local current read.'}</p>
      <p>{comparison.current.sourceId} · {comparison.current.path} · revision {comparison.current.revision}</p>
      <p>Span hash: {comparison.currentSpanHash}. Text equality does not establish the assertion or current graph membership.</p>
      <p>Compared current passage, lines {comparison.review.startLine}–{comparison.review.endLine}, as observed at this exact revision:</p>
      <pre>{comparison.currentPassage}</pre>
    </section> : null}
  </dialog>;
}
