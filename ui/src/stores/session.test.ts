import { describe, expect, test } from 'vitest';

import {
  DEFAULT_SETTINGS,
  SESSION_VERSION,
  deserializeSession,
  sanitizeSettings,
  serializeSession,
  serializeSettings,
} from './session.js';
import type { SessionInput } from './session.js';
import type { TabState } from './types.js';

function tab(id: string, name: string, extra: Partial<TabState> = {}): TabState {
  return {
    id,
    docRef: { path: `/ws/${name}.md`, name: `${name}.md` },
    history: [{ path: `/ws/${name}.md`, name: `${name}.md` }],
    historyIndex: 0,
    scroll: { block: 0, offset: 0 },
    pinned: false,
    originRoot: '/ws',
    ...extra,
  };
}

describe('session round-trip', () => {
  test('serialize → deserialize preserves tabs, active, root, recent', () => {
    const input: SessionInput = {
      root: '/ws',
      activeTabId: 'tab-2',
      recent: [{ path: '/ws/a.md', name: 'a.md', root: '/ws' }],
      tabs: [
        tab('tab-1', 'a', { pinned: true, scroll: { block: 3, offset: 12 } }),
        tab('tab-2', 'b', {
          history: [
            { path: '/ws/b.md', name: 'b.md' },
            { path: '/ws/c.md', name: 'c.md' },
          ],
          historyIndex: 1,
          docRef: { path: '/ws/c.md', name: 'c.md' },
        }),
      ],
    };

    const restored = deserializeSession(serializeSession(input));
    expect(restored).not.toBeNull();
    expect(restored!.root).toBe('/ws');
    expect(restored!.activeTabId).toBe('tab-2');
    expect(restored!.recent).toEqual(input.recent);
    expect(restored!.tabs).toHaveLength(2);
    expect(restored!.tabs[0]!.pinned).toBe(true);
    expect(restored!.tabs[0]!.scroll).toEqual({ block: 3, offset: 12 });
    expect(restored!.tabs[1]!.docRef.name).toBe('c.md');
    expect(restored!.tabs[1]!.historyIndex).toBe(1);
    // Each tab's origin workspace round-trips (drives per-origin retirement).
    expect(restored!.tabs[0]!.originRoot).toBe('/ws');
  });

  test('a legacy tab (no origin) migrates to adopt the session root on load', () => {
    // Simulate a pre-originRoot blob: a tab lacking the field the writer now
    // emits, under a real workspace root.
    const legacy = {
      version: SESSION_VERSION,
      root: '/ws',
      activeTabId: 'tab-1',
      recent: [],
      tabs: [
        {
          id: 'tab-1',
          docRef: { path: '/ws/a.md', name: 'a.md' },
          history: [{ path: '/ws/a.md', name: 'a.md' }],
          historyIndex: 0,
          scroll: { block: 0, offset: 0 },
          pinned: false,
        },
      ],
    };
    const restored = deserializeSession(legacy);
    // Migration: the restored tab adopts the session root as its origin, so a
    // same-root reopen keeps it (retireForeign('/ws') sees origin '/ws').
    expect(restored!.tabs[0]!.originRoot).toBe('/ws');
  });

  test('a legacy tab under no workspace stays null (nothing to adopt)', () => {
    const legacy = {
      version: SESSION_VERSION,
      root: null,
      activeTabId: 'tab-1',
      recent: [],
      tabs: [
        {
          id: 'tab-1',
          docRef: { path: '/a.md', name: 'a.md' },
          history: [{ path: '/a.md', name: 'a.md' }],
          historyIndex: 0,
          scroll: { block: 0, offset: 0 },
          pinned: false,
        },
      ],
    };
    expect(deserializeSession(legacy)!.tabs[0]!.originRoot).toBeNull();
  });

  test('serialized blob carries the schema version', () => {
    const data = serializeSession({ root: null, activeTabId: null, tabs: [], recent: [] });
    expect(data.version).toBe(SESSION_VERSION);
  });
});

describe('deserializeSession defensiveness', () => {
  test('rejects a non-object', () => {
    expect(deserializeSession('nope')).toBeNull();
    expect(deserializeSession(null)).toBeNull();
    expect(deserializeSession(42)).toBeNull();
  });

  test('rejects a mismatched version', () => {
    expect(deserializeSession({ version: 99, tabs: [] })).toBeNull();
  });

  test('drops malformed tabs but keeps valid ones', () => {
    const restored = deserializeSession({
      version: SESSION_VERSION,
      root: '/ws',
      activeTabId: 'tab-9',
      tabs: [
        { id: 'tab-1', docRef: { path: '/ws/a.md', name: 'a.md' } },
        { id: 'bad', docRef: { path: 123 } },
        { nonsense: true },
      ],
    });
    expect(restored!.tabs).toHaveLength(1);
    expect(restored!.tabs[0]!.id).toBe('tab-1');
    // active id was invalid → falls back to the first surviving tab.
    expect(restored!.activeTabId).toBe('tab-1');
  });

  test('tags a legacy recent entry (no root) with the session root', () => {
    const restored = deserializeSession({
      version: SESSION_VERSION,
      root: '/ws',
      tabs: [],
      recent: [
        { path: '/ws/a.md', name: 'a.md' },
        { path: '/ws/b.md', name: 'b.md', root: '/other' },
      ],
    });
    expect(restored!.recent).toEqual([
      { path: '/ws/a.md', name: 'a.md', root: '/ws' },
      { path: '/ws/b.md', name: 'b.md', root: '/other' },
    ]);
  });

  test('clamps an out-of-range historyIndex and back-fills history', () => {
    const restored = deserializeSession({
      version: SESSION_VERSION,
      tabs: [{ id: 'tab-1', docRef: { path: '/ws/a.md', name: 'a.md' }, historyIndex: 9 }],
    });
    const restoredTab = restored!.tabs[0]!;
    expect(restoredTab.history).toHaveLength(1);
    expect(restoredTab.historyIndex).toBe(0);
  });
});

describe('settings', () => {
  test('round-trips a full snapshot', () => {
    const snapshot = {
      theme: 'dark',
      readingFont: 'sans',
      textScaleIndex: 5,
      sidebarCollapsed: true,
      outlineVisible: false,
      relatedRailOpen: true,
    } as const;
    expect(sanitizeSettings(serializeSettings(snapshot))).toEqual(snapshot);
  });

  test('drops invalid fields, keeping the rest', () => {
    const cleaned = sanitizeSettings({
      theme: 'aubergine',
      readingFont: 'sans',
      textScaleIndex: 999,
      sidebarCollapsed: 'yes',
      outlineVisible: true,
      relatedRailOpen: 'open',
    });
    expect(cleaned).toEqual({ readingFont: 'sans', outlineVisible: true });
  });

  test('persists the related-rail open/collapsed choice', () => {
    // The collapsed default is the absence of the flag; an explicit `true`
    // survives the disk round-trip so the rail reopens where the user left it.
    expect(sanitizeSettings({ relatedRailOpen: true })).toEqual({ relatedRailOpen: true });
    expect(sanitizeSettings({ relatedRailOpen: false })).toEqual({ relatedRailOpen: false });
    expect(DEFAULT_SETTINGS.relatedRailOpen).toBe(false);
  });

  test('exposes sane defaults', () => {
    expect(DEFAULT_SETTINGS.theme).toBe('system');
    expect(DEFAULT_SETTINGS.readingFont).toBe('serif');
  });
});
