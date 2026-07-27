/// <reference types="node" />
/**
 * The reported failure, driven through the REAL AppShell: a persisted session
 * naming a workspace is restored while the embedding model cannot be loaded.
 *
 * The bug was that this presented as an *empty state* — "No folder", "Open a
 * workspace folder…" — as though the user had never chosen anything, with no way
 * to tell a broken model load apart from nothing being open. So these tests
 * assert the two halves that make it honest: the workspace is genuinely OPEN
 * (named, listed, and a document renders), and the failure is NAMED (with its
 * reason, its own label, and a retry that re-attempts the load).
 *
 * Only the IPC boundary (`platform/fs.js`), persistence and the Tauri window are
 * mocked; the shell's restore → grant → list → import wiring is the real one.
 */

import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

const ROOT = '/ws/vault';
const NOTE = '/ws/vault/note.md';
const NOTE_BODY = '# Kept Note\n\nThe workspace is open and this text is on screen.\n';

/** The reason the backend observed — a model-load timeout, carried verbatim. */
const REASON = 'the embedding model could not be loaded — timed out after 120s';

const h = vi.hoisted(() => ({
  /** Every `startImport` call, so a "retry" can be proven to re-attempt it. */
  imports: [] as string[],
}));

vi.mock('../platform/fs.js', async (importOriginal) => {
  const actual = await importOriginal<typeof import('../platform/fs.js')>();
  return {
    ...actual,
    readDoc: vi.fn(async () => ({ content: NOTE_BODY, mtime: 0, hash: 'h0' })),
    saveDoc: vi.fn(async () => ({ status: 'saved' as const, newHash: 'h1', mtime: 0 })),
    onFsChanged: vi.fn(async () => () => {}),
    onWatchError: vi.fn(async () => () => {}),
    watchWorkspace: vi.fn(async (root: string) => ({ root, generation: 1 })),
    setWorkspace: vi.fn(async (root: string) => root),
    // With no model there is no index, so the listing is the live filesystem
    // scan — the same files, read straight off disk. That is exactly why the
    // workspace remains usable, so the test asserts against that shape.
    listWorkspaceFiles: vi.fn(async () => ({
      files: [{ path: NOTE, name: 'note.md', dir: ROOT, mtime: 0, size: 0, ignored: false }],
      fromIndex: false,
    })),
    // The backend's `model-unavailable` status: nothing indexed, and a reason.
    startImport: vi.fn(async (root: string) => {
      h.imports.push(root);
      return {
        phase: 'model-unavailable' as const,
        filesDone: 0,
        filesTotal: 0,
        chunks: 0,
        error: REASON,
      };
    }),
    pickWorkspaceFolder: vi.fn(async () => ROOT),
    resolveWikilink: vi.fn(async () => ({ status: 'unresolved' as const })),
    makeSearchAdapter: vi.fn(() => ({
      search: vi.fn(async () => ({ hits: [], semanticUnavailable: REASON })),
      related: vi.fn(async () => []),
    })),
  };
});

vi.mock('../platform/settings.js', async (importOriginal) => {
  const actual = await importOriginal<typeof import('../platform/settings.js')>();
  const source = {
    id: 'd'.repeat(64),
    name: 'vault',
    path: '/ws/vault',
    state: { kind: 'available' as const },
    index_state: 'not-created' as const,
    active: false,
    restore_candidate: true,
  };
  return {
    ...actual,
    reconcileLibraryRestore: vi.fn(async () => source),
    watchLibrarySource: vi.fn(async () => ({ root: source.path, generation: 1 })),
    grantLibrarySource: vi.fn(async () => source.path),
    markLibrarySourceActive: vi.fn(async () => ({ version: 1, sources: [{ ...source, active: true }] })),
    recordDiagnosticEventNonBlocking: vi.fn(),
  };
});

vi.mock('./persistence.js', async (importOriginal) => {
  const actual = await importOriginal<typeof import('./persistence.js')>();
  return {
    ...actual,
    isTauri: () => true,
    // The seeded session the native harness writes before launch: a workspace
    // root, no open tabs. The restore path is the one under test.
    loadPersisted: vi.fn(async () => ({
      session: { root: ROOT, activeTabId: null, tabs: [], recent: [] },
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
    close: vi.fn(async () => {}),
  }),
}));

import App from '../App';
import { useTabsStore } from '../stores/tabs';
import { useSourcesStore } from '../stores/sources';
import { useSettingsStore } from '../stores/settings';
import { useEditorStore } from '../stores/editor';

beforeEach(() => {
  h.imports.length = 0;
  useTabsStore.getState().reset();
  useSourcesStore.getState().reset();
  useSettingsStore.setState({ sidebarCollapsed: false });
  useEditorStore.getState().reset();
});

afterEach(cleanup);

describe('a workspace whose embedding model cannot be loaded', () => {
  /**
   * Revert-RED: make the shell await the index before adopting the root (e.g.
   * `await runImport(...)` before `sources.setRoot(...)` in `openRoot`, which is
   * what gating the open on the model amounts to) and the workspace never
   * appears: `getByText('vault')` and the `note.md` tree item both fail while the
   * "Open a workspace folder…" placeholder is what is on screen.
   */
  it('still opens: the folder is named, its files are listed, and a document renders', async () => {
    render(<App />);

    // The workspace is OPEN — not the "No folder" empty state.
    await waitFor(() => expect(screen.getByText('vault')).toBeTruthy());
    expect(screen.queryByText('No folder')).toBeNull();

    // Its files are listed. (The tree itself is virtualized and does not lay out
    // in jsdom — the repo's convention — so the listing is read off the status
    // bar's count, which is rendered from the very same store slice the tree is.)
    await waitFor(() => expect(screen.getByText('1 files')).toBeTruthy());
    expect(useSourcesStore.getState().files.map((file) => file.path)).toEqual([NOTE]);

    // …and a document reads. Non-emptiness first: real blocks rendered.
    useTabsStore.getState().openTab({ path: NOTE, name: 'note.md' }, { originRoot: ROOT });
    await waitFor(() =>
      expect(document.querySelectorAll('.reader-host .mdreader-block').length).toBeGreaterThan(0),
    );
    expect(screen.getByText(/this text is on screen/)).toBeTruthy();
    expect(screen.queryByText(/Open a workspace folder/)).toBeNull();
  });

  /**
   * Revert-RED: drop the `model-unavailable` branch from `indexIndicator` and
   * this fails on `getByText('Embedding model unavailable')` — the status falls
   * through to the benign "Local files", which is the original bug exactly.
   */
  it('says what actually happened, distinctly from indexed and from index failure', async () => {
    render(<App />);
    await waitFor(() => expect(screen.getByText('vault')).toBeTruthy());

    const label = await waitFor(() => screen.getByText('Embedding model unavailable'));
    expect(label.closest('span')?.getAttribute('title')).toContain(REASON);

    // None of the labels that would misdescribe it.
    expect(screen.queryByText(/^Indexed/)).toBeNull();
    expect(screen.queryByText('Index failed')).toBeNull();
    expect(screen.queryByText('Local files')).toBeNull();
  });

  /**
   * Revert-RED: render the retry only for `phase === 'error'` (its previous
   * condition) and this fails — there is no way forward from the state the user
   * is actually in.
   */
  it('offers a retry that re-attempts the load, labelled as a model retry', async () => {
    render(<App />);
    await waitFor(() => expect(screen.getByText('Embedding model unavailable')).toBeTruthy());
    await waitFor(() => expect(h.imports).toEqual([ROOT]));

    fireEvent.click(screen.getByRole('button', { name: 'Retry model load' }));

    // A second import for the same root: the backend's model step — the step
    // that failed — is entered again.
    await waitFor(() => expect(h.imports).toEqual([ROOT, ROOT]));
  });
});
