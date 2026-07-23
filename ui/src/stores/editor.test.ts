import { beforeEach, describe, expect, it } from 'vitest';

import { useEditorStore } from './editor.js';

function store() {
  return useEditorStore.getState();
}

beforeEach(() => {
  useEditorStore.getState().reset();
});

describe('editor store — dirty lifecycle', () => {
  it('a tab with no entry reads as read mode and not dirty', () => {
    expect(store().mode('tab-1')).toBe('read');
    expect(store().isDirty('tab-1')).toBe(false);
    expect(store().entry('tab-1')).toBeUndefined();
  });

  it('seeding enters a clean buffer anchored to the disk hash', () => {
    store().seed('tab-1', '# Hello\n', 'h0', 'source');
    const entry = store().entry('tab-1')!;
    expect(entry.mode).toBe('source');
    expect(entry.buffer).toBe('# Hello\n');
    expect(entry.savedContent).toBe('# Hello\n');
    expect(entry.baseHash).toBe('h0');
    expect(entry.dirty).toBe(false);
    expect(store().isDirty('tab-1')).toBe(false);
  });

  it('an edit that diverges from disk marks the tab dirty; reverting clears it', () => {
    store().seed('tab-1', 'original', 'h0', 'source');

    store().edit('tab-1', 'original + edit');
    expect(store().isDirty('tab-1')).toBe(true);

    // Typing back to exactly the saved content is clean again — no phantom dirt.
    store().edit('tab-1', 'original');
    expect(store().isDirty('tab-1')).toBe(false);
  });

  it('markSaved adopts the fresh hash, clears dirty, and drops any conflict', () => {
    store().seed('tab-1', 'a', 'h0', 'source');
    store().edit('tab-1', 'a2');
    store().raiseConflict('tab-1', 'hDisk', 'save');
    expect(store().entry('tab-1')!.conflict).not.toBeNull();

    store().markSaved('tab-1', 'h1');
    const entry = store().entry('tab-1')!;
    expect(entry.dirty).toBe(false);
    expect(entry.baseHash).toBe('h1'); // the new anchor (self-write detection)
    expect(entry.savedContent).toBe('a2'); // disk now holds the buffer
    expect(entry.conflict).toBeNull();
  });
});

describe('editor store — conflict state machine', () => {
  it('raiseConflict opens the choose phase without touching the buffer', () => {
    store().seed('tab-1', 'mine', 'h0', 'source');
    store().edit('tab-1', 'mine edited');
    store().raiseConflict('tab-1', 'hDisk', 'external');

    const entry = store().entry('tab-1')!;
    expect(entry.conflict).toEqual({ currentHash: 'hDisk', source: 'external', phase: 'choose' });
    expect(entry.buffer).toBe('mine edited'); // buffer preserved
    expect(entry.dirty).toBe(true);
  });

  it('keep-mine advances to the explicit overwrite confirmation', () => {
    store().seed('tab-1', 'mine', 'h0', 'source');
    store().raiseConflict('tab-1', 'hDisk', 'save');

    store().toConfirmOverwrite('tab-1');
    expect(store().entry('tab-1')!.conflict).toEqual({
      currentHash: 'hDisk',
      source: 'save',
      phase: 'confirm',
    });
  });

  it('clearConflict dismisses the banner and keeps editing', () => {
    store().seed('tab-1', 'mine', 'h0', 'source');
    store().edit('tab-1', 'mine!');
    store().raiseConflict('tab-1', 'hDisk', 'save');

    store().clearConflict('tab-1');
    const entry = store().entry('tab-1')!;
    expect(entry.conflict).toBeNull();
    expect(entry.buffer).toBe('mine!'); // buffer untouched
    expect(entry.dirty).toBe(true);
  });

  it('re-seeding (reload from disk) discards local edits and the conflict', () => {
    store().seed('tab-1', 'mine', 'h0', 'source');
    store().edit('tab-1', 'mine edited');
    store().raiseConflict('tab-1', 'hDisk', 'external');

    // Reload adopts the disk version at its real hash.
    store().seed('tab-1', 'the disk version', 'hDisk', 'source');
    const entry = store().entry('tab-1')!;
    expect(entry.buffer).toBe('the disk version');
    expect(entry.baseHash).toBe('hDisk');
    expect(entry.dirty).toBe(false);
    expect(entry.conflict).toBeNull();
  });
});

describe('editor store — isolation and cleanup', () => {
  it('each tab keeps its own mode and buffer', () => {
    store().seed('tab-1', 'one', 'h1', 'source');
    store().seed('tab-2', 'two', 'h2', 'split');
    store().edit('tab-1', 'one!');

    expect(store().isDirty('tab-1')).toBe(true);
    expect(store().isDirty('tab-2')).toBe(false);
    expect(store().mode('tab-2')).toBe('split');
  });

  it('remove drops a closed tab’s entry', () => {
    store().seed('tab-1', 'x', 'h', 'source');
    store().remove('tab-1');
    expect(store().entry('tab-1')).toBeUndefined();
  });
});
