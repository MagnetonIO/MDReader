import type { CatalogSource } from './backend';

/** Connection metadata keeps a source discoverable before its first document
 * observation. Browsing still asks the native catalog to verify membership. */
export function mergeConnectedSources(
  catalog: readonly CatalogSource[], connected: readonly CatalogSource[],
): CatalogSource[] {
  const rows = new Map(catalog.map((source) => [source.catalogId, source]));
  for (const source of connected) {
    const existing = rows.get(source.catalogId);
    rows.set(source.catalogId, existing
      ? { ...existing, name: source.name, kind: source.kind, enabled: source.enabled }
      : source);
  }
  return [...rows.values()];
}
