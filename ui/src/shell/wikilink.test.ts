import { describe, expect, test, vi } from 'vitest';

import { navigateWikiLink } from './wikilink.js';
import type { WikiResolution } from '../platform/fs.js';

function handlers() {
  return {
    onResolved: vi.fn(),
    onAmbiguous: vi.fn(),
    onUnresolved: vi.fn(),
  };
}

describe('navigateWikiLink', () => {
  test('a single match opens the target, carrying the heading line', async () => {
    const h = handlers();
    const resolution: WikiResolution = {
      status: 'resolved',
      path: '/ws/guide.md',
      name: 'guide.md',
      line: 5,
    };
    await navigateWikiLink({ resolve: async () => resolution, ...h });

    expect(h.onResolved).toHaveBeenCalledWith({ path: '/ws/guide.md', name: 'guide.md' }, 5);
    expect(h.onAmbiguous).not.toHaveBeenCalled();
    expect(h.onUnresolved).not.toHaveBeenCalled();
  });

  test('a resolved match without a heading passes a null line', async () => {
    const h = handlers();
    await navigateWikiLink({
      resolve: async () => ({ status: 'resolved', path: '/ws/a.md', name: 'a.md', line: null }),
      ...h,
    });
    expect(h.onResolved).toHaveBeenCalledWith({ path: '/ws/a.md', name: 'a.md' }, null);
  });

  test('several matches surface the candidate set', async () => {
    const h = handlers();
    const candidates = [
      { path: '/ws/a/page.md', name: 'page.md' },
      { path: '/ws/b/page.md', name: 'page.md' },
    ];
    await navigateWikiLink({
      resolve: async () => ({ status: 'ambiguous', candidates }),
      ...h,
    });

    expect(h.onAmbiguous).toHaveBeenCalledWith(candidates);
    expect(h.onResolved).not.toHaveBeenCalled();
    expect(h.onUnresolved).not.toHaveBeenCalled();
  });

  test('no match routes to the unresolved handler', async () => {
    const h = handlers();
    await navigateWikiLink({ resolve: async () => ({ status: 'unresolved' }), ...h });
    expect(h.onUnresolved).toHaveBeenCalledTimes(1);
    expect(h.onResolved).not.toHaveBeenCalled();
  });

  test('a resolver failure (no backend) is treated as unresolved so the caller can fall back', async () => {
    const h = handlers();
    await navigateWikiLink({
      resolve: async () => {
        throw new Error('engine unavailable');
      },
      ...h,
    });
    expect(h.onUnresolved).toHaveBeenCalledTimes(1);
    expect(h.onAmbiguous).not.toHaveBeenCalled();
  });
});
