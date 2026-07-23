import { describe, expect, test, vi } from 'vitest';

import { retireTabsAfterSwitch } from './switch-retire.js';

describe('retireTabsAfterSwitch', () => {
  test('a stale open success (committed=false) does NOT retire tabs — the newer workspace tabs stay intact', () => {
    const closeAll = vi.fn(() => [{ path: '/wsA/a.md', name: 'a.md' }]);
    const addRecent = vi.fn();

    const retired = retireTabsAfterSwitch({
      committed: false, // openRoot's epoch was superseded → not the current open
      previousRoot: '/wsA',
      closeAll,
      addRecent,
    });

    expect(retired).toBe(false);
    expect(closeAll).not.toHaveBeenCalled();
    expect(addRecent).not.toHaveBeenCalled();
  });

  test('a committed switch retires the previous workspace tabs into its Recent', () => {
    const closeAll = vi.fn(() => [{ path: '/wsA/a.md', name: 'a.md' }]);
    const addRecent = vi.fn();

    const retired = retireTabsAfterSwitch({
      committed: true,
      previousRoot: '/wsA',
      closeAll,
      addRecent,
    });

    expect(retired).toBe(true);
    expect(closeAll).toHaveBeenCalledTimes(1);
    expect(addRecent).toHaveBeenCalledWith({ path: '/wsA/a.md', name: 'a.md' }, '/wsA');
  });

  test('a first open (no previous workspace) retires nothing', () => {
    const closeAll = vi.fn(() => []);
    expect(
      retireTabsAfterSwitch({ committed: true, previousRoot: null, closeAll, addRecent: vi.fn() }),
    ).toBe(false);
    expect(closeAll).not.toHaveBeenCalled();
  });
});
