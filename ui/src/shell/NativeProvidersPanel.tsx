import { useEffect, useId, useRef, useState } from 'react';
import { ProviderConnectionForms } from '../features/settings/ProviderConnectionForms.js';
import { ProviderIcon } from '../features/providers/ProviderIcon.js';
import { ProviderSearchSelection } from '../features/providers/ProviderSearchSelection';
import { DriveFolderBrowser } from '../features/providers/DriveFolderBrowser';
import '../features/settings/providers.css';
import {
  nativeProvidersBackend, NativeProviderError, providerErrorMessage, providerConnectionErrorMessage, providerConnectionDiagnostic, type ProviderConnectionDiagnostic,
  type NativeProvidersBackend, type NativeProvidersSnapshot, type ProviderReaderReference,
  type NativeProviderKind, type ProviderAdmission, type ProviderDocuments, type ProviderSource,
  type DropboxSelection, type DriveFolderSelection, type ProviderDocument, type ProviderCoverage,
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
    case 'scoped_root': return source.selectedScope.recursive ? 'Selected Dropbox folder and its subfolders' : 'Selected Dropbox folder';
    case 'drive_readonly_folder': return source.selectedScope.accountWide ? 'Drive Markdown · Read only' : 'Selected Drive folder · Direct Markdown files · Read only';
    case 'drive_folder': return `${source.selectedScope.folderId === 'root' ? 'My Drive' : 'Selected Drive folder'}${source.selectedScope.recursive ? ' and subfolders' : ' · Direct Markdown files'}`;
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
  initiallyExpanded?: boolean;
  /** Platforms without a guarded-write adapter cannot request write permission. */
  allowEditing?: boolean;
  selectedProviderSources?: readonly string[];
  onProviderSelectionChange?: (sources: string[]) => void;
  /** Changing the active library invalidates late replies and open callbacks. */
  profileKey?: string;
  onOpen?: (reference: ProviderReaderReference) => void | Promise<void>;
  onSourcesChanged?: () => void;
}
/** Native custody owns OAuth and account/selection authority. Opening this panel is read-only. */
export function NativeProvidersPanel({ backend = nativeProvidersBackend, profileKey = 'current', initiallyExpanded = false, allowEditing = true, selectedProviderSources = [], onProviderSelectionChange, onOpen, onSourcesChanged }: NativeProvidersPanelProps) {
  const titleId = useId();
  const [expanded, setExpanded] = useState(initiallyExpanded);
  const [snapshot, setSnapshot] = useState<NativeProvidersSnapshot | null>(null);
  const [selected, setSelected] = useState<string | null>(null);
  const [browseGeneration, setBrowseGeneration] = useState(0);
  const [page, setPage] = useState<ProviderDocuments | null>(null);
  const [loading, setLoading] = useState(false);
  const [documentsLoading, setDocumentsLoading] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [connectionDiagnostic, setConnectionDiagnostic] = useState<ProviderConnectionDiagnostic | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [dropboxPath, setDropboxPath] = useState('/');
  const [recursive, setRecursive] = useState(false);
  const [dropboxEditing, setDropboxEditing] = useState(false);
  const [driveRecursive, setDriveRecursive] = useState(true);
  const [driveEditing, setDriveEditing] = useState(false);
  const [folderSource, setFolderSource] = useState<string | null>(null);
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
    setDropboxEditing(false); setDriveEditing(false); setDriveRecursive(true); setFolderSource(null); setSnapshot(null); setSelected(null); setPage(null); setError(null); setConnectionDiagnostic(null); setNotice(null); setBusy(false); setDocumentsLoading(false);
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
  async function run(work: (epoch: number) => Promise<void>, connection?: NativeProviderKind) {
    if (busy) return;
    const epoch = lifecycle.current;
    const id = ++action.current;
    setBusy(true); setError(null); setConnectionDiagnostic(null);
    setNotice(connection === 'google_drive' ? 'Finish the Google permission request, then return to MDReader.'
      : connection === 'dropbox' ? 'Finish signing in to Dropbox in your browser, then return here.' : null);
    try { await work(epoch); }
    catch (cause) {
      if (lifecycle.current === epoch && action.current === id) {
        setNotice(null);
        setConnectionDiagnostic(connection ? providerConnectionDiagnostic(cause) : null);
        setError(connection ? providerConnectionErrorMessage(cause, connection) : message(cause));
      }
    }
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
    if (kind === 'google_drive') { connectDriveFolder({ id: 'root' }); return; }
    void run(async (epoch) => acceptAdmission(await backend.connect(kind, dropboxSelection(), allowEditing && dropboxEditing), epoch), kind);
  }
  function connectDriveFolder(folder: DriveFolderSelection) {
    if (!canConnect('google_drive')) return;
    void run(async (epoch) => {
      await acceptAdmission(await backend.connect('google_drive', null, allowEditing && driveEditing, { id: folder.id, recursive: driveRecursive }), epoch);
      if (lifecycle.current === epoch) setFolderSource(null);
    }, 'google_drive');
  }
  function connectGoogleFiles() {
    if (canConnect('google_drive')) void run(async (epoch) => acceptAdmission(await backend.connect('google_drive', null, false), epoch), 'google_drive');
  }
  function reconnect(source: ProviderSource) {
    if (!canConnect(source.kind) || source.kind === 'dropbox' && !validDropboxPath(dropboxPath)) return;
    if (source.kind === 'google_drive' && source.selectedScope.kind === 'drive_readonly_folder') {
      const folder = { id: source.selectedScope.folderId };
      void run(async (epoch) => acceptAdmission(await backend.reconnect(source.sourceId, null, false, folder), epoch), source.kind);
      return;
    }
    if (source.kind === 'google_drive' && source.selectedScope.kind === 'drive_folder') {
      const folder = { id: source.selectedScope.folderId, recursive: source.selectedScope.recursive };
      void run(async epoch => acceptAdmission(await backend.reconnect(source.sourceId, null, allowEditing && source.capabilities.guardedWrite, folder), epoch), source.kind);
      return;
    }
    void run(async (epoch) => acceptAdmission(await backend.reconnect(source.sourceId, source.kind === 'google_drive' ? null : dropboxSelection(), allowEditing && source.kind === 'dropbox' && dropboxEditing), epoch), source.kind);
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
    <div className="settings-section__intro"><h2 id={titleId}>Cloud sources</h2><p>Add files from Google Drive or Dropbox to your library. Device sync is managed separately in Account &amp; sync.</p></div>
    <div className="provider-panel__toolbar"><button type="button" aria-expanded={expanded} onClick={() => setExpanded((value) => !value)}>{expanded ? 'Hide cloud sources' : 'Manage cloud sources'}</button>
      {expanded ? <button type="button" disabled={busy || loading} onClick={() => void run(updateSnapshot)}>Refresh status</button> : null}
    </div>
    {expanded ? <>
      {loading ? <p role="status">Loading provider status…</p> : null}
      {error ? <p role="alert">{error}</p> : null}
      {error && connectionDiagnostic ? <details><summary>Connection details</summary>
        <p>Step: {connectionDiagnostic.phase.replaceAll('_', ' ')}{connectionDiagnostic.responseStatus !== null ? ` · HTTP ${connectionDiagnostic.responseStatus}` : ''}{connectionDiagnostic.rejection !== null ? ` · ${connectionDiagnostic.rejection.replaceAll('_', ' ')}` : ''}</p>
      </details> : null}
      {snapshot?.error ? <p role="alert">{providerErrorMessage(snapshot.error)}</p> : null}
      {notice ? <p role="status">{notice}</p> : null}
      {snapshot ? <>
        <ProviderConnectionForms snapshot={snapshot} busy={busy || loading} canConnect={canConnect}
          dropboxPath={dropboxPath} pathValid={validDropboxPath(dropboxPath)} recursive={recursive} editing={dropboxEditing} allowEditing={allowEditing}
          driveRecursive={driveRecursive} driveEditing={driveEditing} onDriveRecursive={setDriveRecursive} onDriveEditing={setDriveEditing}
          onPath={setDropboxPath} onRecursive={setRecursive} onEditing={setDropboxEditing} onConnect={connect} onConnectGoogleFiles={connectGoogleFiles} />
        {snapshot.pendingAdmissions.length ? <section aria-label="Pending connections"><h3>Pending connections</h3>{snapshot.pendingAdmissions.map((attempt) => <div key={attempt.attemptId}><p>{attempt.name} — {attempt.phase === 'vault_pending' ? 'Credential custody outcome pending' : 'Source admission outcome pending'}</p><button type="button" disabled={busy} onClick={() => void run(async (epoch) => acceptAdmission(await backend.reconcile(attempt.attemptId), epoch))}>Reconcile connection</button></div>)}</section> : null}
        {!snapshot.error && !snapshot.sources.length ? <p className="provider-panel__empty">No cloud sources connected yet.</p> : null}
        <ul className="provider-source-list">{snapshot.sources.map((source) => <li key={source.sourceId} aria-label={source.name}>
          <h3 className="provider-source-list__title"><ProviderIcon provider={source.kind} />{source.name}</h3><p>{providerName(source.kind)} · {source.enabled && source.health === 'ready' && source.selectedScope.kind !== 'imported_legacy_broad' ? 'Connected' : source.enabled ? source.health.replaceAll('_', ' ') : 'Paused'} · {scopeText(source)}</p><p>{coverageText(source.coverage)}</p>
          <p>{!allowEditing ? 'Provider documents are read only on this device.' : source.capabilities.guardedWrite ? 'Guarded editing is available for selected current revisions.' : 'This connection is read only. Provider editing is unavailable for this grant.'}</p>
          {onProviderSelectionChange ? <ProviderSearchSelection source={source} selected={selectedProviderSources} onChange={onProviderSelectionChange} onError={setError} /> : null}
          <div className="provider-panel__toolbar"><button type="button" disabled={busy || !source.enabled || source.selectedScope.kind === 'imported_legacy_broad'} onClick={() => { setError(null); setNotice(null); setSelected(source.sourceId); setBrowseGeneration((value) => value + 1); }}>Browse documents</button>
          <button type="button" disabled={busy || !source.enabled || !source.capabilities.list} onClick={() => refresh(source)}>Refresh metadata</button>
          <button type="button" disabled={busy || !source.enabled} onClick={() => changeSource(source, false)}>Pause source</button>
          <button type="button" disabled={busy || !canConnect(source.kind) || source.kind === 'dropbox' && !validDropboxPath(dropboxPath)} onClick={() => reconnect(source)}>Reconnect source</button>
          <button type="button" disabled={busy} onClick={() => changeSource(source, true)}>Forget cached documents</button></div>
          {source.enabled && source.kind === 'google_drive' && (source.selectedScope.kind === 'drive_folder' || source.selectedScope.kind === 'drive_readonly_folder' && source.selectedScope.accountWide) && source.selectedScope.folderId === 'root' && source.capabilities.browseFolders === true && backend.browseFolders ? <>
            <button type="button" disabled={busy} aria-expanded={folderSource === source.sourceId} onClick={() => setFolderSource(value => value === source.sourceId ? null : source.sourceId)}>Connect a Drive folder</button>
            {folderSource === source.sourceId ? <DriveFolderBrowser key={`${profileKey}:${source.sourceId}`} sourceId={source.sourceId} backend={backend} busy={busy}
              recursive={driveRecursive} allowEditing={allowEditing && driveEditing} onChoose={connectDriveFolder} /> : null}
          </> : null}
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
