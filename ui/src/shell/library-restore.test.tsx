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
  persistedGate: null as Promise<void> | null,
  reconcileGate: null as Promise<void> | null,
  reconcileCalls: 0,
  forgottenSourceIds: [] as string[],
  persistedSessionRoot: '/persisted/vault',
  persistedSettings: {} as Record<string, unknown>,
  watch: vi.fn(async (sourceId: string) => ({
    root:
      sourceId === 'c'.repeat(64)
        ? CANONICAL_ROOT
        : sourceId === 'b'.repeat(64)
          ? '/interactive/vault'
          : '/unexpected',
    generation: sourceId === 'c'.repeat(64) ? 7 : 8,
  })),
  grant: vi.fn(async (sourceId: string) =>
    sourceId === 'c'.repeat(64) ? CANONICAL_ROOT : '/unexpected',
  ),
  mark: vi.fn(async () => ({
    version: 4,
    sources: [],
    cleanup: { kind: 'complete' as const },
  })),
}));

const INTERACTIVE_SOURCE: LibrarySourceDto = {
  ...SOURCE,
  id: 'b'.repeat(64),
  name: 'interactive',
  path: '/interactive/vault',
  restore_candidate: false,
};

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
  const settingsBackend = {
    listSources: vi.fn(async () => ({
      version: 4,
      sources: [SOURCE, INTERACTIVE_SOURCE],
      cleanup: { kind: 'complete' as const },
    })),
    pickAndAddSource: vi.fn(async () => ({ source: INTERACTIVE_SOURCE, added: true })),
    reconcileRestore: vi.fn(async () => null),
    markActive: h.mark,
    forgetSource: vi.fn(async (sourceId: string) => {
      h.forgottenSourceIds.push(sourceId);
      return {
        version: 4,
        sources: [SOURCE, INTERACTIVE_SOURCE].filter((source) => source.id !== sourceId),
        cleanup: { kind: 'complete' as const },
      };
    }),
    diagnosticsInfo: vi.fn(async () => ({
      level: 'standard' as const,
      max_file_bytes: 256 * 1_024,
      max_retained_files: 3,
      max_export_bytes: 512 * 1_024,
    })),
    setDiagnosticLevel: vi.fn(async (level: 'errors' | 'standard' | 'verbose') => ({
      level,
      max_file_bytes: 256 * 1_024,
      max_retained_files: 3,
      max_export_bytes: 512 * 1_024,
    })),
    recordDiagnosticEvent: vi.fn(async () => {}),
    revealDiagnostics: vi.fn(async () => {}),
    exportDiagnostics: vi.fn(async () => ({
      status: 'cancelled' as const,
      bytes_written: 0,
    })),
    clearDiagnostics: vi.fn(async () => ({
      level: 'standard' as const,
      max_file_bytes: 256 * 1_024,
      max_retained_files: 3,
      max_export_bytes: 512 * 1_024,
    })),
  };
  return {
    ...actual,
    settingsBackend,
    reconcileLibraryRestore: vi.fn(async () => {
      h.reconcileCalls += 1;
      if (h.reconcileGate) await h.reconcileGate;
      if (h.rejectRestore) throw new Error('corrupt library');
      return h.restore;
    }),
    pickAndAddLibrarySource: vi.fn(async () => ({
      source: INTERACTIVE_SOURCE,
      added: true,
    })),
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
    loadPersisted: vi.fn(async () => {
      if (h.persistedGate) await h.persistedGate;
      return {
        session: {
        root: h.persistedSessionRoot,
        activeTabId: null,
        tabs: [{ id: 'persisted-tab', docRef: { path: `${ROOT}/old.md`, name: 'old.md' } }],
        recent: [{ root: ROOT, path: `${ROOT}/remembered.md`, name: 'remembered.md' }],
      },
      settings: h.persistedSettings,
      };
    }),
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
import { useToastStore } from './toasts';

beforeEach(() => {
  h.restore = null;
  h.rejectRestore = false;
  h.persistedGate = null;
  h.reconcileGate = null;
  h.reconcileCalls = 0;
  h.forgottenSourceIds.length = 0;
  h.persistedSessionRoot = ROOT;
  h.persistedSettings = {};
  vi.clearAllMocks();
  useEditorStore.getState().reset();
  useSettingsStore.getState().reset();
  useSourcesStore.getState().reset();
  useTabsStore.getState().reset();
  useToastStore.setState({ toasts: [] });
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

  it('never lets a late persisted session override a completed interactive activation', async () => {
    let releasePersisted!: () => void;
    h.persistedGate = new Promise<void>((resolve) => {
      releasePersisted = resolve;
    });
    h.persistedSettings = { theme: 'dark', readingFont: 'sans' };
    render(<App />);

    await act(async () => {
      screen.getByRole('button', { name: /Open folder/i }).click();
    });
    await waitFor(() =>
      expect(useSourcesStore.getState().root).toBe(INTERACTIVE_SOURCE.path),
    );
    expect(h.watch).toHaveBeenCalledWith(INTERACTIVE_SOURCE.id);
    expect(h.mark).toHaveBeenCalledWith(INTERACTIVE_SOURCE.id);
    useTabsStore.getState().openTab(
      { path: `${INTERACTIVE_SOURCE.path}/live.md`, name: 'live.md' },
      { originRoot: INTERACTIVE_SOURCE.path },
    );

    await act(async () => {
      releasePersisted();
      await h.persistedGate;
    });
    await act(async () => {});

    expect(useSourcesStore.getState().root).toBe(INTERACTIVE_SOURCE.path);
    expect(useTabsStore.getState().tabs.map((tab) => tab.docRef.path)).toEqual([
      `${INTERACTIVE_SOURCE.path}/live.md`,
    ]);
    expect(useSettingsStore.getState().theme).toBe('dark');
    expect(useSettingsStore.getState().readingFont).toBe('sans');
    expect(h.watch).not.toHaveBeenCalledWith(SOURCE.id);
    expect(h.mark).not.toHaveBeenCalledWith(SOURCE.id);
  });

  it('suppresses a stale reconciliation error after a newer source activation', async () => {
    let releaseReconcile!: () => void;
    h.restore = SOURCE;
    h.rejectRestore = true;
    h.reconcileGate = new Promise<void>((resolve) => {
      releaseReconcile = resolve;
    });
    render(<App />);
    await waitFor(() => expect(h.reconcileCalls).toBe(1));

    await act(async () => {
      screen.getByTitle('Open folder…').click();
    });
    await waitFor(() =>
      expect(useSourcesStore.getState().root).toBe(INTERACTIVE_SOURCE.path),
    );

    await act(async () => {
      releaseReconcile();
      await h.reconcileGate;
    });
    await act(async () => {});

    expect(useSourcesStore.getState().root).toBe(INTERACTIVE_SOURCE.path);
    expect(screen.queryByText(/saved library could not be reconciled/i)).toBeNull();
    expect(h.watch).not.toHaveBeenCalledWith(SOURCE.id);
  });

  it('does not restore a legacy source after the user has forgotten it during persistence load', async () => {
    let releasePersisted!: () => void;
    h.persistedGate = new Promise<void>((resolve) => {
      releasePersisted = resolve;
    });
    h.restore = SOURCE;
    h.persistedSettings = { theme: 'dark' };
    render(<App />);

    screen.getByRole('button', { name: 'Settings' }).click();
    await screen.findByText(SOURCE.name);
    screen.getByRole('button', { name: `Forget ${SOURCE.name}` }).click();
    await waitFor(() => expect(h.forgottenSourceIds).toEqual([SOURCE.id]));

    await act(async () => {
      releasePersisted();
      await h.persistedGate;
    });
    await act(async () => {});

    expect(h.reconcileCalls).toBe(0);
    expect(h.watch).not.toHaveBeenCalledWith(SOURCE.id);
    expect(useSourcesStore.getState().root).toBeNull();
    expect(useTabsStore.getState().tabs).toEqual([]);
    expect(useSettingsStore.getState().theme).toBe('dark');
  });
});
