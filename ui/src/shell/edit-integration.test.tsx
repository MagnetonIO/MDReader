/**
 * Integration tests that drive the REAL AppShell editing wiring, mocking only the
 * IPC boundary (`platform/fs.js`), persistence (`isTauri` + the session writer),
 * the Tauri window, and the CodeMirror leaf (`SourcePane` — its rendering is
 * covered by the e2e suite; here we drive edits through the store so the tests
 * run in jsdom). These cover the paths the pure/component tests cannot:
 *   - a watcher fs-changed event on a dirty doc raising the ConflictBanner via the
 *     real handler→readDoc→classify→store→ReaderPane path, and self-write
 *     suppression through the same wiring;
 *   - the Keep-mine → explicit overwrite confirmation → force-flag-reaches-the-
 *     command chain;
 *   - the unsaved-changes guard on close and on quit.
 */

import { act, cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

const h = vi.hoisted(() => ({
  disk: new Map<string, { content: string; hash: string }>(),
  saveCalls: [] as { path: string; content: string; expectedHash: string; force: boolean }[],
  fsChanged: null as null | ((change: { path: string; kind: string; generation: number }) => void),
  onClose: null as null | ((event: { preventDefault: () => void }) => void | Promise<void>),
  destroy: vi.fn(async () => {}),
  drain: vi.fn(async () => {}),
  seq: 0,
}));

vi.mock('../platform/fs.js', async (importOriginal) => {
  const actual = await importOriginal<typeof import('../platform/fs.js')>();
  return {
    ...actual,
    readDoc: vi.fn(async (path: string) => {
      const entry = h.disk.get(path) ?? { content: '# missing\n', hash: 'h-missing' };
      return { content: entry.content, mtime: 0, hash: entry.hash };
    }),
    saveDoc: vi.fn(async (path: string, content: string, expectedHash: string, opts: { force?: boolean } = {}) => {
      const force = opts.force ?? false;
      h.saveCalls.push({ path, content, expectedHash, force });
      const current = h.disk.get(path);
      if (!force && current && expectedHash !== current.hash) {
        return { status: 'conflict' as const, currentHash: current.hash };
      }
      const newHash = `h-saved-${(h.seq += 1)}`;
      h.disk.set(path, { content, hash: newHash });
      return { status: 'saved' as const, newHash, mtime: 0 };
    }),
    onFsChanged: vi.fn(async (handler: (c: { path: string; kind: string; generation: number }) => void) => {
      h.fsChanged = handler;
      return () => {};
    }),
    onWatchError: vi.fn(async () => () => {}),
    watchWorkspace: vi.fn(async (root: string) => ({ root, generation: 1 })),
    setWorkspace: vi.fn(async (root: string) => root),
    listWorkspaceFiles: vi.fn(async () => ({ files: [], fromIndex: true })),
    startImport: vi.fn(async () => ({ phase: 'done', filesDone: 0, filesTotal: 0, chunks: 0, error: null })),
    pickWorkspaceFolder: vi.fn(async () => '/ws'),
    resolveWikilink: vi.fn(async () => ({ status: 'unresolved' as const })),
    makeSearchAdapter: vi.fn(() => ({ search: vi.fn(async () => []), related: vi.fn(async () => []) })),
  };
});

vi.mock('./persistence.js', async (importOriginal) => {
  const actual = await importOriginal<typeof import('./persistence.js')>();
  return {
    ...actual,
    isTauri: () => true,
    loadPersisted: vi.fn(async () => ({ session: null, settings: {} })),
    saveSettings: vi.fn(async () => {}),
    sessionWriter: { write: vi.fn(), drain: h.drain },
  };
});

vi.mock('@tauri-apps/api/window', () => ({
  getCurrentWindow: () => ({
    onCloseRequested: vi.fn(async (handler: (e: { preventDefault: () => void }) => void) => {
      h.onClose = handler;
      return () => {};
    }),
    destroy: h.destroy,
  }),
}));

// The CodeMirror leaf — rendering is an e2e concern; here it is an inert div so
// the editing WIRING is what's under test. Edits are driven through the store.
vi.mock('./SourcePane.js', () => ({
  SourcePane: ({ value }: { value: string }) => <div data-testid="source-pane">{value}</div>,
}));

import App from '../App';
import { detectMac } from '../keymap/registry';
import { useTabsStore } from '../stores/tabs';
import { useSourcesStore } from '../stores/sources';
import { useSettingsStore } from '../stores/settings';
import { useEditorStore } from '../stores/editor';

const mac = detectMac();
function modKeyDown(key: string, extra: Partial<KeyboardEventInit> = {}) {
  fireEvent.keyDown(window, { key, metaKey: mac, ctrlKey: !mac, ...extra });
}

/** Open the workspace (through the real openRoot, so the watcher generation is
 *  armed to 1), then open a source-mode dirty tab for `/ws/note.md`. */
async function openDirtyDoc() {
  h.disk.set('/ws/note.md', { content: 'hello', hash: 'h0' });

  render(<App />);
  await act(async () => {}); // settle hydration

  // Real openRoot: pick → switchWorkspace → watchWorkspace(generation 1).
  await act(async () => {
    fireEvent.click(screen.getByRole('button', { name: /Open folder/i }));
  });
  await waitFor(() => expect(useSourcesStore.getState().root).toBe('/ws'));

  // Open the doc's tab (via the store; the tree is virtualized) and enter source.
  act(() => {
    useTabsStore.getState().openTab({ path: '/ws/note.md', name: 'note.md' }, { originRoot: '/ws' });
  });
  const tabId = useTabsStore.getState().activeTabId!;
  await act(async () => {
    fireEvent.click(screen.getByRole('button', { name: 'Source view' }));
  });
  await waitFor(() => expect(useEditorStore.getState().entry(tabId)?.mode).toBe('source'));

  // An edit makes it dirty (driven through the store — SourcePane is inert here).
  act(() => useEditorStore.getState().edit(tabId, 'hello + my local edit'));
  expect(useEditorStore.getState().isDirty(tabId)).toBe(true);
  return tabId;
}

beforeEach(() => {
  h.disk.clear();
  h.saveCalls.length = 0;
  h.fsChanged = null;
  h.onClose = null;
  h.seq = 0;
  vi.clearAllMocks();
});

afterEach(() => {
  cleanup();
  useTabsStore.getState().reset();
  useSourcesStore.getState().reset();
  useSettingsStore.getState().reset();
  useEditorStore.getState().reset();
});

describe('conflict wiring (fs-changed → ConflictBanner) and self-write suppression', () => {
  it('an external change to a DIRTY doc raises the conflict banner via the real path', async () => {
    await openDirtyDoc();

    // The file changes on disk out from under us (a NEW hash).
    h.disk.set('/ws/note.md', { content: 'changed by someone else', hash: 'h-external' });

    // Emit the watcher event the backend would send (generation 1, matching the arm).
    act(() => h.fsChanged!({ path: '/ws/note.md', kind: 'any', generation: 1 }));

    // The real handler read the disk hash, classified it as a conflict (dirty +
    // divergent), and the store→ReaderPane path surfaced the banner.
    await waitFor(() =>
      expect(screen.getByRole('alert').textContent).toMatch(/changed on disk while you were editing/i),
    );
  });

  it('the watcher echo of our OWN save is suppressed (no banner, no toast)', async () => {
    const tabId = await openDirtyDoc();

    // Save: the mock writes and hands back a fresh hash the buffer re-anchors to.
    await act(async () => {
      modKeyDown('s');
    });
    await waitFor(() => expect(useEditorStore.getState().isDirty(tabId)).toBe(false));
    const savedHash = useEditorStore.getState().entry(tabId)!.baseHash;
    expect(h.disk.get('/ws/note.md')!.hash).toBe(savedHash);

    // The watcher now echoes OUR write. Disk hash === our anchor → classified as
    // self → no conflict banner and no "Updated" toast.
    act(() => h.fsChanged!({ path: '/ws/note.md', kind: 'any', generation: 1 }));
    await act(async () => {});
    expect(screen.queryByRole('alert')).toBeNull();
    expect(screen.queryByText('Updated')).toBeNull();
  });
});

describe('force-confirmation chain (Keep mine → confirm → force reaches the command)', () => {
  it('Keep-mine requires an explicit second confirmation before the force save', async () => {
    await openDirtyDoc();

    // An external change makes our guarded save conflict.
    h.disk.set('/ws/note.md', { content: 'changed elsewhere', hash: 'h-external' });
    await act(async () => {
      modKeyDown('s');
    });
    await waitFor(() =>
      expect(screen.getByRole('alert').textContent).toMatch(/changed on disk since you opened it/i),
    );
    // The guarded save was attempted, and it was NOT forced.
    expect(h.saveCalls.at(-1)).toMatchObject({ force: false });
    const callsBefore = h.saveCalls.length;

    // Keep mine → advances to the explicit overwrite confirmation. It must NOT
    // save yet (miswiring Keep-mine straight to force would trip this).
    fireEvent.click(screen.getByRole('button', { name: 'Keep mine' }));
    await waitFor(() =>
      expect(screen.getByRole('alert').textContent).toMatch(/overwrite the on-disk changes/i),
    );
    expect(h.saveCalls.length).toBe(callsBefore);

    // Overwrite → the force flag reaches the command.
    await act(async () => {
      fireEvent.click(screen.getByRole('button', { name: 'Overwrite' }));
    });
    await waitFor(() => expect(h.saveCalls.at(-1)?.force).toBe(true));
    // And the forced write landed.
    expect(h.disk.get('/ws/note.md')!.content).toBe('hello + my local edit');
  });
});

describe('unsaved-changes guard (close tab / quit)', () => {
  it('closing a dirty tab prompts; Discard drops the buffer and closes', async () => {
    const tabId = await openDirtyDoc();

    modKeyDown('w'); // ⌘W close-tab
    const dialog = await screen.findByRole('alertdialog', { name: /unsaved changes/i });
    expect(dialog.textContent).toMatch(/unsaved changes/i);

    fireEvent.click(screen.getByRole('button', { name: 'Discard' }));
    await waitFor(() => expect(useTabsStore.getState().tabs.some((t) => t.id === tabId)).toBe(false));
    // The buffer is gone too.
    expect(useEditorStore.getState().entry(tabId)).toBeUndefined();
  });

  it('closing a dirty tab and choosing Cancel keeps it open with edits intact', async () => {
    const tabId = await openDirtyDoc();

    modKeyDown('w');
    await screen.findByRole('alertdialog', { name: /unsaved changes/i });
    fireEvent.click(screen.getByRole('button', { name: 'Cancel' }));

    await waitFor(() => expect(screen.queryByRole('alertdialog')).toBeNull());
    expect(useTabsStore.getState().tabs.some((t) => t.id === tabId)).toBe(true);
    expect(useEditorStore.getState().isDirty(tabId)).toBe(true);
  });

  it('quitting with a dirty tab prompts; Discard performs the real quit (destroy)', async () => {
    await openDirtyDoc();
    await waitFor(() => expect(h.onClose).not.toBeNull());

    // The Tauri window close-requested fires with unsaved edits present.
    await act(async () => {
      await h.onClose!({ preventDefault: () => {} });
    });
    await screen.findByRole('alertdialog', { name: /unsaved changes/i });
    expect(h.destroy).not.toHaveBeenCalled(); // did NOT quit yet

    // Discard → the actual quit runs.
    await act(async () => {
      fireEvent.click(screen.getByRole('button', { name: 'Discard' }));
    });
    await waitFor(() => expect(h.destroy).toHaveBeenCalledTimes(1));
  });
});
