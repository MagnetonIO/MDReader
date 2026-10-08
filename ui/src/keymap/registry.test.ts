import { describe, expect, test, vi } from 'vitest';

import { KeymapRegistry, formatChord, parseChord } from './registry.js';
import type { Binding, KeyLike } from './registry.js';

function key(k: string, mods: Partial<KeyLike> = {}): KeyLike {
  return { key: k, metaKey: false, ctrlKey: false, altKey: false, shiftKey: false, ...mods };
}

function binding(over: Partial<Binding> = {}): Binding {
  return {
    id: 'test',
    keys: 'Mod+K',
    description: 'Test',
    group: 'General',
    run: vi.fn(),
    ...over,
  };
}

describe('parseChord', () => {
  test('maps Mod to Cmd on macOS and Ctrl elsewhere', () => {
    expect(parseChord('Mod+K', true)).toMatchObject({ meta: true, ctrl: false, key: 'k' });
    expect(parseChord('Mod+K', false)).toMatchObject({ meta: false, ctrl: true, key: 'k' });
  });

  test('Ctrl always means the literal Control key', () => {
    expect(parseChord('Ctrl+Tab', true)).toMatchObject({ meta: false, ctrl: true, key: 'tab' });
  });

  test('parses multi-modifier chords and punctuation keys', () => {
    expect(parseChord('Mod+Shift+T', true)).toMatchObject({ meta: true, shift: true, key: 't' });
    expect(parseChord('Mod+.', true)).toMatchObject({ meta: true, key: '.' });
    expect(parseChord('Mod+\\', true)).toMatchObject({ meta: true, key: '\\' });
  });
});

describe('dispatch', () => {
  test('runs the matching binding and reports handled', () => {
    const run = vi.fn();
    const registry = new KeymapRegistry(true);
    registry.register(binding({ keys: 'Mod+K', run }));
    expect(registry.dispatch(key('k', { metaKey: true }))).toBe(true);
    expect(run).toHaveBeenCalledOnce();
  });

  test('modifier matching is exact — Mod+K does not fire under Mod+Shift+K', () => {
    const run = vi.fn();
    const registry = new KeymapRegistry(true);
    registry.register(binding({ keys: 'Mod+K', run }));
    expect(registry.dispatch(key('k', { metaKey: true, shiftKey: true }))).toBe(false);
    expect(run).not.toHaveBeenCalled();
  });

  test('distinguishes Mod+Shift+T from Mod+T', () => {
    const plain = vi.fn();
    const shifted = vi.fn();
    const registry = new KeymapRegistry(true);
    registry.registerAll([
      binding({ id: 'a', keys: 'Mod+T', run: plain }),
      binding({ id: 'b', keys: 'Mod+Shift+T', run: shifted }),
    ]);
    registry.dispatch(key('t', { metaKey: true }));
    registry.dispatch(key('T', { metaKey: true, shiftKey: true }));
    expect(plain).toHaveBeenCalledOnce();
    expect(shifted).toHaveBeenCalledOnce();
  });

  test('skips bindings while a text field is focused unless opted in', () => {
    const guarded = vi.fn();
    const allowed = vi.fn();
    const registry = new KeymapRegistry(true);
    registry.registerAll([
      binding({ id: 'g', keys: 'Mod+K', run: guarded }),
      binding({ id: 'a', keys: 'Mod+K', run: allowed, allowInInput: true }),
    ]);
    // First match is the guarded one; in an editable it must NOT fall through.
    expect(registry.dispatch(key('k', { metaKey: true }), { inEditable: true })).toBe(false);
    expect(guarded).not.toHaveBeenCalled();
  });

  test('returns false when nothing matches', () => {
    const registry = new KeymapRegistry(true);
    registry.register(binding({ keys: 'Mod+K' }));
    expect(registry.dispatch(key('j', { metaKey: true }))).toBe(false);
  });

  test.each([true, false])('editor search uses the platform modifier without enabling form shortcuts (mac=%s)', (mac) => {
    const run = vi.fn();
    const registry = new KeymapRegistry(mac);
    registry.register(binding({ run, allowInSourceEditor: true }));
    const chord = key('k', mac ? { metaKey: true } : { ctrlKey: true });
    expect(registry.dispatch(chord, { inEditable: true })).toBe(false);
    expect(registry.dispatch(chord, { inEditable: true, inSourceEditor: true })).toBe(true);
    expect(registry.dispatch(key('k', mac ? { ctrlKey: true } : { metaKey: true }), { inEditable: true, inSourceEditor: true })).toBe(false);
    expect(run).toHaveBeenCalledOnce();
  });

  test('Ctrl+Tab fires on macOS via ctrlKey, not metaKey', () => {
    const run = vi.fn();
    const registry = new KeymapRegistry(true);
    registry.register(binding({ keys: 'Ctrl+Tab', run }));
    expect(registry.dispatch(key('Tab', { ctrlKey: true }))).toBe(true);
    expect(registry.dispatch(key('Tab', { metaKey: true }))).toBe(false);
  });
});

describe('sheet support', () => {
  test('groups bindings preserving first-seen order', () => {
    const registry = new KeymapRegistry(true);
    registry.registerAll([
      binding({ id: '1', group: 'Tabs' }),
      binding({ id: '2', group: 'View' }),
      binding({ id: '3', group: 'Tabs' }),
    ]);
    const groups = registry.groups();
    expect(groups.map((g) => g.group)).toEqual(['Tabs', 'View']);
    expect(groups[0]!.bindings).toHaveLength(2);
  });

  test('formatChord renders platform-appropriate symbols', () => {
    expect(formatChord('Mod+Shift+T', true)).toBe('⌘⇧T');
    expect(formatChord('Mod+Shift+T', false)).toBe('Ctrl+Shift+T');
    expect(formatChord('Mod+.', true)).toBe('⌘.');
  });
});
