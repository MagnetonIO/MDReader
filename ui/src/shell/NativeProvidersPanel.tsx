import { useEffect, useId, useRef, useState } from 'react';
import {
  nativeProvidersBackend, NativeProviderError, providerErrorMessage,
  type NativeProvidersBackend, type NativeProvidersSnapshot, type ProviderReaderReference,
  type NativeProviderKind, type ProviderAdmission, type ProviderDocuments, type ProviderSource,
  type DropboxSelection, type ProviderDocument, type ProviderCoverage,
} from '../platform/providers.js';

const providerName = (kind: NativeProviderKind) => kind === 'google_drive' ? 'Google Drive' : 'Dropbox';
function coverageText(coverage: ProviderCoverage): string {
  switch (coverage.kind) {
    case 'never_indexed': return 'No documents indexed yet.';
    case 'partial': return `Partial coverage: ${coverage.reason}`;
    case 'metadata_complete_selected': return 'Selected metadata enumerated. Document bodies and indexing may still be incomplete.';
    case 'complete_selected': return 'Selected documents indexed at the last confirmed observation.';
  }
}
function scopeText(source: ProviderSource): string {
  switch (source.selectedScope.kind) {
    case 'exact_documents': return `${source.selectedScope.count} explicitly selected document${source.selectedScope.count === 1 ? '' : 's'}`;
    case 'scoped_root': return source.selectedScope.recursive ? 'Selected App Folder path and its subfolders' : 'Selected App Folder path';
    case 'imported_legacy_broad': return 'Imported broad connection. A new selected-scope connection is required.';
  }
}
const message = (cause: unknown) => cause instanceof NativeProviderError ? cause.message : 'The provider operation could not be completed.';
function validDropboxPath(path: string): boolean {
  return path.length > 0 && path.length <= 4096 && path.startsWith('/') && !path.includes('\\') && !Array.from(path).some((character) => { const code = character.charCodeAt(0); return code < 32 || code >= 127 && code <= 159; })
    && !path.split('/').some((part) => part === '.' || part === '..') && (path === '/' || !path.endsWith('/'));
}
export interface NativeProvidersPanelProps {
  backend?: NativeProvidersBackend;
  /** Changing the active library invalidates late replies and open callbacks. */
  profileKey?: string;
  onOpen?: (reference: ProviderReaderReference) => void | Promise<void>;
  onSourcesChanged?: () => void;
}
/** Native custody owns OAuth and account/selection authority. Opening this panel is read-only. */
export function NativeProvidersPanel({ backend = nativeProvidersBackend, profileKey = 'current', onOpen, onSourcesChanged }: NativeProvidersPanelProps) {
  const titleId = useId();
  const [expanded, setExpanded] = useState(false);
  const [snapshot, setSnapshot] = useState<NativeProvidersSnapshot | null>(null);
  const [selected, setSelected] = useState<string | null>(null);
  const [browseGeneration, setBrowseGeneration] = useState(0);
  const [page, setPage] = useState<ProviderDocuments | null>(null);
  const [loading, setLoading] = useState(false);
  const [documentsLoading, setDocumentsLoading] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [dropboxPath, setDropboxPath] = useState('/');
  const [recursive, setRecursive] = useState(false);
  const [dropboxEditing, setDropboxEditing] = useState(false);
  const lifecycle = useRef(0);
  const loadedProfile = useRef<string | null>(null);
  const documentRequest = useRef(0);
  const action = useRef(0);
  const selection = useRef<string | null>(null);
  selection.current = selected;
  useEffect(() => {
    const epoch = ++lifecycle.current;
    loadedProfile.current = null;
    documentRequest.current += 1;
    action.current += 1;
    setDropboxEditing(false); setSnapshot(null); setSelected(null); setPage(null); setError(null); setNotice(null); setBusy(false); setDocumentsLoading(false);
    if (!expanded) { setLoading(false); return; }
    setLoading(true);
    void backend.state().then((value) => { if (lifecycle.current === epoch) { loadedProfile.current = profileKey; setSnapshot(value); } })
      .catch((cause: unknown) => { if (lifecycle.current === epoch) setError(message(cause)); })
      .finally(() => { if (lifecycle.current === epoch) setLoading(false); });
    return () => { lifecycle.current += 1; documentRequest.current += 1; action.current += 1; };
  }, [backend, expanded, profileKey]);
  useEffect(() => {
    const request = ++documentRequest.current;
    const epoch = lifecycle.current;
    setPage(null); setDocumentsLoading(false);
    if (!selected || !expanded || loadedProfile.current !== profileKey) return;
    setDocumentsLoading(true);
    void backend.documents(selected, 0, 50).then((value) => {
      if (documentRequest.current === request && lifecycle.current === epoch) setPage(value);
    }).catch((cause: unknown) => {
      if (documentRequest.current === request && lifecycle.current === epoch) setError(message(cause));
    }).finally(() => {
      if (documentRequest.current === request && lifecycle.current === epoch) setDocumentsLoading(false);
    });
    return () => { documentRequest.current += 1; };
  }, [backend, selected, profileKey, browseGeneration]);
  const current = snapshot?.sources.find((source) => source.sourceId === selected) ?? null;
  const canConnect = (kind: NativeProviderKind) => !snapshot?.error && (snapshot?.availability.some((value) => value.provider === kind && value.configured && value.supported) ?? false);
  const dropboxSelection = (): DropboxSelection => ({ path: dropboxPath, recursive });
  async function updateSnapshot(epoch: number) {
    const value = await backend.state();
    if (lifecycle.current === epoch) {
      setSnapshot(value);
      if (selection.current && !value.sources.some((source) => source.sourceId === selection.current && source.enabled)) {
        documentRequest.current += 1; setSelected(null); setPage(null);
      }
    }
  }
  async function run(work: (epoch: number) => Promise<void>) {
    if (busy) return;
    const epoch = lifecycle.current;
    const id = ++action.current;
    setBusy(true); setError(null); setNotice(null);
    try { await work(epoch); }
    catch (cause) { if (lifecycle.current === epoch && action.current === id) setError(message(cause)); }
    finally { if (lifecycle.current === epoch && action.current === id) setBusy(false); }
  }
  async function acceptAdmission(result: ProviderAdmission, epoch: number) {
    if (lifecycle.current !== epoch) return;
    if (result.state === 'pending') {
      setNotice('The connection outcome is pending. Use Reconcile connection to check it without starting OAuth again.');
      setError(providerErrorMessage(result.reason));
    } else {
      setNotice(result.enabled ? 'The selected source is connected. Refresh its metadata to browse documents.' : 'This source remains paused. Reconnect it explicitly to grant access again.');
      onSourcesChanged?.();
    }
    await updateSnapshot(epoch);
  }
  function connect(kind: NativeProviderKind) {
    if (!canConnect(kind) || kind === 'dropbox' && !validDropboxPath(dropboxPath)) return;
    void run(async (epoch) => acceptAdmission(await backend.connect(kind, kind === 'google_drive' ? null : dropboxSelection(), kind === 'dropbox' && dropboxEditing), epoch));
  }
  function reconnect(source: ProviderSource) {
    if (!canConnect(source.kind) || source.kind === 'dropbox' && !validDropboxPath(dropboxPath)) return;
    void run(async (epoch) => acceptAdmission(await backend.reconnect(source.sourceId, source.kind === 'google_drive' ? null : dropboxSelection(), source.kind === 'dropbox' && dropboxEditing), epoch));
  }
  function changeSource(source: ProviderSource, forget: boolean) {
    void run(async (epoch) => {
      if (forget) await backend.forget(source.sourceId); else await backend.disable(source.sourceId);
      if (lifecycle.current !== epoch) return;
      setNotice(forget ? 'Cached documents were removed and the source is paused. Uncertain save evidence is retained.' : 'This source is paused. Its cached documents are no longer accessible.');
      onSourcesChanged?.();
      await updateSnapshot(epoch);
    });
  }
  function refresh(source: ProviderSource) {
    void run(async (epoch) => {
      const result = await backend.refresh(source.sourceId);
      if (lifecycle.current !== epoch) return;
      setNotice(result.metadataComplete ? 'Selected metadata refreshed. Fetch document bodies to admit them for indexing.' : 'Metadata refresh is partial. Existing coverage is shown below.');
      onSourcesChanged?.();
      await updateSnapshot(epoch);
      if (lifecycle.current !== epoch || selection.current !== source.sourceId) return;
      const request = ++documentRequest.current;
      const documents = await backend.documents(source.sourceId, 0, 50);
      if (lifecycle.current === epoch && documentRequest.current === request && selection.current === source.sourceId) setPage(documents);
    });
  }
  function open(source: ProviderSource, document: ProviderDocument, cached: boolean) {
    if (!onOpen) return;
    void run(async (epoch) => {
      const projection = cached ? await backend.readCached(source.sourceId, document.documentId) : await backend.fetchCurrent(source.sourceId, document.documentId);
      if (lifecycle.current !== epoch || selection.current !== source.sourceId) return;
      if (!cached) onSourcesChanged?.();
      await onOpen({ sourceId: projection.readerSourceId, path: projection.path, revision: projection.revision, title: document.name, line: 1 });
      if (lifecycle.current === epoch) setNotice(cached ? 'Opened the exact cached revision. Its observation time does not prove the current remote head.' : 'Fetched and admitted the current published revision. Indexing may still be in progress.');
    });
  }
  function more() {
    if (!current || !page || page.nextOffset === null || documentsLoading || busy) return;
    const source = current.sourceId; const offset = page.nextOffset; const epoch = lifecycle.current; const request = ++documentRequest.current;
    setDocumentsLoading(true);
    void backend.documents(source, offset, 50).then((value) => {
      if (lifecycle.current === epoch && documentRequest.current === request && selection.current === source) setPage((previous) => ({ ...value, documents: [...(previous?.documents ?? []), ...value.documents] }));
    }).catch((cause: unknown) => { if (lifecycle.current === epoch && documentRequest.current === request) setError(message(cause)); })
      .finally(() => { if (lifecycle.current === epoch && documentRequest.current === request) setDocumentsLoading(false); });
  }
  return <section className="settings-section settings-cloud-sources" aria-labelledby={titleId}>
    <div className="settings-section__intro"><h2 id={titleId}>Selected cloud sources</h2><p>Read the files you choose from Google Drive or Dropbox. Connecting a source does not turn on device sync.</p></div>
    <button type="button" aria-expanded={expanded} onClick={() => setExpanded((value) => !value)}>{expanded ? 'Hide cloud sources' : 'Manage cloud sources'}</button>
    {expanded ? <>
      <button type="button" disabled={busy || loading} onClick={() => void run(updateSnapshot)}>Reload provider status</button>
      {loading ? <p role="status">Loading provider status…</p> : null}
      {error ? <p role="alert">{error}</p> : null}
      {snapshot?.error ? <p role="alert">{providerErrorMessage(snapshot.error)}</p> : null}
      {notice ? <p role="status">{notice}</p> : null}
      {snapshot ? <>
        <fieldset disabled={busy || loading}>
          <legend>New connections</legend>
          <p>Google Drive includes only the files you select in the native Picker. Reconnect starts a new explicit selection; it does not broaden the old grant automatically.</p>
          <button type="button" disabled={!canConnect('google_drive')} onClick={() => connect('google_drive')}>Choose Google Drive files</button>
          {snapshot.availability.find((value) => value.provider === 'google_drive')?.diagnostic ? <p>{snapshot.availability.find((value) => value.provider === 'google_drive')?.diagnostic}</p> : null}
          <p>Dropbox uses the registered MDReader App Folder. This selection also applies when reconnecting a Dropbox source.</p>
          <label>Dropbox App Folder path<input value={dropboxPath} onChange={(event) => setDropboxPath(event.target.value)} maxLength={4096} /></label>
          <label><input type="checkbox" checked={recursive} onChange={(event) => setRecursive(event.target.checked)} /> Include Dropbox subfolders</label>
          <label><input type="checkbox" checked={dropboxEditing} onChange={(event) => setDropboxEditing(event.target.checked)} /> Allow guarded edits to selected Dropbox files</label>
          <p>Leave unchecked for read-only access. Connecting or reconnecting with editing enabled asks Dropbox for write permission; an existing read-only grant is never upgraded automatically. Reconnect replaces the source’s old selection and authorization. Unsaved editor buffers remain yours to keep or download.</p>
          {!validDropboxPath(dropboxPath) ? <p>Enter an absolute App Folder path without parent traversal, such as /Notes.</p> : null}
          <button type="button" disabled={!canConnect('dropbox') || !validDropboxPath(dropboxPath)} onClick={() => connect('dropbox')}>Connect Dropbox App Folder</button>
          {snapshot.availability.find((value) => value.provider === 'dropbox')?.diagnostic ? <p>{snapshot.availability.find((value) => value.provider === 'dropbox')?.diagnostic}</p> : null}
        </fieldset>
        {snapshot.pendingAdmissions.length ? <section aria-label="Pending connections"><h3>Pending connections</h3>{snapshot.pendingAdmissions.map((attempt) => <div key={attempt.attemptId}><p>{attempt.name} — {attempt.phase === 'vault_pending' ? 'Credential custody outcome pending' : 'Source admission outcome pending'}</p><button type="button" disabled={busy} onClick={() => void run(async (epoch) => acceptAdmission(await backend.reconcile(attempt.attemptId), epoch))}>Reconcile connection</button></div>)}</section> : null}
        {!snapshot.error && !snapshot.sources.length ? <p>No selected cloud sources are connected.</p> : null}
        <ul>{snapshot.sources.map((source) => <li key={source.sourceId} aria-label={source.name}>
          <h3>{source.name}</h3><p>{providerName(source.kind)} · {source.enabled ? source.health.replaceAll('_', ' ') : 'paused'} · {scopeText(source)}</p><p>{coverageText(source.coverage)}</p>
          <p>{source.capabilities.guardedWrite ? 'Guarded editing is available for selected current revisions.' : 'This connection is read only. Provider editing is unavailable for this grant.'}</p>
          <button type="button" disabled={busy || !source.enabled || source.selectedScope.kind === 'imported_legacy_broad'} onClick={() => { setError(null); setNotice(null); setSelected(source.sourceId); setBrowseGeneration((value) => value + 1); }}>Browse documents</button>
          <button type="button" disabled={busy || !source.enabled || !source.capabilities.list} onClick={() => refresh(source)}>Refresh metadata</button>
          <button type="button" disabled={busy || !source.enabled} onClick={() => changeSource(source, false)}>Pause source</button>
          <button type="button" disabled={busy || !canConnect(source.kind) || source.kind === 'dropbox' && !validDropboxPath(dropboxPath)} onClick={() => reconnect(source)}>Reconnect source</button>
          <button type="button" disabled={busy} onClick={() => changeSource(source, true)}>Forget cached documents</button>
        </li>)}</ul>
        {current ? <section aria-label={`Documents in ${current.name}`}><h3>Documents in {current.name}</h3><p>Metadata coverage and cached observation times do not prove the current remote head.</p>
          {documentsLoading ? <p role="status">Loading document metadata…</p> : null}
          {page?.documents.length === 0 ? <p>No published documents. Refresh this source’s metadata to enumerate the selected scope.</p> : null}
          <ul>{page?.documents.map((document) => <li key={document.documentId} aria-label={document.name}><strong>{document.name}</strong><p>{document.displayPath}</p><p>{document.admitted ? 'Exact cached revision admitted' : document.cached ? 'Cached body awaiting admission' : 'Body not cached'}{document.cachedObservedAtMs !== null ? ` · Observed ${new Date(document.cachedObservedAtMs).toLocaleString()}` : ''}</p>
            <button type="button" disabled={busy || !document.admitted || !document.projection || !onOpen} onClick={() => open(current, document, true)}>Read cached revision</button>
            <button type="button" disabled={busy || !current.enabled || !current.capabilities.read || !onOpen} onClick={() => open(current, document, false)}>Fetch current revision</button>
          </li>)}</ul>
          {page?.nextOffset !== null && page?.nextOffset !== undefined ? <button type="button" disabled={busy || documentsLoading} onClick={more}>Load more documents</button> : null}
        </section> : null}
      </> : null}
    </> : null}
  </section>;
}
