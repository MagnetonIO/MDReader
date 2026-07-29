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
  readCalls: [] as string[],
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
  readGate: null as null | { promise: Promise<void>; resolve: () => void },
  gatedReadPath: null as string | null,
  failedReadPath: null as string | null,
  listGate: null as null | { promise: Promise<void>; resolve: () => void },
  sourceListGate: null as null | { promise: Promise<void>; resolve: () => void },
  sourceListCalls: 0,
  watchResponseGate: null as null | { promise: Promise<void>; resolve: () => void },
  gatedWatchSourceId: null as string | null,
  failedWatchSourceId: null as string | null,
  backendRoot: null as string | null,
  backendGeneration: 0,
  searchRoots: [] as (string | null)[],
  libraryEvidence: null as null | Record<string, unknown>,
  libraryQueryFails: false,
  libraryQueryQueue: [] as Promise<Record<string, unknown>>[],
  libraryQueryPhases: [] as Array<'pre-mark' | 'post-mark'>,
  libraryQueryRecordKeys: [] as Array<string | null>,
  libraryQueryContinuations: [] as Array<
    { queryId: string; cursor: string } | undefined
  >,
  libraryCancelGate: null as null | { promise: Promise<void>; resolve: () => void },
  libraryCancelCalls: [] as string[],
  markGate: null as null | { promise: Promise<void>; resolve: () => void },
  markStarted: false,
  markSucceeded: false,
  editResolutions: [] as { sourceId: string; relativePath: string }[],
  closeForgetCalls: [] as string[],
  closeForgetStatus: { kind: 'complete' as const } as
    | { kind: 'complete' }
    | { kind: 'cleanup-pending' }
    | { kind: 'inactive'; error: { kind: 'persist-failed' } },
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
      h.readCalls.push(path);
      if (h.gatedReadPath === path && h.readGate) await h.readGate.promise;
      if (h.failedReadPath === path) throw new Error('injected stale read failure');
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
  const listSources = vi.fn(async () => {
    h.sourceListCalls += 1;
    if (h.sourceListGate) await h.sourceListGate.promise;
    return {
      version: 4,
      sources: [source('/ws', true), source('/ws-b', false), source('/ws-c', false)],
      cloud_sources: [],
      connected_catdb: null,
      connected_catdb_rediscovery_required: false,
      cleanup: { kind: 'complete' as const },
    };
  });
  const pickAndAddSource = vi.fn(async () => ({
    source: source(h.pickedPath, h.pickedPath === '/ws'),
    added: true,
  }));
  const markActive = vi.fn(async () => {
    h.markStarted = true;
    if (h.markGate) await h.markGate.promise;
    h.markSucceeded = true;
    return {
      version: 4,
      sources: [
        source('/ws', h.pickedPath === '/ws'),
        source('/ws-b', h.pickedPath === '/ws-b'),
        source('/ws-c', h.pickedPath === '/ws-c'),
      ],
      cleanup: { kind: 'complete' as const },
    };
  });
  const emptyLibraryEvidence = {
    answer: {
      query_id: '00000000-0000-4000-8000-000000000001',
      pins: {
        catalog_resolver_id: '00000000-0000-4000-8000-000000000002',
        catalog_revision_id: '00000000-0000-4000-8000-000000000003',
        schema_version_id: '00000000-0000-4000-8000-000000000004',
        physical_schema_version_ids: [],
        mapping_version_ids: [],
        source_version_ids: [],
        policy_version_ids: ['00000000-0000-4000-8000-000000000005'],
        provenance_rule_version_id: '00000000-0000-4000-8000-000000000006',
      },
      rows: [],
      sources: [],
      completeness: { state: 'complete' as const },
      next_cursor: null,
    },
    memberships: [],
  };
  const settingsBackend = {
    listSources,
    pickAndAddSource,
    reconcileRestore: vi.fn(async () => null),
    markActive,
    forgetSource: vi.fn(async () => ({
      version: 4,
      sources: [],
      cleanup: { kind: 'complete' as const },
    })),
    diagnosticsInfo: vi.fn(async () => ({
      level: 'standard' as const,
      max_file_bytes: 256 * 1024,
      max_retained_files: 3,
      max_export_bytes: 512 * 1024,
    })),
    setDiagnosticLevel: vi.fn(async (level: 'errors' | 'standard' | 'verbose') => ({
      level,
      max_file_bytes: 256 * 1024,
      max_retained_files: 3,
      max_export_bytes: 512 * 1024,
    })),
    recordDiagnosticEvent: vi.fn(async () => {}),
    revealDiagnostics: vi.fn(async () => {}),
    exportDiagnostics: vi.fn(async () => ({
      status: 'cancelled' as const,
      bytes_written: 0,
    })),
    clearDiagnostics: vi.fn(async () => ({
      level: 'standard' as const,
      max_file_bytes: 256 * 1024,
      max_retained_files: 3,
      max_export_bytes: 512 * 1024,
    })),
    prepareCloudCatalog: vi.fn(async () => ({
      workspace_id: '00000000-0000-4000-8000-000000000006',
      catalog_revision_id: '00000000-0000-4000-8000-000000000007',
      schema_version_id: '00000000-0000-4000-8000-000000000008',
    })),
    queryLibraryDocuments: vi.fn(
      async (
        _workspaceId: string,
        filters: Array<{ field: string; operator: string; value: string }>,
        _fields: string[],
        _requestId: string,
        _sourceIds?: string[],
        continuation?: { queryId: string; cursor: string },
      ) => {
      h.libraryQueryRecordKeys.push(filters[0]?.value ?? null);
      h.libraryQueryContinuations.push(continuation);
      h.libraryQueryPhases.push(h.markSucceeded ? 'post-mark' : 'pre-mark');
      const queued = h.libraryQueryQueue.shift();
      if (queued) return queued;
      if (h.libraryQueryFails) throw new Error('injected Library refresh failure');
      return h.libraryEvidence ?? emptyLibraryEvidence;
      },
    ),
    cancelCloudDocumentQuery: vi.fn(async (requestId: string) => {
      h.libraryCancelCalls.push(requestId);
      if (h.libraryCancelGate) await h.libraryCancelGate.promise;
      return true;
    }),
    resolveLibraryDocumentEdit: vi.fn(async (sourceId: string, relativePath: string) => {
      h.editResolutions.push({ sourceId, relativePath });
      const root = sourceId === 'b'.repeat(64) ? '/ws-b' : '/ws';
      return `${root}/${relativePath}`;
    }),
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
    closeAndForgetLibrarySource: vi.fn(async (sourceId: string) => {
      h.closeForgetCalls.push(sourceId);
      h.backendRoot = null;
      h.backendGeneration += 1;
      return {
        status: h.closeForgetStatus,
        workspace_generation: h.backendGeneration,
      };
    }),
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
import { saveSettings, sessionWriter } from './persistence.js';

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

async function confirmActiveSourceForget() {
  fireEvent.click(screen.getByRole('button', { name: 'Settings' }));
  await screen.findByText('Workspace');
  fireEvent.click(screen.getByRole('button', { name: 'Forget Workspace' }));
  fireEvent.click(screen.getByRole('button', { name: 'Forget source' }));
}

function installSecondWorkspaceLibraryDocument() {
  const sourceId = '00000000-0000-4000-8000-000000000011';
  const sourceVersionId = '00000000-0000-4000-8000-000000000012';
  const mappingVersionId = '00000000-0000-4000-8000-000000000013';
  h.libraryEvidence = {
    memberships: [
      {
        membership_id: '00000000-0000-4000-8000-000000000010',
        registry_id: 'b'.repeat(64),
        name: 'Second workspace',
        kind: 'local-folder',
        source_ids: [sourceId],
        editable: true,
        active: false,
      },
    ],
    answer: {
      query_id: '00000000-0000-4000-8000-000000000014',
      pins: {
        catalog_resolver_id: '00000000-0000-4000-8000-000000000015',
        catalog_revision_id: '00000000-0000-4000-8000-000000000016',
        schema_version_id: '00000000-0000-4000-8000-000000000017',
        physical_schema_version_ids: ['00000000-0000-4000-8000-000000000018'],
        mapping_version_ids: [mappingVersionId],
        source_version_ids: [sourceVersionId],
        policy_version_ids: ['00000000-0000-4000-8000-000000000019'],
        provenance_rule_version_id: '00000000-0000-4000-8000-000000000020',
      },
      rows: [
        {
          record_key: 'notes/local.md',
          fields: {
            path: 'notes/local.md',
            title: 'Local note',
            body: '# Local note',
          },
          provenance: {
            path: {
              field: 'path',
              provenance_rule_version_id: '00000000-0000-4000-8000-000000000020',
              selected: 0,
              considered: [
                {
                  source_version_id: sourceVersionId,
                  record_key: 'notes/local.md',
                  mapping_version_id: mappingVersionId,
                  physical_attribute: 'path',
                  observed_at_ms: 1,
                  state: { state: 'observed_value', value: 'notes/local.md' },
                },
              ],
            },
          },
        },
      ],
      sources: [
        {
          source_id: sourceId,
          source_version_id: sourceVersionId,
          outcome: 'ran',
          freshness_ms: 0,
          scanned: 1,
          returned: 1,
        },
      ],
      completeness: { state: 'complete' },
      next_cursor: null,
    },
  };
  h.disk.set('/ws-b/notes/local.md', { content: '# Local note', hash: 'local-hash' });
}

function installRapidCloudLibraryDocuments() {
  installSecondWorkspaceLibraryDocument();
  const fixture = h.libraryEvidence as {
    memberships: Array<{
      registry_id: string;
      name: string;
      kind: string;
      editable: boolean;
    }>;
    answer: {
      rows: Array<{
        record_key: string;
        fields: Record<string, unknown>;
        provenance: {
          path: {
            considered: Array<{
              record_key: string;
              state: { state: string; value: string };
            }>;
          };
        };
      }>;
    };
  };
  Object.assign(fixture.memberships[0]!, {
    registry_id: 'google-drive',
    name: 'Google Drive',
    kind: 'google-drive',
    editable: false,
  });
  const template = fixture.answer.rows[0]!;
  fixture.answer.rows = [
    ['drive-first', 'First note'],
    ['drive-second', 'Second note'],
    ['drive-third', 'Third note'],
  ].map(([recordKey, title]) => {
    const row = structuredClone(template);
    row.record_key = recordKey!;
    row.fields = {
      path: recordKey,
      title,
      body: `# ${title}`,
    };
    row.provenance.path.considered[0]!.record_key = recordKey!;
    row.provenance.path.considered[0]!.state.value = recordKey!;
    return row;
  });
}

function installDropboxContinuationPages() {
  installSecondWorkspaceLibraryDocument();
  const pageOne = structuredClone(h.libraryEvidence!) as {
    window_truncated?: boolean;
    memberships: Array<{
      registry_id: string;
      name: string;
      kind: string;
      editable: boolean;
      source_ids: string[];
    }>;
    answer: {
      query_id: string;
      rows: Array<{
        record_key: string;
        fields: Record<string, unknown>;
        provenance: {
          path: {
            considered: Array<{
              record_key: string;
              state: { state: string; value: string };
            }>;
          };
        };
      }>;
      next_cursor: string | null;
      sources: Array<{
        source_id: string;
        outcome: string;
      }>;
      completeness:
        | { state: 'complete' }
        | { state: 'partial'; excluded_source_ids: string[] };
    };
  };
  Object.assign(pageOne.memberships[0]!, {
    registry_id: 'dropbox',
    name: 'Dropbox',
    kind: 'dropbox',
    editable: false,
  });
  const pageTwo = structuredClone(pageOne);
  const unavailableSourceId = '00000000-0000-4000-8000-000000000099';
  pageOne.memberships[0]!.source_ids.push(unavailableSourceId);
  pageOne.answer.sources.push({
    ...pageOne.answer.sources[0]!,
    source_id: unavailableSourceId,
    outcome: 'source_failed',
  });
  pageOne.answer.completeness = {
    state: 'partial',
    excluded_source_ids: [unavailableSourceId],
  };
  pageOne.answer.rows = [];
  pageOne.answer.next_cursor = 'opaque-application-page-2';
  pageOne.window_truncated = true;
  pageTwo.answer.rows[0]!.record_key = '/notes/later.md';
  pageTwo.answer.rows[0]!.fields = {
    path: '/notes/later.md',
    title: 'Later Dropbox note.md',
  };
  pageTwo.answer.rows[0]!.provenance.path.considered[0]!.record_key =
    '/notes/later.md';
  pageTwo.answer.rows[0]!.provenance.path.considered[0]!.state.value =
    '/notes/later.md';
  pageTwo.answer.next_cursor = null;
  pageTwo.answer.sources = pageTwo.answer.sources.slice(0, 1);
  pageTwo.answer.completeness = { state: 'complete' };
  pageTwo.window_truncated = false;
  h.libraryQueryQueue.push(Promise.resolve(pageOne), Promise.resolve(pageTwo));
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
  h.readCalls.length = 0;
  h.saveCalls.length = 0;
  h.fsChanged = null;
  h.watchError = null;
  h.diagnosticEvents.length = 0;
  h.onClose = null;
  h.gate = null;
  h.readGate = null;
  h.gatedReadPath = null;
  h.failedReadPath = null;
  h.listGate = null;
  h.sourceListGate = null;
  h.sourceListCalls = 0;
  h.watchResponseGate = null;
  h.gatedWatchSourceId = null;
  h.failedWatchSourceId = null;
  h.backendRoot = null;
  h.backendGeneration = 0;
  h.searchRoots.length = 0;
  h.libraryEvidence = null;
  h.libraryQueryFails = false;
  h.libraryQueryQueue.length = 0;
  h.libraryQueryPhases.length = 0;
  h.libraryQueryRecordKeys.length = 0;
  h.libraryQueryContinuations.length = 0;
  h.libraryCancelGate = null;
  h.libraryCancelCalls.length = 0;
  h.markGate = null;
  h.markStarted = false;
  h.markSucceeded = false;
  h.editResolutions.length = 0;
  h.closeForgetCalls.length = 0;
  h.closeForgetStatus = { kind: 'complete' };
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
  it('loads a later Dropbox Markdown page after an empty first provider page', async () => {
    installDropboxContinuationPages();
    render(<App />);

    await screen.findByText('No documents found.');
    fireEvent.click(screen.getByRole('button', { name: 'Load more' }));

    expect(
      await screen.findByRole('button', {
        name: 'Open Later Dropbox note.md from Dropbox',
      }),
    ).toBeTruthy();
    expect(
      screen.getByText('Some Library sources could not answer this query.'),
    ).toBeTruthy();
    expect(h.libraryQueryContinuations).toEqual([
      undefined,
      {
        queryId: '00000000-0000-4000-8000-000000000014',
        cursor: 'opaque-application-page-2',
      },
    ]);
  });

  it('starts only the newest document query after rapid selections share a pending cancellation', async () => {
    installRapidCloudLibraryDocuments();
    render(<App />);
    const first = await screen.findByRole('button', {
      name: 'Open First note from Google Drive',
    });
    const second = screen.getByRole('button', {
      name: 'Open Second note from Google Drive',
    });
    const third = screen.getByRole('button', {
      name: 'Open Third note from Google Drive',
    });
    h.libraryQueryRecordKeys.length = 0;
    const firstBody = deferred<Record<string, unknown>>();
    h.libraryQueryQueue.push(firstBody.promise);

    fireEvent.click(first);
    await waitFor(() => expect(h.libraryQueryRecordKeys).toEqual(['drive-first']));

    h.libraryCancelGate = deferred();
    fireEvent.click(second);
    await waitFor(() => expect(h.libraryCancelCalls).toHaveLength(1));
    fireEvent.click(third);
    await waitFor(() => expect(h.libraryCancelCalls).toHaveLength(2));

    await act(async () => {
      h.libraryCancelGate!.resolve();
      await h.libraryCancelGate!.promise;
    });
    await waitFor(() => expect(h.libraryQueryRecordKeys).toContain('drive-third'));

    expect(h.libraryQueryRecordKeys).toEqual(['drive-first', 'drive-third']);
  });

  it('publishes only the post-mark Library hydration after workspace activation', async () => {
    installSecondWorkspaceLibraryDocument();
    const preActivation = structuredClone(h.libraryEvidence!) as {
      memberships: Array<{
        membership_id: string;
        registry_id: string;
        name: string;
        kind: 'local-folder';
        source_ids: string[];
        editable: boolean;
        active: boolean;
      }>;
      answer: {
        rows: Array<{ fields: Record<string, unknown> }>;
        sources: Array<Record<string, unknown>>;
        pins: { source_version_ids: string[] };
      };
    };
    preActivation.answer.rows[0]!.fields.title = 'Pre-activation note';

    const postMark = structuredClone(preActivation);
    postMark.answer.rows[0]!.fields.title = 'Post-mark note';
    const activePhysicalSourceId = '00000000-0000-4000-8000-000000000021';
    const activeSourceVersionId = '00000000-0000-4000-8000-000000000022';
    postMark.memberships.push({
      ...postMark.memberships[0]!,
      membership_id: '00000000-0000-4000-8000-000000000023',
      registry_id: 'a'.repeat(64),
      name: 'Workspace',
      source_ids: [activePhysicalSourceId],
      active: true,
    });
    postMark.answer.sources.push({
      ...postMark.answer.sources[0],
      source_id: activePhysicalSourceId,
      source_version_id: activeSourceVersionId,
      scanned: 0,
      returned: 0,
    });
    postMark.answer.pins.source_version_ids.push(activeSourceVersionId);

    const startupRead = deferred<Record<string, unknown>>();
    const preMarkRead = deferred<Record<string, unknown>>();
    const liveRead = deferred<Record<string, unknown>>();
    h.libraryQueryQueue.push(startupRead.promise, preMarkRead.promise, liveRead.promise);
    h.markGate = deferred();

    render(<App />);
    await waitFor(() => expect(h.libraryQueryPhases).toEqual(['pre-mark']));

    fireEvent.click(screen.getByRole('button', { name: /Open folder/i }));
    await waitFor(() => expect(h.markStarted).toBe(true));
    await waitFor(() =>
      expect(h.libraryQueryPhases).toEqual(['pre-mark', 'pre-mark']),
    );

    await act(async () => {
      h.markGate!.resolve();
      await h.markGate!.promise;
    });
    await waitFor(() =>
      expect(h.libraryQueryPhases).toEqual(['pre-mark', 'pre-mark', 'post-mark']),
    );

    await act(async () => {
      liveRead.resolve(postMark);
      await liveRead.promise;
    });
    expect(
      await screen.findByRole('button', {
        name: 'Open Post-mark note from Second workspace',
      }),
    ).toBeTruthy();

    await act(async () => {
      startupRead.resolve(preActivation);
      preMarkRead.resolve(preActivation);
      await Promise.all([startupRead.promise, preMarkRead.promise]);
    });

    expect(
      screen.queryByRole('button', {
        name: 'Open Pre-activation note from Second workspace',
      }),
    ).toBeNull();
    expect(
      screen.getByRole('button', {
        name: 'Open Post-mark note from Second workspace',
      }),
    ).toBeTruthy();
  });

  it('invalidates listed and open evidence when a refresh can no longer verify it', async () => {
    installSecondWorkspaceLibraryDocument();
    render(<App />);
    const open = await screen.findByRole('button', {
      name: 'Open Local note from Second workspace',
    });
    fireEvent.click(open);
    await screen.findByRole('button', { name: 'Edit local file' });

    h.libraryQueryFails = true;
    fireEvent.click(screen.getByRole('button', { name: 'Refresh' }));

    await screen.findByText('Library sources could not be read.');
    expect(
      screen.queryByRole('button', { name: 'Open Local note from Second workspace' }),
    ).toBeNull();
    expect(screen.queryByRole('button', { name: 'Edit local file' })).toBeNull();
  });

  it('does not resume an older Edit after a newer source switch starts', async () => {
    installSecondWorkspaceLibraryDocument();
    render(<App />);
    fireEvent.click(
      await screen.findByRole('button', {
        name: 'Open Local note from Second workspace',
      }),
    );

    const callsBeforeEdit = h.sourceListCalls;
    h.sourceListGate = deferred();
    fireEvent.click(await screen.findByRole('button', { name: 'Edit local file' }));
    await waitFor(() => expect(h.sourceListCalls).toBe(callsBeforeEdit + 1));

    h.pickedPath = '/ws-c';
    fireEvent.click(screen.getByTitle('Open folder…'));
    await waitFor(() => expect(useSourcesStore.getState().root).toBe('/ws-c'));
    h.libraryWatch.mockClear();

    await act(async () => {
      h.sourceListGate!.resolve();
      for (let turn = 0; turn < 12; turn += 1) await Promise.resolve();
    });

    expect(h.libraryWatch).not.toHaveBeenCalledWith('b'.repeat(64));
    expect(useSourcesStore.getState().root).toBe('/ws-c');
    expect(h.editResolutions).toEqual([]);
    expect(h.readCalls).not.toContain('/ws-b/notes/local.md');
  });

  it('does not toast when an older Edit read fails after a newer source switch starts', async () => {
    installSecondWorkspaceLibraryDocument();
    const documentPath = '/ws-b/notes/local.md';
    h.readGate = deferred();
    h.gatedReadPath = documentPath;
    h.failedReadPath = documentPath;
    render(<App />);
    fireEvent.click(
      await screen.findByRole('button', {
        name: 'Open Local note from Second workspace',
      }),
    );
    fireEvent.click(await screen.findByRole('button', { name: 'Edit local file' }));
    await waitFor(() => expect(h.readCalls).toContain(documentPath));

    h.pickedPath = '/ws-c';
    fireEvent.click(screen.getByTitle('Open folder…'));
    await waitFor(() => expect(useSourcesStore.getState().root).toBe('/ws-c'));

    await act(async () => {
      h.readGate!.resolve();
      await Promise.resolve();
    });

    expect(screen.queryByText(/Could not open for editing/i)).toBeNull();
    expect(useSourcesStore.getState().root).toBe('/ws-c');
  });

  it('resolves a Library-relative path through its activated membership before reading for edit', async () => {
    installSecondWorkspaceLibraryDocument();

    render(<App />);
    fireEvent.click(
      await screen.findByRole('button', {
        name: 'Open Local note from Second workspace',
      }),
    );
    fireEvent.click(await screen.findByRole('button', { name: 'Edit local file' }));

    await waitFor(() => expect(h.readCalls).toContain('/ws-b/notes/local.md'));
    expect(h.editResolutions).toEqual([
      { sourceId: 'b'.repeat(64), relativePath: 'notes/local.md' },
    ]);
    expect(h.readCalls).not.toContain('notes/local.md');
    await waitFor(() =>
      expect(
        screen.getByRole('button', { name: 'Source view' }).getAttribute('aria-pressed'),
      ).toBe('true'),
    );
    expect(screen.getByTestId('source-pane').textContent).toContain('# Local note');
    const activeTabId = useTabsStore.getState().activeTabId;
    expect(activeTabId).not.toBeNull();
    expect(
      useEditorStore.getState().entry(activeTabId!, '/ws-b/notes/local.md')?.mode,
    ).toBe('source');
  });

  it.each(['Save', 'Discard'] as const)(
    'dirty cross-workspace Edit resumes after %s without a second click',
    async (decision) => {
      installSecondWorkspaceLibraryDocument();
      await openDirtyDoc();
      fireEvent.click(
        await screen.findByRole('button', {
          name: 'Open Local note from Second workspace',
        }),
      );
      fireEvent.click(await screen.findByRole('button', { name: 'Edit local file' }));

      await screen.findByRole('alertdialog', { name: /unsaved changes/i });
      await act(async () => {
        fireEvent.click(screen.getByRole('button', { name: decision }));
      });

      await waitFor(() =>
        expect(h.editResolutions).toEqual([
          { sourceId: 'b'.repeat(64), relativePath: 'notes/local.md' },
        ]),
      );
      expect(useSourcesStore.getState().root).toBe('/ws-b');
      const activeTabId = useTabsStore.getState().activeTabId;
      expect(activeTabId).not.toBeNull();
      await waitFor(() =>
        expect(
          useEditorStore.getState().entry(activeTabId!, '/ws-b/notes/local.md')?.mode,
        ).toBe('source'),
      );
    },
  );

  it('dirty cross-workspace Edit Cancel leaves the original edit untouched and opens nothing', async () => {
    installSecondWorkspaceLibraryDocument();
    const dirtyTabId = await openDirtyDoc();
    fireEvent.click(
      await screen.findByRole('button', {
        name: 'Open Local note from Second workspace',
      }),
    );
    fireEvent.click(await screen.findByRole('button', { name: 'Edit local file' }));

    await screen.findByRole('alertdialog', { name: /unsaved changes/i });
    fireEvent.click(screen.getByRole('button', { name: 'Cancel' }));
    await waitFor(() => expect(screen.queryByRole('alertdialog')).toBeNull());

    expect(h.editResolutions).toEqual([]);
    expect(useSourcesStore.getState().root).toBe('/ws');
    expect(useEditorStore.getState().isDirty(dirtyTabId, NOTE)).toBe(true);
    expect(h.readCalls).not.toContain('/ws-b/notes/local.md');
  });

  it('dirty cross-workspace Edit does not resume when Save conflicts', async () => {
    installSecondWorkspaceLibraryDocument();
    const dirtyTabId = await openDirtyDoc();
    fireEvent.click(
      await screen.findByRole('button', {
        name: 'Open Local note from Second workspace',
      }),
    );
    fireEvent.click(await screen.findByRole('button', { name: 'Edit local file' }));
    await screen.findByRole('alertdialog', { name: /unsaved changes/i });

    h.disk.set(NOTE, { content: 'newer disk content', hash: 'external-hash' });
    await act(async () => {
      fireEvent.click(screen.getByRole('button', { name: 'Save' }));
    });
    await waitFor(() => expect(screen.queryByRole('alertdialog')).toBeNull());

    expect(h.editResolutions).toEqual([]);
    expect(useSourcesStore.getState().root).toBe('/ws');
    expect(useEditorStore.getState().isDirty(dirtyTabId, NOTE)).toBe(true);
    expect(h.readCalls).not.toContain('/ws-b/notes/local.md');
  });

  it('dirty cross-workspace Edit does not resume after a failed source switch', async () => {
    installSecondWorkspaceLibraryDocument();
    const dirtyTabId = await openDirtyDoc();
    h.failedWatchSourceId = 'b'.repeat(64);
    fireEvent.click(
      await screen.findByRole('button', {
        name: 'Open Local note from Second workspace',
      }),
    );
    fireEvent.click(await screen.findByRole('button', { name: 'Edit local file' }));
    await screen.findByRole('alertdialog', { name: /unsaved changes/i });

    fireEvent.click(screen.getByRole('button', { name: 'Discard' }));
    await waitFor(() => expect(h.libraryWatch).toHaveBeenCalledWith('b'.repeat(64)));
    await act(async () => {});

    expect(h.editResolutions).toEqual([]);
    expect(useSourcesStore.getState().root).toBe('/ws');
    expect(useEditorStore.getState().isDirty(dirtyTabId, NOTE)).toBe(true);
    expect(h.readCalls).not.toContain('/ws-b/notes/local.md');
  });

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
  it('cancels active-source Forget without IPC, then Discard closes authority and clears only that source state', async () => {
    await openDirtyDoc();
    act(() => {
      useSourcesStore
        .getState()
        .addRecent({ path: NOTE, name: 'note.md' }, '/ws');
      useSourcesStore
        .getState()
        .addRecent({ path: '/ws-b/keep.md', name: 'keep.md' }, '/ws-b');
    });

    await confirmActiveSourceForget();
    await screen.findByRole('alertdialog', { name: /unsaved changes/i });
    fireEvent.click(screen.getByRole('button', { name: 'Cancel' }));
    expect(h.closeForgetCalls).toEqual([]);
    expect(useSourcesStore.getState().root).toBe('/ws');
    expect(useEditorStore.getState().dirtyTabIds()).not.toHaveLength(0);

    const writesBeforeForget = vi.mocked(sessionWriter.write).mock.calls.length;
    await confirmActiveSourceForget();
    await screen.findByRole('alertdialog', { name: /unsaved changes/i });
    fireEvent.click(screen.getByRole('button', { name: 'Discard' }));

    await waitFor(() => expect(h.closeForgetCalls).toEqual(['a'.repeat(64)]));
    await waitFor(() => expect(useSourcesStore.getState().root).toBeNull());
    expect(useTabsStore.getState().tabs).toEqual([]);
    expect(useEditorStore.getState().dirtyTabIds()).toEqual([]);
    expect(useSourcesStore.getState().recent).toEqual([
      { root: '/ws-b', path: '/ws-b/keep.md', name: 'keep.md' },
    ]);
    const closeWrites = vi.mocked(sessionWriter.write).mock.calls.slice(writesBeforeForget);
    expect(closeWrites).toContainEqual([
      expect.objectContaining({
        root: null,
        tabs: [],
        recent: [{ root: '/ws-b', path: '/ws-b/keep.md', name: 'keep.md' }],
      }),
    ]);
    expect(h.drain).toHaveBeenCalled();
  });

  it('Save writes every dirty buffer before active-source Forget closes it', async () => {
    await openDirtyDoc();
    await confirmActiveSourceForget();
    await screen.findByRole('alertdialog', { name: /unsaved changes/i });

    fireEvent.click(screen.getByRole('button', { name: 'Save' }));

    await waitFor(() => expect(h.closeForgetCalls).toEqual(['a'.repeat(64)]));
    expect(h.disk.get(NOTE)?.content).toBe('hello + my local edit');
    expect(useSourcesStore.getState().root).toBeNull();
    expect(useTabsStore.getState().tabs).toEqual([]);
  });

  it('a conflicted Save refuses active-source Forget and leaves authority and edits live', async () => {
    const tabId = await openDirtyDoc();
    h.disk.set(NOTE, { content: 'external edit', hash: 'h-external' });
    await confirmActiveSourceForget();
    await screen.findByRole('alertdialog', { name: /unsaved changes/i });

    fireEvent.click(screen.getByRole('button', { name: 'Save' }));

    await waitFor(() =>
      expect(useEditorStore.getState().entry(tabId, NOTE)?.conflict?.source).toBe('save'),
    );
    expect(h.closeForgetCalls).toEqual([]);
    expect(useSourcesStore.getState().root).toBe('/ws');
    expect(useEditorStore.getState().isDirty(tabId, NOTE)).toBe(true);
  });

  it('bounds a stuck session drain so a newer source activation can leave the closed state', async () => {
    render(<App />);
    await act(async () => {});
    await act(async () => {
      fireEvent.click(screen.getByRole('button', { name: /Open folder/i }));
    });
    await waitFor(() => expect(useSourcesStore.getState().root).toBe('/ws'));
    h.drainGate = { promise: new Promise<void>(() => {}) };

    await confirmActiveSourceForget();
    await waitFor(() => expect(useSourcesStore.getState().root).toBeNull());
    fireEvent.click(screen.getByRole('button', { name: 'Settings' }));
    await screen.findByText('Second workspace');
    fireEvent.click(screen.getByRole('button', { name: 'Activate Second workspace' }));

    await waitFor(() => expect(useSourcesStore.getState().root).toBe('/ws-b'), {
      timeout: 3000,
    });
  });

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
