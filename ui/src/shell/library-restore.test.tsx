/**
 * Session roots are history, not filesystem authority. These tests drive the
 * real AppShell startup and prove it asks the backend-owned library registry
 * before the existing watcher/grant path can run.
 */

import { act, cleanup, render, screen, waitFor } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import type { LibrarySourceDto } from '../bindings';

const ROOT = '/persisted/vault';
const CANONICAL_ROOT = '/canonical/vault';
const SOURCE: LibrarySourceDto = {
  id: 'c'.repeat(64),
  name: 'vault',
  path: CANONICAL_ROOT,
  state: { kind: 'available' },
  index_state: 'present',
  active: false,
  restore_candidate: true,
};

const h = vi.hoisted(() => ({
  restore: null as LibrarySourceDto | null,
  rejectRestore: false,
  watch: vi.fn(async (sourceId: string) => ({
    root: sourceId === 'c'.repeat(64) ? CANONICAL_ROOT : '/unexpected',
    generation: 7,
  })),
  grant: vi.fn(async (sourceId: string) =>
    sourceId === 'c'.repeat(64) ? CANONICAL_ROOT : '/unexpected',
  ),
  mark: vi.fn(async () => ({ version: 1, sources: [] })),
}));

vi.mock('../platform/fs.js', async (importOriginal) => {
  const actual = await importOriginal<typeof import('../platform/fs.js')>();
  return {
    ...actual,
    onFsChanged: vi.fn(async () => () => {}),
    onWatchError: vi.fn(async () => () => {}),
    watchWorkspace: vi.fn(async (root: string) => ({ root, generation: 7 })),
    setWorkspace: vi.fn(async (root: string) => root),
    listWorkspaceFiles: vi.fn(async () => ({ files: [], fromIndex: true })),
    startImport: vi.fn(async () => ({
      phase: 'done',
      filesDone: 0,
      filesTotal: 0,
      chunks: 0,
      error: null,
    })),
    resolveWikilink: vi.fn(async () => ({ status: 'unresolved' as const })),
    makeSearchAdapter: vi.fn(() => ({
      search: vi.fn(async () => []),
      related: vi.fn(async () => []),
    })),
  };
});

vi.mock('../platform/settings.js', async (importOriginal) => {
  const actual = await importOriginal<typeof import('../platform/settings.js')>();
  return {
    ...actual,
    reconcileLibraryRestore: vi.fn(async () => {
      if (h.rejectRestore) throw new Error('corrupt library');
      return h.restore;
    }),
    watchLibrarySource: h.watch,
    grantLibrarySource: h.grant,
    markLibrarySourceActive: h.mark,
    recordDiagnosticEventNonBlocking: vi.fn(),
  };
});

vi.mock('./persistence.js', async (importOriginal) => {
  const actual = await importOriginal<typeof import('./persistence.js')>();
  return {
    ...actual,
    isTauri: () => true,
    loadPersisted: vi.fn(async () => ({
      session: {
        root: ROOT,
        activeTabId: null,
        tabs: [],
        recent: [{ root: ROOT, path: `${ROOT}/remembered.md`, name: 'remembered.md' }],
      },
      settings: {},
    })),
    saveSettings: vi.fn(async () => {}),
    sessionWriter: { write: vi.fn(), drain: vi.fn(async () => {}) },
  };
});

vi.mock('@tauri-apps/api/window', () => ({
  getCurrentWindow: () => ({
    onCloseRequested: vi.fn(async () => () => {}),
    destroy: vi.fn(async () => {}),
  }),
}));

import App from '../App';
import { useEditorStore } from '../stores/editor';
import { useSettingsStore } from '../stores/settings';
import { useSourcesStore } from '../stores/sources';
import { useTabsStore } from '../stores/tabs';

beforeEach(() => {
  h.restore = null;
  h.rejectRestore = false;
  vi.clearAllMocks();
  useEditorStore.getState().reset();
  useSettingsStore.getState().reset();
  useSourcesStore.getState().reset();
  useTabsStore.getState().reset();
});

afterEach(cleanup);

describe('library-authorized session restore', () => {
  it('does not grant a persisted root that is no longer an available library member', async () => {
    render(<App />);
    await act(async () => {});

    await waitFor(() => expect(useSourcesStore.getState().recent).toHaveLength(1));
    expect(h.watch).not.toHaveBeenCalled();
    expect(useSourcesStore.getState().root).toBeNull();
    expect(screen.getByText('No folder')).toBeTruthy();
  });

  it('opens the canonical member and marks it active only after the watcher grant commits', async () => {
    h.restore = SOURCE;
    render(<App />);

    await waitFor(() => expect(useSourcesStore.getState().root).toBe(CANONICAL_ROOT));
    expect(h.watch).toHaveBeenCalledWith(SOURCE.id);
    expect(h.mark).toHaveBeenCalledWith(SOURCE.id);
    expect(h.watch.mock.invocationCallOrder[0]).toBeLessThan(h.mark.mock.invocationCallOrder[0]!);
  });

  it('surfaces corrupt membership without silently granting the stale session path', async () => {
    h.rejectRestore = true;
    render(<App />);

    await waitFor(() =>
      expect(screen.getByText(/saved library could not be reconciled/i)).toBeTruthy(),
    );
    expect(h.watch).not.toHaveBeenCalled();
    expect(useSourcesStore.getState().root).toBeNull();
  });
});
