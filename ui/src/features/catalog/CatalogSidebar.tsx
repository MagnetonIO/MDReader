import './catalog.css';
import { useEffect, useRef, useState } from 'react';
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

const message = (error: unknown) =>
  error instanceof Error ? error.message : catalogErrorMessage('unavailable');

type CatalogSidebarProps = {
  backend?: CatalogBackend;
  providers?: Pick<NativeProvidersBackend, 'fetchCurrent'> &
    Partial<Pick<NativeProvidersBackend, 'state'>>;
  selectedProviderSources?: readonly string[];
  onProviderSelectionChange?: (sources: string[]) => void;
  onOpen: (reference: CanonicalReaderReference) => void;
  onManage: () => void;
  refreshKey?: number | string;
};

export function CatalogSidebar({
  backend = catalogBackend,
  providers = nativeProvidersBackend,
  onOpen,
  onManage,
  refreshKey = 0,
  selectedProviderSources = [],
  onProviderSelectionChange,
}: CatalogSidebarProps) {
  const [sources, setSources] = useState<CatalogSource[]>([]);
  const [page, setPage] = useState<CatalogPage | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [reload, setReload] = useState(0);
  const [searchSources, setSearchSources] = useState<Record<string, string>>({});
  const [searchError, setSearchError] = useState<string | null>(null);
  const selected = useRef(selectedProviderSources);
  selected.current = selectedProviderSources;

  useEffect(() => {
    if (!onProviderSelectionChange || !providers.state) return;
    let current = true;
    setSearchSources({});
    setSearchError(null);
    void providers.state().then((value) => {
      if (!current) return;
      const mapping = Object.fromEntries(value.sources
        .filter((source) => source.enabled && source.readerSourceId &&
          /^provider:[a-f0-9]{64}$/.test(source.readerSourceId))
        .map((source) => [source.sourceId, source.readerSourceId as string]));
      setSearchSources(mapping);
      const allowed = new Set(Object.values(mapping));
      const next = selected.current.filter((id) => allowed.has(id));
      if (next.length !== selected.current.length) onProviderSelectionChange(next);
      if (value.error) {
        setSearchError('Cloud search scope could not be completely confirmed.');
      }
    }).catch(() => {
      if (current) {
        setSearchError('Cloud search scope is unavailable.');
        onProviderSelectionChange([]);
      }
    });
    return () => { current = false; };
  }, [providers, onProviderSelectionChange, refreshKey, reload]);

  function selectProvider(id: string, checked: boolean) {
    const next = selectedProviderSources.filter((value) => value !== id);
    if (checked) {
      if (next.length >= 64) {
        setSearchError('Choose at most 64 cloud sources for search.');
        return;
      }
      next.push(id);
    }
    onProviderSelectionChange?.(next);
  }

  const epoch = useRef(0);
  const turn = useRef(0);
  const cursor = useRef<string | null>(null);
  const active = useRef<string | null>(null);

  function closeCursor() {
    const value = cursor.current;
    cursor.current = null;
    if (value) void backend.close(value).catch(() => {});
  }

  useEffect(() => {
    const generation = ++epoch.current;
    ++turn.current;
    active.current = null;
    const old = cursor.current;
    cursor.current = null;
    if (old) void backend.close(old).catch(() => {});
    setPage(null);
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
  }, [backend, refreshKey, reload]);

  async function browse(source: CatalogSource, more = false) {
    if (!source.enabled || source.failure) return;
    const generation = epoch.current;
    const request = ++turn.current;
    active.current = source.catalogId;
    setBusy(true);
    setError(null);
    const previous = more ? cursor.current : null;
    if (!more) {
      closeCursor();
      setPage(null);
    }
    try {
      const value = await backend.page(source.catalogId, previous, 100);
      if (generation !== epoch.current || request !== turn.current ||
        active.current !== source.catalogId) {
        if (value.nextCursor) void backend.close(value.nextCursor).catch(() => {});
        return;
      }
      if (value.source.catalogId !== source.catalogId) {
        throw new Error('The catalog source could not be verified.');
      }
      cursor.current = value.nextCursor;
      setPage((old) => more && old
        ? { ...value, documents: [...old.documents, ...value.documents] } : value);
    } catch (cause) {
      if (generation === epoch.current && request === turn.current) {
        closeCursor();
        setPage(null);
        setError(message(cause));
      }
    } finally {
      if (generation === epoch.current && request === turn.current) setBusy(false);
    }
  }

  async function open(document: CatalogDocument) {
    if (busy || !page) return;
    if (document.open.kind === 'exact' || document.open.kind === 'device') {
      try {
        onOpen(catalogReference(document.open, document.name));
      } catch (cause) {
        setError(message(cause));
      }
      return;
    }
    const generation = epoch.current;
    const request = ++turn.current;
    const source = page.source.catalogId;
    setBusy(true);
    setError(null);
    try {
      // This is the only network-capable action and requires this explicit click.
      const projection = await providers.fetchCurrent(
        document.open.sourceId, document.open.documentId,
      );
      if (generation !== epoch.current || request !== turn.current ||
        active.current !== source) return;
      if (projection.readerSourceId !== document.open.readerSourceId) {
        throw new Error('The fetched document binding could not be verified.');
      }
      onOpen(catalogReference({
        kind: 'exact',
        sourceId: projection.readerSourceId,
        path: projection.path,
        revision: projection.revision,
      }, document.name));
      closeCursor();
      setPage(null); // Cache publication invalidated this continuation.
    } catch (cause) {
      if (generation === epoch.current && request === turn.current) {
        setError(message(cause));
      }
    } finally {
      if (generation === epoch.current && request === turn.current) setBusy(false);
    }
  }

  return (
    <section className="reader-catalog" aria-label="Reader catalog">
      <header>
        <strong>Library catalog</strong>
        <button type="button" disabled={busy} onClick={() => setReload((n) => n + 1)}>
          Reload catalog
        </button>
        <button type="button" onClick={onManage}>Manage catalog sources</button>
      </header>
      {onProviderSelectionChange ? (
        <p role="status">{selectedProviderSources.length
          ? `${selectedProviderSources.length} cloud sources included in search with the active folder.`
          : 'Search uses the active folder. Select admitted cloud sources below to include their cached documents.'}
        </p>
      ) : null}
      {searchError ? <p role="alert">{searchError}</p> : null}
      {error ? <p role="alert">{error}</p> : null}
      {!busy && !sources.length ? <p>No catalog sources. Add or activate a source in Settings.</p> : null}
      {sources.map((source) => {
        const readerSourceId = searchSources[source.catalogId.slice(7)];
        return (
          <div className="reader-catalog__source" key={source.catalogId}>
            <strong>{source.name}</strong>
            <small>
              {source.kind === 'device' ? 'Device library' : source.kind === 'local' ? 'Folder'
                : source.kind === 'google_drive' ? 'Google Drive'
                  : source.kind === 'dropbox' ? 'Dropbox' : 'Cloud source'}
              {' · '}{coverageLabel(source.coverage)}
            </small>
            {onProviderSelectionChange && source.catalogId.startsWith('remote:') ? (
              <label>
                <input
                  type="checkbox"
                  checked={Boolean(readerSourceId && selectedProviderSources.includes(readerSourceId))}
                  disabled={!source.enabled || !readerSourceId}
                  onChange={(event) => {
                    if (readerSourceId) selectProvider(readerSourceId, event.currentTarget.checked);
                  }}
                />
                Include {source.name} in search
              </label>
            ) : null}
            {source.failure ? <p>{catalogErrorMessage(source.failure)}</p> : null}
            {source.coverage === 'needs_activation' ? (
              <button type="button" onClick={onManage}>Activate {source.name} in Settings</button>
            ) : (
              <button
                type="button"
                aria-pressed={page?.source.catalogId === source.catalogId}
                disabled={busy || !source.enabled || Boolean(source.failure)}
                onClick={() => void browse(source)}
              >Browse catalog {source.name}</button>
            )}
          </div>
        );
      })}
      {page ? <CatalogDocumentList key={page.source.catalogId} page={page} busy={busy}
        onOpen={(document) => void open(document)} onMore={() => void browse(page.source, true)} /> : null}
    </section>
  );
}

function coverageLabel(coverage: string) {
  const labels: Record<string, string> = {
    needs_activation: 'Activate to browse', not_indexed: 'Index needed', indexed_observation: 'Indexed',
    historical_index_coverage_unknown: 'Refresh index needed', metadata_complete_selected: 'Selected documents',
    metadata_partial_selected: 'More metadata available', published_device_documents: 'Local copies',
  };
  return labels[coverage] ?? coverage.replaceAll('_', ' ');
}
