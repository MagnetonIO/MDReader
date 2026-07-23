import { describe, expect, test } from 'vitest';

import { LoadingGate } from './loading-gate.js';

function recorder() {
  const states: boolean[] = [];
  return { sink: { setLoading: (v: boolean) => states.push(v) }, states };
}

describe('LoadingGate', () => {
  test('the exact stuck-loading reproduction: an older open sets loading, a newer open fails before listing → cleared', () => {
    const gate = new LoadingGate();
    const { sink, states } = recorder();

    gate.begin(1, sink); // old open (gen 1) sets loading
    // a NEWER open (gen 2) advances the generation and FAILS before it lists →
    // its failure (>= the setter's gen) clears the stuck loading:
    gate.fail(2, sink);

    expect(states).toEqual([true, false]);
  });

  test('an OLDER open failure cannot clear a newer open loading; the newest can', () => {
    const gate = new LoadingGate();
    const { sink, states } = recorder();

    gate.begin(2, sink); // newer open (gen 2) sets loading (setter is 2)
    gate.fail(1, sink); // a straggling OLDER open (gen 1) fails — must NOT clear
    expect(states).toEqual([true]);

    gate.fail(2, sink); // the newest open (gen 2) failing clears
    expect(states).toEqual([true, false]);
  });

  test('a stale (older) completion cannot clear a newer open loading', () => {
    const gate = new LoadingGate();
    const { sink, states } = recorder();

    gate.begin(1, sink); // old open sets loading
    gate.begin(2, sink); // newer open takes over loading (setter is now 2)
    gate.complete(1, sink); // stale completion — must NOT clear
    expect(states).toEqual([true, true]);

    gate.complete(2, sink); // the current open's completion clears
    expect(states).toEqual([true, true, false]);
  });

  test('a completion at (or after) the setter generation clears', () => {
    const gate = new LoadingGate();
    const { sink, states } = recorder();

    gate.begin(3, sink);
    gate.complete(3, sink); // same generation → clears
    expect(states).toEqual([true, false]);
  });
});
