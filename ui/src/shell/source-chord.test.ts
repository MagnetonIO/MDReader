import { describe, expect, it } from 'vitest';

import { sourceChord, type ChordEvent } from './source-chord.js';

function ev(partial: Partial<ChordEvent> & { key: string }): ChordEvent {
  return { metaKey: false, ctrlKey: false, altKey: false, shiftKey: false, ...partial };
}

describe('sourceChord — only the platform primary modifier is reserved', () => {
  it('macOS reserves ⌘S / ⌘E', () => {
    expect(sourceChord(true, ev({ key: 's', metaKey: true }))).toBe('save');
    expect(sourceChord(true, ev({ key: 'e', metaKey: true }))).toBe('toggle');
  });

  it('macOS leaves Ctrl+E and Ctrl+S to CodeMirror (NOT save/toggle)', () => {
    // Ctrl+E on macOS is CM6's cursor-to-line-end — must never toggle the mode.
    expect(sourceChord(true, ev({ key: 'e', ctrlKey: true }))).toBeNull();
    expect(sourceChord(true, ev({ key: 's', ctrlKey: true }))).toBeNull();
  });

  it('non-macOS reserves Ctrl+S / Ctrl+E and leaves ⌘ alone', () => {
    expect(sourceChord(false, ev({ key: 's', ctrlKey: true }))).toBe('save');
    expect(sourceChord(false, ev({ key: 'e', ctrlKey: true }))).toBe('toggle');
    expect(sourceChord(false, ev({ key: 'e', metaKey: true }))).toBeNull();
  });

  it('find and undo/redo use the same host modifier as Save', () => {
    expect(sourceChord(true, ev({ key: 'f', metaKey: true }))).toBe('find');
    expect(sourceChord(false, ev({ key: 'f', ctrlKey: true }))).toBe('find');
    expect(sourceChord(false, ev({ key: 'z', ctrlKey: true }))).toBe('undo');
    expect(sourceChord(true, ev({ key: 'z', metaKey: true, shiftKey: true }))).toBe('redo');
    expect(sourceChord(false, ev({ key: 'y', ctrlKey: true }))).toBe('redo');
  });

  it('the primary modifier must be held ALONE (no extra modifiers)', () => {
    expect(sourceChord(true, ev({ key: 's', metaKey: true, shiftKey: true }))).toBeNull();
    expect(sourceChord(true, ev({ key: 'e', metaKey: true, altKey: true }))).toBeNull();
    expect(sourceChord(true, ev({ key: 's', metaKey: true, ctrlKey: true }))).toBeNull();
  });
});
