import { useMemo, useState } from 'react';
import type { CatalogDocument, CatalogPage } from './backend';

export function CatalogDocumentList({ page, busy, onOpen, onMore }: {
  page: CatalogPage;
  busy: boolean;
  onOpen(document: CatalogDocument): void;
  onMore(): void;
}) {
  const [filter, setFilter] = useState('');
  const shown = useMemo(() => {
    const query = filter.trim().toLocaleLowerCase();
    return query ? page.documents.filter((document) => `${document.name}\n${document.displayPath}`
      .toLocaleLowerCase().includes(query)) : page.documents;
  }, [filter, page.documents]);
  return <section className="reader-catalog__documents" aria-label={`Catalog documents ${page.source.name}`}>
    <h3>{page.source.name}</h3>
    {page.documents.length ? <label className="reader-catalog__filter">
      Filter loaded documents
      <input type="search" value={filter} maxLength={256} onChange={(event) => setFilter(event.target.value)} />
    </label> : null}
    <p role="status">{filter.trim() ? `${shown.length} matching documents · ` : ''}{page.documents.length} documents loaded.</p>
    <details className="reader-catalog__evidence">
      <summary>About this source</summary>
      <p>{page.source.kind === 'local'
        ? 'Folder documents are listed from the index. Opening a document verifies the exact current file.'
        : page.source.kind === 'device' ? 'These are local device copies. Browsing does not contact sync.'
          : 'These documents belong to your selected cloud scope. Cached copies open offline; Fetch current contacts the provider.'}</p>
      <p>{page.source.observedAtMs === null ? 'Observation time unknown.'
        : `Last observed ${new Date(page.source.observedAtMs).toLocaleString()}.`}</p>
    </details>
    {shown.map((document) => <div className="reader-catalog__document" key={document.open.kind === 'exact'
      ? `${document.open.sourceId}:${document.open.path}` : document.open.documentId}>
      <button type="button" disabled={busy} onClick={() => onOpen(document)}>
        {document.open.kind === 'selected_uncached' ? 'Fetch current' : 'Read exact'} {document.name}
      </button>
      <small>{document.displayPath}</small>
      {document.modifiedAtMs !== null || document.sizeBytes !== null ? <small>
        {document.modifiedAtMs !== null ? new Date(document.modifiedAtMs).toLocaleDateString() : ''}
        {document.modifiedAtMs !== null && document.sizeBytes !== null ? ' · ' : ''}
        {document.sizeBytes !== null ? `${document.sizeBytes} bytes` : ''}
      </small> : null}
    </div>)}
    {!shown.length && page.documents.length ? <p>No loaded documents match. Load more or use global search to search beyond this page.</p> : null}
    {!page.documents.length ? <p>{page.source.coverage === 'not_indexed'
      ? 'Index this folder in Settings before browsing.'
      : page.source.coverage === 'historical_index_coverage_unknown'
        ? 'No documents observed. Refresh this source’s index in Settings.'
        : 'No documents in this source observation.'}</p> : null}
    {page.nextCursor && page.documents.length < 1000 ? <button type="button" disabled={busy} onClick={onMore}>More catalog documents</button> : null}
    {page.nextCursor && page.documents.length >= 1000 ? <p>Showing the first 1,000 documents. Use global search for other documents.</p> : null}
  </section>;
}
