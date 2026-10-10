import { useCallback, useMemo, useState } from 'react';
import type { SearchAdapter, SearchIntent } from './model.js';
import type { SearchCoverage } from './coverage.js';

/** A previous query/scope cannot lend its index observation to a new result.
 * Streaming hits never publish coverage; only the resolved final result does. */
export function useSearchCoverage(query: string, intent: SearchIntent, mode: string, adapter: SearchAdapter, workspaceKey: string | null | undefined) {
  const request = useMemo(() => ({}), [query, intent, mode, adapter, workspaceKey]);
  const [result, setResult] = useState<{ request: object; coverage: SearchCoverage | null } | null>(null);
  const publish = useCallback((coverage: SearchCoverage | null | undefined) => {
    setResult({ request, coverage: coverage ?? null });
  }, [request]);
  return { coverage: result?.request === request ? result.coverage : null, publish };
}
