import { useLayoutEffect, useMemo, useRef, useState } from 'react';
import type { CatalogDocument, CatalogPage } from './backend';
import { DocIcon } from '../../shell/icons';
import { catalogRowKey } from './published-pages';

export function CatalogDocumentList({ page, busy, pagingBusy = busy, onOpen, onMore, pending = false, finding = false, failed = false, exactCopiesOnly = false, rowKey = catalogRowKey }: {
  page: CatalogPage;
  busy: boolean;
  pagingBusy?: boolean;
  finding?: boolean;
  failed?: boolean;
  pending?: boolean;
  exactCopiesOnly?: boolean;
  onOpen(document: CatalogDocument): void;
  onMore(): void;
  rowKey?: (document: CatalogDocument) => string;
}) {
  const [filter, setFilter] = useState('');
  const host = useRef<HTMLElement>(null);
  const focused = useRef<string | null>(null);
  useLayoutEffect(() => {
    if (!focused.current || document.activeElement !== document.body && !host.current?.contains(document.activeElement)) return;
    const target = Array.from(host.current?.querySelectorAll<HTMLButtonElement>('[data-row-key]') ?? [])
      .find(button => button.dataset.rowKey === focused.current);
    if (target && document.activeElement !== target) target.focus({ preventScroll: true });
  }, [page.documents, rowKey]);
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
  return <section ref={host} onFocusCapture={event => {
    const target = event.target as HTMLElement;
    focused.current = target.closest<HTMLButtonElement>('[data-row-key]')?.dataset.rowKey ?? null;
  }} onBlurCapture={event => {
    if (event.relatedTarget && !host.current?.contains(event.relatedTarget as Node)) focused.current = null;
  }} className="reader-catalog__documents" aria-label={`Catalog documents ${page.source.name}`}>
    {page.documents.length > 8 ? <label className="reader-catalog__filter">
      <input type="search" aria-label="Filter files" placeholder="Find a file…" value={filter} maxLength={256} onChange={(event) => setFilter(event.target.value)} />
    </label> : null}
    {shown.map((document) => <div className="reader-catalog__document" title={document.displayPath} key={rowKey(document)}>
      <button type="button" data-row-key={rowKey(document)} disabled={busy || exactCopiesOnly && document.open.kind === 'selected_uncached'}
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
    {!finding && !failed && !page.documents.length ? <p>{pending ? 'Loading paused. Resume to keep looking for Markdown files.' : page.source.coverage === 'not_indexed'
      ? 'Index this folder in Settings to see its files.'
      : page.source.coverage === 'historical_index_coverage_unknown'
        ? 'Refresh this folder in Settings to see its files.'
        : page.source.catalogId.startsWith('remote:')
          ? page.source.coverage === 'never_indexed'
            ? 'Refresh to find Markdown files.'
            : 'No Markdown files found.'
          : 'No files found.'}</p> : null}
    {page.nextCursor && page.documents.length < 1000 ? <button type="button" disabled={pagingBusy} onClick={onMore}>Show more</button> : null}
    {page.nextCursor && page.documents.length >= 1000 ? <p>More files are available; this list reached its browsing limit.</p> : null}
  </section>;
}
