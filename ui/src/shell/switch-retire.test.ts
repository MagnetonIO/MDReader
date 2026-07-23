import { describe, expect, test, vi } from 'vitest';

import { retireTabsAfterSwitch } from './switch-retire.js';
import type { DocRef } from '../stores/types.js';

const ref = (name: string): DocRef => ({ path: `/${name}.md`, name: `${name}.md` });

describe('retireTabsAfterSwitch', () => {
  test('a stale open (committed=false) retires nothing', () => {
    const retireForeign = vi.fn(() => [{ docRef: ref('a'), originRoot: '/wsA' }]);
    const addRecent = vi.fn();

    const retired = retireTabsAfterSwitch({
      committed: false,
      targetRoot: '/wsB',
      fallbackRoot: '/wsA',
      retireForeign,
      addRecent,
    });

    expect(retired).toBe(false);
    expect(retireForeign).not.toHaveBeenCalled();
    expect(addRecent).not.toHaveBeenCalled();
  });

  test('a committed switch retires foreign tabs, each under its OWN origin', () => {
    const retireForeign = vi.fn(() => [
      { docRef: ref('a'), originRoot: '/wsA' },
      { docRef: ref('b'), originRoot: '/wsB' }, // a mixed-origin tab
    ]);
    const addRecent = vi.fn();

    const retired = retireTabsAfterSwitch({
      committed: true,
      targetRoot: '/wsC',
      fallbackRoot: '/wsA',
      retireForeign,
      addRecent,
    });

    expect(retired).toBe(true);
    expect(retireForeign).toHaveBeenCalledWith('/wsC'); // selects by the NEW root
    // Attribution is by each tab's own origin — not a single snapshotted root.
    expect(addRecent).toHaveBeenCalledWith(ref('a'), '/wsA');
    expect(addRecent).toHaveBeenCalledWith(ref('b'), '/wsB');
  });

  test('a legacy tab with no origin falls back to the switched-away-from root', () => {
    const retireForeign = vi.fn(() => [{ docRef: ref('a'), originRoot: null }]);
    const addRecent = vi.fn();

    retireTabsAfterSwitch({
      committed: true,
      targetRoot: '/wsB',
      fallbackRoot: '/wsA',
      retireForeign,
      addRecent,
    });

    expect(addRecent).toHaveBeenCalledWith(ref('a'), '/wsA');
  });

  test('a same-root re-open commits but retires nothing (no foreign tabs)', () => {
    const retireForeign = vi.fn(() => []); // origin === target → nothing foreign
    const addRecent = vi.fn();

    const retired = retireTabsAfterSwitch({
      committed: true,
      targetRoot: '/wsA',
      fallbackRoot: '/wsA',
      retireForeign,
      addRecent,
    });

    expect(retired).toBe(false);
    expect(addRecent).not.toHaveBeenCalled();
  });
});
