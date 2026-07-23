import { describe, expect, test, vi } from 'vitest';

import { createReindexer } from './reindex.js';

/** A manually-resolvable promise, for driving an in-flight import deterministically. */
function deferred<T = void>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((r) => {
    resolve = r;
  });
  return { promise, resolve };
}

describe('createReindexer', () => {
  test('when idle, runs the import THEN refreshes the listing (in that order)', async () => {
    const order: string[] = [];
    const startImport = vi.fn(async () => {
      order.push('import');
    });
    const refreshFiles = vi.fn(async () => {
      order.push('refresh');
    });

    const outcome = await createReindexer({ startImport, refreshFiles }).onChange();

    expect(outcome).toBe('reindexed');
    expect(startImport).toHaveBeenCalledTimes(1);
    expect(refreshFiles).toHaveBeenCalledTimes(1);
    // The change must drive the index BEFORE re-listing, so the refreshed tree
    // reflects the just-imported change.
    expect(order).toEqual(['import', 'refresh']);
  });

  test('a change during an active import queues exactly one follow-up reindex', async () => {
    const gate = deferred(); // holds the first (in-flight) import open
    const startImport = vi
      .fn()
      // first import: blocks until we release the gate
      .mockImplementationOnce(() => gate.promise)
      // any follow-up import: resolves immediately
      .mockImplementation(async () => {});
    const refreshFiles = vi.fn(async () => {});

    const reindexer = createReindexer({ startImport, refreshFiles });

    // First change starts the import (pump running, awaiting the gate).
    const first = reindexer.onChange();
    // A save lands WHILE that import is in flight — it can't be folded in.
    const second = await reindexer.onChange();
    expect(second).toBe('queued');
    // It refreshed immediately for feedback, but has NOT yet re-imported.
    expect(startImport).toHaveBeenCalledTimes(1);
    expect(refreshFiles).toHaveBeenCalledTimes(1);

    // Let the first import settle → exactly one follow-up import runs.
    gate.resolve();
    await first;

    expect(startImport).toHaveBeenCalledTimes(2); // one rerun, not zero, not many
    // refresh: the immediate queued one + one after each import pass.
    expect(refreshFiles).toHaveBeenCalledTimes(3);

    // The coordinator is idle again: a later change re-imports fresh.
    await reindexer.onChange();
    expect(startImport).toHaveBeenCalledTimes(3);
  });

  test('multiple changes during one import coalesce into a single rerun', async () => {
    const gate = deferred();
    const startImport = vi
      .fn()
      .mockImplementationOnce(() => gate.promise)
      .mockImplementation(async () => {});
    const refreshFiles = vi.fn(async () => {});

    const reindexer = createReindexer({ startImport, refreshFiles });
    const first = reindexer.onChange();
    await reindexer.onChange();
    await reindexer.onChange();
    await reindexer.onChange();

    gate.resolve();
    await first;

    // Three queued changes collapse to ONE follow-up import (not three).
    expect(startImport).toHaveBeenCalledTimes(2);
  });
});
