import { act, renderHook, waitFor } from '@testing-library/react';
import { describe, expect, test, vi } from 'vitest';

import { useWikiDimming } from './wiki-dimming.js';

// A STABLE files reference — the reset effect is keyed on it, so a fresh array
// each render would re-fire forever (in the real ReaderPane `files` is a stable
// store reference).
const FILES: unknown[] = [];

/** Drive the dimming hook through the REAL predicate path (cache + in-flight +
 *  re-render), exercising eviction and re-resolution — not just the cache unit. */
describe('useWikiDimming — eviction re-resolves through the real predicate path', () => {
  test('an evicted target is re-resolved when it reappears', async () => {
    const resolveCounts = new Map<string, number>();
    const resolve = vi.fn(async (target: string) => {
      resolveCounts.set(target, (resolveCounts.get(target) ?? 0) + 1);
      return true; // everything "resolves"
    });
    const fallback = () => false;

    // Cap 2 so eviction happens quickly.
    const { result } = renderHook(() =>
      useWikiDimming({ path: '/doc.md', files: FILES, resolve, fallback, cap: 2 }),
    );

    const call = (target: string) => act(() => void result.current(target));

    // Resolve a, b (fills the cap), then c → evicts the LRU (a).
    call('a');
    call('b');
    await waitFor(() => expect(resolveCounts.get('b')).toBe(1));
    call('c');
    await waitFor(() => expect(resolveCounts.get('c')).toBe(1));

    // `a` was evicted AND dropped from in-flight, so touching it again schedules
    // a fresh resolution (the real ReaderPane path re-resolves it).
    call('a');
    await waitFor(() => expect(resolveCounts.get('a')).toBe(2));
  });

  test('a resolved (title) target stops dimming after its resolution lands', async () => {
    const resolve = vi.fn(async () => true); // resolves by title (fallback says no)
    const fallback = () => false;

    const { result } = renderHook(() =>
      useWikiDimming({ path: '/doc.md', files: FILES, resolve, fallback }),
    );

    // First read: unknown → provisional fallback (does NOT resolve → would dim).
    let first = false;
    act(() => {
      first = result.current('title-only');
    });
    expect(first).toBe(false);

    // Once the authoritative resolution lands, the predicate reports resolved.
    await waitFor(() => expect(result.current('title-only')).toBe(true));
  });
});
