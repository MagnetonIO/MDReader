import type { ProviderDocument, ProviderDocuments, ProviderSource } from '../../platform/providers';
import { providerName } from './source-presentation';

/** Metadata coverage and saved-copy check times do not prove the current remote head. */
export function SourceDocuments({ source, page, loading, busy, canOpen, onOpen, onMore, onClose }: {
  source: ProviderSource; page: ProviderDocuments | null; loading: boolean; busy: boolean; canOpen: boolean;
  onOpen(document: ProviderDocument, cached: boolean): void; onMore(): void; onClose(): void;
}) {
  return <section className="provider-documents" aria-label={`Documents in ${source.name}`}>
    <div className="provider-documents__header">
      <h4>Documents</h4>
      <button type="button" className="provider-button--quiet" onClick={onClose}>Close documents</button>
    </div>
    <p className="provider-documents__hint">Saved copies can be older than the file in {providerName(source.kind)}.</p>
    {loading ? <p role="status">Loading the file list…</p> : null}
    {page?.documents.length === 0 ? <p className="provider-documents__hint">No Markdown files found yet. Choose Refresh file list from the More menu.</p> : null}
    <ul className="provider-documents__list">{page?.documents.map((document) => <li key={document.documentId} aria-label={document.name}>
      <div className="provider-documents__text">
        <strong>{document.name}</strong>
        <span>{document.displayPath}</span>
        <span>{documentState(document)}{document.cachedObservedAtMs !== null ? ` · Saved ${new Date(document.cachedObservedAtMs).toLocaleString()}` : ''}</span>
      </div>
      <div className="provider-documents__actions">
        <button type="button" disabled={busy || !document.admitted || !document.projection || !canOpen} onClick={() => onOpen(document, true)}>Open saved copy</button>
        <button type="button" disabled={busy || !source.enabled || !source.capabilities.read || !canOpen} onClick={() => onOpen(document, false)}>Download latest</button>
      </div>
    </li>)}</ul>
    {page?.nextOffset !== null && page?.nextOffset !== undefined ? <button type="button" disabled={busy || loading} onClick={onMore}>Load more documents</button> : null}
  </section>;
}

function documentState(document: ProviderDocument): string {
  return document.admitted ? 'Saved copy ready' : document.cached ? 'Downloaded, not yet in library' : 'Not downloaded';
}
