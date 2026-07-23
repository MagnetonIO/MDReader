import { describe, expect, test } from 'vitest';

import { LruBoolCache } from './wiki-cache.js';

describe('LruBoolCache', () => {
  test('a get refreshes recency so a touched key survives eviction', () => {
    const cache = new LruBoolCache(3);
    cache.set('a', true);
    cache.set('b', false);
    cache.set('c', true);

    // Touch the oldest key → it becomes most-recent.
    expect(cache.get('a')).toBe(true);

    // Inserting a 4th key evicts the now-least-recent (`b`), not `a`.
    const evicted = cache.set('d', true);
    expect(evicted).toEqual(['b']);
    expect(cache.has('a')).toBe(true);
    expect(cache.has('b')).toBe(false);
    expect(cache.size).toBe(3);
  });

  test('an evicted key is gone (undefined) so it can re-resolve if seen again', () => {
    const cache = new LruBoolCache(2);
    cache.set('a', true);
    cache.set('b', true);
    const evicted = cache.set('c', true); // evicts the LRU, `a`
    expect(evicted).toEqual(['a']);
    // A re-get returns undefined → the caller re-schedules a resolution.
    expect(cache.get('a')).toBeUndefined();
  });

  test('at the 512 cap it keeps hot keys and reports every evicted key', () => {
    const cache = new LruBoolCache(512);
    for (let i = 0; i < 512; i += 1) cache.set(`k${i}`, true); // full to the cap
    cache.get('k0'); // hit the oldest → survives

    const evicted = cache.set('k512', true); // over cap by one → evict the next-oldest
    expect(evicted).toEqual(['k1']);
    expect(cache.has('k0')).toBe(true);
    expect(cache.has('k1')).toBe(false);
    expect(cache.size).toBe(512);
  });
});
