import { describe, expect, test, vi } from 'vitest';

import { findByWikiTarget, headingLineFromOutline, navigateWikiLink } from './wikilink.js';
import type { WikiResolution, WorkspaceFile } from '../platform/fs.js';

function file(path: string): WorkspaceFile {
  const name = path.split('/').pop()!;
  return { path, name, dir: path.slice(0, path.length - name.length - 1), mtime: 0, size: 0, ignored: false };
}

describe('findByWikiTarget — exact basename only', () => {
  const files = [file('/ws/foobar.md'), file('/ws/bar.md'), file('/ws/notes/Guide.md')];

  test('matches an exact basename, case- and extension-insensitively', () => {
    expect(findByWikiTarget(files, 'bar')?.path).toBe('/ws/bar.md');
    expect(findByWikiTarget(files, 'GUIDE')?.path).toBe('/ws/notes/Guide.md');
    expect(findByWikiTarget(files, 'bar.md')?.path).toBe('/ws/bar.md');
  });

  test('[[bar]] does NOT match foobar.md (no substring/endsWith matches)', () => {
    // `bar` must resolve to bar.md, never foobar.md — the old endsWith fallback
    // would have wrongly opened foobar.md when bar.md was absent.
    const withoutBar = [file('/ws/foobar.md')];
    expect(findByWikiTarget(withoutBar, 'bar')).toBeNull();
  });

  test('an empty target resolves to nothing', () => {
    expect(findByWikiTarget(files, '   ')).toBeNull();
  });
});

describe('headingLineFromOutline', () => {
  const outline = [
    { text: 'Intro', lineStart: 1 },
    { text: 'Section B', lineStart: 7 },
  ];

  test('finds the source line of a heading by (case-insensitive) text', () => {
    expect(headingLineFromOutline(outline, 'Section B')).toBe(7);
    expect(headingLineFromOutline(outline, 'section b')).toBe(7);
  });

  test('returns null for an unknown or empty heading', () => {
    expect(headingLineFromOutline(outline, 'Nope')).toBeNull();
    expect(headingLineFromOutline(outline, null)).toBeNull();
    expect(headingLineFromOutline(outline, '')).toBeNull();
  });
});

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
