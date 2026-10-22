import type { ProviderSource } from '../../platform/providers';

/** A search preference selects only the native-confirmed canonical source; it grants no provider access. */
export function ProviderSearchSelection({ source, selected, onChange, onError }: {
  source: ProviderSource; selected: readonly string[];
  onChange(sources: string[]): void; onError(message: string): void;
}) {
  const id = source.readerSourceId;
  if (!id || !/^provider:[a-f0-9]{64}$/.test(id)) return source.enabled
    ? <p className="provider-search__hint">Open a document from this source to include it in search.</p> : null;
  return <label className="provider-search">
    <input type="checkbox" aria-label={`Include ${source.name} in search`} checked={selected.includes(id)} disabled={!source.enabled}
      onChange={(event) => {
        const next = selected.filter(value => value !== id);
        if (event.currentTarget.checked) {
          if (next.length >= 64) { onError('Choose at most 64 cloud sources for search.'); return; }
          next.push(id);
        }
        onChange(next);
      }} />
    <span>Include in search</span>
  </label>;
}
