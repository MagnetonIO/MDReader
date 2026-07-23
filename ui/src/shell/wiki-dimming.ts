/**
 * The reader's synchronous "does this wikilink resolve?" predicate, extracted
 * from the ReaderPane so its cache + eviction + re-resolution behaviour is
 * testable through the real hook (not just the cache unit).
 *
 * It reflects ACTUAL index resolution (filename OR title): each unique target is
 * resolved once through the injected `resolve`, cached in a bounded LRU, and the
 * hook re-renders (so title-resolved links un-dim). Every resolution captures the
 * cache epoch and discards its result if the cache was reset meanwhile
 * (document / file-set change). When a key is evicted it is dropped from the
 * in-flight set too, so it can re-resolve if it reappears. Until an authoritative
 * answer lands the predicate falls back to `fallback` (exact basename).
 */
import { useCallback, useEffect, useRef, useState } from 'react';

import { LruBoolCache } from './wiki-cache.js';

/** Default cap on the per-document dimming-resolution LRU. */
export const WIKI_CACHE_CAP = 512;

export interface WikiDimmingConfig {
  /** Active document — a change resets the cache. */
  path: string | null;
  /** Workspace file set — a change resets the cache. */
  files: unknown;
  /** Authoritative resolver: does `target` resolve? (async; may throw). */
  resolve: (target: string) => Promise<boolean>;
  /** Synchronous provisional/fallback check (exact basename). */
  fallback: (target: string) => boolean;
  /** Bounded LRU cap (defaults to {@link WIKI_CACHE_CAP}). */
  cap?: number;
}

export function useWikiDimming(config: WikiDimmingConfig): (target: string) => boolean {
  const { path, files, resolve, fallback, cap = WIKI_CACHE_CAP } = config;

  const cacheRef = useRef<LruBoolCache | null>(null);
  if (cacheRef.current === null) cacheRef.current = new LruBoolCache(cap);
  const inFlightRef = useRef<Set<string>>(new Set());
  const epochRef = useRef(0);
  const [version, setVersion] = useState(0);

  // Forget cached resolutions when the document or file set changes; the epoch
  // bump makes any in-flight resolution discard its late result.
  useEffect(() => {
    cacheRef.current = new LruBoolCache(cap);
    inFlightRef.current = new Set();
    epochRef.current += 1;
    setVersion((value) => value + 1);
  }, [path, files, cap]);

  return useCallback(
    (target: string): boolean => {
      const key = target.trim().toLowerCase();
      if (key === '') return false;
      const cache = cacheRef.current!;
      const known = cache.get(key); // a hit refreshes LRU recency
      if (known !== undefined) return known;
      if (!inFlightRef.current.has(key)) {
        inFlightRef.current.add(key);
        const epoch = epochRef.current;
        const record = (value: boolean) => {
          if (epochRef.current !== epoch) return;
          const evicted = cacheRef.current!.set(key, value);
          // Drop evicted keys from in-flight so they can re-resolve if seen again.
          for (const gone of evicted) inFlightRef.current.delete(gone);
          setVersion((current) => current + 1);
        };
        void resolve(target)
          .then(record)
          .catch(() => record(fallback(target)));
      }
      return fallback(target);
    },
    // `version` re-derives the predicate when a resolution lands so the reader
    // re-renders with corrected dimming.
    [resolve, fallback, version],
  );
}
