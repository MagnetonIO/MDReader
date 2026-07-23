import { describe, expect, test } from 'vitest';

import { watcherStampCurrent } from './watcher-guard.js';

describe('watcherStampCurrent', () => {
  test('an event arriving at gen N, then a switch to N+1, is discarded when the timer fires', () => {
    // Arrival: open generation 5, watcher generation N=1.
    const stamp = { openGen: 5, watcherGen: 1 };

    // No switch → still current.
    expect(watcherStampCurrent(stamp, { openGen: 5, watcherGen: 1 })).toBe(true);

    // A switch to N+1 advances BOTH counters → discarded.
    expect(watcherStampCurrent(stamp, { openGen: 6, watcherGen: 2 })).toBe(false);
  });

  test('either counter advancing alone also discards (both must match)', () => {
    const stamp = { openGen: 5, watcherGen: 1 };
    // Only the watcher generation advanced (e.g. a re-arm) — the arrival-only
    // open-gen check would have MISSED this; both-check catches it.
    expect(watcherStampCurrent(stamp, { openGen: 5, watcherGen: 2 })).toBe(false);
    // Only the open generation advanced.
    expect(watcherStampCurrent(stamp, { openGen: 6, watcherGen: 1 })).toBe(false);
  });
});
