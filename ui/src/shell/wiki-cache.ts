/**
 * A tiny bounded LRU for wikilink dimming resolutions (target → resolves?),
 * factored out of the ReaderPane so the recency + eviction behaviour is
 * unit-testable without a DOM. A `Map` preserves insertion order, so the oldest
 * live key is always `keys().next()`; a cache HIT re-inserts the key to refresh
 * its recency, and `set` evicts from the front until within the cap, returning
 * the evicted keys so the caller can drop them from its in-flight set (an
 * evicted key can then re-resolve if it reappears).
 */
export class LruBoolCache {
  private readonly map = new Map<string, boolean>();

  constructor(private readonly cap: number) {}

  /** The value for `key`, refreshing its recency (most-recently-used), or
   *  `undefined` when absent. */
  get(key: string): boolean | undefined {
    const value = this.map.get(key);
    if (value === undefined) return undefined;
    this.map.delete(key);
    this.map.set(key, value);
    return value;
  }

  has(key: string): boolean {
    return this.map.has(key);
  }

  /** Insert/update `key` as most-recent; returns the keys evicted to stay within
   *  the cap (least-recently-used first). */
  set(key: string, value: boolean): string[] {
    this.map.delete(key);
    this.map.set(key, value);
    const evicted: string[] = [];
    while (this.map.size > this.cap) {
      const oldest = this.map.keys().next().value as string | undefined;
      if (oldest === undefined) break;
      this.map.delete(oldest);
      evicted.push(oldest);
    }
    return evicted;
  }

  get size(): number {
    return this.map.size;
  }
}
