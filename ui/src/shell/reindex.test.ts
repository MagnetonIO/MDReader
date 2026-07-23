import { describe, expect, test, vi } from 'vitest';

import { reindexOnChange } from './reindex.js';

describe('reindexOnChange', () => {
  test('when idle, runs the import THEN refreshes the listing (in that order)', async () => {
    const order: string[] = [];
    const startImport = vi.fn(async () => {
      order.push('import');
    });
    const refreshFiles = vi.fn(async () => {
      order.push('refresh');
    });

    const outcome = await reindexOnChange({ importing: false, startImport, refreshFiles });

    expect(outcome).toBe('reindexed');
    expect(startImport).toHaveBeenCalledTimes(1);
    expect(refreshFiles).toHaveBeenCalledTimes(1);
    // The watcher must drive the index BEFORE re-listing, so the refreshed tree
    // reflects the just-imported change.
    expect(order).toEqual(['import', 'refresh']);
  });

  test('while an import is already running, only refreshes (no duplicate import)', async () => {
    const startImport = vi.fn(async () => {});
    const refreshFiles = vi.fn(async () => {});

    const outcome = await reindexOnChange({ importing: true, startImport, refreshFiles });

    expect(outcome).toBe('refreshed');
    expect(startImport).not.toHaveBeenCalled();
    expect(refreshFiles).toHaveBeenCalledTimes(1);
  });
});
