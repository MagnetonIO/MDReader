import { useEffect, useRef } from 'react';
import type { CatalogBackend } from './backend';

/** Reobserve retained local catalog metadata, without fetching or reindexing bodies.
 * Focus and a visible, single-flight 30s check also cover jobs started elsewhere. */
export function useLocalCatalogObservation({ backend, scopeKey, observationKey, enabled, onObserved }: {
  backend: CatalogBackend;
  scopeKey: string;
  observationKey: number;
  enabled: boolean;
  onObserved(value: Awaited<ReturnType<CatalogBackend['list']>>): void;
}) {
  const callback = useRef(onObserved);
  callback.current = onObserved;
  const scope = useRef({ backend, scopeKey });
  scope.current = { backend, scopeKey };
  const check = useRef<(() => void) | null>(null);
  const previousKey = useRef(observationKey);
  useEffect(() => {
    if (!enabled) return;
    let live = true;
    let flight = false;
    let again = false;
    const current = () => live && scope.current.backend === backend && scope.current.scopeKey === scopeKey;
    const observe = () => {
      if (!current() || document.visibilityState === 'hidden') return;
      if (flight) { again = true; return; }
      flight = true;
      void backend.list().then(value => {
        if (current() && !value.localFailure) callback.current(value);
      }).catch(() => { /* Retain the last observation; explicit refresh exposes errors. */ })
        .finally(() => {
          flight = false;
          if (again && current()) { again = false; observe(); }
        });
    };
    check.current = observe;
    window.addEventListener('focus', observe);
    document.addEventListener('visibilitychange', observe);
    const timer = window.setInterval(observe, 30_000);
    return () => {
      live = false;
      check.current = null;
      window.clearInterval(timer);
      window.removeEventListener('focus', observe);
      document.removeEventListener('visibilitychange', observe);
    };
  }, [backend, scopeKey, enabled]);
  useEffect(() => {
    if (previousKey.current === observationKey) return;
    previousKey.current = observationKey;
    check.current?.();
  }, [observationKey]);
}
