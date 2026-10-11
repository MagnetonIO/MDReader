import type { CatalogDocument } from './backend';

export function catalogRowKey(document: CatalogDocument): string {
  const reference = document.open;
  return (reference.kind === 'exact' || reference.kind === 'local_pending') ? `exact:${reference.sourceId}:${reference.path}`
    : `${reference.kind}:${reference.sourceId}:${reference.documentId}`;
}
/** Keep published rows during an incomplete refresh, using native references
 * rather than titles or renderer-constructed filesystem paths. New receipts win. */
export function retainPublishedRows(previous: readonly CatalogDocument[], published: readonly CatalogDocument[]): CatalogDocument[] {
  const rows = new Map(published.map(document => [catalogRowKey(document), document]));
  for (const document of previous) {
    const key = catalogRowKey(document);
    if (!rows.has(key) && rows.size < 1000) rows.set(key, document);
  }
  return [...rows.values()].slice(0, 1000);
}

/** Pagination may revisit rows retained from an earlier partial batch. */
export function appendPublishedRows(previous: readonly CatalogDocument[], published: readonly CatalogDocument[]): CatalogDocument[] {
  const rows = new Map(previous.map(document => [catalogRowKey(document), document]));
  for (const document of published) {
    const key = catalogRowKey(document);
    if (rows.has(key) || rows.size < 1000) rows.set(key, document);
  }
  return [...rows.values()].slice(0, 1000);
}
