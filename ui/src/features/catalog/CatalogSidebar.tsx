import './catalog.css';
import { useEffect, useMemo, useRef, useState } from 'react';
import {
  catalogBackend,
  catalogReference,
  catalogErrorMessage,
  type CatalogBackend,
  type CatalogDocument,
  type CatalogPage,
  type CatalogSource,
} from './backend';
import {
  nativeProvidersBackend,
  type NativeProvidersBackend,
} from '../../platform/providers';
import type { CanonicalReaderReference } from '../reader/contracts';
import { CatalogDocumentList } from './CatalogDocumentList';
import { CatalogSourceRow } from './CatalogSourceRow';
import { loadMetadata, type MetadataProgress } from './metadata-loading';
import { retainPublishedRows, appendPublishedRows } from './published-pages';
import { mergeConnectedSources } from './connected-sources';

const message = (error: unknown) =>
  error instanceof Error ? error.message : catalogErrorMessage('unavailable');

type CatalogSidebarProps = {
  backend?: CatalogBackend;
  providers?: Pick<NativeProvidersBackend, 'fetchCurrent'> &
    Partial<Pick<NativeProvidersBackend, 'state' | 'refresh'>>;
  selectedProviderSources?: readonly string[];
  onProviderSelectionChange?: (sources: string[]) => void;
  onOpen: (reference: CanonicalReaderReference) => void;
  onManage?: () => void;
  exactCopiesOnly?: boolean;
  refreshKey?: number | string;
};

export function CatalogSidebar({
  backend = catalogBackend,
  providers = nativeProvidersBackend,
  onOpen,
  onManage,
  exactCopiesOnly = false,
  refreshKey = 0,
  selectedProviderSources = [],
  onProviderSelectionChange,
}: CatalogSidebarProps) {
  const [catalogSources, setSources] = useState<CatalogSource[]>([]);
  const [connectedSources, setConnectedSources] = useState<CatalogSource[]>([]);
  const sources = useMemo(() => mergeConnectedSources(catalogSources, connectedSources), [catalogSources, connectedSources]);
  const [page, setPage] = useState<CatalogPage | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [sourceError, setSourceError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [reload, setReload] = useState(0);
  const [metadataProgress, setMetadataProgress] = useState<MetadataProgress | null>(null);
  const metadataStop = useRef(false);
  const metadataFlight = useRef<Promise<boolean> | null>(null);
  const [incomplete, setIncomplete] = useState<Record<string, boolean>>({});
  useEffect(() => { setIncomplete({}); }, [backend, providers, refreshKey]);
  const [searchError, setSearchError] = useState<string | null>(null);
  const confirmedBindings = useRef<Record<string, string>>({});
  const selected = useRef(selectedProviderSources);
  selected.current = selectedProviderSources;

  useEffect(() => {
    if (!providers.state) return;
    let current = true;
    confirmedBindings.current = {};
    setConnectedSources([]);
    setSearchError(null);
    void providers.state().then((value) => {
      if (!current) return;
      setConnectedSources(value.sources.map((source) => ({
        catalogId: `remote:${source.sourceId}`, name: source.name, kind: source.kind,
        enabled: source.enabled, librarySourceId: null, failure: null,
        coverage: source.coverage.kind === 'never_indexed' ? 'never_indexed'
          : source.coverage.kind === 'partial' ? 'partial_selected_metadata' : 'selected_metadata',
        observedAtMs: 'observedAtMs' in source.coverage ? source.coverage.observedAtMs : null,
      })));
      const mapping = Object.fromEntries(value.sources
        .filter((source) => source.enabled && source.readerSourceId &&
          /^provider:[a-f0-9]{64}$/.test(source.readerSourceId))
        .map((source) => [source.sourceId, source.readerSourceId as string]));
      for (const source of value.sources) {
        const confirmed = confirmedBindings.current[source.sourceId];
        if (source.enabled && confirmed && (!source.readerSourceId || source.readerSourceId === confirmed)) {
          mapping[source.sourceId] = confirmed;
        }
      }
      const allowed = new Set(Object.values(mapping));
      const next = selected.current.filter((id) => allowed.has(id));
      if (next.length !== selected.current.length) onProviderSelectionChange?.(next);
      if (value.error) {
        setSearchError('Cloud search scope could not be completely confirmed.');
      }
    }).catch(() => {
      if (current) {
        setSearchError('Cloud search scope is unavailable.');
        onProviderSelectionChange?.([]);
      }
    });
    return () => { current = false; };
  }, [providers, onProviderSelectionChange, refreshKey, reload]);

  const epoch = useRef(0);
  const turn = useRef(0);
  const cursor = useRef<string | null>(null);
  const active = useRef<string | null>(null);
  const [activeSource, setActiveSource] = useState<string | null>(null);

  function closeCursor() {
    const value = cursor.current;
    cursor.current = null;
    if (value) void backend.close(value).catch(() => {});
  }

  useEffect(() => {
    const generation = ++epoch.current;
    ++turn.current;
    active.current = null;
    setActiveSource(null);
    const old = cursor.current;
    cursor.current = null;
    if (old) void backend.close(old).catch(() => {});
    metadataStop.current = true;
    setMetadataProgress(null);
    setPage(null);
    setSourceError(null);
    setSources([]);
    setError(null);
    setBusy(true);
    void backend.list().then((value) => {
      if (epoch.current !== generation) return;
      setSources(value.sources);
      const errors = [
        value.localFailure
          ? `Folder catalog: ${catalogErrorMessage(value.localFailure)}` : null,
        value.providerFailure
          ? `Cloud catalog: ${catalogErrorMessage(value.providerFailure)}` : null,
      ].filter(Boolean);
      if (errors.length) setError(errors.join(' '));
    }).catch((cause: unknown) => {
      if (epoch.current === generation) setError(message(cause));
    }).finally(() => {
      if (epoch.current === generation) setBusy(false);
    });
    return () => {
      ++epoch.current;
      ++turn.current;
      active.current = null;
      const value = cursor.current;
      cursor.current = null;
      if (value) void backend.close(value).catch(() => {});
    };
  }, [backend, providers, refreshKey, reload]);

  function stopMetadata() {
    metadataStop.current = true;
    setMetadataProgress(previous => previous ? { ...previous, stopping: true } : previous);
  }

  async function refreshMetadata(source: CatalogSource, generation: number, request: number) {
    const refresh = providers.refresh;
    if (!refresh) return;
    const current = () => generation === epoch.current && request === turn.current;
    metadataStop.current = false;
    setMetadataProgress({ sourceId: source.catalogId, batches: 0, pages: 0, stopping: false });
    const preceding = metadataFlight.current;
    let complete = false;
    const run = (async () => {
      if (preceding) await preceding.catch(() => {});
      if (!current() || metadataStop.current) return false;
      await loadMetadata({
        refresh: () => refresh(source.catalogId.slice(7)),
        page: () => backend.page(source.catalogId, null, 10),
        current,
        retire: value => { if (value.nextCursor) void backend.close(value.nextCursor).catch(() => {}); },
        stopped: () => metadataStop.current,
        progress: (batches, pages) => { if (current()) setMetadataProgress(previous => previous ? { ...previous, batches, pages } : previous); },
        observed: settled => { complete = settled; if (current()) setIncomplete(previous => ({ ...previous, [source.catalogId]: !settled })); },
        publish: value => {
          if (value.source.catalogId !== source.catalogId || !value.source.enabled || value.source.failure) {
            if (value.nextCursor) void backend.close(value.nextCursor).catch(() => {});
            throw new Error('The catalog source could not be verified.');
          }
          const oldCursor = cursor.current;
          cursor.current = value.nextCursor;
          if (oldCursor && oldCursor !== value.nextCursor) void backend.close(oldCursor).catch(() => {});
          if (active.current === source.catalogId) setPage(value);
          setSources(old => old.map(row => row.catalogId === source.catalogId ? value.source : row));
        },
      });
      return complete;
    })();
    metadataFlight.current = run;
    try { return await run; }
    finally {
      if (metadataFlight.current === run) {
        metadataFlight.current = null;
        if (generation === epoch.current) setMetadataProgress(null);
      }
    }
  }

  async function browse(source: CatalogSource, more = false, options: { preserveRows?: boolean; preserveError?: boolean; retainRows?: boolean; refreshCold?: boolean } = {}) {
    if (!source.enabled || source.failure) return;
    const generation = epoch.current;
    const request = ++turn.current;
    active.current = source.catalogId;
    setActiveSource(source.catalogId);
    setBusy(true);
    if (!options.preserveError) setSourceError(null);
    let refreshingMetadata = false;
    const previous = more ? cursor.current : null;
    if (!more) {
      closeCursor();
      if (!options.preserveRows) setPage(null);
    }
    try {
      let value = await backend.page(source.catalogId, previous, source.catalogId.startsWith('remote:') ? 10 : 100);
      if (generation !== epoch.current || request !== turn.current || active.current !== source.catalogId) {
        if (value.nextCursor) void backend.close(value.nextCursor).catch(() => {});
        return;
      }
      if (value.source.catalogId !== source.catalogId || !value.source.enabled || value.source.failure) {
        throw new Error('The catalog source could not be verified.');
      }
      // Browse is explicit permission to list the existing selected scope, never to fetch bodies.
      if (options.refreshCold !== false && !more && !exactCopiesOnly && ['never_indexed', 'partial_selected_metadata'].includes(value.source.coverage) &&
        source.catalogId.startsWith('remote:') && providers.refresh) {
        if (value.nextCursor) await backend.close(value.nextCursor);
        setPage({ ...value, nextCursor: null });
        refreshingMetadata = true;
        await refreshMetadata(source, generation, request);
        return;
      }
      if (generation !== epoch.current || request !== turn.current ||
        active.current !== source.catalogId) {
        if (value.nextCursor) void backend.close(value.nextCursor).catch(() => {});
        return;
      }
      if (value.source.catalogId !== source.catalogId || !value.source.enabled || value.source.failure) {
        throw new Error('The catalog source could not be verified.');
      }
      cursor.current = value.nextCursor;
      setPage((old) => old?.source.catalogId === source.catalogId
        ? more ? { ...value, documents: appendPublishedRows(old.documents, value.documents) }
          : options.retainRows ? { ...value, documents: retainPublishedRows(old.documents, value.documents) } : value
        : value);
    } catch (cause) {
      if (generation === epoch.current && request === turn.current) {
        closeCursor();
        if (!options.preserveRows && !refreshingMetadata) setPage(null);
        setSourceError(message(cause));
      }
    } finally {
      if (generation === epoch.current && request === turn.current) setBusy(false);
    }
  }

  async function refreshDocuments(source: CatalogSource) {
    if (busy || !providers.refresh || !source.enabled || source.failure || !source.catalogId.startsWith('remote:')) return;
    const generation = epoch.current;
    const request = ++turn.current;
    active.current = source.catalogId;
    setActiveSource(source.catalogId);
    setBusy(true);
    setSourceError(null);
    closeCursor();
    setPage(old => old?.source.catalogId === source.catalogId ? { ...old, nextCursor: null } : old);
    try {
      // Explicit metadata refresh stays within the already selected provider scope.
      await refreshMetadata(source, generation, request);
    } catch (cause) {
      if (generation === epoch.current && request === turn.current) {
        if (active.current === source.catalogId) setSourceError(message(cause));
        else setError(`${source.name}: ${message(cause)}`);
      }
    } finally {
      if (generation === epoch.current && request === turn.current) setBusy(false);
    }
  }

  async function open(document: CatalogDocument) {
    if (busy && !metadataProgress || !page || exactCopiesOnly && document.open.kind === 'selected_uncached') return;
    if (document.open.kind === 'exact' || document.open.kind === 'device') {
      try {
        onOpen(catalogReference(document.open, document.name));
      } catch (cause) {
        setSourceError(message(cause));
      }
      return;
    }
    const generation = epoch.current;
    const request = ++turn.current;
    const source = page.source.catalogId;
    const continueDiscovery = metadataProgress?.sourceId === source && !metadataProgress.stopping;
    const observedSource = page.source;
    let discoveryCompleted = false;
    setBusy(true);
    setSourceError(null);
    try {
      if (metadataFlight.current) {
        stopMetadata();
        discoveryCompleted = await metadataFlight.current.catch(() => false);
        if (generation !== epoch.current || request !== turn.current || active.current !== source) return;
      }
      if (source !== `remote:${document.open.sourceId}`) {
        throw new Error('The fetched document binding could not be verified.');
      }
      // A document body is downloaded only for this explicit file click.
      const projection = await providers.fetchCurrent(
        document.open.sourceId, document.open.documentId,
      );
      if (generation !== epoch.current || request !== turn.current ||
        active.current !== source) return;
      if (source !== `remote:${document.open.sourceId}` ||
        projection.readerSourceId !== document.open.readerSourceId ||
        !/^provider:[a-f0-9]{64}$/.test(projection.readerSourceId)) {
        throw new Error('The fetched document binding could not be verified.');
      }
      const exact = { kind: 'exact' as const, sourceId: projection.readerSourceId,
        path: projection.path, revision: projection.revision };
      const reference = catalogReference(exact, document.name);
      // Only a validated native publication receipt can enable cloud search.
      confirmedBindings.current[document.open.sourceId] = projection.readerSourceId;
      closeCursor(); // Cache publication invalidated pagination, not the visible rows.
      setPage((old) => old?.source.catalogId === source ? { ...old, nextCursor: null,
        documents: old.documents.map((row) => row === document
          ? { ...row, open: exact, observation: 'cached_provider_revision' } : row) } : old);
      onOpen(reference);
    } catch (cause) {
      if (generation === epoch.current && request === turn.current) {
        setSourceError(message(cause));
      }
    } finally {
      if (continueDiscovery && generation === epoch.current && request === turn.current && active.current === source) {
        try {
          if (discoveryCompleted) {
            setIncomplete(previous => ({ ...previous, [source]: false }));
            await browse(observedSource, false, { preserveRows: true, preserveError: true, refreshCold: false });
          } else await refreshMetadata(observedSource, generation, request);
        }
        catch (cause) { if (generation === epoch.current && request === turn.current) setSourceError(message(cause)); }
      }
      if (generation === epoch.current && request === turn.current) setBusy(false);
    }
  }

  return (
    <section className="reader-catalog" aria-label="Reader catalog">
      <header className="reader-catalog__header">
        <h2>Library</h2>
        <div className="reader-catalog__actions">
          <button className="reader-catalog__refresh" type="button" disabled={busy}
            aria-label="Refresh library" title="Refresh library" onClick={() => setReload((n) => n + 1)}>
            <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor"
              strokeWidth="1.7" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
              <path d="M20 7v5h-5M4 17v-5h5" />
              <path d="M6 7a7 7 0 0 1 12-1l2 3M4 15l2 3a7 7 0 0 0 12-1" />
            </svg>
          </button>
          {onManage ? <button type="button" onClick={onManage}>Add source</button> : null}
        </div>
      </header>
      {searchError ? <p role="alert">{searchError}</p> : null}
      {error ? <p role="alert">{error}</p> : null}
      {busy && !sources.length ? <p role="status">Loading your library…</p> : null}
      {!busy && !sources.length ? <p className="reader-catalog__empty">{exactCopiesOnly ? 'No existing sources are available. Add or connect a source from Settings before matching this entry.' : 'Add a folder or connect a cloud source to start reading.'}</p> : null}
      {sources.map((source) => {
        const expanded = activeSource === source.catalogId;
        const observedSource = page?.source.catalogId === source.catalogId ? page.source : source;
        return <CatalogSourceRow key={source.catalogId} source={observedSource}
          expanded={expanded} busy={busy && !metadataProgress} refreshBusy={busy} exactCopiesOnly={exactCopiesOnly}
          canManage={Boolean(onManage)}
          onContinue={!metadataProgress && !exactCopiesOnly && incomplete[source.catalogId] && providers.refresh ? () => void refreshDocuments(observedSource) : undefined}
          onRefresh={!exactCopiesOnly && source.catalogId.startsWith('remote:') && providers.refresh ? () => void refreshDocuments(observedSource) : undefined}
          onBrowse={() => {
            if (metadataProgress) stopMetadata();
            if (source.coverage === 'needs_activation') onManage?.();
            else if (expanded) {
              ++turn.current;
              active.current = null;
              setActiveSource(null);
              setBusy(false);
              closeCursor();
              setPage(null);
            } else void browse(source);
          }}>
          {expanded && metadataProgress?.sourceId === source.catalogId ? <div className="reader-catalog__loading">
            <p role="status">{metadataProgress.stopping ? 'Stopping after the current batch…'
              : page?.source.catalogId === source.catalogId && page.documents.length ? 'Finding more files…' : 'Finding Markdown files…'}</p>
            <button type="button" disabled={metadataProgress.stopping} onClick={stopMetadata}>Stop loading</button>
          </div> : expanded && busy ? <p role="status">Loading documents…</p> : null}
          {expanded && sourceError ? <div>
            <p role="alert">{sourceError}</p>
            {!busy ? <button type="button" onClick={() => { if (!exactCopiesOnly && source.catalogId.startsWith('remote:') && providers.refresh) void refreshDocuments(source); else void browse(source); }}>Retry documents</button> : null}
          </div> : null}
          {expanded && page?.source.catalogId === source.catalogId ? <>
            <CatalogDocumentList page={page} busy={busy && !metadataProgress} pagingBusy={busy} pending={Boolean(incomplete[source.catalogId])} finding={metadataProgress?.sourceId === source.catalogId} exactCopiesOnly={exactCopiesOnly}
              onOpen={(document) => void open(document)} onMore={() => void browse(page.source, true)} />
          </> : null}
        </CatalogSourceRow>;
      })}
    </section>
  );
}
