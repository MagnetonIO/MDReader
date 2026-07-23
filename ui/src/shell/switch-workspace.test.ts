import { describe, expect, test, vi } from 'vitest';

import { performSwitch } from './switch-workspace.js';
import { useTabsStore } from '../stores/tabs.js';
import type { DocRef } from '../stores/types.js';

const ref = (name: string): DocRef => ({ path: `/${name}.md`, name: `${name}.md` });

describe('performSwitch — same-root supersession (closure 2)', () => {
  test('a same-root user pick supersedes an in-flight session-restore open', async () => {
    // Model openRoot's epoch supersede: each open bumps a shared epoch and is
    // "current" only if a later open has not superseded it; the listing is
    // refreshed once per open.
    let epoch = 0;
    const listingRefreshedUnder: number[] = [];
    const openRoot = vi.fn(async () => {
      const mine = (epoch += 1);
      await Promise.resolve(); // in-flight (yields the event loop)
      listingRefreshedUnder.push(mine); // openRoot refreshes the listing
      return epoch === mine; // committed iff still the current epoch
    });
    const deps = {
      openRoot,
      previousRoot: '/R', // hydration already installed root R
      currentRoot: () => '/R',
      retireForeign: vi.fn(() => []),
      addRecent: vi.fn(),
    };

    // Hydration begins openRoot(R); the user picks the SAME R before it settles.
    const restore = performSwitch('/R', deps);
    const pick = performSwitch('/R', deps);
    const [restoreCommitted, pickCommitted] = await Promise.all([restore, pick]);

    // The user pick invoked openRoot (NO early return) → two opens ran, the
    // second superseding the first: a fresh epoch, the listing refreshed under
    // it. With the old `previousRoot === folder` early return, the pick (and the
    // restore) would skip openRoot entirely and these assertions fail.
    expect(openRoot).toHaveBeenCalledTimes(2);
    expect(epoch).toBe(2);
    expect(listingRefreshedUnder).toEqual([1, 2]);
    expect(restoreCommitted).toBe(false); // superseded
    expect(pickCommitted).toBe(true); // the current open
  });
});

describe('performSwitch — overlapping switches (closure 3)', () => {
  test('B then C retire A-origin tabs under A, never the transient root B', async () => {
    const store = useTabsStore;
    store.getState().reset();
    store.getState().openTab(ref('a'), { originRoot: '/A' });
    store.getState().openTab(ref('doc'), { originRoot: '/A' });

    const addRecent = vi.fn();
    const root = { current: '/A' };
    const retireForeign = (r: string | null) => store.getState().retireForeign(r);

    // Switch B installs its root synchronously (listing pending), then is
    // superseded by C (resolves committed=false).
    let releaseB: (v: boolean) => void = () => {};
    const openRootB = vi.fn(
      (folder: string) =>
        new Promise<boolean>((resolve) => {
          root.current = folder; // B installs '/B' mid-flight
          releaseB = resolve;
        }),
    );
    // Switch C commits, installing '/C'.
    const openRootC = vi.fn(async (folder: string) => {
      root.current = folder;
      return true;
    });

    // Start B (installs '/B', pending). C's previousRoot is captured AFTER B
    // installed '/B' — the stale snapshot the old code misattributed to.
    const bP = performSwitch('/B', {
      openRoot: openRootB,
      previousRoot: '/A',
      currentRoot: () => root.current,
      retireForeign,
      addRecent,
    });
    const previousRootForC = root.current; // '/B'
    const cP = performSwitch('/C', {
      openRoot: openRootC,
      previousRoot: previousRootForC,
      currentRoot: () => root.current,
      retireForeign,
      addRecent,
    });

    releaseB(false); // B superseded → retires nothing
    await Promise.all([bP, cP]);

    // C committed and retired the A-origin tabs under /A (their true origin),
    // never under /B (C's stale previousRoot). Old previousRoot-based attribution
    // would have recorded them under /B.
    expect(addRecent).toHaveBeenCalledWith(ref('a'), '/A');
    expect(addRecent).toHaveBeenCalledWith(ref('doc'), '/A');
    expect(addRecent).not.toHaveBeenCalledWith(ref('a'), '/B');
    expect(addRecent).not.toHaveBeenCalledWith(ref('doc'), '/B');
    expect(store.getState().tabs).toHaveLength(0);
  });
});
