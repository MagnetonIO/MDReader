import { useEffect, useRef } from 'react';
import type { CatalogSource } from './backend';

export function canRefreshCloud(source: CatalogSource): boolean {
  return source.enabled && source.catalogId.startsWith('remote:') &&
    (!source.failure || ['busy', 'unavailable', 'stale'].includes(source.failure));
}

/** Opening a cached cloud list never waits for this metadata-only delta check.
 * Per-source automatic checks coalesce and start at most once each minute. */
export function useCloudRefresh({ source, scopeKey, owner, refresh }: {
  source: CatalogSource | null;
  scopeKey: string;
  owner?: unknown;
  refresh(source: CatalogSource): Promise<void>;
}) {
  const current = useRef({ source, scopeKey, owner, refresh });
  current.current = { source, scopeKey, owner, refresh };
  const runs = useRef(new Map<string, { last: number; flight: Promise<void> | null }>());
  const selectedScope = useRef({ scopeKey, owner });
  if (selectedScope.current.scopeKey !== scopeKey || selectedScope.current.owner !== owner) {
    selectedScope.current = { scopeKey, owner };
    runs.current = new Map();
  }
  const request = (target: CatalogSource, manual = false): Promise<void> => {
    if (!canRefreshCloud(target)) return Promise.resolve();
    const entries = runs.current;
    const preceding = entries.get(target.catalogId);
    if (preceding?.flight) return preceding.flight;
    if (!manual && preceding && Date.now() - preceding.last < 60_000) return Promise.resolve();
    const run = { last: Date.now(), flight: null as Promise<void> | null };
    entries.set(target.catalogId, run);
    run.flight = Promise.resolve().then(() => {
      if (runs.current === entries && (manual || current.current.source?.catalogId === target.catalogId)) return current.current.refresh(target);
    }).finally(() => { run.flight = null; });
    return run.flight;
  };
  const invoke = useRef(request); invoke.current = request;
  useEffect(() => {
    const check = () => {
      const value = current.current;
      if (value.scopeKey !== scopeKey || !value.source || document.visibilityState === 'hidden' || !navigator.onLine) return;
      void invoke.current(value.source).catch(() => {});
    };
    check();
    window.addEventListener('focus', check);
    window.addEventListener('online', check);
    document.addEventListener('visibilitychange', check);
    const timer = window.setInterval(check, 60_000);
    return () => {
      window.clearInterval(timer);
      window.removeEventListener('focus', check);
      window.removeEventListener('online', check);
      document.removeEventListener('visibilitychange', check);
    };
  }, [source?.catalogId, scopeKey, owner]);
  return request;
}
