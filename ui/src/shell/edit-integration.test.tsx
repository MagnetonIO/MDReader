/// <reference types="node" />
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
 *   - the TRANSACTIONAL unsaved-changes guard: Save-then-Cancel and typing during
 *     an in-flight save both leave the tab open; the dirty-quit hook preventDefaults.
 */

import { act, cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

function deferred<T = void>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((r) => {
    resolve = r;
  });
  return { promise, resolve };
}

const h = vi.hoisted(() => ({
  disk: new Map<string, { content: string; hash: string }>(),
  saveCalls: [] as { path: string; content: string; expectedHash: string; force: boolean }[],
  fsChanged: null as null | ((change: { path: string; kind: string; generation: number }) => void),
  watchError: null as null | ((error: { message: string; generation: number }) => void),
  diagnosticEvents: [] as unknown[],
  onClose: null as null | ((event: { preventDefault: () => void }) => void | Promise<void>),
  // --- FORCE-CLOSE PRIMITIVE -------------------------------------------------
  // The guard force-closes with destroy() (NOT close()), so destroy() is what
  // marks the window "closed" here.
  //
  // HONESTY BOUNDARY — read before trusting these close/quit tests: this mock
  // exercises the JS control flow ONLY (preventDefault → bounded drain →
  // dirty-guard → destroy). It does NOT reproduce real Tauri window semantics or
  // the capability ACL. A window mock CANNOT catch a missing/incorrect
  // `core:window:allow-destroy` capability, and it cannot catch the
  // close()-vs-destroy() distinction — a vetoed window ignores a fresh close()
  // in the real webview but tears down on destroy(). That distinction is EXACTLY
  // the bug that shipped past these very tests. The macOS GUI smoke
  // (apps/mdreader/e2e-native, milestone PT) is the ONLY thing that validates the
  // red X actually ends the process.
  destroy: vi.fn(async () => {
    h.closed = true;
  }),
  // The guard no longer calls close(); kept as a harmless no-op so the mocked
  // window matches Tauri's shape (and to prove nothing invokes it).
  close: vi.fn(async () => {}),
  closed: false,
  // When `drainGate` is set, drain() returns ITS (caller-controlled) promise so a
  // test can hold the drain pending / never-resolving; otherwise it resolves now.
  drainGate: null as null | { promise: Promise<void> },
  drain: vi.fn((): Promise<void> => (h.drainGate ? h.drainGate.promise : Promise.resolve())),
  // When set, saves block until it resolves (to model an in-flight save).
  gate: null as null | { promise: Promise<void>; resolve: () => void },
  listGate: null as null | { promise: Promise<void>; resolve: () => void },
  watchResponseGate: null as null | { promise: Promise<void>; resolve: () => void },
  gatedWatchSourceId: null as string | null,
  failedWatchSourceId: null as string | null,
  backendRoot: null as string | null,
  backendGeneration: 0,
  searchRoots: [] as (string | null)[],
  seq: 0,
  pickedPath: '/ws',
  watch: vi.fn(async (root: string) => ({ root, generation: 1 })),
  libraryWatch: vi.fn(async (sourceId: string) => {
    if (h.failedWatchSourceId === sourceId) throw new Error('watch refused');
    const root =
      sourceId === 'a'.repeat(64)
        ? '/ws'
        : sourceId === 'b'.repeat(64)
          ? '/ws-b'
          : '/ws-c';
    const generation =
      sourceId === 'a'.repeat(64) ? 1 : sourceId === 'b'.repeat(64) ? 2 : 3;
    // Model the native command's commit-before-response boundary: the backend
    // epoch is already live even when delivery of the IPC result is delayed.
    h.backendRoot = root;
    h.backendGeneration = generation;
    if (h.gatedWatchSourceId === sourceId && h.watchResponseGate) {
      await h.watchResponseGate.promise;
    }
    return { root, generation };
  }),
}));

vi.mock('../platform/fs.js', async (importOriginal) => {
  const actual = await importOriginal<typeof import('../platform/fs.js')>();
  return {
    ...actual,
    readDoc: vi.fn(async (path: string) => {
      const entry = h.disk.get(path) ?? { content: '# missing\n', hash: 'h-missing' };
      return { content: entry.content, mtime: 0, hash: entry.hash };
    }),
    saveDoc: vi.fn(
      async (path: string, content: string, expectedHash: string, opts: { force?: boolean } = {}) => {
        const force = opts.force ?? false;
        h.saveCalls.push({ path, content, expectedHash, force });
        if (h.gate) await h.gate.promise; // hold the save in flight
        const current = h.disk.get(path);
        if (!force && current && expectedHash !== current.hash) {
          return { status: 'conflict' as const, currentHash: current.hash };
        }
        const newHash = `h-saved-${(h.seq += 1)}`;
        h.disk.set(path, { content, hash: newHash });
        return { status: 'saved' as const, newHash, mtime: 0 };
      },
    ),
    onFsChanged: vi.fn(async (handler: (c: { path: string; kind: string; generation: number }) => void) => {
      h.fsChanged = handler;
      return () => {};
    }),
    onWatchError: vi.fn(
      async (handler: (error: { message: string; generation: number }) => void) => {
        h.watchError = handler;
        return () => {};
      },
    ),
    watchWorkspace: h.watch,
    setWorkspace: vi.fn(async (root: string) => root),
    listWorkspaceFiles: vi.fn(async () => {
      if (h.listGate) await h.listGate.promise;
      return { files: [], fromIndex: true };
    }),
    startImport: vi.fn(async () => ({ phase: 'done', filesDone: 0, filesTotal: 0, chunks: 0, error: null })),
    pickWorkspaceFolder: vi.fn(async () => '/ws'),
    resolveWikilink: vi.fn(async () => ({ status: 'unresolved' as const })),
    makeSearchAdapter: vi.fn(() => ({
      search: vi.fn(async () => {
        h.searchRoots.push(h.backendRoot);
        return [];
      }),
      related: vi.fn(async () => []),
    })),
  };
});

vi.mock('../platform/settings.js', async (importOriginal) => {
  const actual = await importOriginal<typeof import('../platform/settings.js')>();
  const source = (path: string, active: boolean) => ({
    id: (path === '/ws' ? 'a' : path === '/ws-b' ? 'b' : 'c').repeat(64),
    name:
      path === '/ws' ? 'Workspace' : path === '/ws-b' ? 'Second workspace' : 'Third workspace',
    path,
    state: { kind: 'available' as const },
    index_state: 'present' as const,
    active,
    restore_candidate: active,
  });
  const listSources = vi.fn(async () => ({
    version: 1,
    sources: [source('/ws', true), source('/ws-b', false), source('/ws-c', false)],
  }));
  const pickAndAddSource = vi.fn(async () => ({
    source: source(h.pickedPath, h.pickedPath === '/ws'),
    added: true,
  }));
  const markActive = vi.fn(async () => ({
    version: 1,
    sources: [
      source('/ws', h.pickedPath === '/ws'),
      source('/ws-b', h.pickedPath === '/ws-b'),
      source('/ws-c', h.pickedPath === '/ws-c'),
    ],
  }));
  const settingsBackend = {
    listSources,
    pickAndAddSource,
    reconcileRestore: vi.fn(async () => null),
    markActive,
    forgetSource: vi.fn(async () => ({ version: 1, sources: [] })),
    diagnosticsInfo: vi.fn(async () => ({
      level: 'standard' as const,
      directory: '/local/logs',
      max_file_bytes: 256 * 1024,
      retained_files: 3,
      max_export_bytes: 512 * 1024,
    })),
    setDiagnosticLevel: vi.fn(async (level: 'errors' | 'standard' | 'verbose') => ({
      level,
      directory: '/local/logs',
      max_file_bytes: 256 * 1024,
      retained_files: 3,
      max_export_bytes: 512 * 1024,
    })),
    recordDiagnosticEvent: vi.fn(async () => {}),
    revealDiagnostics: vi.fn(async () => {}),
    exportDiagnostics: vi.fn(async () => ({
      status: 'cancelled' as const,
      path: null,
      bytes_written: 0,
    })),
    clearDiagnostics: vi.fn(async () => ({
      level: 'standard' as const,
      directory: '/local/logs',
      max_file_bytes: 256 * 1024,
      retained_files: 3,
      max_export_bytes: 512 * 1024,
    })),
  };
  return {
    ...actual,
    settingsBackend,
    pickAndAddLibrarySource: pickAndAddSource,
    watchLibrarySource: h.libraryWatch,
    grantLibrarySource: vi.fn(async (sourceId: string) =>
      sourceId === 'a'.repeat(64)
        ? '/ws'
        : sourceId === 'b'.repeat(64)
          ? '/ws-b'
          : '/ws-c',
    ),
    markLibrarySourceActive: markActive,
    reconcileLibraryRestore: settingsBackend.reconcileRestore,
    recordDiagnosticEventNonBlocking: vi.fn((event: unknown) => {
      h.diagnosticEvents.push(event);
    }),
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
    close: h.close,
  }),
}));

// The CodeMirror leaf — rendering is an e2e concern; here it is an inert div so
// the editing WIRING is what's under test. Edits are driven through the store.
vi.mock('./SourcePane.js', () => ({
  SourcePane: ({ value }: { value: string }) => <div data-testid="source-pane">{value}</div>,
}));

import { readFileSync } from 'node:fs';

import App from '../App';
import { detectMac } from '../keymap/registry';
import { useTabsStore } from '../stores/tabs';
import { useSourcesStore } from '../stores/sources';
import { useSettingsStore } from '../stores/settings';
import { useEditorStore } from '../stores/editor';
import { saveSettings } from './persistence.js';

const NOTE = '/ws/note.md';
const mac = detectMac();
function modKeyDown(key: string, extra: Partial<KeyboardEventInit> = {}) {
  fireEvent.keyDown(window, { key, metaKey: mac, ctrlKey: !mac, ...extra });
}

/** Open the workspace (through the real openRoot, so the watcher generation is
 *  armed to 1), then open a source-mode dirty tab for `/ws/note.md`. */
async function openDirtyDoc() {
  h.disk.set(NOTE, { content: 'hello', hash: 'h0' });

  render(<App />);
  await act(async () => {}); // settle hydration

  await act(async () => {
    fireEvent.click(screen.getByRole('button', { name: /Open folder/i }));
  });
  await waitFor(() => expect(useSourcesStore.getState().root).toBe('/ws'));

  act(() => {
    useTabsStore.getState().openTab({ path: NOTE, name: 'note.md' }, { originRoot: '/ws' });
  });
  const tabId = useTabsStore.getState().activeTabId!;
  await act(async () => {
    fireEvent.click(screen.getByRole('button', { name: 'Source view' }));
  });
  await waitFor(() => expect(useEditorStore.getState().entry(tabId, NOTE)?.mode).toBe('source'));

  act(() => useEditorStore.getState().edit(tabId, NOTE, 'hello + my local edit'));
  expect(useEditorStore.getState().isDirty(tabId, NOTE)).toBe(true);
  return tabId;
}

beforeEach(() => {
  vi.stubGlobal(
    'ResizeObserver',
    class {
      observe() {}
      unobserve() {}
      disconnect() {}
    },
  );
  Element.prototype.scrollIntoView = vi.fn();
  h.disk.clear();
  h.saveCalls.length = 0;
  h.fsChanged = null;
  h.watchError = null;
  h.diagnosticEvents.length = 0;
  h.onClose = null;
  h.gate = null;
  h.listGate = null;
  h.watchResponseGate = null;
  h.gatedWatchSourceId = null;
  h.failedWatchSourceId = null;
  h.backendRoot = null;
  h.backendGeneration = 0;
  h.searchRoots.length = 0;
  h.closed = false;
  h.drainGate = null;
  h.seq = 0;
  h.pickedPath = '/ws';
  vi.clearAllMocks();
});

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
  useTabsStore.getState().reset();
  useSourcesStore.getState().reset();
  useSettingsStore.getState().reset();
  useEditorStore.getState().reset();
});

describe('conflict wiring (fs-changed → ConflictBanner) and self-write suppression', () => {
  it('an external change to a DIRTY doc raises the conflict banner via the real path', async () => {
    await openDirtyDoc();
    h.disk.set(NOTE, { content: 'changed by someone else', hash: 'h-external' });
    act(() => h.fsChanged!({ path: NOTE, kind: 'any', generation: 1 }));
    await waitFor(() =>
      expect(screen.getByRole('alert').textContent).toMatch(/changed on disk while you were editing/i),
    );
  });

  it('the watcher echo of our OWN save is suppressed (no banner, no toast)', async () => {
    const tabId = await openDirtyDoc();
    await act(async () => {
      modKeyDown('s');
    });
    await waitFor(() => expect(useEditorStore.getState().isDirty(tabId, NOTE)).toBe(false));
    const savedHash = useEditorStore.getState().entry(tabId, NOTE)!.baseHash;
    expect(h.disk.get(NOTE)!.hash).toBe(savedHash);

    act(() => h.fsChanged!({ path: NOTE, kind: 'any', generation: 1 }));
    await act(async () => {});
    expect(screen.queryByRole('alert')).toBeNull();
    expect(screen.queryByText('Updated')).toBeNull();
  });
});

describe('library source identity follows the committed backend epoch', () => {
  it('attributes a new watcher failure to the new source while its listing is still pending', async () => {
    render(<App />);
    await act(async () => {});
    await act(async () => {
      fireEvent.click(screen.getByRole('button', { name: /Open folder/i }));
    });
    await waitFor(() =>
      expect(h.diagnosticEvents).toContainEqual({
        kind: 'workspace',
        source_id: 'a'.repeat(64),
        active: true,
      }),
    );

    h.diagnosticEvents.length = 0;
    h.pickedPath = '/ws-b';
    h.listGate = deferred();
    fireEvent.click(screen.getByRole('button', { name: 'Settings' }));
    await screen.findByText('Second workspace');
    fireEvent.click(screen.getByRole('button', { name: 'Activate Second workspace' }));
    await waitFor(() => expect(useSourcesStore.getState().root).toBe('/ws-b'));

    act(() => {
      h.watchError?.({ message: 'watch failed', generation: 2 });
    });
    expect(h.diagnosticEvents.at(-1)).toEqual({
      kind: 'watcher',
      source_id: 'b'.repeat(64),
      state: 'error',
      error: 'watcher-unavailable',
    });

    await act(async () => {
      h.listGate!.resolve();
      await Promise.resolve();
    });
  });

  it('serializes a committed A response ahead of a newer failing B request', async () => {
    render(<App />);
    await act(async () => {});
    await act(async () => {
      fireEvent.click(screen.getByRole('button', { name: /Open folder/i }));
    });
    await waitFor(() => expect(useSourcesStore.getState().root).toBe('/ws'));
    expect(h.backendRoot).toBe('/ws');
    expect(h.backendGeneration).toBe(1);

    const delayedA = deferred();
    h.gatedWatchSourceId = 'b'.repeat(64);
    h.watchResponseGate = delayedA;
    h.pickedPath = '/ws-b';
    fireEvent.click(screen.getByTitle('Open folder…'));
    await waitFor(() => expect(h.backendRoot).toBe('/ws-b'));
    expect(useSourcesStore.getState().root).toBe('/ws');

    h.failedWatchSourceId = 'c'.repeat(64);
    h.pickedPath = '/ws-c';
    fireEvent.click(screen.getByTitle('Open folder…'));

    await act(async () => {
      delayedA.resolve();
      await Promise.resolve();
    });
    await waitFor(() => expect(h.libraryWatch).toHaveBeenCalledWith('c'.repeat(64)));
    await waitFor(() => expect(useSourcesStore.getState().root).toBe('/ws-b'));

    expect(h.backendRoot).toBe('/ws-b');
    expect(h.backendGeneration).toBe(2);
    expect(useSourcesStore.getState().watcher).toBe('watching');
    expect(h.diagnosticEvents).toContainEqual({
      kind: 'workspace',
      source_id: 'b'.repeat(64),
      active: true,
    });
    expect(h.diagnosticEvents).not.toContainEqual(
      expect.objectContaining({ source_id: 'c'.repeat(64) }),
    );

    act(() => {
      h.watchError?.({ message: 'current watcher failed', generation: 2 });
    });
    expect(useSourcesStore.getState().watcher).toBe('error');
    expect(h.diagnosticEvents.at(-1)).toEqual({
      kind: 'watcher',
      source_id: 'b'.repeat(64),
      state: 'error',
      error: 'watcher-unavailable',
    });

    act(() => modKeyDown('k'));
    fireEvent.change(screen.getByRole('combobox'), { target: { value: 'authoritative' } });
    await waitFor(() => expect(h.searchRoots.at(-1)).toBe('/ws-b'));
    expect(useSourcesStore.getState().root).toBe(h.searchRoots.at(-1));
  });
});

describe('force-confirmation chain (Keep mine → confirm → force reaches the command)', () => {
  it('Keep-mine requires an explicit second confirmation before the force save', async () => {
    await openDirtyDoc();
    h.disk.set(NOTE, { content: 'changed elsewhere', hash: 'h-external' });
    await act(async () => {
      modKeyDown('s');
    });
    await waitFor(() =>
      expect(screen.getByRole('alert').textContent).toMatch(/changed on disk since you opened it/i),
    );
    expect(h.saveCalls.at(-1)).toMatchObject({ force: false });
    const callsBefore = h.saveCalls.length;

    fireEvent.click(screen.getByRole('button', { name: 'Keep mine' }));
    await waitFor(() =>
      expect(screen.getByRole('alert').textContent).toMatch(/overwrite the on-disk changes/i),
    );
    expect(h.saveCalls.length).toBe(callsBefore); // Keep-mine alone must NOT save

    await act(async () => {
      fireEvent.click(screen.getByRole('button', { name: 'Overwrite' }));
    });
    await waitFor(() => expect(h.saveCalls.at(-1)?.force).toBe(true));
    expect(h.disk.get(NOTE)!.content).toBe('hello + my local edit');
  });
});

describe('memory: navigating away from a clean doc evicts its buffer (real wiring)', () => {
  it('a clean, navigated-away source buffer is evicted by the AppShell eviction effect', async () => {
    // Open note.md in source mode but DO NOT edit it — a clean entry.
    h.disk.set(NOTE, { content: 'hello', hash: 'h0' });
    render(<App />);
    await act(async () => {});
    await act(async () => {
      fireEvent.click(screen.getByRole('button', { name: /Open folder/i }));
    });
    await waitFor(() => expect(useSourcesStore.getState().root).toBe('/ws'));
    act(() => {
      useTabsStore.getState().openTab({ path: NOTE, name: 'note.md' }, { originRoot: '/ws' });
    });
    const tabId = useTabsStore.getState().activeTabId!;
    await act(async () => {
      fireEvent.click(screen.getByRole('button', { name: 'Source view' }));
    });
    await waitFor(() => expect(useEditorStore.getState().entry(tabId, NOTE)?.mode).toBe('source'));
    expect(useEditorStore.getState().isDirty(tabId, NOTE)).toBe(false);

    // Navigate the SAME tab to a different doc through the real store action; the
    // AppShell eviction effect (keyed on `tabs`) must drop the clean note.md buffer.
    h.disk.set('/ws/other.md', { content: 'other', hash: 'hO' });
    await act(async () => {
      useTabsStore.getState().navigateInTab(tabId, { path: '/ws/other.md', name: 'other.md' });
    });

    await waitFor(() => expect(useEditorStore.getState().entry(tabId, NOTE)).toBeUndefined());
  });
});

describe('unsaved-changes guard is transactional (close / quit)', () => {
  it('routes a Settings library activation through the same unsaved-switch guard', async () => {
    await openDirtyDoc();
    h.watch.mockClear();

    fireEvent.click(screen.getByRole('button', { name: 'Settings' }));
    await screen.findByText('Second workspace');
    fireEvent.click(screen.getByRole('button', { name: 'Activate Second workspace' }));

    await screen.findByRole('alertdialog', { name: /unsaved changes/i });
    expect(h.watch).not.toHaveBeenCalledWith('/ws-b');
    expect(useSourcesStore.getState().root).toBe('/ws');

    fireEvent.click(screen.getByRole('button', { name: 'Discard' }));
    await waitFor(() => expect(h.libraryWatch).toHaveBeenCalledWith('b'.repeat(64)));
    expect(useSourcesStore.getState().root).toBe('/ws-b');
  });

  it('closing a dirty tab prompts; Discard drops the buffer and closes', async () => {
    const tabId = await openDirtyDoc();
    modKeyDown('w');
    await screen.findByRole('alertdialog', { name: /unsaved changes/i });
    fireEvent.click(screen.getByRole('button', { name: 'Discard' }));
    await waitFor(() => expect(useTabsStore.getState().tabs.some((t) => t.id === tabId)).toBe(false));
    expect(useEditorStore.getState().entry(tabId, NOTE)).toBeUndefined();
  });

  it('the discard prompt LISTS every dirty document in the tab (not just the displayed one)', async () => {
    const tabId = await openDirtyDoc(); // note.md is dirty AND displayed
    // Two MORE dirty docs the tab has edited but is not currently showing.
    act(() => {
      useEditorStore.getState().seed(tabId, '/ws/alpha.md', 'a', 'ha', 'source');
      useEditorStore.getState().edit(tabId, '/ws/alpha.md', 'a edited');
      useEditorStore.getState().seed(tabId, '/ws/beta.md', 'b', 'hb', 'source');
      useEditorStore.getState().edit(tabId, '/ws/beta.md', 'b edited');
    });

    modKeyDown('w');
    const dialog = await screen.findByRole('alertdialog', { name: /unsaved changes/i });
    // The prompt must name ALL three — a Discard would lose the two hidden ones too.
    expect(dialog.textContent).toMatch(/3 documents have unsaved changes/i);
    expect(dialog.textContent).toContain('note.md');
    expect(dialog.textContent).toContain('alpha.md');
    expect(dialog.textContent).toContain('beta.md');
  });

  it('Save-then-Cancel during the in-flight save leaves the tab OPEN', async () => {
    const tabId = await openDirtyDoc();
    h.gate = deferred(); // hold the save in flight

    modKeyDown('w');
    await screen.findByRole('alertdialog', { name: /unsaved changes/i });
    // Click Save — its save is now held; the modal stays up during the await.
    await act(async () => {
      fireEvent.click(screen.getByRole('button', { name: 'Save' }));
    });
    // Cancel while the save is still in flight → the close must be aborted.
    fireEvent.click(screen.getByRole('button', { name: 'Cancel' }));
    // Release the (now-moot) save.
    await act(async () => {
      h.gate!.resolve();
      await Promise.resolve();
    });

    expect(useTabsStore.getState().tabs.some((t) => t.id === tabId)).toBe(true);
  });

  it('typing during the in-flight save keeps the tab open (still dirty)', async () => {
    const tabId = await openDirtyDoc();
    h.gate = deferred();

    modKeyDown('w');
    await screen.findByRole('alertdialog', { name: /unsaved changes/i });
    await act(async () => {
      fireEvent.click(screen.getByRole('button', { name: 'Save' }));
    });
    // Type MORE while the save is in flight — newer than the saved snapshot.
    act(() => useEditorStore.getState().edit(tabId, NOTE, 'even newer content'));
    await act(async () => {
      h.gate!.resolve();
      await Promise.resolve();
    });
    await waitFor(() => expect(screen.queryByRole('alertdialog')).toBeNull());

    expect(useTabsStore.getState().tabs.some((t) => t.id === tabId)).toBe(true);
    expect(useEditorStore.getState().isDirty(tabId, NOTE)).toBe(true);
  });

  it('clean red-X: BOUNDED-drains the session, then force-closes via destroy()', async () => {
    render(<App />);
    await act(async () => {});
    await waitFor(() => expect(h.onClose).not.toBeNull());

    // The OS close request: the handler holds it (preventDefault) to run a bounded
    // session drain, then force-closes PAST its own veto with destroy().
    const event = { preventDefault: vi.fn() };
    await act(async () => {
      await h.onClose!(event);
    });
    expect(event.preventDefault).toHaveBeenCalledTimes(1); // held to drain
    expect(h.drain).toHaveBeenCalled(); // persistence contract: drained
    expect(h.destroy).toHaveBeenCalled(); // force-closed via destroy()
    expect(h.closed).toBe(true); // …which tore the window down
    expect(h.close).not.toHaveBeenCalled(); // never the vetoed-window close()
  });

  it('dirty red-X → Save → drains, clears dirty, then force-closes via destroy()', async () => {
    const tabId = await openDirtyDoc();
    await waitFor(() => expect(h.onClose).not.toBeNull());

    const event = { preventDefault: vi.fn() };
    await act(async () => {
      await h.onClose!(event);
    });
    expect(event.preventDefault).toHaveBeenCalledTimes(1);
    expect(h.drain).toHaveBeenCalled();
    await screen.findByRole('alertdialog', { name: /unsaved changes/i });
    expect(h.closed).toBe(false); // not closed yet — waiting on the prompt
    expect(h.destroy).not.toHaveBeenCalled(); // no force-close before the guard resolves

    await act(async () => {
      fireEvent.click(screen.getByRole('button', { name: 'Save' }));
    });
    await waitFor(() => expect(h.closed).toBe(true)); // saved, cleared, force-closed
    expect(h.saveCalls.at(-1)).toMatchObject({ force: false });
    expect(useEditorStore.getState().isDirty(tabId, NOTE)).toBe(false);
    expect(h.destroy).toHaveBeenCalled(); // performQuit force-closes via destroy()
  });

  it('dirty red-X → Discard → clears dirty and force-closes via destroy()', async () => {
    const tabId = await openDirtyDoc();
    await waitFor(() => expect(h.onClose).not.toBeNull());
    await act(async () => {
      await h.onClose!({ preventDefault: vi.fn() });
    });
    await screen.findByRole('alertdialog', { name: /unsaved changes/i });

    await act(async () => {
      fireEvent.click(screen.getByRole('button', { name: 'Discard' }));
    });
    await waitFor(() => expect(h.closed).toBe(true));
    expect(h.destroy).toHaveBeenCalled(); // performQuit force-closes via destroy()
    expect(useEditorStore.getState().isDirty(tabId, NOTE)).toBe(false);
  });

  it('dirty red-X → Cancel → stays open and remains closeable (closing not latched)', async () => {
    await openDirtyDoc();
    await waitFor(() => expect(h.onClose).not.toBeNull());
    await act(async () => {
      await h.onClose!({ preventDefault: vi.fn() });
    });
    await screen.findByRole('alertdialog', { name: /unsaved changes/i });

    fireEvent.click(screen.getByRole('button', { name: 'Cancel' }));
    await waitFor(() => expect(screen.queryByRole('alertdialog')).toBeNull());
    expect(h.closed).toBe(false); // stayed open
    expect(h.destroy).not.toHaveBeenCalled(); // Cancel force-closes nothing
    expect(useTabsStore.getState().tabs.length).toBeGreaterThan(0);

    // `closing` did NOT latch: a later close request (after the user saves) still
    // works. Save the doc, then re-request close → drains + force-closes via
    // destroy().
    await act(async () => {
      modKeyDown('s');
    });
    await act(async () => {
      await h.onClose!({ preventDefault: vi.fn() });
    });
    await waitFor(() => expect(h.closed).toBe(true));
    expect(h.destroy).toHaveBeenCalled();
  });

  it('the force-close AWAITS the drain — it does not destroy while the drain is pending', async () => {
    render(<App />);
    await act(async () => {});
    await waitFor(() => expect(h.onClose).not.toBeNull());

    // A drain that stays pending until we resolve it.
    let resolveDrain!: () => void;
    h.drainGate = { promise: new Promise<void>((resolve) => (resolveDrain = resolve)) };

    let handled: Promise<unknown> | undefined;
    await act(async () => {
      handled = Promise.resolve(h.onClose!({ preventDefault: vi.fn() }));
      await Promise.resolve();
    });
    // The drain is in flight → destroy() has NOT been reached. (A fire-and-forget
    // drain would have destroyed the window by now → this assertion goes RED.)
    expect(h.drain).toHaveBeenCalled();
    expect(h.destroy).not.toHaveBeenCalled();
    expect(h.closed).toBe(false);

    // Resolve the drain → the force-close (destroy) now proceeds.
    await act(async () => {
      resolveDrain();
      await handled;
    });
    expect(h.destroy).toHaveBeenCalled();
    expect(h.closed).toBe(true);
  });

  it('the 2000ms timeout RELEASES the close even if the drain never resolves', async () => {
    render(<App />);
    await act(async () => {});
    await waitFor(() => expect(h.onClose).not.toBeNull());

    // A drain that never resolves — only the bounded-timeout branch can release it.
    h.drainGate = { promise: new Promise<void>(() => {}) };

    vi.useFakeTimers();
    try {
      let handled: Promise<unknown> | undefined;
      await act(async () => {
        handled = Promise.resolve(h.onClose!({ preventDefault: vi.fn() }));
      });
      // Before the cap fires, the close is still held on the (never-resolving)
      // drain — destroy() has not run.
      expect(h.destroy).not.toHaveBeenCalled();
      expect(h.closed).toBe(false);

      // Advance past the 2s cap → the Promise.race timeout fires and releases the
      // close, which force-closes via destroy().
      await act(async () => {
        await vi.advanceTimersByTimeAsync(2100);
        await handled;
      });
      expect(h.destroy).toHaveBeenCalled();
      expect(h.closed).toBe(true);
    } finally {
      vi.useRealTimers();
    }
  });
});

// The Related rail as a reading-first, INTEGRATED right column: collapsed by
// default, opened on demand (persisted), and — when open — its own grid track so
// opening it REFLOWS the reader rather than floating over it. jsdom can't compute
// pixel widths, so these assert the layout STRUCTURE/CLASS the CSS grid keys off.
describe('related rail: collapsed default, integrated reflow, persistence, focus', () => {
  async function openReadDoc() {
    h.disk.set(NOTE, { content: '# Note\n\nbody\n', hash: 'h0' });
    render(<App />);
    await act(async () => {}); // settle hydration
    await act(async () => {
      fireEvent.click(screen.getByRole('button', { name: /Open folder/i }));
    });
    await waitFor(() => expect(useSourcesStore.getState().root).toBe('/ws'));
    act(() => {
      useTabsStore.getState().openTab({ path: NOTE, name: 'note.md' }, { originRoot: '/ws' });
    });
    await waitFor(() => expect(useTabsStore.getState().activeTabId).toBeTruthy());
  }

  it('is COLLAPSED by default: the thin strip shows, no open column, no floating panel', async () => {
    await openReadDoc();
    const row = document.querySelector('.shell__reader-row');
    expect(row).not.toBeNull();
    // Nothing persisted → the settings default keeps it collapsed.
    expect(useSettingsStore.getState().relatedRailOpen).toBe(false);
    expect(row!.classList.contains('shell__reader-row--related-open')).toBe(false);
    // The collapsed strip (a toggle) is present; the open aside is not.
    expect(screen.getByTitle('Show related notes')).toBeTruthy();
    expect(document.querySelector('.related-rail')).toBeNull();
  });

  it('the Toggle-related shortcut OPENS it and reflows the reader into its own grid column', async () => {
    await openReadDoc();
    act(() => modKeyDown('R', { shiftKey: true }));
    // The binding drives the PERSISTED setting (not throwaway local state)...
    await waitFor(() => expect(useSettingsStore.getState().relatedRailOpen).toBe(true));

    const row = document.querySelector('.shell__reader-row')!;
    // ...and the shell reflows: the row carries the open modifier (which pins the
    // rail its own fixed grid column, narrowing the reader) and the rail renders
    // as a direct CHILD of the reader row — a column, not an overlay.
    expect(row.classList.contains('shell__reader-row--related-open')).toBe(true);
    const rail = document.querySelector('.related-rail');
    expect(rail).not.toBeNull();
    expect(rail!.parentElement).toBe(row);
    // The collapsed strip is gone; the open header's Hide control took its place.
    expect(screen.queryByTitle('Show related notes')).toBeNull();
    expect(screen.getByTitle('Hide related notes')).toBeTruthy();

    // Toggling again collapses it — the reader reclaims the column.
    act(() => modKeyDown('R', { shiftKey: true }));
    await waitFor(() => expect(useSettingsStore.getState().relatedRailOpen).toBe(false));
    expect(
      document
        .querySelector('.shell__reader-row')!
        .classList.contains('shell__reader-row--related-open'),
    ).toBe(false);
  });

  it('clicking the collapsed strip opens the rail too', async () => {
    await openReadDoc();
    await act(async () => {
      fireEvent.click(screen.getByTitle('Show related notes'));
    });
    await waitFor(() => expect(document.querySelector('.related-rail')).not.toBeNull());
    expect(useSettingsStore.getState().relatedRailOpen).toBe(true);
  });

  it('flushes pending SETTINGS on the native close so a last-second change is not lost', async () => {
    await openReadDoc();
    // The close hook is wired once the shell is hydrated in the (mocked) Tauri shell.
    await waitFor(() => expect(h.onClose).not.toBeNull());

    // Toggle a preference and DO NOT wait out the 1s settings debounce — this is the
    // last-second change that the session-only close flush used to drop.
    vi.mocked(saveSettings).mockClear();
    act(() => useSettingsStore.getState().setRelatedRailOpen(true));

    // The native close must persist settings on the way out (mirroring the session
    // flush). Reverting the settings close-flush makes this write never happen.
    await act(async () => {
      await h.onClose!({ preventDefault: vi.fn() });
    });
    const flushedOpen = vi
      .mocked(saveSettings)
      .mock.calls.some(([snapshot]) => snapshot.relatedRailOpen === true);
    expect(flushedOpen).toBe(true);
  });

  it('focus mode hides the Related rail in BOTH forms (the focus-hide rule covers it)', () => {
    // jsdom won't cascade the stylesheet, so assert the rule itself: the bug was
    // that .shell--focus hid the other rails but NOT the Related rail (open OR
    // collapsed), so focus mode looked like a no-op with the rail still on screen.
    // jsdom does not cascade imported stylesheets and vite strips CSS to empty
    // under `?raw` in test mode, so read the real file and inspect the rule text.
    // (jsdom makes `import.meta.url` an http: URL, so anchor on the package cwd.)
    const shellCss = readFileSync(`${process.cwd()}/src/shell/shell.css`, 'utf8');
    const hideBlocks: string[] = shellCss.match(/[^{}]+\{\s*display:\s*none;?\s*\}/g) ?? [];
    const focusHide = hideBlocks.find((block) => block.includes('.shell--focus .sidebar'));
    expect(focusHide, 'the focus-mode display:none rule').toBeTruthy();
    // Both the OPEN aside and the COLLAPSED strip must be in the hide list.
    expect(focusHide!).toContain('.shell--focus .related-rail,');
    expect(focusHide!).toContain('.shell--focus .related-rail__toggle--collapsed,');
  });
});
