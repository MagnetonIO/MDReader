import { afterEach, describe, expect, test } from 'vitest';

import { MAX_RECENT, useSourcesStore } from './sources.js';

afterEach(() => useSourcesStore.getState().reset());

const ref = (n: number) => ({ path: `/ws/${n}.md`, name: `${n}.md` });
const cloudRef = (sourceId: string, recordKey: string) => ({
  kind: 'cloud' as const,
  sourceId,
  recordKey,
  documentKey: `${sourceId}:${recordKey}`,
  name: 'README.md',
});

describe('addRecent — per-workspace scoping', () => {
  test('dedupes within a workspace by path, moving the entry to the front', () => {
    const { addRecent } = useSourcesStore.getState();
    addRecent(ref(1), '/wsA');
    addRecent(ref(2), '/wsA');
    addRecent(ref(1), '/wsA'); // re-open 1 → single entry, now newest

    const recent = useSourcesStore.getState().recent;
    expect(recent.map((e) => e.path)).toEqual(['/ws/1.md', '/ws/2.md']);
    expect(recent.every((e) => e.root === '/wsA')).toBe(true);
  });

  test('keeps the same path independently across workspaces', () => {
    const { addRecent } = useSourcesStore.getState();
    addRecent(ref(1), '/wsA');
    addRecent(ref(1), '/wsB'); // same path, different root → not a dedupe

    const recent = useSourcesStore.getState().recent;
    expect(recent).toHaveLength(2);
    expect(recent.filter((e) => e.path === '/ws/1.md').map((e) => e.root)).toEqual([
      '/wsB',
      '/wsA',
    ]);
  });

  test('caps per root, so activity in B cannot evict A history', () => {
    const { addRecent } = useSourcesStore.getState();
    // Fill A to the cap, then overflow it and add B activity.
    for (let n = 0; n < MAX_RECENT; n += 1) addRecent({ path: `/a/${n}.md`, name: `${n}.md` }, '/wsA');
    for (let n = 0; n < 5; n += 1) addRecent({ path: `/b/${n}.md`, name: `${n}.md` }, '/wsB');
    addRecent({ path: '/a/extra.md', name: 'extra.md' }, '/wsA'); // pushes A past the cap

    const recent = useSourcesStore.getState().recent;
    const aCount = recent.filter((e) => e.root === '/wsA').length;
    const bCount = recent.filter((e) => e.root === '/wsB').length;
    expect(aCount).toBe(MAX_RECENT); // A capped, not exceeded
    expect(bCount).toBe(5); // B untouched by A's overflow
    // The newest A entry survived; the oldest A entry was evicted.
    expect(recent.some((e) => e.path === '/a/extra.md')).toBe(true);
    expect(recent.some((e) => e.path === '/a/0.md')).toBe(false);
  });

  test('keeps same-named cloud documents distinct by provider identity', () => {
    const { addRecent } = useSourcesStore.getState();
    addRecent(cloudRef('google-drive', 'drive-readme'), null);
    addRecent(cloudRef('dropbox', '/docs/README.md'), null);
    addRecent(cloudRef('google-drive', 'drive-readme'), null);

    const recent = useSourcesStore.getState().recent;
    expect(recent).toHaveLength(2);
    expect(recent.map((entry) => entry.kind)).toEqual(['cloud', 'cloud']);
    expect(
      recent.map((entry) =>
        entry.kind === 'cloud' ? [entry.sourceId, entry.recordKey] : null,
      ),
    ).toEqual([
      ['google-drive', 'drive-readme'],
      ['dropbox', '/docs/README.md'],
    ]);
  });
});
