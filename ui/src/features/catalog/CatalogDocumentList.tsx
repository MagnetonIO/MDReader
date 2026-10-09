import { useMemo, useState } from 'react';
import type { CatalogDocument, CatalogPage } from './backend';
import { DocIcon } from '../../shell/icons';

export function CatalogDocumentList({ page, busy, pagingBusy = busy, onOpen, onMore, pending = false, finding = false, exactCopiesOnly = false }: {
  page: CatalogPage;
  busy: boolean;
  pagingBusy?: boolean;
  finding?: boolean;
  pending?: boolean;
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
      <input type="search" aria-label="Filter files" placeholder="Find a file…" value={filter} maxLength={256} onChange={(event) => setFilter(event.target.value)} />
    </label> : null}
    {shown.map((document) => <div className="reader-catalog__document" title={document.displayPath} key={document.open.kind === 'exact'
      ? `${document.open.sourceId}:${document.open.path}` : document.open.documentId}>
      <button type="button" disabled={busy || exactCopiesOnly && document.open.kind === 'selected_uncached'}
        title={`${document.displayPath}${document.open.kind === 'selected_uncached' ? exactCopiesOnly ? '\nNot saved on this device' : '\nOpen to download this Markdown file' : ''}`}
        aria-label={`${document.open.kind === 'selected_uncached' ? exactCopiesOnly ? 'Not saved here' : 'Fetch current' : 'Open'} ${document.name}${duplicateNames.has(document.name) ? ` — ${document.displayPath}` : ''}`}
        onClick={() => onOpen(document)}>
        <DocIcon />
        <span>{document.name}</span>
        {exactCopiesOnly && document.open.kind === 'selected_uncached' ? <small className="reader-catalog__not-saved">Not saved</small> : null}
      </button>
      {duplicateNames.has(document.name) ? <small className="reader-catalog__document-path" title={document.displayPath}>{document.displayPath}</small> : null}
    </div>)}
    {!shown.length && page.documents.length ? <p>No matching files on this page.</p> : null}
    {!finding && !page.documents.length ? <p>{pending ? 'Loading paused. Resume to keep looking for Markdown files.' : page.source.coverage === 'not_indexed'
      ? 'Index this folder in Settings to see its files.'
      : page.source.coverage === 'historical_index_coverage_unknown'
        ? 'Refresh this folder in Settings to see its files.'
        : page.source.catalogId.startsWith('remote:')
          ? page.source.coverage === 'never_indexed'
            ? 'Refresh to find Markdown files.'
            : 'No Markdown files found.'
          : 'No files found.'}</p> : null}
    {page.nextCursor && page.documents.length < 1000 ? <button type="button" disabled={pagingBusy} onClick={onMore}>Load more documents</button> : null}
    {page.nextCursor && page.documents.length >= 1000 ? <p>More files are available; this list reached its browsing limit.</p> : null}
  </section>;
}
