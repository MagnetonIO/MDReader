import { useEffect, useRef, useState } from 'react';
import { nativeProvidersBackend, NativeProviderError, providerErrorMessage, type NativeProvidersBackend, type NativeProvidersSnapshot, type ProviderDocuments, type ProviderSource, type ProviderReaderReference, type ProviderDocument } from '../platform/providers';
import { isCanonicalProviderSource, providerProjectionReference } from './provider-reading';

type Backend = Pick<NativeProvidersBackend, 'state' | 'documents' | 'readCached' | 'fetchCurrent'>;
const errorMessage = (cause: unknown) => cause instanceof NativeProviderError ? cause.message : 'The cloud source operation could not be completed.';
export function ProviderSourcesSidebar({ backend = nativeProvidersBackend, selectedSources, onSelectionChange, onOpen, onManage, refreshKey = 0 }: {
  backend?: Backend;
  selectedSources: readonly string[];
  onSelectionChange: (sources: string[]) => void;
  onOpen: (reference: ProviderReaderReference) => void;
  onManage: () => void;
  refreshKey?: number;
}) {
  const [snapshot, setSnapshot] = useState<NativeProvidersSnapshot | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [expanded, setExpanded] = useState<string | null>(null);
  const [page, setPage] = useState<ProviderDocuments | null>(null);
  const [busy, setBusy] = useState(false);
  const [reload, setReload] = useState(0);
  const lifecycle = useRef(0);
  const documentRequest = useRef(0);
  const opening = useRef(false);
  const currentSource = useRef<string | null>(null);
  const selected = useRef(selectedSources);
  selected.current = selectedSources;
  useEffect(() => {
    const epoch = ++lifecycle.current;
    ++documentRequest.current;
    currentSource.current = null;
    setExpanded(null); setPage(null); setError(null); setLoading(true); setBusy(false);
    void backend.state().then((value) => {
      if (epoch !== lifecycle.current) return;
      setSnapshot(value);
      if (value.error) setError(providerErrorMessage(value.error));
      const available = new Set(value.sources.filter((source) => source.enabled && source.readerSourceId && isCanonicalProviderSource(source.readerSourceId)).map((source) => source.readerSourceId));
      const next = selected.current.filter((id) => available.has(id));
      if (next.length !== selected.current.length) onSelectionChange(next);
    }).catch((cause: unknown) => { if (epoch === lifecycle.current) { setSnapshot(null); setError(errorMessage(cause)); onSelectionChange([]); } })
      .finally(() => { if (epoch === lifecycle.current) setLoading(false); });
    return () => { ++lifecycle.current; ++documentRequest.current; currentSource.current = null; };
  }, [backend, onSelectionChange, refreshKey, reload]);
  async function browse(source: ProviderSource, more = false) {
    if (!source.enabled) return;
    const epoch = lifecycle.current; const request = ++documentRequest.current;
    currentSource.current = source.sourceId; setExpanded(source.sourceId); setError(null); setBusy(true);
    if (!more) setPage(null);
    try {
      const value = await backend.documents(source.sourceId, more ? page?.nextOffset ?? 0 : 0, 50);
      if (epoch === lifecycle.current && request === documentRequest.current && currentSource.current === source.sourceId) setPage((previous) => more && previous ? { ...value, documents: [...previous.documents, ...value.documents] } : value);
    } catch (cause) { if (epoch === lifecycle.current && request === documentRequest.current) setError(errorMessage(cause)); }
    finally { if (epoch === lifecycle.current && request === documentRequest.current) setBusy(false); }
  }
  async function open(source: ProviderSource, document: ProviderDocument, cached: boolean) {
    if (opening.current || !source.enabled) return;
    opening.current = true; setBusy(true); setError(null);
    const epoch = lifecycle.current;
    try {
      const projection = await (cached ? backend.readCached(source.sourceId, document.documentId) : backend.fetchCurrent(source.sourceId, document.documentId));
      if (epoch !== lifecycle.current || currentSource.current !== source.sourceId) return;
      if (source.readerSourceId && projection.readerSourceId !== source.readerSourceId) throw new Error('Wrong source');
      onOpen(providerProjectionReference(projection, document.name));
      if (!cached) setReload((value) => value + 1);
    } catch (cause) { if (epoch === lifecycle.current) setError(errorMessage(cause)); }
    finally { opening.current = false; if (epoch === lifecycle.current) setBusy(false); }
  }
  function select(source: ProviderSource, checked: boolean) {
    if (!source.enabled || !source.readerSourceId || !isCanonicalProviderSource(source.readerSourceId)) return;
    const next = selectedSources.filter((id) => id !== source.readerSourceId);
    if (checked) { if (next.length >= 64) { setError('Choose at most 64 cloud sources for search.'); return; } next.push(source.readerSourceId); }
    onSelectionChange(next);
  }
  return <section className="provider-sources" aria-label="Selected cloud sources">
    <header><strong>Cloud sources</strong><button type="button" onClick={onManage}>Manage</button><button type="button" disabled={loading || busy} onClick={() => setReload((value) => value + 1)}>Reload sources</button></header>
    <p role="status">{selectedSources.length ? `${selectedSources.length} cloud source${selectedSources.length === 1 ? '' : 's'} included in search, together with the active folder.` : 'Search uses the active folder. Choose cloud sources below to include their cached documents.'}</p>
    {error ? <p role="alert">{error}</p> : null}
    {loading ? <p>Reading sources…</p> : null}
    {!loading && snapshot?.sources.length === 0 ? <p>No selected cloud sources. Connect one in Settings.</p> : null}
    {snapshot?.sources.map((source) => <section key={source.sourceId} aria-label={source.name}>
      <strong>{source.name}</strong><small>{source.kind === 'google_drive' ? 'Google Drive' : 'Dropbox'} · {source.enabled ? source.health : 'Paused'}</small>
      <label><input type="checkbox" checked={Boolean(source.readerSourceId && selectedSources.includes(source.readerSourceId))} disabled={!source.enabled || !source.readerSourceId || !isCanonicalProviderSource(source.readerSourceId)} onChange={(event) => select(source, event.currentTarget.checked)} />Include {source.name} in search</label>
      <p>{source.coverage.kind === 'never_indexed' ? 'No documents indexed yet.' : source.coverage.kind === 'partial' ? `Partial coverage: ${source.coverage.reason}` : 'Showing selected metadata. Search includes admitted cached bodies only.'}</p>
      <button type="button" disabled={!source.enabled || busy} aria-expanded={expanded === source.sourceId} onClick={() => void browse(source)}>Browse {source.name}</button>
      {expanded === source.sourceId ? <>
        <p>Cached revisions reflect their last observation. Fetch current explicitly to check the provider.</p>
        {page?.documents.map((document) => <div key={document.documentId}>
          <strong>{document.name}</strong><small>{document.displayPath}</small>
          <button type="button" disabled={busy || !document.admitted || !document.projection} onClick={() => void open(source, document, true)}>Read cached {document.name}</button>
          <button type="button" disabled={busy || !source.capabilities.read} onClick={() => void open(source, document, false)}>Fetch current {document.name}</button>
        </div>)}
        {page?.nextOffset != null ? <button type="button" disabled={busy} onClick={() => void browse(source, true)}>More selected documents</button> : null}
        {page && page.documents.length === 0 ? <p>No published selected metadata. Refresh this source in Settings.</p> : null}
      </> : null}
    </section>)}
  </section>;
}
