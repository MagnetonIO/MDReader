import { useMemo, useState } from 'react';
import type { CatalogDocument, CatalogPage } from './backend';
import { DocIcon } from '../../shell/icons';

export function CatalogDocumentList({ page, busy, onOpen, onMore, exactCopiesOnly = false }: {
  page: CatalogPage;
  busy: boolean;
  exactCopiesOnly?: boolean;
  onOpen(document: CatalogDocument): void;
  onMore(): void;
}) {
  const [filter, setFilter] = useState('');
  const shown = useMemo(() => {
    const query = filter.trim().toLocaleLowerCase();
    return query ? page.documents.filter((document) => `${document.name}\n${document.displayPath}`
      .toLocaleLowerCase().includes(query)) : page.documents;
  }, [filter, page.documents]);
  const duplicateNames = useMemo(() => {
    const counts = new Map<string, number>();
    for (const document of page.documents) counts.set(document.name, (counts.get(document.name) ?? 0) + 1);
    return new Set([...counts].filter(([, count]) => count > 1).map(([name]) => name));
  }, [page.documents]);
  return <section className="reader-catalog__documents" aria-label={`Catalog documents ${page.source.name}`}>
    {page.documents.length > 8 ? <label className="reader-catalog__filter">
      Find in these documents
      <input type="search" value={filter} maxLength={256} onChange={(event) => setFilter(event.target.value)} />
    </label> : null}
    {shown.map((document) => <div className="reader-catalog__document" title={document.displayPath} key={document.open.kind === 'exact'
      ? `${document.open.sourceId}:${document.open.path}` : document.open.documentId}>
      <button type="button" disabled={busy || exactCopiesOnly && document.open.kind === 'selected_uncached'}
        aria-label={`${document.open.kind === 'selected_uncached' ? exactCopiesOnly ? 'Not saved here' : 'Fetch current' : 'Open'} ${document.name}${duplicateNames.has(document.name) ? ` — ${document.displayPath}` : ''}`}
        onClick={() => onOpen(document)}>
        <DocIcon />
        <span>{document.name}{document.open.kind === 'selected_uncached' ? exactCopiesOnly ? ' · Not saved here' : ' · Open from cloud' : ''}</span>
      </button>
      <small className={duplicateNames.has(document.name) ? 'reader-catalog__document-path' : undefined}>{document.displayPath}</small>
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
        : page.source.catalogId.startsWith('remote:')
          ? page.source.coverage === 'never_indexed'
            ? 'Load documents to list Markdown files from your selected cloud source.'
            : 'No Markdown documents found in your selected cloud source.'
          : 'No documents in this source observation.'}</p> : null}
    {page.nextCursor && page.documents.length < 1000 ? <button type="button" disabled={busy} onClick={onMore}>Load more documents</button> : null}
    {page.nextCursor && page.documents.length >= 1000 ? <p>Showing the first 1,000 documents. Use global search for other documents.</p> : null}
  </section>;
}
