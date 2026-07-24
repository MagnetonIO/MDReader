/**
 * A GENUINE settings-persistence round-trip for the Related rail: the app writes
 * `relatedRailOpen` through the REAL persistence path (real `saveSettings` → an
 * in-memory tauri-plugin-store), then a FRESH AppShell mount reads it back through
 * the REAL `loadPersisted` (NOT a stubbed `{}`), so a regression anywhere on the
 * save→store→load→sanitize→hydrate→UI chain is caught. This is deliberately NOT
 * the pattern in edit-integration.test.tsx (which mocks `loadPersisted`/`saveSettings`
 * to no-ops) — that cannot prove a preference survives an actual relaunch.
 *
 * Only the OS/IPC boundary is faked: the tauri store (in-memory, shared across the
 * two mounts), the tauri window, the fs IPC, and the CodeMirror leaf. `persistence.ts`
 * itself runs for real, guarded on `isTauri()` — so we set `__TAURI_INTERNALS__` to
 * make the real guard pass.
 */

import { act, cleanup, render, screen, waitFor } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

// The in-memory backing for the mocked tauri store, keyed `${file}::${key}`. Shared
// across mounts within a test (persistence.ts memoizes the store handle), cleared
// between tests.
const h = vi.hoisted(() => ({ store: new Map<string, unknown>() }));

vi.mock('@tauri-apps/plugin-store', () => ({
  load: async (name: string) => ({
    get: async (key: string) => h.store.get(`${name}::${key}`),
    set: async (key: string, value: unknown) => {
      h.store.set(`${name}::${key}`, value);
    },
    save: async () => {},
  }),
}));

vi.mock('@tauri-apps/api/window', () => ({
  getCurrentWindow: () => ({
    onCloseRequested: vi.fn(async () => () => {}),
    destroy: vi.fn(async () => {}),
    close: vi.fn(async () => {}),
  }),
}));

vi.mock('../platform/fs.js', async (importOriginal) => {
  const actual = await importOriginal<typeof import('../platform/fs.js')>();
  return {
    ...actual,
    readDoc: vi.fn(async () => ({ content: '# Note\n\nbody\n', mtime: 0, hash: 'h0' })),
    saveDoc: vi.fn(async () => ({ status: 'saved' as const, newHash: 'h1', mtime: 0 })),
    onFsChanged: vi.fn(async () => () => {}),
    onWatchError: vi.fn(async () => () => {}),
    watchWorkspace: vi.fn(async (root: string) => ({ root, generation: 1 })),
    setWorkspace: vi.fn(async (root: string) => root),
    listWorkspaceFiles: vi.fn(async () => ({ files: [], fromIndex: true })),
    startImport: vi.fn(async () => ({
      phase: 'done',
      filesDone: 0,
      filesTotal: 0,
      chunks: 0,
      error: null,
    })),
    pickWorkspaceFolder: vi.fn(async () => '/ws'),
    resolveWikilink: vi.fn(async () => ({ status: 'unresolved' as const })),
    makeSearchAdapter: vi.fn(() => ({ search: vi.fn(async () => []), related: vi.fn(async () => []) })),
  };
});

// The CodeMirror leaf is an e2e concern; keep it inert so this runs in jsdom.
vi.mock('./SourcePane.js', () => ({
  SourcePane: ({ value }: { value: string }) => <div data-testid="source-pane">{value}</div>,
}));

import App from '../App';
import { useTabsStore } from '../stores/tabs';
import { useSourcesStore } from '../stores/sources';
import { useSettingsStore } from '../stores/settings';
import { useEditorStore } from '../stores/editor';

/** Make the real `isTauri()` return true so the real persistence path is live. */
const tauriWindow = window as unknown as { __TAURI_INTERNALS__?: unknown };

beforeEach(() => {
  h.store.clear();
  tauriWindow.__TAURI_INTERNALS__ = {};
});

afterEach(() => {
  cleanup();
  delete tauriWindow.__TAURI_INTERNALS__;
  useTabsStore.getState().reset();
  useSourcesStore.getState().reset();
  useSettingsStore.getState().reset();
  useEditorStore.getState().reset();
});

describe('Related rail persistence: real save → fresh mount → real load', () => {
  it('a rail opened in one run is restored OPEN after a full relaunch', async () => {
    // --- run 1: open the rail, then the app persists on the way out ------------
    render(<App />);
    await act(async () => {}); // settle startup (real loadPersisted: empty store → collapsed)
    expect(useSettingsStore.getState().relatedRailOpen).toBe(false);

    act(() => useSettingsStore.getState().setRelatedRailOpen(true));
    // The window going away flushes settings through the REAL saveSettings.
    act(() => window.dispatchEvent(new Event('beforeunload')));

    // The choice actually reached the (in-memory) store via the real write path.
    await waitFor(() => {
      const persisted = h.store.get('settings.json::settings') as
        | { relatedRailOpen?: boolean }
        | undefined;
      expect(persisted?.relatedRailOpen).toBe(true);
    });

    // --- relaunch: drop all in-memory app state; only the store survives -------
    cleanup();
    useSettingsStore.getState().reset();
    useTabsStore.getState().reset();
    useSourcesStore.getState().reset();
    expect(useSettingsStore.getState().relatedRailOpen).toBe(false); // truly reset

    // --- run 2: a FRESH mount reads the ACTUAL persisted settings -------------
    render(<App />);
    // Real loadPersisted → sanitizeSettings → hydrate restores the preference.
    await waitFor(() => expect(useSettingsStore.getState().relatedRailOpen).toBe(true));

    // The restored preference drives the shell: opening a doc shows the rail OPEN
    // in its own reflow column, with no collapsed strip — no toggle needed.
    act(() => {
      useTabsStore.getState().openTab({ path: '/ws/note.md', name: 'note.md' }, { originRoot: '/ws' });
    });
    await waitFor(() => expect(document.querySelector('.related-rail')).not.toBeNull());

    const row = document.querySelector('.shell__reader-row')!;
    expect(row.classList.contains('shell__reader-row--related-open')).toBe(true);
    expect(document.querySelector('.related-rail')!.parentElement).toBe(row);
    expect(screen.queryByTitle('Show related notes')).toBeNull();
  });
});
