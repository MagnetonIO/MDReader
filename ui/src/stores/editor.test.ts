import { beforeEach, describe, expect, it } from 'vitest';

import { useEditorStore } from './editor.js';

function store() {
  return useEditorStore.getState();
}

const A = '/ws/a.md';
const B = '/ws/b.md';

beforeEach(() => {
  useEditorStore.getState().reset();
});

describe('editor store — dirty lifecycle', () => {
  it('a (tab, doc) with no entry reads as read mode and not dirty', () => {
    expect(store().mode('tab-1', A)).toBe('read');
    expect(store().isDirty('tab-1', A)).toBe(false);
    expect(store().entry('tab-1', A)).toBeUndefined();
  });

  it('seeding enters a clean buffer anchored to the disk hash and its doc path', () => {
    store().seed('tab-1', A, '# Hello\n', 'h0', 'source');
    const entry = store().entry('tab-1', A)!;
    expect(entry.tabId).toBe('tab-1');
    expect(entry.path).toBe(A);
    expect(entry.mode).toBe('source');
    expect(entry.buffer).toBe('# Hello\n');
    expect(entry.baseHash).toBe('h0');
    expect(entry.dirty).toBe(false);
  });

  it('an edit that diverges from disk marks it dirty; reverting clears it', () => {
    store().seed('tab-1', A, 'original', 'h0', 'source');
    store().edit('tab-1', A, 'original + edit');
    expect(store().isDirty('tab-1', A)).toBe(true);
    store().edit('tab-1', A, 'original');
    expect(store().isDirty('tab-1', A)).toBe(false);
  });

  it('markSaved adopts the fresh hash, clears dirty, and drops any conflict', () => {
    store().seed('tab-1', A, 'a', 'h0', 'source');
    store().edit('tab-1', A, 'a2');
    store().raiseConflict('tab-1', A, 'hDisk', 'save');
    store().markSaved('tab-1', A, 'a2', 'h1');
    const entry = store().entry('tab-1', A)!;
    expect(entry.dirty).toBe(false);
    expect(entry.baseHash).toBe('h1');
    expect(entry.savedContent).toBe('a2');
    expect(entry.conflict).toBeNull();
  });

  it('typing DURING a save keeps it dirty (the newer buffer was not written)', () => {
    store().seed('tab-1', A, 'v0', 'h0', 'source');
    store().edit('tab-1', A, 'v1'); // launched with snapshot v1
    store().edit('tab-1', A, 'v2'); // typed on while in flight
    store().markSaved('tab-1', A, 'v1', 'h1');
    const entry = store().entry('tab-1', A)!;
    expect(entry.savedContent).toBe('v1');
    expect(entry.buffer).toBe('v2');
    expect(entry.dirty).toBe(true);
  });
});

describe('editor store — navigation NEVER drops an unsaved buffer', () => {
  it('a tab that navigates away keeps its (tab, doc) entry; returning restores it', () => {
    // Editing A in tab-1…
    store().seed('tab-1', A, 'A on disk', 'hA', 'source');
    store().edit('tab-1', A, 'A with my unsaved edit');
    expect(store().isDirty('tab-1', A)).toBe(true);

    // …the tab navigates to B. B has its own (absent) entry — read mode — while
    // A's dirty buffer is untouched under its own key (nothing pruned it).
    expect(store().entry('tab-1', B)).toBeUndefined();
    expect(store().mode('tab-1', B)).toBe('read');
    expect(store().entry('tab-1', A)!.buffer).toBe('A with my unsaved edit');

    // The tab still shows as dirty (it has unsaved work, just not on the shown doc).
    expect(store().tabIsDirty('tab-1')).toBe(true);

    // Returning to A restores the exact unsaved buffer.
    expect(store().entry('tab-1', A)!.buffer).toBe('A with my unsaved edit');
    expect(store().isDirty('tab-1', A)).toBe(true);
  });

  it('a save can only reach the CURRENT document (wrong-doc save is structural)', () => {
    // tab-1 has a dirty A buffer but is now showing B. A save keyed to B finds no
    // entry, so A's buffer can never be written to B.
    store().seed('tab-1', A, 'A body', 'hA', 'source');
    store().edit('tab-1', A, 'A edited');
    expect(store().entry('tab-1', B)).toBeUndefined();
    // The A entry is still there, dirty — preserved, not saved to B.
    expect(store().isDirty('tab-1', A)).toBe(true);
  });
});

describe('editor store — conflict state machine', () => {
  it('raiseConflict opens the choose phase without touching the buffer', () => {
    store().seed('tab-1', A, 'mine', 'h0', 'source');
    store().edit('tab-1', A, 'mine edited');
    store().raiseConflict('tab-1', A, 'hDisk', 'external');
    const entry = store().entry('tab-1', A)!;
    expect(entry.conflict).toEqual({ currentHash: 'hDisk', source: 'external', phase: 'choose' });
    expect(entry.buffer).toBe('mine edited');
  });

  it('keep-mine advances to the explicit overwrite confirmation', () => {
    store().seed('tab-1', A, 'mine', 'h0', 'source');
    store().raiseConflict('tab-1', A, 'hDisk', 'save');
    store().toConfirmOverwrite('tab-1', A);
    expect(store().entry('tab-1', A)!.conflict).toEqual({
      currentHash: 'hDisk',
      source: 'save',
      phase: 'confirm',
    });
  });

  it('clearConflict dismisses the banner and keeps editing', () => {
    store().seed('tab-1', A, 'mine', 'h0', 'source');
    store().edit('tab-1', A, 'mine!');
    store().raiseConflict('tab-1', A, 'hDisk', 'save');
    store().clearConflict('tab-1', A);
    const entry = store().entry('tab-1', A)!;
    expect(entry.conflict).toBeNull();
    expect(entry.buffer).toBe('mine!');
    expect(entry.dirty).toBe(true);
  });

  it('re-seeding (reload from disk) discards local edits and the conflict', () => {
    store().seed('tab-1', A, 'mine', 'h0', 'source');
    store().edit('tab-1', A, 'mine edited');
    store().raiseConflict('tab-1', A, 'hDisk', 'external');
    store().seed('tab-1', A, 'the disk version', 'hDisk', 'source');
    const entry = store().entry('tab-1', A)!;
    expect(entry.buffer).toBe('the disk version');
    expect(entry.dirty).toBe(false);
    expect(entry.conflict).toBeNull();
  });
});

describe('editor store — dirty aggregation and cleanup', () => {
  it('tracks dirty tabs and every dirty entry across docs and tabs', () => {
    store().seed('tab-1', A, 'a', 'hA', 'source');
    store().seed('tab-1', B, 'b', 'hB', 'source'); // same tab, two docs
    store().seed('tab-2', A, 'a2', 'hA2', 'source');
    store().edit('tab-1', B, 'b!'); // dirty (tab-1, B)
    store().edit('tab-2', A, 'a2!'); // dirty (tab-2, A)

    expect(store().tabIsDirty('tab-1')).toBe(true);
    expect(store().dirtyTabIds().sort()).toEqual(['tab-1', 'tab-2']);
    expect(store().allDirtyEntries().map((e) => e.path).sort()).toEqual([A, B]);
  });

  it('removeTab drops every entry for a closed tab (and leaves others)', () => {
    store().seed('tab-1', A, 'a', 'hA', 'source');
    store().seed('tab-1', B, 'b', 'hB', 'source');
    store().seed('tab-2', A, 'a', 'hA', 'source');
    store().removeTab('tab-1');
    expect(store().entry('tab-1', A)).toBeUndefined();
    expect(store().entry('tab-1', B)).toBeUndefined();
    expect(store().entry('tab-2', A)).toBeDefined();
  });
});
